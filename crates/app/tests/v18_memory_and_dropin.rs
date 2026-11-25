use std::path::PathBuf;
use std::sync::Arc;

use tavern_core::{AppConfig, GameEngine, JevConfig, RoomStore};
use tavern_domain::{EngineResponse, IncomingMessage};
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn test_engine(store: Arc<SqliteRoomStore>) -> GameEngine {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let cfg = AppConfig {
        kook_bot_token: None,
        llm_base_url: "https://mock.local/v1".into(),
        llm_api_key: None,
        llm_model: "mock".into(),
        llm_summary_model: "mock".into(),
        llm_temperature: 1.15,
        llm_max_tokens: 1024,
        db_path: PathBuf::from(":memory:"),
        scenarios_dir: root.join("scenarios"),
        prompts_dir: root.join("prompts"),
        exports_dir: PathBuf::from("/tmp/kook_tavern_v18_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 8,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec!["g_v18".into()],
        allowed_channel_ids: vec![],
        admin_user_ids: vec!["u_admin".into()],
        export_channel_id: None,
        web_bind_addr: Some("127.0.0.1:8787".into()),
        web_public_base_url: Some("https://tavern.example.test".into()),
        daily_max_llm_calls: 0,
        daily_max_cny: 0.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
        jev: JevConfig::default(),
    };
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    GameEngine::new(cfg, store, catalog, Arc::new(MockLlmProvider))
}

fn msg(channel_id: &str, user_id: &str, user_name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g_v18".into()),
        channel_id: channel_id.into(),
        user_id: user_id.into(),
        user_name: user_name.into(),
        content: content.into(),
        is_direct: false,
    }
}

#[tokio::test]
async fn v18_midgame_dropin_briefing_whisper_ledger_and_persisted_live_cards() {
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let engine = test_engine(store.clone());

    // 1. Start room & veteran joins on Turn 0 -> normal public Notice (NOT a drop-in briefing)
    let _ = engine.handle_message(msg("ch_18", "u_vet", "魂芳白", "/start ember_tavern")).await.unwrap();
    let r0 = engine
        .handle_button_in_guild(Some("g_v18"), "ch_18", "u_vet", "魂芳白", "tavern:pickrole:ch_18:0")
        .await
        .unwrap();
    assert!(matches!(r0, EngineResponse::Notice { .. }), "Turn 0 join stays standard Notice");

    // Persist Turn 0 live story card msg_id and verify rebuild works across a fresh engine instance!
    engine.set_persisted_story_card("ch_18", Some(("msg_turn0_abc", 0))).unwrap();
    let restarted = test_engine(store.clone());
    assert_eq!(restarted.get_persisted_story_card("ch_18").unwrap(), Some(("msg_turn0_abc".into(), 0)));
    let rebuilt0 = restarted.rebuild_story_turn_for_archive("ch_18", 0).unwrap().unwrap();
    assert!(matches!(rebuilt0, EngineResponse::StoryTurn { turn_number: 0, .. }));

    // 2. Advance to Turn 1 & inject a summary + private whisper for veteran
    let t1 = engine.handle_message(msg("ch_18", "u_vet", "魂芳白", "/act 我向老雷恩打听断钥匙")).await.unwrap();
    assert!(matches!(t1, EngineResponse::StoryTurn { turn_number: 1, .. }));
    let mut room = store.get_room("ch_18").unwrap().unwrap();
    room.summary = "【编年史封卷（永不覆写）】\n### 卷 1（第 1~1 轮）· 蓝血信使\n魂芳白救下托比，从老雷恩处拿到地下酒窖钥匙。".into();
    room.summary_upto_turn = 1;
    store.upsert_room(&room).unwrap();

    // Also verify Turn 1 persisted story card rebuild survives restart!
    restarted.set_persisted_story_card("ch_18", Some(("msg_turn1_xyz", 1))).unwrap();
    let rebuilt1 = restarted.rebuild_story_turn_for_archive("ch_18", 1).unwrap().unwrap();
    assert!(matches!(rebuilt1, EngineResponse::StoryTurn { turn_number: 1, .. }));

    // 3. Brand-new friend drops in mid-game via `pickrole` -> PrivateNotice briefing + HUD button!
    let drop_pick = engine
        .handle_button_in_guild(Some("g_v18"), "ch_18", "u_new1", "阿杰", "tavern:pickrole:ch_18:1")
        .await
        .unwrap();
    match drop_pick {
        EngineResponse::PrivateNotice { user_id, title, body, actions } => {
            assert_eq!(user_id, "u_new1");
            assert!(title.contains("半路空降"));
            assert!(body.contains("老雷恩处拿到地下酒窖钥匙"));
            assert!(body.contains("魂芳白"));
            assert!(actions.iter().any(|a| a.label.contains("悬浮 HUD") && a.value.contains("https://tavern.example.test/hud/")));
        }
        other => panic!("expected PrivateNotice drop-in briefing, got {other:?}"),
    }

    // 4. Another friend drops in via `quickjoin` -> PrivateNotice briefing with working `rolemenu` CTA!
    let drop_quick = engine
        .handle_button_in_guild(Some("g_v18"), "ch_18", "u_new2", "小鹿", "tavern:quickjoin:ch_18")
        .await
        .unwrap();
    match drop_quick {
        EngineResponse::PrivateNotice { user_id, body, actions, .. } => {
            assert_eq!(user_id, "u_new2");
            assert!(body.contains("魂芳白"));
            let role_btn = actions.iter().find(|a| a.label.contains("选预设职业")).expect("rolemenu button");
            assert_eq!(role_btn.value, "tavern:rolemenu:ch_18");
            // Gate 10 P1-1 round-trip verification: clicking the button ACTUALLY opens RolePicker!
            let menu = engine
                .handle_button_in_guild(Some("g_v18"), "ch_18", "u_new2", "小鹿", &role_btn.value)
                .await
                .unwrap();
            assert!(matches!(menu, EngineResponse::RolePicker { .. }));
        }
        other => panic!("expected PrivateNotice for quickjoin mid-game, got {other:?}"),
    }

    // 5. Third friend drops in via `/join` -> PrivateNotice briefing!
    let drop_join = engine
        .handle_message(msg("ch_18", "u_new3", "老周", "/join 铁面老周 赏金猎人 | 眼尖"))
        .await
        .unwrap();
    assert!(matches!(drop_join, EngineResponse::PrivateNotice { ref user_id, .. } if user_id == "u_new3"));

    // 6. Existing veteran switching role mid-game stays standard `Notice`, NOT a newcomer briefing!
    let vet_switch = engine
        .handle_button_in_guild(Some("g_v18"), "ch_18", "u_vet", "魂芳白", "tavern:pickrole:ch_18:2")
        .await
        .unwrap();
    assert!(matches!(vet_switch, EngineResponse::Notice { ref title, .. } if title.contains("切换")));

    // 7. DM-only room whisper ledger sees all players' whispers, while per-player query isolates strictly!
    let room_whispers = store.list_room_whispers("ch_18", 20).unwrap();
    let vet_whispers = store.list_player_whispers("ch_18", "u_vet", 20).unwrap();
    let new1_whispers = store.list_player_whispers("ch_18", "u_new1", 20).unwrap();
    assert_eq!(room_whispers.len(), vet_whispers.len());
    assert!(new1_whispers.is_empty());

    // 8. After `/reset`, persisted story card record remains readable so dispatcher can archive old buttons
    //    even across a restart, while `rebuild_story_turn_for_archive` gracefully returns `None`!
    let _ = engine.handle_message(msg("ch_18", "u_vet", "魂芳白", "/reset")).await.unwrap();
    assert_eq!(restarted.get_persisted_story_card("ch_18").unwrap(), Some(("msg_turn1_xyz".into(), 1)));
    assert!(restarted.rebuild_story_turn_for_archive("ch_18", 1).unwrap().is_none());
    restarted.set_persisted_story_card("ch_18", None).unwrap();
    assert_eq!(restarted.get_persisted_story_card("ch_18").unwrap(), None);
}
