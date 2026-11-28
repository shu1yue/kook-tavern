use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tavern_adapter_kook::live_cards::render_story_turn_card;
use tavern_adapter_web::{WebState, hud_views::build_player_hud_view};
use tavern_core::{AppConfig, GameEngine, LlmProvider, RoomStore, ScenarioCatalog};
use tavern_domain::{
    AppliedTurnEffects, CheckSystem, ClockKind, ClockUpdate, DmStructuredReply, EngineResponse,
    ForgeScenarioRequest, IncomingMessage, Player, ReplyAuditVerdict, RoomState, Scenario,
    ScenarioAct, ScenarioEnding, ScenarioMapEdge, ScenarioMapNode, StarterCharacterDefaults,
    WorldbookEntry, WorkshopDraft, apply_story_progression, detect_consistency_nudge_with_audit,
    detect_cryptic_riddle_slop, detect_plot_stall_slop, init_room_clocks, merge_patch_delta,
};
use tavern_infra_llm::{
    MockLlmProvider, OpenAiLlmProvider, generator::finalize_forged_scenario,
    parser::parse_patch_delta, prompts::build_dm_messages,
};
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn v20_config() -> AppConfig {
    let root = workspace_root();
    let mut cfg = AppConfig::from_env().unwrap();
    cfg.scenarios_dir = root.join("scenarios");
    cfg.prompts_dir = root.join("prompts");
    cfg.db_path = root.join("data/test_v20.db");
    cfg.exports_dir = root.join("exports");
    cfg.llm_api_key = Some("sk-test-v20".into());
    cfg.llm_model = "deepseek-chat".into();
    cfg
}

fn mk_msg(ch: &str, uid: &str, uname: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g_v20".into()),
        channel_id: ch.into(),
        user_id: uid.into(),
        user_name: uname.into(),
        content: content.into(),
        is_direct: false,
    }
}

#[tokio::test]
async fn v20_clocks_auto_tick_clamp_full_trigger_and_undo_rollback() {
    let cfg = v20_config();
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let engine = GameEngine::new(
        cfg.clone(),
        store.clone(),
        catalog.clone(),
        Arc::new(MockLlmProvider),
    );

    // 1. Starting ember_tavern initializes both clocks in SQLite v13 and exposes them on live HUD + /story
    let start_resp = engine
        .handle_message(mk_msg("ch_clk", "u1", "阿明", "/start ember_tavern"))
        .await
        .unwrap();
    let room0 = store.get_room("ch_clk").unwrap().unwrap();
    assert_eq!(room0.clocks.len(), 2);
    assert_eq!(room0.clocks[0].id, "star_overload");
    assert_eq!(room0.clocks[0].kind, ClockKind::Threat);
    assert_eq!(room0.clocks[0].auto_tick_every, 2);
    assert_eq!(room0.clocks[1].id, "shutdown_progress");
    assert_eq!(room0.clocks[1].kind, ClockKind::Progress);

    let hud = engine.get_live_story_hud("ch_clk").unwrap().unwrap();
    assert_eq!(hud.clock_lines.len(), 2);
    let live_card = render_story_turn_card(&start_resp, Some(&hud), false);
    assert!(live_card.contains("午夜星仪过载"));
    assert!(live_card.contains("安全停机突破"));

    let story_resp = engine
        .handle_message(mk_msg("ch_clk", "u1", "阿明", "/story"))
        .await
        .unwrap();
    let EngineResponse::PrivateNotice { body: story_body, .. } = story_resp else {
        panic!("expected /story PrivateNotice in guild channel");
    };
    assert!(story_body.contains("局势进度钟") && story_body.contains("午夜星仪过载"));

    // 2. Run 2 mock turns: Turn 1 (clue -> progress +1), Turn 2 (even turn -> auto-tick threat +1!), then /undo rolls clocks back!
    let _ = engine
        .handle_message(mk_msg("ch_clk", "u1", "阿明", "/join 阿明 | 侦探"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(mk_msg("ch_clk", "u1", "阿明", "我仔细调查柜台下的钥匙"))
        .await
        .unwrap();
    let room1 = store.get_room("ch_clk").unwrap().unwrap();
    assert_eq!(room1.turn_number, 1);
    assert_eq!(room1.clocks[1].filled, 1, "mock clue discovery ticks progress clock +1");
    assert_eq!(room1.clocks[0].filled, 0, "odd turn should not auto-tick even threat clock");

    let _ = engine
        .handle_message(mk_msg("ch_clk", "u1", "阿明", "我观察窗外浓雾动静"))
        .await
        .unwrap();
    let room2 = store.get_room("ch_clk").unwrap().unwrap();
    assert_eq!(room2.turn_number, 2);
    assert_eq!(
        room2.clocks[0].filled, 1,
        "Turn 2 MUST deterministically auto-tick threat clock +1 even if LLM omitted clock_updates"
    );

    let _ = engine
        .handle_message(mk_msg("ch_clk", "u1", "阿明", "/undo"))
        .await
        .unwrap();
    let restored = store.get_room("ch_clk").unwrap().unwrap();
    assert_eq!(restored.turn_number, 1);
    assert_eq!(restored.clocks[0].filled, 0, "/undo MUST roll threat clock back to Turn 1");
    assert_eq!(restored.clocks[1].filled, 1, "/undo MUST preserve Turn 1 progress clock");

    // 3. Direct unit check of delta clamping (+9 -> +2), no double-tick on same turn, full eruption & prompt injection
    let sc = catalog.get("ember_tavern").unwrap();
    let mut r = restored;
    let mut players = store.list_players("ch_clk").unwrap();
    let big_reply = DmStructuredReply {
        scene_title: "惊动教会".into(),
        turn_outcome: "强行冲击引发星仪警报".into(),
        narrative: "钟楼蓝焰暴涨！".into(),
        private_whispers: vec![],
        state_patches: vec![],
        new_clues: vec![],
        npc_updates: vec![],
        world_time: None,
        location: None,
        suggested_actions: vec![],
        suggested_check: None,
        add_flags: vec![],
        advance_act: false,
        trigger_ending_id: None,
        world_bulletin: None,
        reincarnation_echo: None,
        grant_inspiration: vec![],
        clock_updates: vec![ClockUpdate {
            id: "star_overload".into(),
            delta: 9, // MUST clamp to +2!
            reason: "强闯触发警报".into(),
        }],
    };
    let mut fx = AppliedTurnEffects::default();
    let _ = apply_story_progression(&mut r, &mut players, &sc, &big_reply, 4, &mut fx);
    assert_eq!(
        r.clocks[0].filled, 2,
        "delta +9 must clamp to +2 and NOT double-tick with auto_tick_every on Turn 4"
    );

    // Push threat clock to 5/6 (near-full warning) then 6/6 (mandatory full eruption directive)
    r.clocks[0].filled = 5;
    let near_msgs = build_dm_messages(&cfg.prompts_dir, &sc, &r, &players, &[], &[], &[]);
    assert!(near_msgs[0].content.contains("\"turn_outcome\""));
    assert!(near_msgs[0].content.contains("严禁 AI 顺从症"));
    assert!(near_msgs.last().unwrap().content.contains("局势时钟临近满格预警"));

    let fill_reply = DmStructuredReply {
        clock_updates: vec![ClockUpdate {
            id: "午夜星仪过载".into(), // match by Chinese name too!
            delta: 1,
            reason: "午夜将至".into(),
        }],
        ..big_reply
    };
    let mut fx2 = AppliedTurnEffects::default();
    let _ = apply_story_progression(&mut r, &mut players, &sc, &fill_reply, 5, &mut fx2);
    assert_eq!(r.clocks[0].filled, 6);
    assert!(r.clocks[0].triggered);
    assert!(fx2.state_changes.iter().any(|s| s.contains("威胁钟已满")));

    let full_msgs = build_dm_messages(&cfg.prompts_dir, &sc, &r, &players, &[], &[], &[]);
    assert!(full_msgs.last().unwrap().content.contains("时钟满格·强制爆发指令"));
}

#[test]
fn v20_d100_act_milestone_awards_investigator_breather_not_levelup() {
    let cfg = v20_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("fog_manor").unwrap();
    let now = chrono::Utc::now();

    let mut room = RoomState {
        channel_id: "ch_coc".into(),
        guild_id: "g1".into(),
        scenario_id: sc.id.clone(),
        title: sc.name.clone(),
        location: "二楼长画廊".into(),
        world_time: "22:30".into(),
        turn_number: 4,
        state_version: 5,
        turn_phase: tavern_domain::TurnPhase::Collecting,
        chat_mode: tavern_domain::ChatMode::Roleplay,
        narrative_style: tavern_domain::NarrativeStyle::Prose,
        manual_resolve: false,
        gm_user_id: "u1".into(),
        web_token: "tok".into(),
        clues: vec![],
        npc_states: vec![],
        suggested_actions: vec![],
        current_act: 1,
        world_flags: vec![],
        ended_id: None,
        ended_title: None,
        bulletins: vec![],
        clocks: init_room_clocks(&sc),
        check_system: CheckSystem::D100,
        last_rest_turn: 0,
        summary: String::new(),
        summary_upto_turn: 0,
        active: true,
        created_at: now,
        updated_at: now,
    };
    let mut players = vec![Player {
        channel_id: "ch_coc".into(),
        user_id: "u1".into(),
        character_name: "林医生".into(),
        role_desc: "法医调查员".into(),
        traits: vec![],
        hp: 10,
        max_hp: 10,
        san: 6,
        max_san: 10,
        inventory: vec![],
        status_note: String::new(),
        is_afk: false,
        stats: Default::default(),
        location: "二楼长画廊".into(),
        dnd: Default::default(),
    }];
    let reply = DmStructuredReply {
        scene_title: "步入二楼画廊".into(),
        turn_outcome: "进入二楼长画廊，正式推进到第二幕".into(),
        narrative: "你们踏上橡木楼梯，二楼画廊的玻璃倒影令人脊背发凉。".into(),
        private_whispers: vec![],
        state_patches: vec![],
        new_clues: vec![],
        npc_updates: vec![],
        world_time: None,
        location: Some("二楼长画廊".into()),
        suggested_actions: vec![],
        suggested_check: None,
        add_flags: vec![],
        advance_act: true,
        trigger_ending_id: None,
        world_bulletin: None,
        reincarnation_echo: None,
        grant_inspiration: vec![],
        clock_updates: vec![],
    };

    let mut fx = AppliedTurnEffects::default();
    let _ = apply_story_progression(&mut room, &mut players, &sc, &reply, 5, &mut fx);
    assert_eq!(room.current_act, 2);
    assert_eq!(players[0].dnd.level, 1, "d100 mode MUST NOT turn CoC into RPG leveling");
    assert_eq!(players[0].dnd.pending_levelups, 0);
    assert_eq!(players[0].san, 8, "d100 act breather should restore +2 SAN");
    assert!(players[0].dnd.inspiration, "d100 act breather grants investigator intuition");
    assert!(fx.state_changes.iter().any(|s| s.contains("阶段领悟（d100 调查喘息）")));
    assert!(!fx.state_changes.iter().any(|s| s.contains("升级啦")));
}

async fn read_http_req(socket: &mut tokio::net::TcpStream) -> String {
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
            let head = String::from_utf8_lossy(&buf[..pos]);
            let cl = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            expected_total = Some(pos + 4 + cl);
        }
        if let Some(tot) = expected_total
            && buf.len() >= tot
        {
            break;
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn v20_anti_riddle_and_anti_stall_gate_rewrites_cleanly() {
    let cfg = v20_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();

    let room = RoomState {
        channel_id: "ch_riddle".into(),
        guild_id: "g1".into(),
        scenario_id: sc.id.clone(),
        title: sc.name.clone(),
        location: sc.initial_location.clone(),
        world_time: sc.initial_time.clone(),
        turn_number: 3,
        state_version: 4,
        turn_phase: tavern_domain::TurnPhase::Collecting,
        chat_mode: tavern_domain::ChatMode::Roleplay,
        narrative_style: tavern_domain::NarrativeStyle::Prose,
        manual_resolve: false,
        gm_user_id: "u1".into(),
        web_token: "tok".into(),
        clues: vec![],
        npc_states: vec![],
        suggested_actions: vec![],
        current_act: 1,
        world_flags: vec![],
        ended_id: None,
        ended_title: None,
        bulletins: vec![],
        clocks: init_room_clocks(&sc),
        check_system: CheckSystem::TwoD6,
        last_rest_turn: 0,
        summary: String::new(),
        summary_upto_turn: 0,
        active: true,
        created_at: now,
        updated_at: now,
    };
    let players = vec![Player {
        channel_id: "ch_riddle".into(),
        user_id: "u1".into(),
        character_name: "阿明".into(),
        role_desc: "侦探".into(),
        traits: vec![],
        hp: 12,
        max_hp: 12,
        san: 10,
        max_san: 10,
        inventory: vec![],
        status_note: String::new(),
        is_afk: false,
        stats: Default::default(),
        location: sc.initial_location.clone(),
        dnd: Default::default(),
    }];

    // 1. Local anti-riddle detector catches cryptic slop, passes plain Chinese!
    let cryptic = "老雷恩只是淡淡一笑，眼神讳莫如深……他欲言又止地摆摆手：“有些事知道太多对你没好处……时机未到，时候到了你自然会懂……谜底藏在浓雾尽头……”";
    let plain = "老雷恩把酒杯重重墩在桌上，压低声音直接说：‘别瞎猜了，半截钥匙是白鸦十年前留下的，另一半就在我地下酒窖的17号旧酒桶夹层里，教会修女艾琳今晚已经在码头堵路了。’";
    assert!(detect_cryptic_riddle_slop(cryptic));
    assert!(!detect_cryptic_riddle_slop(plain));

    // 2. Local anti-stall detector catches bloated 230+ char empty-progress filler
    let bloated_stall = DmStructuredReply {
        scene_title: "沉默的酒馆".into(),
        turn_outcome: String::new(),
        narrative: "空气仿佛凝固了一般，沉默蔓延在整个余烬酒馆大厅里。摇曳的烛火把每个人的影子拉得很长，却无人开口打破这份压抑的寂静。窗外的海雾一阵阵拍打玻璃，炉火偶尔发出细微的噼啪声，桌上的木纹在阴影里显得模糊不清。周围几位旅人彼此对视了一眼，又各自低下头端起杯子，一切都显得扑朔迷离，似乎隐藏着什么不为人知的秘密，但此刻谁也没有率先迈出一步，大家只是静静等待着夜色继续加深，时间一分一秒地悄然流逝。壁炉边腾起一缕灰烟，很快又被门缝钻进来的冷风吹散了。".into(),
        private_whispers: vec![], state_patches: vec![], new_clues: vec![], npc_updates: vec![],
        world_time: None, location: Some(room.location.clone()), suggested_actions: vec![],
        suggested_check: None, add_flags: vec![], advance_act: false, trigger_ending_id: None,
        world_bulletin: None, reincarnation_echo: None, grant_inspiration: vec![], clock_updates: vec![],
    };
    assert!(detect_plot_stall_slop(&bloated_stall, &room));

    // 3. System One audit verdict triggers `want_narrative_rewrite` + `want_clocks` and merges cleanly!
    let mut jev_riddle_reply = DmStructuredReply {
        scene_title: "雾中低语".into(),
        turn_outcome: "试探酒保".into(),
        narrative: "老雷恩盯着你看了很久，慢慢把抹布搭回肩上，什么也没明说，只是摇了摇头转过身去。".into(),
        private_whispers: vec![], state_patches: vec![], new_clues: vec![], npc_updates: vec![],
        world_time: None, location: Some(room.location.clone()), suggested_actions: vec![],
        suggested_check: None, add_flags: vec![], advance_act: false, trigger_ending_id: None,
        world_bulletin: None, reincarnation_echo: None, grant_inspiration: vec![], clock_updates: vec![],
    };
    let jev_verdict = ReplyAuditVerdict {
        prob_missed_vitals: 0.01,
        prob_missed_item: 0.01,
        prob_whisper_leak: 0.0,
        prob_cryptic_riddle: 0.91,
        prob_plot_stall: 0.87,
        provider: "bailian",
        latency_ms: 95.0,
    };
    let plan = detect_consistency_nudge_with_audit(
        &sc, &room, &players, &jev_riddle_reply, false, Some(&jev_verdict), 0.80,
    )
    .expect("JEV riddle/stall verdict MUST trigger rewrite plan");
    assert!(plan.want_narrative_rewrite);
    assert!(!plan.want_narrative_redact);
    assert!(plan.want_clocks);
    let patch_delta = parse_patch_delta(
        r#"{"state_patches":[],"location":null,"world_bulletin":null,"redacted_narrative":"老雷恩放下抹布直截了当地告诉你：白鸦真名叫卡修斯，十年前跟我一起修了旧钟楼里的潮汐星仪，另一半钥匙就藏在我地下酒窖的17号桶里！","clock_updates":[{"id":"shutdown_progress","delta":1,"reason":"老雷恩摊牌交代位置"}]}"#,
    )
    .expect("patch delta with clock_updates must parse");
    merge_patch_delta(&mut jev_riddle_reply, &plan, patch_delta);
    assert!(jev_riddle_reply.narrative.contains("白鸦真名叫卡修斯"));
    assert_eq!(jev_riddle_reply.clock_updates.len(), 1);
    assert_eq!(jev_riddle_reply.clock_updates[0].id, "shutdown_progress");

    // 4. End-to-end OpenAiLlmProvider auto-rewrite against local mock LLM server (zero JEV required!)
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut s1, _) = listener.accept().await.unwrap();
        let _req1 = read_http_req(&mut s1).await;
        let bad_draft = json!({
            "scene_title": "神秘的笑容",
            "turn_outcome": "",
            "narrative": cryptic,
            "private_whispers": [],
            "state_patches": [],
            "new_clues": [],
            "npc_updates": [],
            "suggested_actions": ["继续追问"],
            "clock_updates": []
        });
        let r1 = json!({
            "choices": [{ "message": { "content": bad_draft.to_string() } }],
            "usage": { "prompt_tokens": 500, "completion_tokens": 120 }
        })
        .to_string();
        let http1 = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            r1.len(),
            r1
        );
        s1.write_all(http1.as_bytes()).await.unwrap();

        let (mut s2, _) = listener.accept().await.unwrap();
        let req2 = read_http_req(&mut s2).await;
        let fix = json!({
            "state_patches": [],
            "location": null,
            "world_bulletin": null,
            "redacted_narrative": plain,
            "clock_updates": [{"id": "shutdown_progress", "delta": 1, "reason": "问出另一半钥匙去向"}]
        });
        let r2 = json!({
            "choices": [{ "message": { "content": fix.to_string() } }],
            "usage": { "prompt_tokens": 600, "completion_tokens": 130 }
        })
        .to_string();
        let http2 = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            r2.len(),
            r2
        );
        s2.write_all(http2.as_bytes()).await.unwrap();
        req2
    });

    let mut llm_cfg = cfg.clone();
    llm_cfg.llm_base_url = format!("http://{addr}/v1");
    let provider = OpenAiLlmProvider::from_config(&llm_cfg).unwrap();
    let out = provider
        .generate_turn(&sc, &room, &players, &[], &[], &[], &[], true)
        .await
        .unwrap();
    let second_prompt = server.await.unwrap();
    assert!(second_prompt.contains("反谜语/反水剧情拦截"));
    assert_eq!(out.extra_usages.len(), 1);
    assert!(out.reply.narrative.contains("17号旧酒桶夹层里"));
    assert!(!out.reply.narrative.contains("时机未到"));
    assert_eq!(out.reply.clock_updates.len(), 1);
}

#[tokio::test]
async fn v20_builtin_workshop_clocks_and_hud_spoiler_safety() {
    let cfg = v20_config();
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    for id in ["ember_tavern", "fog_manor", "midnight_metro", "sunforge_keep"] {
        let sc = catalog.get(id).expect("built-in scenario should load");
        assert!(
            sc.clocks.iter().any(|c| c.kind == ClockKind::Threat),
            "{id} must define a Threat clock"
        );
        assert!(
            sc.clocks.iter().any(|c| c.kind == ClockKind::Progress),
            "{id} must define a Progress clock"
        );
    }

    // Workshop forged scenario without clocks gets auto-equipped with Threat + Progress clocks
    let mut draft = WorkshopDraft::new("u_ws");
    draft.title = "雾港灯塔".into();
    draft.premise = "灯塔午夜熄灭，渔船接连失联".into();
    draft.act_count = 5;
    let req = ForgeScenarioRequest { draft, base_pack: None };
    let raw_sc = Scenario {
        id: "fog_lighthouse".into(),
        name: "雾港灯塔".into(),
        tagline: "午夜熄灭的灯塔危机".into(),
        genre: "悬疑".into(),
        theme: "fog".into(),
        dm_style: "大白话悬疑".into(),
        initial_location: "港口码头".into(),
        initial_time: "22:00".into(),
        opening_scene: "午夜浓雾笼罩港口，老灯塔突然熄灭，三艘渔船正朝着暗礁驶去，必须立刻查明真相。".into(),
        opening_actions: vec!["前往灯塔".into()],
        player_defaults: StarterCharacterDefaults { max_hp: 12, max_san: 10, starter_items: vec![] },
        preset_roles: vec![tavern_domain::PresetRole {
            name: "侦探".into(), traits: vec![], bonus_items: vec![],
            stats: Default::default(), alignment: String::new(), proficient_skills: vec![], abilities: vec![],
        }],
        acts: (1..=5).map(|i| ScenarioAct {
            id: format!("act{i}"), title: format!("第{i}幕"), goal: format!("目标{i}"),
            advance_flags: vec![], enter_nodes: vec![],
        }).collect(),
        endings: vec![ScenarioEnding { id: "e_true".into(), title: "重燃".into(), kind: "true".into(), summary: "重启灯塔".into() }],
        worldbook: vec![WorldbookEntry {
            id: "wb1".into(), title: "背景".into(), keywords: vec![],
            always_active: true, player_visible: true, priority: 100, content: "灯塔背景".into(),
        }],
        map_nodes: vec![
            ScenarioMapNode { id: "dock".into(), name: "港口码头".into(), x: 20.0, y: 50.0, aliases: vec![], kind: "hub".into() },
            ScenarioMapNode { id: "tower".into(), name: "灯塔顶层".into(), x: 80.0, y: 50.0, aliases: vec![], kind: "danger".into() },
        ],
        map_edges: vec![ScenarioMapEdge { from: "dock".into(), to: "tower".into(), label: "石堤".into(), locked: false }],
        rules: Default::default(),
        clocks: vec![],
        player_brief: Default::default(),
    };
    let forged = finalize_forged_scenario(raw_sc, &req, None).unwrap();
    assert_eq!(forged.clocks.len(), 2);
    assert_eq!(forged.clocks[0].kind, ClockKind::Threat);
    assert_eq!(forged.clocks[0].segments, 8, "5-act forged campaign should get 8-segment threat clock");
    assert_eq!(forged.clocks[1].kind, ClockKind::Progress);
    assert_eq!(forged.clocks[1].segments, 6, "5-act forged campaign should get 6-segment progress clock");

    // Web Floating HUD exposes clocks WITHOUT leaking DM-only `on_full` spoiler text!
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let sc = catalog.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();
    let room = RoomState {
        channel_id: "ch_hud_v20".into(), guild_id: "g1".into(), scenario_id: sc.id.clone(),
        title: sc.name.clone(), location: sc.initial_location.clone(), world_time: sc.initial_time.clone(),
        turn_number: 1, state_version: 2, turn_phase: tavern_domain::TurnPhase::Collecting,
        chat_mode: tavern_domain::ChatMode::Roleplay, narrative_style: tavern_domain::NarrativeStyle::Prose, manual_resolve: false,
        gm_user_id: "u1".into(), web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![], ended_id: None, ended_title: None,
        bulletins: vec![], clocks: init_room_clocks(&sc), check_system: CheckSystem::TwoD6,
        last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
        created_at: now, updated_at: now,
    };
    store.upsert_room(&room).unwrap();
    store.upsert_player(&Player {
        channel_id: "ch_hud_v20".into(), user_id: "u1".into(), character_name: "阿明".into(),
        role_desc: "侦探".into(), traits: vec![], hp: 12, max_hp: 12, san: 10, max_san: 10,
        inventory: vec![], status_note: String::new(), is_afk: false, stats: Default::default(),
        location: sc.initial_location.clone(), dnd: Default::default(),
    }).unwrap();
    let (tx, _) = tokio::sync::broadcast::channel(8);
    let web_state = WebState::new(store.clone(), catalog, tx);
    let hud_view = build_player_hud_view(&web_state, "ch_hud_v20", "u1", room)
        .unwrap()
        .unwrap();
    assert_eq!(hud_view.clocks.len(), 2);
    let hud_json = serde_json::to_string(&hud_view).unwrap();
    assert!(hud_json.contains("午夜星仪过载"));
    assert!(hud_json.contains("安全停机突破"));
    assert!(
        !hud_json.contains("白鸦拍窗急吼求救"),
        "HUD JSON MUST NOT leak DM-only `on_full` spoiler text before eruption!"
    );
}
