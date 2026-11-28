use std::path::PathBuf;
use std::sync::Arc;

use tavern_core::{AppConfig, GameEngine, RoomStore, ScenarioCatalog};
use tavern_domain::{EngineResponse, IncomingMessage};
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn build_m3_engine() -> (GameEngine, Arc<SqliteRoomStore>, Arc<FsScenarioCatalog>) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest_dir.parent().unwrap().parent().unwrap();
    let cfg = AppConfig {
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
        exports_dir: PathBuf::from("/tmp/kook_tavern_m3_exports"),
        recent_turns_window: 8,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec!["g_m3".into()],
        allowed_channel_ids: vec![],
        admin_user_ids: vec!["u_admin".into()],
        export_channel_id: Some("wall_chan".into()),
        web_bind_addr: Some("127.0.0.1:8787".into()),
        web_public_base_url: None,
        daily_max_llm_calls: 200,
        daily_max_cny: 20.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
        jev: Default::default(),
    };
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(
        cfg,
        store.clone(),
        catalog.clone(),
        Arc::new(MockLlmProvider),
    );
    (engine, store, catalog)
}

fn msg(channel: &str, uid: &str, name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g_m3".into()),
        channel_id: channel.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct: false,
    }
}

#[tokio::test]
async fn m3_five_dim_stats_three_tier_check_acts_multi_ending_and_undo() {
    let (engine, store, catalog) = build_m3_engine();

    // 1. Verify all 3 v2 scenarios loaded with theme, 4 statted roles, 3 acts, 3 endings
    for (id, theme) in [
        ("ember_tavern", "ember"),
        ("fog_manor", "fog"),
        ("midnight_metro", "metro"),
    ] {
        let sc = catalog.get(id).expect("scenario present");
        assert_eq!(sc.theme, theme);
        assert_eq!(sc.preset_roles.len(), 4);
        assert_eq!(sc.acts.len(), 3);
        assert_eq!(sc.endings.len(), 3);
    }

    // 2. Start ember_tavern & pick role 0 ("齿轮机械师": agility +2, investigate +2)
    let start_resp = engine
        .handle_message(msg("ch_m3", "u1", "小王", "/start ember_tavern"))
        .await
        .unwrap();
    match start_resp {
        EngineResponse::StoryTurn { act_badge, .. } => {
            assert!(act_badge.unwrap().contains("第 1/3 幕"));
        }
        other => panic!("expected opening StoryTurn, got {other:?}"),
    }

    let role_resp = engine
        .handle_button_in_guild(Some("g_m3"), "ch_m3", "u1", "小王", "tavern:pickrole:ch_m3:0")
        .await
        .unwrap();
    match role_resp {
        EngineResponse::Notice { body, .. } => assert!(body.contains("🔍+2")),
        other => panic!("expected role Notice, got {other:?}"),
    }

    let p = store.get_player("ch_m3", "u1").unwrap().unwrap();
    assert_eq!(p.stats.investigate, 2);
    assert_eq!(p.stats.agility, 2);

    // 3. Click check button & verify deterministic 2d6+2 check attaches to turn 1
    let ck_resp = engine
        .handle_button_in_guild(
            Some("g_m3"),
            "ch_m3",
            "u1",
            "小王",
            "tavern:check:ch_m3:investigate",
        )
        .await
        .unwrap();
    match ck_resp {
        EngineResponse::Notice { body, .. } => {
            assert!(body.contains("侦查检定"));
            assert!(body.contains("2d6+2"));
        }
        other => panic!("expected check Notice, got {other:?}"),
    }

    let pending = store.list_pending_actions("ch_m3", 1).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].roll_notes[0].contains("侦查检定"));

    // 4. Turn 1: premature ending keyword MUST be ignored on Turn 1, while advance_act moves to Act 2!
    let t1 = engine
        .handle_message(msg("ch_m3", "u1", "小王", "我要推进剧情顺便试着触发真结局"))
        .await
        .unwrap();
    match t1 {
        EngineResponse::StoryTurn {
            act_badge,
            ending_reached,
            suggested_check,
            ..
        } => {
            assert!(ending_reached.is_none(), "must block premature Turn-1 ending");
            assert!(act_badge.unwrap().contains("第 2/3 幕"));
            assert!(suggested_check.is_some());
        }
        other => panic!("expected StoryTurn 1, got {other:?}"),
    }
    assert_eq!(store.get_room("ch_m3").unwrap().unwrap().current_act, 2);

    // 5. Turn 2: advance to Act 3 AND try premature true ending -> MUST STILL BE BLOCKED on Turn 2!
    let t2 = engine
        .handle_message(msg("ch_m3", "u1", "小王", "继续推进前往钟楼三层并强行触发真结局"))
        .await
        .unwrap();
    match t2 {
        EngineResponse::StoryTurn {
            ending_reached,
            act_badge,
            ..
        } => {
            assert!(
                ending_reached.is_none(),
                "Turn 2 act-jump MUST NOT bypass premature ending guard!"
            );
            assert!(act_badge.unwrap().contains("第 3/3 幕"));
        }
        other => panic!("expected StoryTurn 2, got {other:?}"),
    }
    assert_eq!(store.get_room("ch_m3").unwrap().unwrap().current_act, 3);

    // 6. Turn 3: now reaches true ending & unlocks gallery!
    let t3 = engine
        .handle_message(msg("ch_m3", "u1", "小王", "插入怀表安全停机达成真结局"))
        .await
        .unwrap();
    match t3 {
        EngineResponse::StoryTurn {
            ending_reached,
            suggested_actions,
            ..
        } => {
            let end = ending_reached.expect("should reach ending in Act 3 / Turn 3");
            assert_eq!(end.id, "ending_dawn");
            assert_eq!(end.title, "晨雾散尽");
            assert!(end.newly_unlocked);
            assert!(suggested_actions.is_empty());
        }
        other => panic!("expected final StoryTurn, got {other:?}"),
    }

    // 7. Ended room: ordinary RP chat silently ignored (no spam!), while explicit commands & buttons are blocked!
    let rp_chat = engine
        .handle_message(msg("ch_m3", "u1", "小王", "哈哈刚才好刺激啊"))
        .await
        .unwrap();
    assert!(matches!(rp_chat, EngineResponse::Ignore));

    for cmd_str in ["/act 继续乱逛", "/check 侦查", "/afk", "/join 新名 剑士", "/roles"] {
        let resp = engine
            .handle_message(msg("ch_m3", "u1", "小王", cmd_str))
            .await
            .unwrap();
        match resp {
            EngineResponse::Notice {
                title, show_actions, ..
            } => {
                assert!(title.contains("晨雾散尽"), "cmd={cmd_str}");
                assert!(!show_actions, "ended notice must not show live action bar");
            }
            other => panic!("expected ended Notice for {cmd_str}, got {other:?}"),
        }
    }
    for btn in [
        "tavern:check:ch_m3:investigate",
        "tavern:pickrole:ch_m3:1",
        "tavern:quickjoin:ch_m3",
        "tavern:afk:ch_m3",
        "tavern:resolve:ch_m3",
    ] {
        let resp = engine
            .handle_button_in_guild(Some("g_m3"), "ch_m3", "u1", "小王", btn)
            .await
            .unwrap();
        match resp {
            EngineResponse::Notice {
                title, show_actions, ..
            } => {
                assert!(title.contains("晨雾散尽"), "btn={btn}");
                assert!(!show_actions);
            }
            other => panic!("expected ended Notice for {btn}, got {other:?}"),
        }
    }
    // `/roll` after ending works as casual roll WITHOUT writing orphan pending_actions!
    let _ = engine
        .handle_message(msg("ch_m3", "u1", "小王", "/roll 2d6"))
        .await
        .unwrap();
    assert!(store.list_pending_actions("ch_m3", 4).unwrap().is_empty());
    // SQLite tx guard rejects direct resolving on ended room!
    assert!(store.try_begin_resolving("ch_m3").unwrap().is_none());

    let gallery = engine
        .handle_message(msg("ch_m3", "u1", "小王", "/endings"))
        .await
        .unwrap();
    match gallery {
        EngineResponse::Notice { body, .. } => {
            assert!(body.contains("1 / 13"));
            assert!(body.contains("晨雾散尽"));
        }
        other => panic!("expected endings Notice, got {other:?}"),
    }

    // Non-GM cannot restart ended room via `/start`, while GM CAN cleanly swap scenarios!
    let non_gm_start = engine
        .handle_message(msg("ch_m3", "u2", "路人", "/start fog_manor"))
        .await
        .unwrap();
    match non_gm_start {
        EngineResponse::Notice {
            title, show_actions, ..
        } => {
            assert!(title.contains("房主"));
            assert!(!show_actions);
        }
        other => panic!("expected non-GM start guard, got {other:?}"),
    }

    // `/undo` restores room back to pre-ending Turn 2 so party can rewrite fate!
    let undo_resp = engine
        .handle_message(msg("ch_m3", "u1", "小王", "/undo"))
        .await
        .unwrap();
    assert!(matches!(undo_resp, EngineResponse::Notice { .. }));
    let restored = store.get_room("ch_m3").unwrap().unwrap();
    assert_eq!(restored.turn_number, 2);
    assert!(restored.ended_id.is_none());

    // After re-triggering ending, GM clicking "换剧本 -> 开启新剧本" smoothly resets and starts new game!
    let _ = engine
        .handle_message(msg("ch_m3", "u1", "小王", "再次达成真结局"))
        .await
        .unwrap();
    let gm_swap = engine
        .handle_button_in_guild(
            Some("g_m3"),
            "ch_m3",
            "u1",
            "小王",
            "tavern:start:ch_m3:fog_manor",
        )
        .await
        .unwrap();
    assert!(matches!(gm_swap, EngineResponse::StoryTurn { .. }));
    assert_eq!(store.get_room("ch_m3").unwrap().unwrap().scenario_id, "fog_manor");
}

#[tokio::test]
async fn m3_red_team_regressions_afk_vacuous_truth_dm_gallery_isolation_and_atomic_roles() {
    let (engine, store, _) = build_m3_engine();

    // 1. All-AFK party MUST NOT trigger Vacuous Truth `party_down` on Turn 1!
    let _ = engine
        .handle_message(msg("ch_afk", "u1", "甲", "/start ember_tavern"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(msg("ch_afk", "u1", "甲", "/join 陆沉 机械师"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(msg("ch_afk", "u2", "乙", "/join 薇拉 诗人"))
        .await
        .unwrap();
    // Submit action for u1, then mark BOTH players AFK, then force `/go` with premature ending text
    let _ = engine
        .handle_message(msg("ch_afk", "u1", "甲", "试着在第一回合触发真结局"))
        .await
        .unwrap();
    store.set_player_afk("ch_afk", "u1", true).unwrap();
    store.set_player_afk("ch_afk", "u2", true).unwrap();
    let t1 = engine
        .handle_message(msg("ch_afk", "u1", "甲", "/go"))
        .await
        .unwrap();
    match t1 {
        EngineResponse::StoryTurn { ending_reached, .. } => {
            assert!(
                ending_reached.is_none(),
                "all-AFK party MUST NOT vacuous-truth bypass Turn-1 ending guard!"
            );
        }
        other => panic!("expected StoryTurn, got {other:?}"),
    }

    // 2. Empty guild_id / DM channels MUST NEVER share a global "local" ending gallery bucket!
    let dm_a = tavern_domain::EndingUnlock {
        guild_id: tavern_domain::ending_scope_key(None, "dm:user_a"),
        scenario_id: "ember_tavern".into(),
        ending_id: "ending_dawn".into(),
        ending_title: "晨雾散尽".into(),
        ending_kind: "true".into(),
        turn_count: 5,
        party_names: "私聊秘密角色A".into(),
        unlocked_at: chrono::Utc::now(),
    };
    assert!(store.record_ending_unlock(&dm_a).unwrap());
    // Fail-closed on blank key
    let mut blank = dm_a.clone();
    blank.guild_id = "   ".into();
    assert!(!store.record_ending_unlock(&blank).unwrap());
    assert!(store.list_ending_unlocks("").unwrap().is_empty());

    // User B DM `/endings` MUST NOT see User A's DM unlock or character name!
    let dm_b_endings = engine
        .handle_message(IncomingMessage {
            guild_id: None,
            channel_id: "dm:user_b".into(),
            user_id: "u_admin".into(),
            user_name: "AdminB".into(),
            content: "/endings".into(),
            is_direct: true,
        })
        .await
        .unwrap();
    match dm_b_endings {
        EngineResponse::Notice { body, .. } => {
            assert!(body.contains("0 / 13"));
            assert!(!body.contains("私聊秘密角色A"));
            assert!(!body.contains("晨雾散尽"));
        }
        other => panic!("expected Notice, got {other:?}"),
    }

    // 3. Switching preset role preserves player's custom `character_name` AND existing HP/inventory!
    let _ = engine
        .handle_message(msg("ch_role", "u1", "原昵称", "/start ember_tavern"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(msg("ch_role", "u1", "原昵称", "/join 专属大名 剑客"))
        .await
        .unwrap();
    let _ = engine
        .handle_button_in_guild(Some("g_m3"), "ch_role", "u1", "原昵称", "tavern:pickrole:ch_role:0")
        .await
        .unwrap();
    let p = store.get_player("ch_role", "u1").unwrap().unwrap();
    assert_eq!(p.character_name, "专属大名");
    assert_eq!(p.role_desc, "齿轮机械师");
    assert_eq!(p.stats.investigate, 2);
}
