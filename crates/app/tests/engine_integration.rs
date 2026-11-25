use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tavern_core::{AppConfig, BoxFuture, GameEngine, LlmProvider, RoomStore, ScenarioCatalog};
use tavern_domain::{
    CardTheme, DmStructuredReply, DmTurnOutput, EngineResponse, IncomingMessage, LlmUsage,
    PendingAction, Player, RoomState, Scenario, SummaryOutput, TurnRecord,
};
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_llm::prompts::build_dm_messages;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};
use tokio::sync::Notify;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn test_config() -> AppConfig {
    let root = workspace_root();
    AppConfig {
        kook_bot_token: None,
        llm_base_url: "http://localhost".into(),
        llm_api_key: None,
        llm_model: "mock".into(),
        llm_summary_model: "mock".into(),
        llm_temperature: 0.7,
        llm_max_tokens: 1024,
        db_path: PathBuf::from(":memory:"),
        scenarios_dir: root.join("scenarios"),
        prompts_dir: root.join("prompts"),
        exports_dir: PathBuf::from("/tmp/kook_tavern_ws_test_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec![],
        allowed_channel_ids: vec![],
        admin_user_ids: vec![],
        export_channel_id: None,
        web_bind_addr: None,
        web_public_base_url: None,
        daily_max_llm_calls: 200,
        daily_max_cny: 20.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
    }
}

fn msg(user_id: &str, user_name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g1".into()),
        channel_id: "chan_1".into(),
        user_id: user_id.into(),
        user_name: user_name.into(),
        content: content.into(),
        is_direct: false,
    }
}

struct ControlledSlowLlm {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

impl LlmProvider for ControlledSlowLlm {
    fn generate_turn<'a>(
        &'a self,
        _scenario: &'a Scenario,
        room: &'a RoomState,
        _players: &'a [Player],
        _recent_turns: &'a [TurnRecord],
        _actions: &'a [PendingAction],
    ) -> BoxFuture<'a, Result<DmTurnOutput>> {
        let entered = self.entered.clone();
        let release = self.release.clone();
        let next_turn = room.turn_number + 1;
        Box::pin(async move {
            entered.notify_one();
            release.notified().await;
            Ok(DmTurnOutput {
                reply: DmStructuredReply {
                    scene_title: format!("慢速回应第{next_turn}幕"),
                    narrative: "延迟到达的剧情".into(),
                    private_whispers: vec![],
                    state_patches: vec![],
                    new_clues: vec!["延迟线索".into()],
                    npc_updates: vec![],
                    world_time: None,
                    location: None,
                    suggested_actions: vec![],
                    suggested_check: None,
                    add_flags: vec![],
                    advance_act: false,
                    trigger_ending_id: None,
                },
                usage: LlmUsage::default(),
            })
        })
    }

    fn summarize_turns<'a>(
        &'a self,
        _scenario: &'a Scenario,
        existing_summary: &'a str,
        _turns: &'a [TurnRecord],
    ) -> BoxFuture<'a, Result<SummaryOutput>> {
        let s = existing_summary.to_string();
        Box::pin(async move {
            Ok(SummaryOutput {
                summary: s,
                usage: LlmUsage::default(),
            })
        })
    }
}

#[tokio::test]
async fn end_to_end_multiplayer_and_undo_flow() {
    let cfg = test_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store, scenarios, Arc::new(MockLlmProvider));

    let resp = engine.handle_message(msg("u1", "小明", "/start")).await.unwrap();
    assert!(matches!(resp, EngineResponse::StoryTurn { turn_number: 0, .. }));

    engine
        .handle_message(msg("u1", "小明", "/join 陆沉 机械师 | 眼尖,手巧"))
        .await
        .unwrap();
    engine
        .handle_message(msg("u2", "小红", "/join 薇拉 吟游诗人 | 嘴甜"))
        .await
        .unwrap();

    // Chinese IME punctuation works cleanly too
    let roll = engine
        .handle_message(msg("u1", "小明", "、r 2d6+1 侦查信使"))
        .await
        .unwrap();
    assert!(matches!(roll, EngineResponse::Notice { .. }));

    let wait_resp = engine
        .handle_message(msg("u1", "小明", "/act 检查信使手里的钥匙"))
        .await
        .unwrap();
    assert!(matches!(wait_resp, EngineResponse::Notice { .. }));

    let turn_resp = engine
        .handle_message(msg("u2", "小红", "> 观察老酒保反应"))
        .await
        .unwrap();
    match turn_resp {
        EngineResponse::StoryTurn {
            turn_number,
            direct_messages,
            ..
        } => {
            assert_eq!(turn_number, 1);
            assert_eq!(direct_messages.len(), 1);
        }
        other => panic!("expected StoryTurn, got {other:?}"),
    }

    let undo = engine.handle_message(msg("u1", "小明", "/undo")).await.unwrap();
    assert!(matches!(undo, EngineResponse::Notice { theme: CardTheme::Success, .. }));
}

#[tokio::test]
async fn mid_resolving_action_buffers_to_next_turn_and_reset_invalidates_stale_llm() {
    let cfg = test_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let slow_llm = Arc::new(ControlledSlowLlm {
        entered: entered.clone(),
        release: release.clone(),
    });
    let engine = GameEngine::new(cfg, store.clone(), scenarios, slow_llm);

    engine.handle_message(msg("u1", "小明", "/start")).await.unwrap();
    engine.handle_message(msg("u1", "小明", "/join 陆沉 机械师")).await.unwrap();

    // Start turn 1 resolution in background (will pause inside slow LLM)
    let eng_bg = engine.clone();
    let task = tokio::spawn(async move {
        eng_bg
            .handle_message(msg("u1", "小明", "/act 第一轮推门"))
            .await
            .unwrap()
    });
    entered.notified().await;

    // Duplicate /go while Resolving is rejected cleanly
    let dup = engine.handle_message(msg("u1", "小明", "/go")).await.unwrap();
    assert!(matches!(dup, EngineResponse::Notice { theme: CardTheme::Warning, .. }));

    // Mid-resolving new /act lands safely in turn 2 buffer!
    let buffered = engine
        .handle_message(msg("u1", "小明", "/act 第二轮准备拔剑"))
        .await
        .unwrap();
    assert!(matches!(buffered, EngineResponse::Notice { theme: CardTheme::Info, .. }));
    assert_eq!(store.list_pending_actions("chan_1", 2).unwrap().len(), 1);

    // Release slow LLM -> commits turn 1 AND preserves turn 2 buffer!
    release.notify_one();
    let turn1 = task.await.unwrap();
    assert!(matches!(turn1, EngineResponse::StoryTurn { turn_number: 1, .. }));
    let turn2_buf = store.list_pending_actions("chan_1", 2).unwrap();
    assert_eq!(turn2_buf.len(), 1);
    assert_eq!(turn2_buf[0].action_text, "第二轮准备拔剑");

    // Now test stale LLM response invalidation after `/undo` mid-flight!
    let eng_bg2 = engine.clone();
    let task2 = tokio::spawn(async move { eng_bg2.resolve_turn("chan_1").await.unwrap() });
    entered.notified().await;

    // User triggers `/undo` while turn 2 LLM is in-flight!
    let undo_resp = engine.handle_message(msg("u1", "小明", "/undo")).await.unwrap();
    assert!(matches!(undo_resp, EngineResponse::Notice { theme: CardTheme::Success, .. }));

    // Now the stale LLM for turn 2 finishes -> MUST be discarded cleanly via CAS!
    release.notify_one();
    let stale_resp = task2.await.unwrap();
    assert!(matches!(stale_resp, EngineResponse::Notice { theme: CardTheme::Warning, .. }));
    let room_now = store.get_room("chan_1").unwrap().unwrap();
    assert_eq!(room_now.turn_number, 0);
    assert!(room_now.clues.is_empty());
}

#[test]
fn external_prompts_hot_load_from_disk() {
    let dir = PathBuf::from("/tmp/kook_tavern_prompt_hot_test");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("dm_system.md"),
        "CUSTOM_TONE_MARKER {{scenario_name}} {{genre}} {{dm_style}} {{narrative_style_guide}} {{always_lore}}",
    )
    .unwrap();

    let cfg = test_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let scenario = catalog.get("ember_tavern").unwrap();
    let store = SqliteRoomStore::open_in_memory().unwrap();
    let now = chrono::Utc::now();
    let room = RoomState {
        channel_id: "c1".into(),
        guild_id: "g1".into(),
        scenario_id: "ember_tavern".into(),
        title: "余烬酒馆".into(),
        location: "大厅".into(),
        world_time: "21:00".into(),
        turn_number: 0,
        state_version: 1,
        turn_phase: tavern_domain::TurnPhase::Collecting,
        chat_mode: tavern_domain::ChatMode::Strict,
        narrative_style: tavern_domain::NarrativeStyle::Voice,
        gm_user_id: "u1".into(),
        web_token: "t".into(),
        clues: vec![],
        npc_states: vec![],
        suggested_actions: vec![],
        current_act: 1,
        world_flags: vec![],
        ended_id: None,
        ended_title: None,
        summary: String::new(),
        summary_upto_turn: 0,
        active: true,
        created_at: now,
        updated_at: now,
    };
    store.upsert_room(&room).unwrap();

    let msgs = build_dm_messages(&dir, &scenario, &room, &[], &[], &[]);
    assert!(msgs[0].content.contains("CUSTOM_TONE_MARKER"));
    assert!(msgs[0].content.contains("语音开黑极简档"));
    let _ = fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn raii_guard_recovers_on_cancel_and_mode_change_safe_during_resolving() {
    let cfg = test_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let slow_llm = Arc::new(ControlledSlowLlm {
        entered: entered.clone(),
        release: release.clone(),
    });
    let engine = GameEngine::new(cfg, store.clone(), scenarios, slow_llm);

    engine.handle_message(msg("u1", "小明", "/start")).await.unwrap();
    engine.handle_message(msg("u1", "小明", "/join 陆沉 机械师")).await.unwrap();

    // Start resolving, then ABORT task mid-flight -> RAII guard MUST restore Collecting!
    let eng_cancel = engine.clone();
    let handle = tokio::spawn(async move {
        let _ = eng_cancel
            .handle_message(msg("u1", "小明", "/act 测试取消"))
            .await;
    });
    entered.notified().await;
    assert_eq!(
        store.get_room("chan_1").unwrap().unwrap().turn_phase,
        tavern_domain::TurnPhase::Resolving
    );
    handle.abort();
    let _ = handle.await;
    assert_eq!(
        store.get_room("chan_1").unwrap().unwrap().turn_phase,
        tavern_domain::TurnPhase::Collecting
    );

    // Start resolving again, switch `/mode rp` mid-flight, then finish -> MUST commit cleanly and keep Roleplay!
    let eng_bg = engine.clone();
    let task = tokio::spawn(async move { eng_bg.resolve_turn("chan_1").await.unwrap() });
    entered.notified().await;

    let mode_resp = engine.handle_message(msg("u1", "小明", "/mode rp")).await.unwrap();
    assert!(matches!(mode_resp, EngineResponse::Notice { theme: CardTheme::Success, .. }));

    release.notify_one();
    let turn1 = task.await.unwrap();
    assert!(matches!(turn1, EngineResponse::StoryTurn { turn_number: 1, .. }));

    let room_after = store.get_room("chan_1").unwrap().unwrap();
    assert_eq!(room_after.turn_number, 1);
    assert_eq!(room_after.turn_phase, tavern_domain::TurnPhase::Collecting);
    assert_eq!(room_after.chat_mode, tavern_domain::ChatMode::Roleplay);

    // Cross-channel forged button payload MUST be ignored!
    let forged = engine
        .handle_button("chan_1", "u1", "小明", "tavern:undo:other_chan")
        .await
        .unwrap();
    assert!(matches!(forged, EngineResponse::Ignore));
}

#[tokio::test]
async fn roleplay_mode_accepts_punctuation_leading_ic_and_ignores_double_slash_ooc() {
    let mut cfg = test_config();
    cfg.auto_resolve_all_ready = false;
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store, scenarios, Arc::new(MockLlmProvider));

    engine.handle_message(msg("u1", "小明", "/start")).await.unwrap();
    engine.handle_message(msg("u1", "小明", "/join 陆沉 机械师")).await.unwrap();
    engine.handle_message(msg("u1", "小明", "/mode rp")).await.unwrap();

    let ooc = engine.handle_message(msg("u1", "小明", "// 场外吐槽")).await.unwrap();
    assert!(matches!(ooc, EngineResponse::Ignore));

    let ellipsis_ic = engine
        .handle_message(msg("u1", "小明", "...我默默握紧匕首"))
        .await
        .unwrap();
    assert!(matches!(ellipsis_ic, EngineResponse::Notice { theme: CardTheme::Secondary, .. }));

    let bang_ic = engine
        .handle_message(msg("u1", "小明", "！快趴下躲避"))
        .await
        .unwrap();
    assert!(matches!(bang_ic, EngineResponse::Notice { theme: CardTheme::Secondary, .. }));
}
