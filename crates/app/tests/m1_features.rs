use std::path::{Path, PathBuf};
use std::sync::Arc;

use tavern_core::{AppConfig, GameEngine, RoomStore, ScenarioCatalog};
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, NarrativeStyle};
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_llm::prompts::build_dm_messages;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn m1_config() -> AppConfig {
    let root = root_dir();
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
        exports_dir: PathBuf::from("/tmp/kook_tavern_m1_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec!["friend_guild".into()],
        allowed_channel_ids: vec![],
        admin_user_ids: vec!["admin_u".into()],
        export_channel_id: Some("wall_chan_1".into()),
        web_bind_addr: Some("0.0.0.0:8787".into()),
        web_public_base_url: Some("https://tavern.example.com".into()),
        daily_max_llm_calls: 2,
        daily_max_cny: 20.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
    }
}

fn gmsg(guild: &str, user_id: &str, name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some(guild.into()),
        channel_id: "m1_chan".into(),
        user_id: user_id.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct: false,
    }
}

#[tokio::test]
async fn m1_whitelist_style_afk_whisper_gm_cost_and_budget_breaker() {
    let cfg = m1_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg.clone(), store.clone(), scenarios.clone(), Arc::new(MockLlmProvider));

    // 1. Unauthorized guild is completely ignored (messages & buttons)
    let blocked = engine
        .handle_message(gmsg("stranger_guild", "u1", "路人", "/start"))
        .await
        .unwrap();
    assert!(matches!(blocked, EngineResponse::Ignore));
    let blocked_btn = engine
        .handle_button_in_guild(Some("stranger_guild"), "m1_chan", "u1", "路人", "tavern:status:m1_chan")
        .await
        .unwrap();
    assert!(matches!(blocked_btn, EngineResponse::Ignore));

    // 2. Authorized guild starts room (`u1` becomes GM) & switches to voice style
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/start"))
        .await
        .unwrap();
    let style_resp = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/style voice"))
        .await
        .unwrap();
    assert!(matches!(style_resp, EngineResponse::Notice { theme: CardTheme::Success, .. }));
    assert_eq!(
        store.get_room("m1_chan").unwrap().unwrap().narrative_style,
        NarrativeStyle::Voice
    );

    // 3. Two players join; player 1 submits action (contains 钥匙 -> triggers secret whisper!),
    //    player 2 runs `/afk` -> remaining active players are ready -> auto-resolves Turn 1!
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/join 陆沉 机械师"))
        .await
        .unwrap();
    engine
        .handle_message(gmsg("friend_guild", "u2", "小红", "/join 薇拉 诗人"))
        .await
        .unwrap();

    let wait_r = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/act 检查信使手里的钥匙"))
        .await
        .unwrap();
    assert!(matches!(wait_r, EngineResponse::Notice { .. }));

    let afk_resolve = engine
        .handle_message(gmsg("friend_guild", "u2", "小红", "/afk"))
        .await
        .unwrap();
    assert!(matches!(afk_resolve, EngineResponse::StoryTurn { turn_number: 1, .. }));

    // 4. `/whisper` returns PrivateNotice with saved secret ONLY for u1, empty for u2
    let w1 = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/whisper"))
        .await
        .unwrap();
    match w1 {
        EngineResponse::PrivateNotice { user_id, body, .. } => {
            assert_eq!(user_id, "u1");
            assert!(body.contains("霰弹枪"));
        }
        other => panic!("expected PrivateNotice, got {other:?}"),
    }

    // 5. `/gm` permission check: u2 rejected, u1 (GM) & admin_u allowed
    let denied = engine
        .handle_message(gmsg("friend_guild", "u2", "小红", "/gm hp 陆沉 +5"))
        .await
        .unwrap();
    assert!(matches!(denied, EngineResponse::Notice { theme: CardTheme::Danger, .. }));

    let ok_gm = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/gm hp 陆沉 -3"))
        .await
        .unwrap();
    assert!(matches!(ok_gm, EngineResponse::Notice { theme: CardTheme::Success, .. }));
    assert_eq!(store.get_player("m1_chan", "u1").unwrap().unwrap().hp, 9);

    let ok_clue = engine
        .handle_message(gmsg("friend_guild", "admin_u", "管理", "/gm clue 地下室暗格在17号桶"))
        .await
        .unwrap();
    assert!(matches!(ok_clue, EngineResponse::Notice { theme: CardTheme::Success, .. }));
    assert!(store.get_room("m1_chan").unwrap().unwrap().clues.iter().any(|c| c.contains("17号桶")));

    // 6. `/cost` shows recorded cache hit rate (800/1200 = 66.7%)
    let cost = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/cost"))
        .await
        .unwrap();
    match cost {
        EngineResponse::Notice { body, .. } => {
            assert!(body.contains("66.7%"));
            assert!(body.contains("1 / 2"));
        }
        other => panic!("expected cost Notice, got {other:?}"),
    }

    // 7. `/leave` removes u2 cleanly
    let leave = engine
        .handle_message(gmsg("friend_guild", "u2", "小红", "/leave"))
        .await
        .unwrap();
    assert!(matches!(leave, EngineResponse::Notice { theme: CardTheme::Info, .. }));
    assert!(store.get_player("m1_chan", "u2").unwrap().is_none());

    // 8. Turn 2 hits daily_max_llm_calls=2; Turn 3 triggers circuit breaker!
    let t2 = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/act 继续追问老酒保"))
        .await
        .unwrap();
    assert!(matches!(t2, EngineResponse::StoryTurn { turn_number: 2, .. }));

    let t3_blocked = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/act 前往地下室"))
        .await
        .unwrap();
    match t3_blocked {
        EngineResponse::Notice { title, theme, .. } => {
            assert_eq!(theme, CardTheme::Danger);
            assert!(title.contains("安全熔断"));
        }
        other => panic!("expected circuit breaker Danger notice, got {other:?}"),
    }

    // 9. Static system prefix byte-equality verification for DeepSeek Prefix Caching!
    let sc = scenarios.get("ember_tavern").unwrap();
    let room_now = store.get_room("m1_chan").unwrap().unwrap();
    let mut room_next = room_now.clone();
    room_next.turn_number += 1;
    room_next.location = "地下酒窖".into();
    room_next.clues.push("新线索".into());
    let m1 = build_dm_messages(&cfg.prompts_dir, &sc, &room_now, &[], &[], &[]);
    let m2 = build_dm_messages(&cfg.prompts_dir, &sc, &room_next, &[], &[], &[]);
    assert_eq!(m1[0].role, "system");
    assert_eq!(m1[0].content, m2[0].content);
    assert_eq!(m1[1].role, "user");
}

#[tokio::test]
async fn red_team_regressions_dm_whitelist_button_auth_join_preservation_and_reset_rbac() {
    let mut cfg = m1_config();
    cfg.daily_max_llm_calls = 10;
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), scenarios, Arc::new(MockLlmProvider));

    // P0-1: When guild whitelist is active, stranger DM is ignored while admin DM is allowed
    let stranger_dm = IncomingMessage {
        guild_id: None,
        channel_id: "dm:stranger_u".into(),
        user_id: "stranger_u".into(),
        user_name: "陌生人".into(),
        content: "/help".into(),
        is_direct: true,
    };
    assert!(matches!(
        engine.handle_message(stranger_dm).await.unwrap(),
        EngineResponse::Ignore
    ));
    let admin_dm = IncomingMessage {
        guild_id: None,
        channel_id: "dm:admin_u".into(),
        user_id: "admin_u".into(),
        user_name: "管理员".into(),
        content: "/help".into(),
        is_direct: true,
    };
    assert!(matches!(
        engine.handle_message(admin_dm).await.unwrap(),
        EngineResponse::Notice { .. }
    ));

    // Start room in whitelisted guild; guild_id must be persisted
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/start"))
        .await
        .unwrap();
    assert_eq!(
        store.get_room("m1_chan").unwrap().unwrap().guild_id,
        "friend_guild"
    );

    // P0-2: KOOK button clicks omit guild_id (None) -> authorized via stored room.guild_id!
    let ok_btn = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:status:m1_chan")
        .await
        .unwrap();
    assert!(matches!(ok_btn, EngineResponse::StatusBoard { .. }));

    // Unknown channel button with guild_id=None MUST be ignored under whitelist!
    let unknown_btn = engine
        .handle_button_in_guild(None, "unknown_chan", "u1", "小明", "tavern:status:unknown_chan")
        .await
        .unwrap();
    assert!(matches!(unknown_btn, EngineResponse::Ignore));

    // P1-2 & P2-2: Duplicate character name rejected; re-joining updates profile without clobbering HP/inventory
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/join 陆沉 机械师"))
        .await
        .unwrap();
    let dup_name = engine
        .handle_message(gmsg("friend_guild", "u2", "小红", "/join 陆沉 盗贼"))
        .await
        .unwrap();
    assert!(matches!(
        dup_name,
        EngineResponse::Notice { theme: CardTheme::Warning, .. }
    ));

    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/gm hp 陆沉 -4"))
        .await
        .unwrap();
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/gm add 陆沉 银钥匙"))
        .await
        .unwrap();
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/join 陆沉 资深机械师 | 冷静"))
        .await
        .unwrap();
    let after_rejoin = store.get_player("m1_chan", "u1").unwrap().unwrap();
    assert_eq!(after_rejoin.role_desc, "资深机械师");
    assert_eq!(after_rejoin.hp, 8);
    assert!(after_rejoin.inventory.contains(&"银钥匙".to_string()));

    // P1-5: Empty /act or unauthorized guild never triggers thinking placeholder
    assert!(!engine.will_likely_resolve_turn(Some("friend_guild"), "m1_chan", "u1", "/act", false));
    assert!(!engine.will_likely_resolve_turn(Some("stranger_guild"), "m1_chan", "u1", "/go", false));

    // P1-1 & P1-3: During Resolving, /gm returns Warning and /afk returns clean confirmation without resolving
    let lease = store.try_begin_resolving("m1_chan").unwrap().unwrap();
    let gm_busy = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/gm hp 陆沉 +2"))
        .await
        .unwrap();
    assert!(matches!(
        gm_busy,
        EngineResponse::Notice { theme: CardTheme::Warning, .. }
    ));
    let afk_busy = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/afk"))
        .await
        .unwrap();
    assert!(matches!(
        afk_busy,
        EngineResponse::Notice { theme: CardTheme::Info, .. }
    ));
    store
        .abort_resolving("m1_chan", lease.expected_version)
        .unwrap();

    // P2-3: Non-GM player cannot /reset; GM can /reset
    let denied_reset = engine
        .handle_message(gmsg("friend_guild", "u2", "小红", "/reset"))
        .await
        .unwrap();
    assert!(matches!(
        denied_reset,
        EngineResponse::Notice { theme: CardTheme::Danger, .. }
    ));
    assert!(store.get_room("m1_chan").unwrap().is_some());

    let ok_reset = engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/reset"))
        .await
        .unwrap();
    assert!(matches!(
        ok_reset,
        EngineResponse::Notice { theme: CardTheme::Success, .. }
    ));
    assert!(store.get_room("m1_chan").unwrap().is_none());
}

#[tokio::test]
async fn zero_code_full_button_gameplay_and_stale_pick_guard() {
    let mut cfg = m1_config();
    cfg.daily_max_llm_calls = 10;
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), scenarios, Arc::new(MockLlmProvider));

    // 1. Open scenario menu via button (brand-new channel with resolved guild_id)
    let menu = engine
        .handle_button_in_guild(
            Some("friend_guild"),
            "m1_chan",
            "u1",
            "小明",
            "tavern:menu:m1_chan",
        )
        .await
        .unwrap();
    assert!(matches!(menu, EngineResponse::ScenarioMenu { .. }));

    // 2. Start ember_tavern via button -> defaults to Roleplay mode + opening_actions saved!
    let start = engine
        .handle_button_in_guild(
            Some("friend_guild"),
            "m1_chan",
            "u1",
            "小明",
            "tavern:start:m1_chan:ember_tavern",
        )
        .await
        .unwrap();
    assert!(matches!(start, EngineResponse::StoryTurn { turn_number: 0, .. }));
    let room0 = store.get_room("m1_chan").unwrap().unwrap();
    assert_eq!(room0.chat_mode, tavern_domain::ChatMode::Roleplay);
    assert_eq!(room0.suggested_actions.len(), 3);

    // Accidental re-click of start button MUST NOT wipe active room!
    let dup_start = engine
        .handle_button_in_guild(
            Some("friend_guild"),
            "m1_chan",
            "u2",
            "路人",
            "tavern:start:m1_chan:fog_manor",
        )
        .await
        .unwrap();
    assert!(matches!(
        dup_start,
        EngineResponse::Notice { theme: CardTheme::Warning, .. }
    ));
    assert_eq!(store.get_room("m1_chan").unwrap().unwrap().scenario_id, "ember_tavern");

    // 3. Quick-join with colliding nicknames automatically suffixes `#2`
    let qj1 = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:quickjoin:m1_chan")
        .await
        .unwrap();
    assert!(matches!(qj1, EngineResponse::Notice { theme: CardTheme::Success, .. }));
    let qj2 = engine
        .handle_button_in_guild(None, "m1_chan", "u2", "小明", "tavern:quickjoin:m1_chan")
        .await
        .unwrap();
    assert!(matches!(qj2, EngineResponse::Notice { theme: CardTheme::Success, .. }));
    assert_eq!(store.get_player("m1_chan", "u2").unwrap().unwrap().character_name, "小明#2");

    // 4. Button roll 2d6 works; forged bad roll expression is ignored
    let roll_ok = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:roll:m1_chan:2d6")
        .await
        .unwrap();
    assert!(matches!(roll_ok, EngineResponse::Notice { .. }));
    let roll_bad = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:roll:m1_chan:999d999")
        .await
        .unwrap();
    assert!(matches!(roll_bad, EngineResponse::Ignore));

    // 5. Player 1 clicks suggestion button ①, Player 2 speaks plain text (zero slash commands!)
    //    -> auto-resolves Turn 1!
    assert!(!engine.will_button_likely_resolve_turn(None, "m1_chan", "u1", "tavern:pick:m1_chan:0:0"));
    let pick1 = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:pick:m1_chan:0:0")
        .await
        .unwrap();
    assert!(matches!(pick1, EngineResponse::Notice { .. }));

    // Player 2 clicking pick button now WOULD trigger auto-resolve!
    assert!(engine.will_button_likely_resolve_turn(None, "m1_chan", "u2", "tavern:pick:m1_chan:0:1"));
    let plain_act = engine
        .handle_message(gmsg("friend_guild", "u2", "小明", "我也凑过去检查信使的钥匙"))
        .await
        .unwrap();
    assert!(matches!(plain_act, EngineResponse::StoryTurn { turn_number: 1, .. }));

    // 6. Stale Turn 0 pick button is rejected cleanly!
    let stale_pick = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:pick:m1_chan:0:0")
        .await
        .unwrap();
    assert!(matches!(
        stale_pick,
        EngineResponse::Notice { theme: CardTheme::Info, .. }
    ));

    // 7. Whisper button returns PrivateNotice ONLY for caller; toggle mode & style buttons flip state
    let wb = engine
        .handle_button_in_guild(None, "m1_chan", "u2", "小明", "tavern:whisper:m1_chan")
        .await
        .unwrap();
    assert!(matches!(wb, EngineResponse::PrivateNotice { .. }));

    engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:toggle_mode:m1_chan")
        .await
        .unwrap();
    assert_eq!(
        store.get_room("m1_chan").unwrap().unwrap().chat_mode,
        tavern_domain::ChatMode::Strict
    );
    engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:toggle_style:m1_chan")
        .await
        .unwrap();
    assert_eq!(
        store.get_room("m1_chan").unwrap().unwrap().narrative_style,
        NarrativeStyle::Voice
    );
}

#[tokio::test]
async fn wave1_three_scenarios_preset_roles_anti_farm_npc_memory_and_wall_export() {
    let mut cfg = m1_config();
    cfg.daily_max_llm_calls = 10;
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), scenarios.clone(), Arc::new(MockLlmProvider));

    // All 3 scenarios load with 10 lore entries, 4 preset roles, and 3 opening actions
    for id in ["ember_tavern", "fog_manor", "midnight_metro"] {
        let sc = scenarios.get(id).expect("built-in scenario exists");
        assert_eq!(sc.worldbook.len(), 10, "{id} should have 10 lore entries");
        assert_eq!(sc.preset_roles.len(), 4, "{id} should have 4 preset roles");
        assert_eq!(sc.opening_actions.len(), 3, "{id} should have 3 opening actions");
    }

    // Start Midnight Metro and pick preset role #0 via button
    engine
        .handle_button_in_guild(
            Some("friend_guild"),
            "m1_chan",
            "u1",
            "小明",
            "tavern:start:m1_chan:midnight_metro",
        )
        .await
        .unwrap();
    let picker = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:rolemenu:m1_chan")
        .await
        .unwrap();
    assert!(matches!(picker, EngineResponse::RolePicker { .. }));

    let joined = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:pickrole:m1_chan:0")
        .await
        .unwrap();
    assert!(matches!(joined, EngineResponse::Notice { theme: CardTheme::Success, .. }));
    let p_initial = store.get_player("m1_chan", "u1").unwrap().unwrap();
    assert_eq!(p_initial.role_desc, "加班程序员");
    assert!(p_initial.inventory.iter().any(|i| i.contains("降噪蓝牙耳机")));

    // Damage player then re-pick another preset role -> MUST NOT restore HP or grant duplicate bonus items!
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "/gm hp 小明 -3"))
        .await
        .unwrap();
    engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:pickrole:m1_chan:2")
        .await
        .unwrap();
    let p_switched = store.get_player("m1_chan", "u1").unwrap().unwrap();
    assert_eq!(p_switched.role_desc, "地铁夜班值班员");
    assert_eq!(p_switched.hp, p_initial.max_hp - 3);
    assert!(!p_switched.inventory.iter().any(|i| i.contains("三角通用钥匙")));

    // Trigger a turn that updates NPC state + secret whisper
    engine
        .handle_message(gmsg("friend_guild", "u1", "小明", "检查车厢里的钥匙"))
        .await
        .unwrap();
    let room_after = store.get_room("m1_chan").unwrap().unwrap();
    assert_eq!(room_after.npc_states.len(), 1);
    assert_eq!(room_after.npc_states[0].name, "老雷恩");

    // Export publishes highlight card to wall_chan_1 and NEVER leaks whispers!
    let exp = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:export:m1_chan")
        .await
        .unwrap();
    match exp {
        EngineResponse::ExportReport {
            turn_count,
            wall_report: Some(wall),
            ..
        } => {
            assert_eq!(turn_count, 1);
            assert_eq!(wall.target_channel_id, "wall_chan_1");
            assert!(wall.body.contains("老雷恩"));
            assert!(!wall.body.contains("霰弹枪"));
        }
        other => panic!("expected ExportReport with WallReport, got {other:?}"),
    }

    // Undo restores NPC states cleanly to empty!
    engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:undo:m1_chan")
        .await
        .unwrap();
    assert!(store.get_room("m1_chan").unwrap().unwrap().npc_states.is_empty());

    // M2 `/web` button returns tokenized read-only dashboard URL!
    let web_resp = engine
        .handle_button_in_guild(None, "m1_chan", "u1", "小明", "tavern:web:m1_chan")
        .await
        .unwrap();
    match web_resp {
        EngineResponse::Notice { body, .. } => {
            assert!(body.contains("https://tavern.example.com/room/m1_chan?token="));
        }
        other => panic!("expected web URL Notice, got {other:?}"),
    }
}
