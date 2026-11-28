use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use tavern_adapter_kook::cards::{
    render_kook_card_json, render_kook_card_json_with_system, render_private_card_json,
};
use tavern_adapter_kook::live_cards::render_story_turn_card;
use tavern_adapter_web::WebState;
use tavern_adapter_web::hud_views::build_player_hud_view;
use tavern_adapter_web::views::build_room_public_view;
use tavern_core::{AppConfig, GameEngine, RoomStore, ScenarioCatalog, TableStore};
use tavern_domain::{
    CardTheme, CheckSystem, EngineResponse, IncomingMessage, ManagedTable, effective_player_brief,
};
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn test_config() -> AppConfig {
    let root = repo_root();
    let mut cfg = AppConfig::from_env().unwrap();
    cfg.scenarios_dir = root.join("scenarios");
    cfg.prompts_dir = root.join("prompts");
    cfg.exports_dir = std::env::temp_dir().join("tavern_v21_exports");
    cfg.allowed_guild_ids.clear();
    cfg.allowed_channel_ids.clear();
    cfg.admin_user_ids.clear();
    cfg.auto_resolve_all_ready = true;
    cfg
}

fn test_engine_with_cfg(cfg: AppConfig) -> (GameEngine, Arc<SqliteRoomStore>, Arc<FsScenarioCatalog>) {
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let llm = Arc::new(MockLlmProvider);
    let engine = GameEngine::new(cfg, store.clone(), catalog.clone(), llm);
    (engine, store, catalog)
}

fn gmsg(guild: &str, ch: &str, uid: &str, name: &str, text: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some(guild.into()),
        channel_id: ch.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: text.into(),
        is_direct: false,
    }
}

fn assert_kook_card_valid(raw: &str) {
    let v: Value = serde_json::from_str(raw).expect("valid card json");
    let mods = v[0]["modules"].as_array().expect("modules array");
    assert!(mods.len() <= 50, "KOOK max 50 modules");
    for m in mods {
        if m["type"] == "action-group" {
            let elems = m["elements"].as_array().unwrap();
            assert!(elems.len() <= 4, "KOOK max 4 buttons per action-group, got {}", elems.len());
        }
    }
}

#[tokio::test]
async fn v21_dynamic_tables_guardrails_cap_cooldown_and_whitelist_inheritance() {
    let mut cfg = test_config();
    cfg.allowed_guild_ids = vec!["g_main".into()];
    cfg.allowed_channel_ids = vec!["ch_lobby".into()]; // strict lobby-only static whitelist!
    let (engine, store, _) = test_engine_with_cfg(cfg);

    // 1. `/table` without arg returns interactive ScenarioMenu with BOTH table & local-start buttons
    let menu_resp = engine
        .handle_message(gmsg("g_main", "ch_lobby", "u1", "沐涵", "/table"))
        .await
        .unwrap();
    let menu_card = render_kook_card_json("ch_lobby", &menu_resp);
    assert_kook_card_valid(&menu_card);
    assert!(menu_card.contains("tavern:table:ch_lobby:ember_tavern"));
    assert!(menu_card.contains("tavern:start:ch_lobby:ember_tavern"));
    assert!(menu_card.contains("🚪 开独立包间"));

    // 2. `/table ember_tavern` returns CreateTableRequest
    let req = engine
        .handle_message(gmsg("g_main", "ch_lobby", "u1", "沐涵", "/table ember_tavern"))
        .await
        .unwrap();
    match req {
        EngineResponse::CreateTableRequest {
            lobby_channel_id,
            guild_id,
            scenario_id,
            creator_user_id,
            ..
        } => {
            assert_eq!(lobby_channel_id, "ch_lobby");
            assert_eq!(guild_id, "g_main");
            assert_eq!(scenario_id, "ember_tavern");
            assert_eq!(creator_user_id, "u1");
        }
        other => panic!("expected CreateTableRequest, got {other:?}"),
    }

    // Simulate KOOK adapter registering the newly created table channel
    store
        .register_managed_table(&ManagedTable {
            channel_id: "ch_tbl_1".into(),
            guild_id: "g_main".into(),
            lobby_channel_id: "ch_lobby".into(),
            scenario_id: "ember_tavern".into(),
            scenario_name: "余烬酒馆与迷雾委托".into(),
            creator_user_id: "u1".into(),
            created_at: Utc::now(),
        })
        .unwrap();

    // 3. Dynamic table channel bypasses static ALLOWED_CHANNEL_IDS while remaining locked to ALLOWED_GUILD_IDS!
    let start_in_tbl = engine
        .handle_message(gmsg("g_main", "ch_tbl_1", "u1", "沐涵", "/start ember_tavern"))
        .await
        .unwrap();
    assert!(matches!(start_in_tbl, EngineResponse::StoryTurn { turn_number: 0, .. }));

    // 4. Cooldown blocks same user from spamming `/table` immediately
    let cd_resp = engine
        .handle_message(gmsg("g_main", "ch_lobby", "u1", "沐涵", "/table fog_manor"))
        .await
        .unwrap();
    match cd_resp {
        EngineResponse::PrivateNotice { title, .. } => assert!(title.contains("冷却中")),
        other => panic!("expected cooldown PrivateNotice, got {other:?}"),
    }

    // 5. HARD SAFETY GUARD: attempting to close the human lobby channel is strictly BLOCKED!
    let block_ask = engine
        .handle_button_in_guild(Some("g_main"), "ch_lobby", "u1", "沐涵", "tavern:close_table_ask:ch_lobby")
        .await
        .unwrap();
    assert!(matches!(block_ask, EngineResponse::PrivateNotice { ref title, .. } if title.contains("安全护栏")));
    let block_confirm = engine
        .handle_button_in_guild(Some("g_main"), "ch_lobby", "u1", "沐涵", "tavern:close_table_confirm:ch_lobby")
        .await
        .unwrap();
    assert!(matches!(block_confirm, EngineResponse::PrivateNotice { ref title, .. } if title.contains("安全护栏")));

    // 6. Unauthorized teammate cannot close table; creator/GM CAN via 2-step confirm
    let denied = engine
        .handle_button_in_guild(Some("g_main"), "ch_tbl_1", "u_other", "路人", "tavern:close_table_ask:ch_tbl_1")
        .await
        .unwrap();
    assert!(matches!(denied, EngineResponse::PrivateNotice { ref title, .. } if title.contains("仅房主")));

    let ask_ok = engine
        .handle_button_in_guild(Some("g_main"), "ch_tbl_1", "u1", "沐涵", "tavern:close_table_ask:ch_tbl_1")
        .await
        .unwrap();
    assert!(matches!(ask_ok, EngineResponse::PrivateNotice { ref actions, .. }
        if actions.iter().any(|a| a.value == "tavern:close_table_confirm:ch_tbl_1")));

    let closed = engine
        .handle_button_in_guild(Some("g_main"), "ch_tbl_1", "u1", "沐涵", "tavern:close_table_confirm:ch_tbl_1")
        .await
        .unwrap();
    match closed {
        EngineResponse::CloseTableRequest { channel_id, lobby_channel_id, .. } => {
            assert_eq!(channel_id, "ch_tbl_1");
            assert_eq!(lobby_channel_id, "ch_lobby");
        }
        other => panic!("expected CloseTableRequest, got {other:?}"),
    }
    assert!(store.get_managed_table("ch_tbl_1").unwrap().is_none());
    assert!(store.get_room("ch_tbl_1").unwrap().is_none());

    // Cooldown STILL applies even after closing the table (cannot delete-and-spam!)
    assert!(store.user_table_cooldown_remaining("u1", 30).unwrap() > 0);

    // 7. Guild cap of 6 active tables blocks 7th creation
    for i in 1..=6 {
        store
            .register_managed_table(&ManagedTable {
                channel_id: format!("ch_cap_{i}"),
                guild_id: "g_main".into(),
                lobby_channel_id: "ch_lobby".into(),
                scenario_id: "ember_tavern".into(),
                scenario_name: "余烬酒馆".into(),
                creator_user_id: format!("other_u{i}"),
                created_at: Utc::now(),
            })
            .unwrap();
    }
    let cap_resp = engine
        .handle_message(gmsg("g_main", "ch_lobby", "fresh_user", "新玩家", "/table ember_tavern"))
        .await
        .unwrap();
    assert!(matches!(cap_resp, EngineResponse::Notice { ref title, .. } if title.contains("上限")));
}

#[tokio::test]
async fn v21_spoiler_free_briefs_directional_hints_dynamic_dice_and_private_story() {
    let (engine, _, catalog) = test_engine_with_cfg(test_config());

    // 1. All 4 built-in scenarios have handcrafted spoiler-free player_brief & directional hints
    for id in ["ember_tavern", "fog_manor", "midnight_metro", "sunforge_keep"] {
        let sc = catalog.get(id).expect("builtin scenario");
        let b = effective_player_brief(&sc);
        assert!(!b.world_stage.is_empty(), "{id} world_stage");
        assert!(!b.mission.is_empty(), "{id} mission");
        assert!(!b.cast_intro.is_empty(), "{id} cast_intro");
        assert!(!b.ending_paths.is_empty(), "{id} ending_paths");
    }
    // Spoiler check on fog_manor & midnight_metro briefs
    let fog_b = effective_player_brief(&catalog.get("fog_manor").unwrap());
    let fog_all = format!("{} {} {:?}", fog_b.world_stage, fog_b.mission, fog_b.ending_paths);
    assert!(!fog_all.contains("黑曜双生镜") && !fog_all.contains("镜像替身") && !fog_all.contains("左手签名"));
    let metro_b = effective_player_brief(&catalog.get("midnight_metro").unwrap());
    let metro_all = format!("{} {} {:?}", metro_b.world_stage, metro_b.mission, metro_b.ending_paths);
    assert!(!metro_all.contains("第3条") && !metro_all.contains("绿色逃生灯"));

    // 2. Turn 0 opening card embeds compact brief, directional hints, ZERO `tavern:pick:`, and dynamic dice!
    let open = engine
        .handle_message(gmsg("g1", "ch_ux", "u1", "阿明", "/start sunforge_keep"))
        .await
        .unwrap();
    let hud = engine.get_live_story_hud("ch_ux").unwrap().unwrap();
    assert_eq!(hud.check_system, CheckSystem::D20);
    let card = render_story_turn_card(&open, Some(&hud), false);
    assert_kook_card_valid(&card);
    assert!(!card.contains("tavern:pick:"), "MUST NOT emit clickable choice buttons");
    assert!(card.contains("眼下可切入焦点"));
    assert!(card.contains("开局任务书"));
    assert!(card.contains("🎲 掷 d20"), "d20 scenario should show d20 dice button");

    // Legacy `tavern:pick` button degrades cleanly without triggering resolve
    assert!(!engine.will_button_likely_resolve_turn(Some("g1"), "ch_ux", "u1", "tavern:pick:ch_ux:0:0"));
    let legacy = engine
        .handle_button_in_guild(Some("g1"), "ch_ux", "u1", "阿明", "tavern:pick:ch_ux:0:0")
        .await
        .unwrap();
    assert!(matches!(legacy, EngineResponse::PrivateNotice { ref title, .. } if title.contains("自由行动")));

    // Switch to d100 -> Notice action bar & StoryTurn dice button adapt to d100, and Lv.1 hides!
    let _ = engine
        .handle_message(gmsg("g1", "ch_ux", "u1", "阿明", "/rule d100"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(gmsg("g1", "ch_ux", "u1", "阿明", "/join 阿明 调查员"))
        .await
        .unwrap();
    let hud_coc = engine.get_live_story_hud("ch_ux").unwrap().unwrap();
    let card_coc = render_story_turn_card(&open, Some(&hud_coc), false);
    assert!(card_coc.contains("🎲 掷 d100"));
    let notice_coc = render_kook_card_json_with_system(
        "ch_ux",
        &EngineResponse::Notice { title: "测试".into(), body: "正文".into(), theme: CardTheme::Info, show_actions: true },
        CheckSystem::D100,
    );
    assert!(notice_coc.contains("🎲 掷 d100"));

    // StatusBoard & `/me` hide Lv.X in d100 mode, and StatusBoard displays clocks!
    let st = engine
        .handle_message(gmsg("g1", "ch_ux", "u1", "阿明", "/status"))
        .await
        .unwrap();
    let st_card = render_kook_card_json("ch_ux", &st);
    assert_kook_card_valid(&st_card);
    assert!(!st_card.contains("Lv.1"), "d100 StatusBoard MUST hide Lv.1");
    assert!(st_card.contains("局势进度钟"), "StatusBoard MUST display clocks");
    assert!(st_card.contains("tavern:toggle_pause:ch_ux"), "StatusBoard MUST include manual resolve toggle");

    let me = engine
        .handle_message(gmsg("g1", "ch_ux", "u1", "阿明", "/me"))
        .await
        .unwrap();
    assert!(matches!(me, EngineResponse::PrivateNotice { ref body, .. } if !body.contains("Lv.**1**")));

    // Channel `/story` is PrivateNotice (never spams teammates)
    let story = engine
        .handle_message(gmsg("g1", "ch_ux", "u1", "阿明", "/story"))
        .await
        .unwrap();
    assert!(matches!(story, EngineResponse::PrivateNotice { ref user_id, ref body, .. }
        if user_id == "u1" && body.contains("世界舞台") && body.contains("命运分歧预告")));
}

#[tokio::test]
async fn v21_manual_resolve_pause_five_stat_levelup_and_web_views() {
    let (engine, store, catalog) = test_engine_with_cfg(test_config());

    let _ = engine
        .handle_message(gmsg("g1", "ch_rp", "u1", "阿明", "/start ember_tavern"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(gmsg("g1", "ch_rp", "u1", "阿明", "/join 阿明 机械师"))
        .await
        .unwrap();

    // 1. Toggle `/pause` ON -> solo player's message does NOT auto-resolve!
    let p_on = engine
        .handle_message(gmsg("g1", "ch_rp", "u1", "阿明", "/pause"))
        .await
        .unwrap();
    assert!(matches!(p_on, EngineResponse::Notice { ref title, .. } if title.contains("手动结算")));
    assert!(!engine.will_likely_resolve_turn(Some("g1"), "ch_rp", "u1", "先和队友聊两句", false));

    let act_paused = engine
        .handle_message(gmsg("g1", "ch_rp", "u1", "阿明", "我凑近老雷恩小声嘀咕"))
        .await
        .unwrap();
    assert!(matches!(act_paused, EngineResponse::Notice { ref body, .. } if body.contains("手动结算")),
        "paused room MUST buffer action instead of auto-resolving");
    assert_eq!(store.get_room("ch_rp").unwrap().unwrap().turn_number, 0);

    // Explicit `/go` STILL triggers resolution even when paused!
    assert!(engine.will_likely_resolve_turn(Some("g1"), "ch_rp", "u1", "/go", false));
    let resolved = engine
        .handle_message(gmsg("g1", "ch_rp", "u1", "阿明", "/go"))
        .await
        .unwrap();
    assert!(matches!(resolved, EngineResponse::StoryTurn { turn_number: 1, .. }));

    // 2. Grant pending levelup and verify 5-stat clickable `/levelup` buttons
    let mut p = store.get_player("ch_rp", "u1").unwrap().unwrap();
    p.dnd.pending_levelups = 1;
    let before_inv = p.stats.investigate;
    store.upsert_player(&p).unwrap();

    let lv_menu = engine
        .handle_message(gmsg("g1", "ch_rp", "u1", "阿明", "/levelup"))
        .await
        .unwrap();
    match &lv_menu {
        EngineResponse::PrivateNotice { title, body, actions, .. } => {
            assert!(actions.iter().any(|a| a.value == "tavern:lv:ch_rp:investigate"));
            assert!(actions.iter().any(|a| a.value == "tavern:lv:ch_rp:body"));
            assert!(actions.iter().any(|a| a.value == "tavern:lv:ch_rp:insight"));
            let card = render_private_card_json(title, body, actions);
            assert_kook_card_valid(&card);
        }
        other => panic!("expected interactive PrivateNotice for /levelup, got {other:?}"),
    }
    let lv_done = engine
        .handle_button_in_guild(Some("g1"), "ch_rp", "u1", "阿明", "tavern:lv:ch_rp:investigate")
        .await
        .unwrap();
    assert!(matches!(lv_done, EngineResponse::Notice { ref title, .. } if title.contains("成长强化完成")));
    let p_after = store.get_player("ch_rp", "u1").unwrap().unwrap();
    assert_eq!(p_after.stats.investigate, before_inv + 1);
    assert_eq!(p_after.dnd.pending_levelups, 0);

    // 3. Web Dashboard & Floating HUD both expose clocks, player_brief, manual_resolve & hide_level!
    let (tx, _) = tokio::sync::broadcast::channel(8);
    let web_state = WebState::new(store.clone(), catalog, tx);
    let room = store.get_room("ch_rp").unwrap().unwrap();
    let pub_view = build_room_public_view(&web_state, "ch_rp", room.clone()).unwrap();
    assert_eq!(pub_view.clocks.len(), 2);
    assert!(pub_view.manual_resolve);
    assert!(!pub_view.hide_level);
    assert!(!pub_view.player_brief.mission.is_empty());

    let hud_view = build_player_hud_view(&web_state, "ch_rp", "u1", room)
        .unwrap()
        .unwrap();
    assert_eq!(hud_view.clocks.len(), 2);
    assert!(hud_view.manual_resolve);
    assert!(!hud_view.player_brief.world_stage.is_empty());

    // 4. First-principles DM prompt guard check
    let prompt = std::fs::read_to_string(repo_root().join("prompts/dm_system.md")).unwrap();
    assert!(prompt.contains("第一性原则：事实由世界决定，行动由玩家决定"));
    assert!(prompt.contains("开放式方向焦点"));
}
