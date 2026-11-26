use std::path::PathBuf;
use std::sync::Arc;

use tavern_adapter_kook::cards::render_kook_card_json;
use tavern_core::{AppConfig, GameEngine, RoomStore, ScenarioCatalog};
use tavern_domain::{
    DmPatchDelta, EngineResponse, IncomingMessage, StatePatch, detect_consistency_nudge,
    ending_scope_key, merge_patch_delta, validate_scenario,
};
use tavern_infra_llm::prompts::build_dm_messages;
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn v12_config() -> AppConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    AppConfig {
        kook_bot_token: None,
        llm_base_url: "https://mock.local/v1".into(),
        llm_api_key: None,
        llm_model: "mock".into(),
        llm_summary_model: "mock".into(),
        llm_temperature: 0.7,
        llm_max_tokens: 1024,
        db_path: PathBuf::from(":memory:"),
        scenarios_dir: root.join("scenarios"),
        prompts_dir: root.join("prompts"),
        exports_dir: PathBuf::from("/tmp/kook_tavern_v12_exports"),
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
        jev: Default::default(),
    }
}

fn guild_msg(guild: &str, chan: &str, uid: &str, name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some(guild.into()),
        channel_id: chan.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct: false,
    }
}

#[tokio::test]
async fn all_builtin_scenarios_validate_with_maps_factions_and_beats() {
    let cfg = v12_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    for id in ["ember_tavern", "fog_manor", "midnight_metro"] {
        let sc = catalog.get(id).unwrap_or_else(|| panic!("missing {id}"));
        validate_scenario(&sc).unwrap_or_else(|errs| panic!("{id} invalid: {errs:?}"));
        assert!(sc.map_nodes.len() >= 6, "{id} should have >=6 map nodes");
        assert!(sc.map_edges.len() >= 5, "{id} should have >=5 map edges");
        assert!(sc.rules.factions.len() >= 3, "{id} should have >=3 factions");
        assert!(sc.rules.must_hit_beats.len() >= 3, "{id} should have >=3 director beats");
        assert!(!sc.rules.bulletin_name.trim().is_empty());
    }
}

#[tokio::test]
async fn undercurrent_bulletin_triggers_renders_and_rolls_back_on_undo() {
    let cfg = v12_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), scenarios, Arc::new(MockLlmProvider));

    let _ = engine
        .handle_message(guild_msg("g_bul", "ch_bul", "u1", "阿明", "/start ember_tavern"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(guild_msg("g_bul", "ch_bul", "u1", "阿明", "/join 阿明 机械师"))
        .await
        .unwrap();

    // Turns 1..=5: interval=6 so no scheduled bulletin yet
    for step in [
        "检查柜台线索",
        "顺着活板门走下地下酒窖",
        "打量酒窖四周环境",
        "核对酒桶编号",
        "聆听楼上动静",
    ] {
        let _ = engine
            .handle_message(guild_msg("g_bul", "ch_bul", "u1", "阿明", step))
            .await
            .unwrap();
    }
    let room_t5 = store.get_room("ch_bul").unwrap().unwrap();
    assert_eq!(room_t5.turn_number, 5);
    assert!(room_t5.bulletins.is_empty());
    // Player location followed move into cellar
    let p_t5 = store.get_player("ch_bul", "u1").unwrap().unwrap();
    assert!(p_t5.location.contains("酒窖"), "got {}", p_t5.location);

    // Turn 6: hits bulletin_interval=6 -> emits world_bulletin & renders inside KOOK Card
    let t6 = engine
        .handle_message(guild_msg("g_bul", "ch_bul", "u1", "阿明", "细查17号酒桶"))
        .await
        .unwrap();
    match &t6 {
        EngineResponse::StoryTurn { world_bulletin, .. } => {
            let b = world_bulletin.as_ref().expect("expected Turn 6 bulletin");
            assert!(b.masthead.contains("余烬流言板"));
            assert!(!b.headline.is_empty());
        }
        other => panic!("expected StoryTurn, got {other:?}"),
    }
    let card_json = render_kook_card_json("ch_bul", &t6);
    assert!(card_json.contains("📰"));
    assert!(card_json.contains("余烬流言板"));
    assert_eq!(store.get_room("ch_bul").unwrap().unwrap().bulletins.len(), 1);

    // /undo cleanly rolls back the Turn 6 bulletin!
    let _ = engine
        .handle_message(guild_msg("g_bul", "ch_bul", "u1", "阿明", "/undo"))
        .await
        .unwrap();
    let after_undo = store.get_room("ch_bul").unwrap().unwrap();
    assert_eq!(after_undo.turn_number, 5);
    assert!(after_undo.bulletins.is_empty());
}

#[tokio::test]
async fn reincarnation_echoes_persist_within_guild_and_never_leak_across_guilds() {
    let cfg = v12_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg.clone(), store.clone(), scenarios.clone(), Arc::new(MockLlmProvider));

    // Guild A completes a run and triggers ending on Turn 3
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "/start ember_tavern"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "/join 陆沉 机械师"))
        .await
        .unwrap();
    for act in ["调查信使", "推进线索进入下一幕", "启动停机触发结局"] {
        let _ = engine
            .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", act))
            .await
            .unwrap();
    }
    let room_a1 = store.get_room("ch_a1").unwrap().unwrap();
    assert!(room_a1.ended_id.is_some());

    let scope_a = ending_scope_key(Some("guild_alpha"), "ch_a1");
    let echoes_a = store
        .list_world_echoes(&scope_a, "ember_tavern", None, 3)
        .unwrap();
    assert_eq!(echoes_a.len(), 1);
    assert!(echoes_a[0].echo_text.contains("星雾余温"));

    // Active room's own created_at filter prevents self-contamination!
    let before_a1 = room_a1.created_at.to_rfc3339();
    assert!(
        store
            .list_world_echoes(&scope_a, "ember_tavern", Some(&before_a1), 3)
            .unwrap()
            .is_empty()
    );

    // /undo on ended room MUST erase the undone ending's world_echo!
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "/undo"))
        .await
        .unwrap();
    assert!(
        store
            .list_world_echoes(&scope_a, "ember_tavern", None, 3)
            .unwrap()
            .is_empty()
    );

    // Re-trigger ending to persist the real completed echo cleanly
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "再次启动停机触发结局"))
        .await
        .unwrap();
    let echoes_a = store
        .list_world_echoes(&scope_a, "ember_tavern", None, 3)
        .unwrap();
    assert_eq!(echoes_a.len(), 1);

    // Same-channel Run 2: /start sees Run 1 echo, and /undo during Run 2 MUST NOT wipe Run 1 echo!
    let restart_same = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "/start ember_tavern"))
        .await
        .unwrap();
    assert!(matches!(
        restart_same,
        EngineResponse::StoryTurn { ref state_changes, .. }
            if state_changes.iter().any(|s| s.contains("世界线轮回残响"))
    ));
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "/join 陆沉 机械师"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "环顾大厅"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(guild_msg("guild_alpha", "ch_a1", "u1", "陆沉", "/undo"))
        .await
        .unwrap();
    assert_eq!(
        store
            .list_world_echoes(&scope_a, "ember_tavern", None, 3)
            .unwrap()
            .len(),
        1,
        "undo in Run 2 must preserve Run 1 echo in same channel"
    );

    // New table in SAME Guild A sees the reincarnation echo on /start!
    let start_a2 = engine
        .handle_message(guild_msg("guild_alpha", "ch_a2", "u2", "苏晴", "/start ember_tavern"))
        .await
        .unwrap();
    match start_a2 {
        EngineResponse::StoryTurn { state_changes, .. } => {
            assert!(state_changes.iter().any(|s| s.contains("世界线轮回残响")));
        }
        other => panic!("expected StoryTurn, got {other:?}"),
    }

    // New table in DIFFERENT Guild B sees ZERO echoes (strict isolation!)
    let start_b = engine
        .handle_message(guild_msg("guild_beta", "ch_b1", "u3", "陌生人", "/start ember_tavern"))
        .await
        .unwrap();
    match start_b {
        EngineResponse::StoryTurn { state_changes, .. } => {
            assert!(!state_changes.iter().any(|s| s.contains("世界线轮回残响")));
        }
        other => panic!("expected StoryTurn, got {other:?}"),
    }

    // DeepSeek Prefix Cache invariant: echoes go in dynamic user message, NEVER mutating static system message!
    let sc = scenarios.get("ember_tavern").unwrap();
    let room_a2 = store.get_room("ch_a2").unwrap().unwrap();
    let msgs_no_echo = build_dm_messages(&cfg.prompts_dir, &sc, &room_a2, &[], &[], &[], &[]);
    let msgs_with_echo = build_dm_messages(&cfg.prompts_dir, &sc, &room_a2, &[], &[], &[], &echoes_a);
    assert_eq!(msgs_no_echo[0].content, msgs_with_echo[0].content);
    assert!(msgs_with_echo.last().unwrap().content.contains("轮回残响"));
}

#[tokio::test]
async fn director_beats_spotlight_and_consistency_nudge_work_end_to_end() {
    let cfg = v12_config();
    let scenarios = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = scenarios.get("ember_tavern").unwrap();
    let now = chrono::Utc::now();

    let mut room = tavern_domain::RoomState {
        channel_id: "c_dir".into(), guild_id: "g1".into(), scenario_id: "ember_tavern".into(),
        title: sc.name.clone(), location: "余烬酒馆大厅".into(), world_time: "21:30".into(),
        turn_number: 5, state_version: 6, turn_phase: tavern_domain::TurnPhase::Collecting,
        chat_mode: tavern_domain::ChatMode::Roleplay, narrative_style: tavern_domain::NarrativeStyle::Prose,
        gm_user_id: "u1".into(), web_token: "tok".into(), clues: vec![], npc_states: vec![],
        suggested_actions: vec![], current_act: 1, world_flags: vec![], ended_id: None,
        ended_title: None, bulletins: vec![], check_system: Default::default(), last_rest_turn: 0,
        summary: String::new(), summary_upto_turn: 0, active: true, created_at: now, updated_at: now,
    };
    let players = vec![
        tavern_domain::Player {
            channel_id: "c_dir".into(), user_id: "u1".into(), character_name: "话痨甲".into(),
            role_desc: "猎人".into(), traits: vec![], hp: 12, max_hp: 12, san: 10, max_san: 10,
            inventory: vec![], status_note: String::new(), is_afk: false,
            stats: Default::default(), location: "余烬酒馆大厅".into(), dnd: Default::default(),
        },
        tavern_domain::Player {
            channel_id: "c_dir".into(), user_id: "u2".into(), character_name: "内向乙".into(),
            role_desc: "学者".into(), traits: vec![], hp: 12, max_hp: 12, san: 10, max_san: 10,
            inventory: vec![], status_note: String::new(), is_afk: false,
            stats: Default::default(), location: "余烬酒馆大厅".into(), dnd: Default::default(),
        },
    ];
    let recent = vec![
        tavern_domain::TurnRecord {
            id: 1, channel_id: "c_dir".into(), turn_number: 2, batched_input: "".into(),
            scene_title: "t2".into(), narrative: "话痨甲拍案而起，跟酒保争论不休。".into(),
            state_summary: "".into(), location: "余烬酒馆大厅".into(), world_time: "21:10".into(),
            created_at: now,
        },
        tavern_domain::TurnRecord {
            id: 2, channel_id: "c_dir".into(), turn_number: 3, batched_input: "".into(),
            scene_title: "t3".into(), narrative: "话痨甲继续追问白鸦的下落。".into(),
            state_summary: "".into(), location: "余烬酒馆大厅".into(), world_time: "21:20".into(),
            created_at: now,
        },
    ];

    // Next turn is Turn 4 -> triggers Act 1 director beat ("教会暗哨现身") + spotlight on 内向乙 + bulletin window!
    let msgs = build_dm_messages(&cfg.prompts_dir, &sc, &room, &players, &recent, &[], &[]);
    let dyn_msg = &msgs.last().unwrap().content;
    assert!(dyn_msg.contains("聚光灯均衡提醒") && dyn_msg.contains("内向乙"));
    assert!(dyn_msg.contains("导演大纲必达提醒") && dyn_msg.contains("教会暗哨现身"));
    assert!(dyn_msg.contains("幕间暗流号外窗口已开启"));

    // Consistency nudge catches missing item + location drift, and merge_patch_delta repairs cleanly
    let mut reply = tavern_domain::DmStructuredReply {
        scene_title: "地窖探秘".into(),
        narrative: "老雷恩把防雾铜哨递给你，你推门进入地下酿酒窖。".into(),
        private_whispers: vec![], state_patches: vec![], new_clues: vec![], npc_updates: vec![],
        world_time: None, location: Some("余烬酒馆大厅".into()), suggested_actions: vec![],
        suggested_check: None, add_flags: vec![], advance_act: false, trigger_ending_id: None,
        world_bulletin: None, reincarnation_echo: None, grant_inspiration: vec![],
    };
    let plan = detect_consistency_nudge(&sc, &room, &players, &reply, false).unwrap();
    assert!(plan.want_items);
    assert!(plan.want_location);
    merge_patch_delta(
        &mut reply,
        &plan,
        DmPatchDelta {
            state_patches: vec![StatePatch {
                player_name: "内向乙".into(),
                hp_delta: -9, // MUST be ignored since want_vitals == false!
                add_items: vec!["防雾铜哨".into()],
                ..Default::default()
            }],
            location: Some("地下酿酒窖".into()),
            world_bulletin: None,
            redacted_narrative: None,
        },
    );
    assert_eq!(reply.location.as_deref(), Some("地下酿酒窖"));
    assert_eq!(reply.state_patches[0].hp_delta, 0);
    assert_eq!(reply.state_patches[0].add_items, vec!["防雾铜哨"]);

    // Verify parse_patch_delta skips stray non-patch `{}` and extracts real delta!
    let stray = r#"提示：{"note":"ok"} 补丁：{"location":"地下酿酒窖","state_patches":[]}"#;
    let parsed_delta = tavern_infra_llm::parser::parse_patch_delta(stray).unwrap();
    assert_eq!(parsed_delta.location.as_deref(), Some("地下酿酒窖"));
    assert!(tavern_infra_llm::parser::parse_patch_delta("{}").is_none());

    // Verify validate_scenario rejects self-loop and prototype-polluting node IDs
    let mut bad_sc = sc.clone();
    bad_sc.map_nodes[0].id = " constructor ".into();
    bad_sc.map_edges.push(tavern_domain::ScenarioMapEdge {
        from: "wine_cellar".into(),
        to: "wine_cellar".into(),
        label: "自环".into(),
        locked: false,
    });
    assert!(validate_scenario(&bad_sc).is_err());

    // Applying repaired reply moves room & players cleanly
    room.turn_number = 4;
    let mut mut_players = players;
    let _ = tavern_domain::apply_dm_reply(&mut room, &mut mut_players, &reply);
    assert_eq!(room.location, "地下酿酒窖");
    assert!(mut_players[1].inventory.contains(&"防雾铜哨".to_string()));
}
