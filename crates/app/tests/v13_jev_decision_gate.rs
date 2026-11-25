use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use tavern_core::{
    AppConfig, BoxFuture, DecisionGate, GameEngine, JevConfig, LlmProvider, OocPrecheck,
    ScenarioCatalog,
};
use tavern_domain::{
    ChatMode, DmStructuredReply, EngineResponse, IncomingMessage, LoreCandidate, NarrativeStyle,
    OocVerdict, PendingAction, Player, PrivateWhisper, ReplyAuditVerdict, RoomState, TurnPhase,
    detect_consistency_nudge, detect_consistency_nudge_with_audit, merge_patch_delta,
};
use tavern_infra_llm::parser::parse_patch_delta;
use tavern_infra_llm::prompt_ext::{build_lore_corpus, collect_unmatched_lore_candidates};
use tavern_infra_llm::prompts::{build_dm_messages, build_dm_messages_with_extra_lore};
use tavern_infra_llm::{MockLlmProvider, OpenAiLlmProvider, SystemOneJevGate};
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn test_config() -> AppConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    AppConfig {
        kook_bot_token: None,
        llm_base_url: "http://127.0.0.1:1/v1".into(),
        llm_api_key: Some("sk-test".into()),
        llm_model: "deepseek-chat".into(),
        llm_summary_model: "deepseek-chat".into(),
        llm_temperature: 0.7,
        llm_max_tokens: 1024,
        db_path: PathBuf::from(":memory:"),
        scenarios_dir: root.join("scenarios"),
        prompts_dir: root.join("prompts"),
        exports_dir: PathBuf::from("/tmp/kook_tavern_v13_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec![],
        allowed_channel_ids: vec![],
        admin_user_ids: vec!["u_admin".into()],
        export_channel_id: None,
        web_bind_addr: None,
        web_public_base_url: None,
        daily_max_llm_calls: 0,
        daily_max_cny: 0.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
        jev: JevConfig::default(),
    }
}

struct MockJevGate {
    ooc_calls: AtomicUsize,
    lore_ids: Vec<String>,
    audit_verdict: Option<ReplyAuditVerdict>,
}

impl MockJevGate {
    fn new(lore_ids: Vec<String>, audit_verdict: Option<ReplyAuditVerdict>) -> Self {
        Self {
            ooc_calls: AtomicUsize::new(0),
            lore_ids,
            audit_verdict,
        }
    }
}

impl DecisionGate for MockJevGate {
    fn is_enabled(&self) -> bool {
        true
    }

    fn evaluate_ooc<'a>(
        &'a self,
        _scenario_name: &'a str,
        _location: &'a str,
        _recent_scene: &'a str,
        _sender_name: &'a str,
        message: &'a str,
    ) -> BoxFuture<'a, Option<OocVerdict>> {
        self.ooc_calls.fetch_add(1, Ordering::SeqCst);
        let msg = message.to_string();
        Box::pin(async move {
            if msg.contains("超时") {
                return None;
            }
            let prob_ooc = if msg.contains("外卖") { 0.94 } else { 0.08 };
            Some(OocVerdict {
                prob_ooc,
                provider: "mock",
                latency_ms: 12.0,
            })
        })
    }

    fn rank_hidden_lore<'a>(
        &'a self,
        _location: &'a str,
        _action_corpus: &'a str,
        _candidates: &'a [LoreCandidate],
    ) -> BoxFuture<'a, Vec<String>> {
        let ids = self.lore_ids.clone();
        Box::pin(async move { ids })
    }

    fn audit_dm_reply<'a>(
        &'a self,
        _narrative: &'a str,
        _whispers_summary: &'a str,
        _has_vitals_patch: bool,
        _has_item_patch: bool,
    ) -> BoxFuture<'a, Option<ReplyAuditVerdict>> {
        let v = self.audit_verdict.clone();
        Box::pin(async move { v })
    }
}

fn mk_msg(chan: &str, uid: &str, name: &str, content: &str, is_direct: bool) -> IncomingMessage {
    IncomingMessage {
        guild_id: if is_direct { None } else { Some("g1".into()) },
        channel_id: chan.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct,
    }
}

#[tokio::test]
async fn ooc_gate_filters_group_banter_but_protects_ic_commands_dm_and_fail_open() {
    let cfg = test_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let gate = Arc::new(MockJevGate::new(vec![], None));
    let engine = GameEngine::new(cfg, store, catalog, Arc::new(MockLlmProvider))
        .with_decision_gate(gate.clone());
    assert!(engine.has_decision_gate());

    // Setup group room in Roleplay mode
    let _ = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "/start ember_tavern", false))
        .await
        .unwrap();
    let _ = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "/mode rp", false))
        .await
        .unwrap();
    let _ = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "/join 阿明 | 侦探", false))
        .await
        .unwrap();

    // 1. Plain OOC banter -> precheck catches BEFORE placeholder, and dispatch ignores
    assert!(
        engine
            .is_plain_roleplay_ooc(Some("g1"), "c_ooc", "u1", "阿明", "外卖到了先吃两口", false)
            .await
    );
    let resp = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "外卖到了先吃两口", false))
        .await
        .unwrap();
    assert!(matches!(resp, EngineResponse::Ignore));

    // 2. Explicit `/act` and `>` ALWAYS bypass OOC gate even if text mentions "外卖"
    let calls_before = gate.ooc_calls.load(Ordering::SeqCst);
    let r_act = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "/act 假装外卖到了试探老板", false))
        .await
        .unwrap();
    assert!(matches!(r_act, EngineResponse::StoryTurn { .. }));
    let r_gt = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "> 假装外卖到了敲桌子", false))
        .await
        .unwrap();
    assert!(matches!(r_gt, EngineResponse::StoryTurn { .. }));
    assert_eq!(gate.ooc_calls.load(Ordering::SeqCst), calls_before);

    // 3. Plain IC action with precheck -> exactly ONE Jev call, zero duplicate call
    let calls_before_ic = gate.ooc_calls.load(Ordering::SeqCst);
    let pre_ooc = engine
        .is_plain_roleplay_ooc(Some("g1"), "c_ooc", "u1", "阿明", "悄悄拔出短刃护在胸前", false)
        .await;
    assert!(!pre_ooc);
    let r_ic = engine
        .handle_message_prechecked(mk_msg("c_ooc", "u1", "阿明", "悄悄拔出短刃护在胸前", false), true)
        .await
        .unwrap();
    assert!(matches!(r_ic, EngineResponse::StoryTurn { .. }));
    assert_eq!(gate.ooc_calls.load(Ordering::SeqCst), calls_before_ic + 1);

    // 4. Timeout/error fails open (never swallows player's action!)
    let r_timeout = engine
        .handle_message(mk_msg("c_ooc", "u1", "阿明", "超时测试消息我去推门", false))
        .await
        .unwrap();
    assert!(matches!(r_timeout, EngineResponse::StoryTurn { .. }));

    // 5. 1-on-1 DM ALWAYS bypasses OOC gate (0 Jev calls!)
    let _ = engine
        .handle_message(mk_msg("dm:u1", "u1", "阿明", "/start ember_tavern", true))
        .await
        .unwrap();
    let _ = engine
        .handle_message(mk_msg("dm:u1", "u1", "阿明", "/mode rp", true))
        .await
        .unwrap();
    let _ = engine
        .handle_message(mk_msg("dm:u1", "u1", "阿明", "/join 阿明 | 侦探", true))
        .await
        .unwrap();
    let calls_before_dm = gate.ooc_calls.load(Ordering::SeqCst);
    let r_dm = engine
        .handle_message(mk_msg("dm:u1", "u1", "阿明", "外卖到了先吃两口", true))
        .await
        .unwrap();
    assert!(matches!(r_dm, EngineResponse::StoryTurn { .. }));
    assert_eq!(gate.ooc_calls.load(Ordering::SeqCst), calls_before_dm);
}

#[test]
fn semantic_lore_union_preserves_prefix_cache_and_keyword_floor() {
    let cfg = test_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();
    let room = RoomState {
        channel_id: "c_lore".into(), guild_id: "g1".into(), scenario_id: sc.id.clone(),
        title: sc.name.clone(), location: sc.initial_location.clone(),
        world_time: sc.initial_time.clone(), turn_number: 1, state_version: 1,
        turn_phase: TurnPhase::Collecting, chat_mode: ChatMode::Strict,
        narrative_style: NarrativeStyle::Prose, gm_user_id: "u1".into(),
        web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![],
        ended_id: None, ended_title: None, bulletins: vec![], check_system: Default::default(),
        last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
        created_at: now, updated_at: now,
    };
    let actions = vec![PendingAction {
        channel_id: "c_lore".into(), target_turn: 2, user_id: "u1".into(),
        character_name: "阿明".into(), action_text: "我凑近炉火仔细聆听地板下的动静".into(),
        roll_notes: vec![], updated_at: now,
    }];
    let corpus = build_lore_corpus(&room, &[], &actions);
    let unmatched = collect_unmatched_lore_candidates(&sc, &corpus);
    assert!(!unmatched.is_empty(), "ember_tavern should have unmatched hidden worldbook entries");
    let picked_entry = sc.worldbook.iter().find(|e| e.id == unmatched[0].id).unwrap();

    let base_msgs = build_dm_messages(&cfg.prompts_dir, &sc, &room, &[], &[], &actions, &[]);
    let enhanced_msgs = build_dm_messages_with_extra_lore(
        &cfg.prompts_dir, &sc, &room, &[], &[], &actions, &[], std::slice::from_ref(&picked_entry.id),
    );
    // Message 0 (`system`) MUST be 100% byte-identical for DeepSeek Prefix Caching!
    assert_eq!(base_msgs[0].content, enhanced_msgs[0].content);
    assert!(!base_msgs.last().unwrap().content.contains(&picked_entry.title));
    assert!(enhanced_msgs.last().unwrap().content.contains(&picked_entry.title));
}

#[test]
fn semantic_reply_audit_suppresses_false_positives_and_redacts_whisper_leak() {
    let cfg = test_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();
    let room = RoomState {
        channel_id: "c_aud".into(), guild_id: "g1".into(), scenario_id: sc.id.clone(),
        title: sc.name.clone(), location: sc.initial_location.clone(),
        world_time: sc.initial_time.clone(), turn_number: 2, state_version: 2,
        turn_phase: TurnPhase::Collecting, chat_mode: ChatMode::Strict,
        narrative_style: NarrativeStyle::Prose, gm_user_id: "u1".into(),
        web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![],
        ended_id: None, ended_title: None, bulletins: vec![], check_system: Default::default(),
        last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
        created_at: now, updated_at: now,
    };
    let players = vec![Player {
        channel_id: "c_aud".into(), user_id: "u1".into(), character_name: "阿明".into(),
        role_desc: "侦探".into(), traits: vec![], hp: 10, max_hp: 10, san: 10, max_san: 10,
        inventory: vec![], status_note: String::new(), is_afk: false,
        stats: Default::default(), location: String::new(), dnd: Default::default(),
    }];

    // Case A: Keyword heuristic false positive ("幸好没受了伤") suppressed by semantic audit!
    let fp_reply = DmStructuredReply {
        scene_title: "惊险闪避".into(),
        narrative: "你灵巧地侧身闪避，幸好没受了伤，稳稳落回原位。".into(),
        private_whispers: vec![], state_patches: vec![], new_clues: vec![],
        npc_updates: vec![], world_time: None, location: None, suggested_actions: vec![],
        suggested_check: None, add_flags: vec![], advance_act: false,
        trigger_ending_id: None, world_bulletin: None, reincarnation_echo: None,
        grant_inspiration: vec![],
    };
    assert!(detect_consistency_nudge(&sc, &room, &players, &fp_reply, false).is_some());
    let clean_audit = ReplyAuditVerdict {
        prob_missed_vitals: 0.04,
        prob_missed_item: 0.02,
        prob_whisper_leak: 0.0,
        provider: "mock",
        latency_ms: 88.0,
    };
    assert!(
        detect_consistency_nudge_with_audit(&sc, &room, &players, &fp_reply, false, Some(&clean_audit), 0.80)
            .is_none()
    );

    // Case B: Whisper leak detected -> redacted_narrative cleanly scrubs public text!
    let mut leak_reply = DmStructuredReply {
        scene_title: "吧台低语".into(),
        narrative: "老雷恩擦拭着酒杯，其实他抽屉里藏着红月教团的密信，大厅众人一时无言。".into(),
        private_whispers: vec![PrivateWhisper {
            player_name: "阿明".into(),
            secret: "你趁隙瞥见老板抽屉里藏着红月教团的密信。".into(),
        }],
        state_patches: vec![], new_clues: vec![], npc_updates: vec![], world_time: None,
        location: None, suggested_actions: vec![], suggested_check: None, add_flags: vec![],
        advance_act: false, trigger_ending_id: None, world_bulletin: None, reincarnation_echo: None,
        grant_inspiration: vec![],
    };
    let leak_audit = ReplyAuditVerdict {
        prob_missed_vitals: 0.02,
        prob_missed_item: 0.01,
        prob_whisper_leak: 0.93,
        provider: "mock",
        latency_ms: 110.0,
    };
    let plan = detect_consistency_nudge_with_audit(
        &sc, &room, &players, &leak_reply, false, Some(&leak_audit), 0.80,
    )
    .unwrap();
    assert!(plan.want_narrative_redact);
    let delta = parse_patch_delta(
        r#"{"state_patches":[],"location":null,"world_bulletin":null,"redacted_narrative":"老雷恩低头擦拭着酒杯，神色莫测地扫了眼门外风雪，大厅众人一时无言。"}"#,
    )
    .unwrap();
    merge_patch_delta(&mut leak_reply, &plan, delta);
    assert!(!leak_reply.narrative.contains("密信"));
    assert_eq!(leak_reply.private_whispers.len(), 1);
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let mut expected_total: Option<usize> = None;
    loop {
        let n = socket.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        if expected_total.is_none()
            && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
        {
            let headers = String::from_utf8_lossy(&buf[..pos]);
            let content_len = headers
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            expected_total = Some(pos + 4 + content_len);
        }
        if let Some(total) = expected_total
            && buf.len() >= total
        {
            break;
        }
    }
    String::from_utf8_lossy(&buf).to_string()
}

async fn write_http_json(socket: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(resp.as_bytes()).await;
}

#[tokio::test]
async fn system_one_jev_gate_wire_format_fallback_and_hard_timeout() {
    // 1. Mock Bailian System One server (handles 2 sequential requests)
    let bailian_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bailian_addr = bailian_listener.local_addr().unwrap();
    let bailian_srv = tokio::spawn(async move {
        let mut captured = Vec::new();
        for _ in 0..2 {
            let (mut sock, _) = bailian_listener.accept().await.unwrap();
            let raw = read_http_request(&mut sock).await;
            captured.push(raw);
            let body = r#"{"answers":{"is_in_game_action":{"probabilities":{"yes":0.09,"no":0.91},"noul":null}},"latency_ms":123.5}"#;
            write_http_json(&mut sock, "200 OK", body).await;
        }
        captured
    });

    // 2. Mock failing Google Decisions server (returns HTTP 500 -> triggers fallback to Bailian!)
    let google_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let google_addr = google_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = google_listener.accept().await.unwrap();
        let _ = read_http_request(&mut sock).await;
        write_http_json(&mut sock, "500 Internal Server Error", r#"{"error":"boom"}"#).await;
    });

    let cfg = JevConfig {
        api_key: Some("test-bailian-key".into()),
        endpoint: format!("http://{bailian_addr}/compatible-mode/v1/systemone"),
        model: "decision-model-preview".into(),
        timeout_ms: 1500,
        ooc_threshold: 0.85,
        lore_threshold: 0.72,
        audit_threshold: 0.80,
        google_endpoint: None,
        google_api_key: None,
        google_proxy_secret: None,
        google_model: "gemini-decision-1".into(),
    };
    let gate = SystemOneJevGate::from_config(&cfg).unwrap();
    let v1 = gate
        .evaluate_ooc("余烬酒馆", "酒馆大厅", "风雪拍窗", "阿明", "哈哈外卖到了")
        .await
        .unwrap();
    assert_eq!(v1.provider, "bailian");
    assert!((v1.prob_ooc - 0.91).abs() < 1e-6);
    assert!((v1.latency_ms - 123.5).abs() < 1e-6);

    // Now configure Google primary + Bailian fallback -> Google 500 falls back to Bailian!
    let mut fallback_cfg = cfg.clone();
    fallback_cfg.google_endpoint = Some(format!("http://{google_addr}/v1/decisions"));
    fallback_cfg.google_api_key = Some("goog-key".into());
    let gate_fb = SystemOneJevGate::from_config(&fallback_cfg).unwrap();
    let v2 = gate_fb
        .evaluate_ooc("余烬酒馆", "酒馆大厅", "风雪拍窗", "阿明", "哈哈外卖到了")
        .await
        .unwrap();
    assert_eq!(v2.provider, "bailian");

    let reqs = bailian_srv.await.unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[0].to_ascii_lowercase().contains("authorization: bearer test-bailian-key"));
    let body_part = reqs[0].split("\r\n\r\n").nth(1).unwrap();
    let json: Value = serde_json::from_str(body_part).unwrap();
    assert_eq!(json["model"], "decision-model-preview");
    assert!(json["state"].as_str().unwrap().contains("哈哈外卖到了"));
    assert!(json["questions"]["is_in_game_action"]["criteria"]["yes"].is_string());

    // 3. Hard timeout fail-open test (server sleeps 1.6s, client timeout = 500ms)
    let slow_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let slow_addr = slow_listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = slow_listener.accept().await.unwrap();
        let _ = read_http_request(&mut sock).await;
        tokio::time::sleep(Duration::from_millis(1600)).await;
        write_http_json(&mut sock, "200 OK", "{}").await;
    });
    let slow_cfg = JevConfig {
        api_key: Some("k".into()),
        endpoint: format!("http://{slow_addr}/slow"),
        timeout_ms: 500,
        ..JevConfig::default()
    };
    let slow_gate = SystemOneJevGate::from_config(&slow_cfg).unwrap();
    let t0 = Instant::now();
    let res = slow_gate.evaluate_ooc("S", "L", "R", "U", "测试").await;
    assert!(res.is_none());
    assert!(t0.elapsed() < Duration::from_millis(1300));
}

#[tokio::test]
async fn openai_provider_wires_jev_lore_recall_and_whisper_redaction_end_to_end() {
    let llm_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let llm_addr = llm_listener.local_addr().unwrap();
    let llm_srv = tokio::spawn(async move {
        let mut prompts = Vec::new();
        // Call 1: Primary DM reply that leaks a private whisper
        let (mut s1, _) = llm_listener.accept().await.unwrap();
        prompts.push(read_http_request(&mut s1).await);
        let dm_json = serde_json::json!({
            "scene_title": "暗流涌动",
            "narrative": "老雷恩擦拭酒杯时，其实柜台下藏着红月密信，众人面面相觑。",
            "private_whispers": [{"player_name": "阿明", "secret": "柜台下藏着红月密信"}],
            "state_patches": [], "new_clues": [], "npc_updates": [],
            "suggested_actions": ["观察老板"]
        });
        let resp1 = serde_json::json!({
            "choices": [{"message": {"content": dm_json.to_string()}}],
            "usage": {"prompt_tokens": 500, "completion_tokens": 120, "prompt_cache_hit_tokens": 300}
        });
        write_http_json(&mut s1, "200 OK", &resp1.to_string()).await;

        // Call 2: One-shot redacted_narrative repair triggered by Jev audit
        let (mut s2, _) = llm_listener.accept().await.unwrap();
        prompts.push(read_http_request(&mut s2).await);
        let fix_json = serde_json::json!({
            "state_patches": [], "location": null, "world_bulletin": null,
            "redacted_narrative": "老雷恩低头擦拭酒杯，神色冷峻地瞥了眼门口风雪，大厅陷入片刻沉默。"
        });
        let resp2 = serde_json::json!({
            "choices": [{"message": {"content": fix_json.to_string()}}],
            "usage": {"prompt_tokens": 600, "completion_tokens": 60, "prompt_cache_hit_tokens": 500}
        });
        write_http_json(&mut s2, "200 OK", &resp2.to_string()).await;
        prompts
    });

    let mut cfg = test_config();
    cfg.llm_base_url = format!("http://{llm_addr}/v1");
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();
    let room = RoomState {
        channel_id: "c_e2e".into(), guild_id: "g1".into(), scenario_id: sc.id.clone(),
        title: sc.name.clone(), location: sc.initial_location.clone(),
        world_time: sc.initial_time.clone(), turn_number: 1, state_version: 1,
        turn_phase: TurnPhase::Collecting, chat_mode: ChatMode::Strict,
        narrative_style: NarrativeStyle::Prose, gm_user_id: "u1".into(),
        web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![],
        ended_id: None, ended_title: None, bulletins: vec![], check_system: Default::default(),
        last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
        created_at: now, updated_at: now,
    };
    let players = vec![Player {
        channel_id: "c_e2e".into(), user_id: "u1".into(), character_name: "阿明".into(),
        role_desc: "侦探".into(), traits: vec![], hp: 10, max_hp: 10, san: 10, max_san: 10,
        inventory: vec![], status_note: String::new(), is_afk: false,
        stats: Default::default(), location: String::new(), dnd: Default::default(),
    }];
    let actions = vec![PendingAction {
        channel_id: "c_e2e".into(), target_turn: 2, user_id: "u1".into(),
        character_name: "阿明".into(), action_text: "我安静观察吧台动静".into(),
        roll_notes: vec![], updated_at: now,
    }];
    let corpus = build_lore_corpus(&room, &[], &actions);
    let unmatched = collect_unmatched_lore_candidates(&sc, &corpus);
    let picked = sc.worldbook.iter().find(|e| e.id == unmatched[0].id).unwrap();

    let gate = Arc::new(MockJevGate::new(
        vec![picked.id.clone()],
        Some(ReplyAuditVerdict {
            prob_missed_vitals: 0.01,
            prob_missed_item: 0.02,
            prob_whisper_leak: 0.94,
            provider: "mock",
            latency_ms: 95.0,
        }),
    ));
    let provider = OpenAiLlmProvider::from_config(&cfg)
        .unwrap()
        .with_decision_gate(gate, 0.80);
    let out = provider
        .generate_turn(&sc, &room, &players, &[], &actions, &[], true)
        .await
        .unwrap();

    assert_eq!(out.extra_usages.len(), 1);
    assert!(!out.reply.narrative.contains("红月密信"));
    assert_eq!(out.reply.private_whispers.len(), 1);

    let reqs = llm_srv.await.unwrap();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[0].contains(&picked.title), "Jev semantic lore should be injected into prompt");
}

#[tokio::test]
async fn gate7_red_team_regressions_null_fields_bounds_soft_prefix_and_partial_fallback() {
    // 1. `"state_patches": null` parses cleanly, and short meta-chatter cannot nuke long narrative!
    let delta = parse_patch_delta(
        r#"{"state_patches":null,"location":null,"world_bulletin":null,"redacted_narrative":"原公开正文并未泄露私密纸条内容，保持原文即可。"}"#,
    )
    .expect("state_patches: null must still parse");
    let cfg = test_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();
    let room = RoomState {
        channel_id: "c_g7".into(), guild_id: "g1".into(), scenario_id: sc.id.clone(),
        title: sc.name.clone(), location: sc.initial_location.clone(),
        world_time: sc.initial_time.clone(), turn_number: 1, state_version: 1,
        turn_phase: TurnPhase::Collecting, chat_mode: ChatMode::Roleplay,
        narrative_style: NarrativeStyle::Prose, gm_user_id: "u1".into(),
        web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![],
        ended_id: None, ended_title: None, bulletins: vec![], check_system: Default::default(),
        last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
        created_at: now, updated_at: now,
    };
    let players = vec![Player {
        channel_id: "c_g7".into(), user_id: "u1".into(), character_name: "阿明".into(),
        role_desc: "侦探".into(), traits: vec![], hp: 10, max_hp: 10, san: 10, max_san: 10,
        inventory: vec![], status_note: String::new(), is_afk: false,
        stats: Default::default(), location: String::new(), dnd: Default::default(),
    }];
    let long_narrative = "风雪在余烬酒馆的窗棂外呼啸拍打，壁炉里干柴噼啪作响。老雷恩擦拭着一只缺口陶杯，目光在吧台阴影与墙上焦黑徽记之间缓缓游移，大厅里的旅人们各怀心事地压低了交谈声，空气中弥漫着松脂与冷铁交织的紧张气息。";
    let mut reply = DmStructuredReply {
        scene_title: "风雪余烬".into(),
        narrative: long_narrative.into(),
        private_whispers: vec![PrivateWhisper { player_name: "阿明".into(), secret: "密信线索".into() }],
        state_patches: vec![], new_clues: vec![], npc_updates: vec![], world_time: None,
        location: None, suggested_actions: vec![], suggested_check: None, add_flags: vec![],
        advance_act: false, trigger_ending_id: None, world_bulletin: None, reincarnation_echo: None,
        grant_inspiration: vec![],
    };
    let leak_audit = ReplyAuditVerdict {
        prob_missed_vitals: 0.0, prob_missed_item: 0.0, prob_whisper_leak: 0.91,
        provider: "mock", latency_ms: 50.0,
    };
    let plan = detect_consistency_nudge_with_audit(
        &sc, &room, &players, &reply, false, Some(&leak_audit), 0.80,
    )
    .unwrap();
    merge_patch_delta(&mut reply, &plan, delta);
    assert_eq!(reply.narrative, long_narrative, "short meta-chatter must not overwrite long narrative");

    // 2. Soft-prefixed messages ("...外卖", "...我退后") behave uniformly + bare ">" ignored!
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let gate = Arc::new(MockJevGate::new(vec![], None));
    let engine = GameEngine::new(cfg, store, Arc::new(catalog), Arc::new(MockLlmProvider))
        .with_decision_gate(gate);
    let _ = engine.handle_message(mk_msg("c_soft", "u1", "阿明", "/start ember_tavern", false)).await.unwrap();
    let _ = engine.handle_message(mk_msg("c_soft", "u1", "阿明", "/mode rp", false)).await.unwrap();
    let _ = engine.handle_message(mk_msg("c_soft", "u1", "阿明", "/join 阿明 | 侦探", false)).await.unwrap();
    assert!(engine.will_likely_resolve_turn(Some("g1"), "c_soft", "u1", "...我慢慢退后", false));
    assert_eq!(
        engine.precheck_roleplay_ooc(Some("g1"), "c_soft", "u1", "阿明", "...外卖到了先吃", false).await,
        OocPrecheck::IgnoreOoc
    );
    assert_eq!(
        engine.precheck_roleplay_ooc(Some("g1"), "c_soft", "u1", "阿明", "...我慢慢退后", false).await,
        OocPrecheck::Allow
    );
    let bare_gt = engine.handle_message(mk_msg("c_soft", "u1", "阿明", ">", false)).await.unwrap();
    assert!(matches!(bare_gt, EngineResponse::Ignore));

    // 3. Jev rejects out-of-range probability (5.0), accepts explicit nulls, and falls back on partial lore!
    let google_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let google_addr = google_listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Req 1: Out-of-range prob 5.0 (must NOT clamp to 1.0 -> triggers fallback!)
        let (mut s1, _) = google_listener.accept().await.unwrap();
        let _ = read_http_request(&mut s1).await;
        write_http_json(&mut s1, "200 OK", r#"{"answers":{"is_in_game_action":{"probabilities":{"yes":5.0}}},"latency_ms":null}"#).await;
        // Req 2: Partial lore response (only lore_0, missing lore_1 -> triggers fallback!)
        let (mut s2, _) = google_listener.accept().await.unwrap();
        let _ = read_http_request(&mut s2).await;
        write_http_json(&mut s2, "200 OK", r#"{"answers":{"lore_0":{"probabilities":{"yes":0.9}}}}"#).await;
    });

    let bailian_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let bailian_addr = bailian_listener.local_addr().unwrap();
    tokio::spawn(async move {
        // Req 1: Valid response with explicit nulls (latency_ms: null, no: null) -> yes: 0.88 => prob_ooc: 0.12
        let (mut s1, _) = bailian_listener.accept().await.unwrap();
        let _ = read_http_request(&mut s1).await;
        write_http_json(&mut s1, "200 OK", r#"{"answers":{"is_in_game_action":{"probabilities":{"yes":0.88,"no":null}}},"latency_ms":null}"#).await;
        // Req 2: Complete lore_0 + lore_1 response
        let (mut s2, _) = bailian_listener.accept().await.unwrap();
        let _ = read_http_request(&mut s2).await;
        write_http_json(&mut s2, "200 OK", r#"{"answers":{"lore_0":{"probabilities":{"yes":0.10}},"lore_1":{"probabilities":{"yes":0.88}}}}"#).await;
    });

    let jcfg = JevConfig {
        api_key: Some("b-key".into()),
        endpoint: format!("http://{bailian_addr}/systemone"),
        google_endpoint: Some(format!("http://{google_addr}/decisions")),
        google_api_key: Some("g-key".into()),
        timeout_ms: 1500,
        lore_threshold: 0.72,
        ..JevConfig::default()
    };
    let live_gate = SystemOneJevGate::from_config(&jcfg).unwrap();
    let v = live_gate.evaluate_ooc("S", "L", "R", "U", "测试").await.unwrap();
    assert_eq!(v.provider, "bailian");
    assert!((v.prob_ooc - 0.12).abs() < 1e-6);

    let cands = vec![
        LoreCandidate { id: "a".into(), title: "A".into(), snippet: "sa".into() },
        LoreCandidate { id: "b".into(), title: "B".into(), snippet: "sb".into() },
    ];
    let picked = live_gate.rank_hidden_lore("L", "行动", &cands).await;
    assert_eq!(picked, vec!["b"]);
}
