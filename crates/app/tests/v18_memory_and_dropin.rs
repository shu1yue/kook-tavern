use std::path::PathBuf;
use std::sync::Arc;

use tavern_core::{AppConfig, GameEngine, JevConfig, RoomStore, ScenarioCatalog};
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
    let _ = engine.handle_message(msg("ch_18", "u_vet", "林星河", "/start ember_tavern")).await.unwrap();
    let r0 = engine
        .handle_button_in_guild(Some("g_v18"), "ch_18", "u_vet", "林星河", "tavern:pickrole:ch_18:0")
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
    let t1 = engine.handle_message(msg("ch_18", "u_vet", "林星河", "/act 我向老雷恩打听断钥匙")).await.unwrap();
    assert!(matches!(t1, EngineResponse::StoryTurn { turn_number: 1, .. }));
    let mut room = store.get_room("ch_18").unwrap().unwrap();
    room.summary = "【编年史封卷（永不覆写）】\n### 卷 1（第 1~1 轮）· 蓝血信使\n林星河救下托比，从老雷恩处拿到地下酒窖钥匙。".into();
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
            assert!(body.contains("林星河"));
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
            assert!(body.contains("林星河"));
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
        .handle_button_in_guild(Some("g_v18"), "ch_18", "u_vet", "林星河", "tavern:pickrole:ch_18:2")
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
    let _ = engine.handle_message(msg("ch_18", "u_vet", "林星河", "/reset")).await.unwrap();
    assert_eq!(restarted.get_persisted_story_card("ch_18").unwrap(), Some(("msg_turn1_xyz".into(), 1)));
    assert!(restarted.rebuild_story_turn_for_archive("ch_18", 1).unwrap().is_none());
    restarted.set_persisted_story_card("ch_18", None).unwrap();
    assert_eq!(restarted.get_persisted_story_card("ch_18").unwrap(), None);
}

#[test]
fn v182_map_node_entry_deterministically_advances_act_and_awards_levelup() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("scenarios");
    let catalog = FsScenarioCatalog::load_from_dir(&dir).unwrap();
    let sc = catalog.get("ember_tavern").unwrap();
    let store = SqliteRoomStore::open_in_memory().unwrap();
    let now = chrono::Utc::now();
    let mut room = tavern_domain::RoomState {
        channel_id: "ch_act".into(), guild_id: "g".into(), scenario_id: "ember_tavern".into(),
        title: sc.name.clone(), location: "旧钟楼一二层".into(), world_time: "23:10".into(),
        turn_number: 32, state_version: 1, turn_phase: tavern_domain::TurnPhase::Collecting,
        chat_mode: tavern_domain::ChatMode::Roleplay, narrative_style: tavern_domain::NarrativeStyle::Prose,
        gm_user_id: "u1".into(), web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![], ended_id: None, ended_title: None,
        bulletins: vec![], check_system: tavern_domain::CheckSystem::TwoD6, last_rest_turn: 0,
        summary: String::new(), summary_upto_turn: 0, active: true, created_at: now, updated_at: now,
    };
    let mut players = vec![tavern_domain::Player {
        channel_id: "ch_act".into(), user_id: "u1".into(), character_name: "林星河".into(),
        role_desc: "星象神秘学者".into(), traits: vec![], hp: 11, max_hp: 12, san: 2, max_san: 10,
        inventory: vec![], status_note: String::new(), is_afk: false, stats: Default::default(),
        location: "旧钟楼一二层".into(), dnd: Default::default(),
    }];
    let reply = tavern_domain::DmStructuredReply {
        scene_title: "爬梯".into(), narrative: "抵达二层。".into(), state_patches: vec![],
        private_whispers: vec![], new_clues: vec![], npc_updates: vec![],
        world_time: Some("23:12".into()), location: Some("旧钟楼一二层".into()), suggested_actions: vec![],
        suggested_check: None, add_flags: vec![], advance_act: false, trigger_ending_id: None,
        world_bulletin: None, reincarnation_echo: None, grant_inspiration: vec![],
    };
    let mut effects = tavern_domain::AppliedTurnEffects::default();
    let _ = tavern_domain::apply_story_progression(&mut room, &mut players, &sc, &reply, 33, &mut effects);
    assert_eq!(room.current_act, 2, "entering clocktower_low MUST deterministically advance to Act 2 even when LLM says advance_act: false");
    assert_eq!(players[0].dnd.level, 2);
    assert_eq!(players[0].dnd.pending_levelups, 1);
    assert!(effects.state_changes.iter().any(|s| s.contains("第 2/3 幕")));
    let _ = store;
}
