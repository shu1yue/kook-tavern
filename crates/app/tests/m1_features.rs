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
