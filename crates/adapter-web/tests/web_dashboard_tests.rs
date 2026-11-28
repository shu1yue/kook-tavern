use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use reqwest::StatusCode;
use tavern_adapter_web::{BroadcastEventBus, WebState, build_router};
use tavern_core::{EventBus, RoomStore};
use tavern_domain::{
    ChatMode, NarrativeStyle, NpcState, Player, RoomEvent, RoomState, TurnPhase,
};
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};
use tokio::net::TcpListener;

async fn start_test_server() -> (SocketAddr, Arc<SqliteRoomStore>, BroadcastEventBus, WebState) {
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("scenarios");
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&scenarios_dir).unwrap());
    let bus = BroadcastEventBus::new(64);
    let state = WebState::new(store.clone(), scenarios, bus.sender());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = build_router(state.clone()).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, store, bus, state)
}

#[tokio::test]
async fn web_dashboard_enforces_readonly_token_auth_sanitization_and_live_sse() {
    let (addr, store, bus, state) = start_test_server().await;
    let base = format!("http://{addr}");
    let http = reqwest::Client::new();

    // 1. `/` private tavern showcase page and `/healthz` return 200 OK with strict security headers
    let home = http.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    assert_eq!(home.headers().get("x-frame-options").unwrap(), "DENY");
    let home_html = home.text().await.unwrap();
    assert!(home_html.contains("私人 AI 跑团酒馆"));
    assert!(home_html.contains("noindex, nofollow"));

    let hz = http.get(format!("{base}/healthz")).send().await.unwrap();
    assert_eq!(hz.status(), StatusCode::OK);
    assert_eq!(hz.headers().get("x-frame-options").unwrap(), "DENY");
    assert_eq!(hz.headers().get("x-content-type-options").unwrap(), "nosniff");
    assert_eq!(hz.headers().get("referrer-policy").unwrap(), "no-referrer");
    assert!(hz.text().await.unwrap().contains("\"status\":\"ok\""));

    // Seed a room with secrets that MUST NEVER leak in API responses
    let now = Utc::now();
    let room = RoomState {
        channel_id: "chan_web_1".into(),
        guild_id: "secret_guild_999".into(),
        scenario_id: "ember_tavern".into(),
        title: "余烬酒馆".into(),
        location: "大厅".into(),
        world_time: "21:15".into(),
        turn_number: 1,
        state_version: 2,
        turn_phase: TurnPhase::Collecting,
        chat_mode: ChatMode::Roleplay,
        narrative_style: NarrativeStyle::Voice,
        manual_resolve: false,
        gm_user_id: "secret_gm_uid_777".into(),
        web_token: "tok_secret_abc123".into(),
        clues: vec!["03-17 断钥匙".into()],
        npc_states: vec![NpcState {
            name: "老雷恩".into(),
            attitude: "信任".into(),
            status: "递出酒窖钥匙".into(),
            location: "余烬酒馆大厅".into(),
        }],
        suggested_actions: vec![],
        current_act: 2,
        world_flags: vec!["取得老雷恩信任".into()],
        ended_id: None,
        ended_title: None,
        bulletins: vec![tavern_domain::WorldBulletin {
            turn_number: 1,
            masthead: "《余烬流言板》".into(),
            headline: "灰帽修女封锁东栈桥".into(),
            body: "码头守夜人目击教会暗哨在浓雾中集结。".into(),
            faction: "圣光教会".into(),
        }],
        clocks: vec![],
        check_system: tavern_domain::CheckSystem::D20,
        last_rest_turn: 0,
        summary: "【编年史封卷（永不覆写）】\n### 卷 1（第 1~2 轮）· 酒馆密会\n老雷恩交出钥匙。".into(),
        summary_upto_turn: 2,
        active: true,
        created_at: now,
        updated_at: now,
    };
    store.upsert_room(&room).unwrap();
    store
        .upsert_player(&Player {
            channel_id: "chan_web_1".into(),
            user_id: "secret_player_uid_888".into(),
            character_name: "陆沉".into(),
            role_desc: "齿轮机械师".into(),
            traits: vec!["手巧".into()],
            hp: 10,
            max_hp: 12,
            san: 9,
            max_san: 10,
            inventory: vec!["精密黄铜扳手".into()],
            status_note: "精神饱满".into(),
            is_afk: false,
            stats: tavern_domain::CharacterStats {
                body: 0,
                agility: 2,
                investigate: 2,
                social: -1,
                insight: 0,
            },
            location: "地下酿酒窖".into(),
            dnd: tavern_domain::PlayerDndProfile {
                level: 2,
                inspiration: true,
                alignment: "中立善良".into(),
                proficient_skills: vec!["机关".into()],
                personal_secret: "TOP_SECRET_PLAYER_MOTIVE_999".into(),
                ..Default::default()
            },
        })
        .unwrap();

    // 2. Missing / wrong token / unknown room ALL return 403 Forbidden (anti-enumeration)
    for bad_url in [
        format!("{base}/room/chan_web_1"),
        format!("{base}/room/chan_web_1?token=wrong"),
        format!("{base}/room/no_such_room?token=wrong"),
        format!("{base}/api/room/chan_web_1"),
        format!("{base}/api/room/chan_web_1?token=wrong"),
        format!("{base}/api/room/chan_web_1/events?token=wrong"),
    ] {
        let r = http.get(&bad_url).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "url={bad_url}");
    }

    // 3. Strictly read-only: POST to API MUST be rejected (405 Method Not Allowed)
    let post_resp = http
        .post(format!("{base}/api/room/chan_web_1?token=tok_secret_abc123"))
        .send()
        .await
        .unwrap();
    assert_eq!(post_resp.status(), StatusCode::METHOD_NOT_ALLOWED);

    // 4. Valid token serves HTML page and sanitized JSON API (ZERO sensitive fields!)
    let page = http
        .get(format!("{base}/room/chan_web_1?token=tok_secret_abc123"))
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), StatusCode::OK);
    let page_html = page.text().await.unwrap();
    assert!(page_html.contains("实况沙盘看板"));
    assert!(page_html.contains("导出故事长图"));
    assert!(page_html.contains("map-svg"));

    let api = http
        .get(format!("{base}/api/room/chan_web_1?token=tok_secret_abc123"))
        .send()
        .await
        .unwrap();
    assert_eq!(api.status(), StatusCode::OK);
    let body = api.text().await.unwrap();
    assert!(body.contains("余烬酒馆"));
    assert!(body.contains("陆沉"));
    assert!(body.contains("老雷恩"));
    assert!(body.contains("03-17 断钥匙"));
    assert!(body.contains("\"theme\":\"ember\""));
    assert!(body.contains("\"current_act\":2"));
    assert!(body.contains("取得老雷恩信任"));
    assert!(body.contains("\"investigate\":2"));
    assert!(body.contains("\"node_id\":\"wine_cellar\""));
    assert!(body.contains("灰帽修女封锁东栈桥"));
    assert!(body.contains("\"tag\":\"号外\""));
    // Unvisited secret map node (`clocktower_core`) MUST be masked to prevent spoilers!
    assert!(!body.contains("钟楼三层星仪机房"));
    // v1.4 D&D public fields exposed, while player's private `personal_secret` MUST NEVER leak!
    assert!(body.contains("D&D d20"));
    assert!(body.contains("\"level\":2"));
    assert!(body.contains("\"inspiration\":true"));
    assert!(!body.contains("TOP_SECRET_PLAYER_MOTIVE_999"));
    assert!(!body.contains("03-17总钥"));
    // Un-unlocked endings & future acts MUST stay masked and never spoil!
    assert!(body.contains("？？？"));
    assert!(!body.contains("晨雾散尽"));
    assert!(body.contains("未解锁幕次"));
    assert!(!body.contains("午夜潮汐星仪终局"));
    assert!(!body.contains("tok_secret_abc123"));
    assert!(!body.contains("secret_gm_uid_777"));
    assert!(!body.contains("secret_guild_999"));
    assert!(!body.contains("secret_player_uid_888"));

    // 5. SSE stream delivers real-time update event for matching channel only
    let mut sse_resp = http
        .get(format!(
            "{base}/api/room/chan_web_1/events?token=tok_secret_abc123"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(sse_resp.status(), StatusCode::OK);

    // Publish unrelated room event first, then matching room event
    bus.publish(RoomEvent::RoomUpdated {
        channel_id: "other_room".into(),
        turn_number: 99,
    });
    bus.publish(RoomEvent::RoomUpdated {
        channel_id: "chan_web_1".into(),
        turn_number: 2,
    });

    let chunk = tokio::time::timeout(Duration::from_secs(3), sse_resp.chunk())
        .await
        .expect("SSE event arrived in time")
        .unwrap()
        .unwrap();
    let chunk_str = String::from_utf8_lossy(&chunk);
    assert!(chunk_str.contains("event: update"));
    assert!(chunk_str.contains("\"turn\":2"));
    assert!(!chunk_str.contains("99"));

    // 6. RoomReset MUST emit final `reset` frame and forcibly terminate the SSE stream (EOF)!
    bus.publish(RoomEvent::RoomReset {
        channel_id: "chan_web_1".into(),
    });
    let reset_chunk = tokio::time::timeout(Duration::from_secs(3), sse_resp.chunk())
        .await
        .expect("SSE reset event arrived")
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&reset_chunk).contains("event: reset"));

    let eof = tokio::time::timeout(Duration::from_secs(3), sse_resp.chunk())
        .await
        .expect("SSE stream must close cleanly on reset")
        .unwrap();
    assert!(eof.is_none(), "expected server-enforced EOF after RoomReset");
    drop(sse_resp);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(state.available_sse_permits(), 64);

    // 7. Percent-encoded channel_id (e.g., DM room `dm:12345`) authenticates cleanly
    let mut dm_room = room.clone();
    dm_room.channel_id = "dm:12345".into();
    dm_room.web_token = "dm_tok_999".into();
    store.upsert_room(&dm_room).unwrap();
    let dm_api = http
        .get(format!("{base}/api/room/dm%3A12345?token=dm_tok_999"))
        .send()
        .await
        .unwrap();
    assert_eq!(dm_api.status(), StatusCode::OK);
}

#[tokio::test]
async fn player_floating_hud_enforces_hmac_isolation_spoiler_safety_and_live_sse() {
    let (addr, store, bus, state) = start_test_server().await;
    let base = format!("http://{addr}");
    let http = reqwest::Client::new();

    let now = Utc::now();
    let room = RoomState {
        channel_id: "chan_hud_1".into(),
        guild_id: "g_hud".into(),
        scenario_id: "ember_tavern".into(),
        title: "余烬酒馆与迷雾委托".into(),
        location: "余烬酒馆大厅".into(),
        world_time: "21:16".into(),
        turn_number: 2,
        state_version: 3,
        turn_phase: TurnPhase::Collecting,
        chat_mode: ChatMode::Roleplay,
        narrative_style: NarrativeStyle::Prose,
        manual_resolve: false,
        gm_user_id: "gm_hud".into(),
        web_token: "room_tok_hud_1".into(),
        clues: vec!["老雷恩年轻时是旧钟楼守望团首席机械师".into()],
        npc_states: vec![NpcState {
            name: "老雷恩".into(),
            attitude: "信任".into(),
            status: "交出酒窖钥匙与防雾铜哨".into(),
            location: "余烬酒馆大厅".into(),
        }],
        suggested_actions: vec![],
        current_act: 1,
        world_flags: vec!["已获得老雷恩信任".into()],
        ended_id: None,
        ended_title: None,
        bulletins: vec![],
        clocks: vec![],
        check_system: tavern_domain::CheckSystem::TwoD6,
        last_rest_turn: 0,
        summary: "【编年史封卷（永不覆写）】\n### 卷 1（第 1~2 轮）· 酒馆密会\n老雷恩交出钥匙。".into(),
        summary_upto_turn: 2,
        active: true,
        created_at: now,
        updated_at: now,
    };
    store.upsert_room(&room).unwrap();

    let mut seeded_players = Vec::new();
    for (uid, name, role) in [
        ("u_alice", "林星河", "星象神秘学者"),
        ("u_bob", "铁手阿凯", "齿轮机械师"),
    ] {
        let p = Player {
            channel_id: "chan_hud_1".into(),
            user_id: uid.into(),
            character_name: name.into(),
            role_desc: role.into(),
            traits: vec!["博学".into()],
            hp: 12,
            max_hp: 12,
            san: 10,
            max_san: 10,
            inventory: vec!["星图黄铜罗盘".into()],
            status_note: "精神集中".into(),
            is_afk: false,
            stats: Default::default(),
            location: "余烬酒馆大厅".into(),
            dnd: Default::default(),
        };
        store.upsert_player(&p).unwrap();
        seeded_players.push(p);
    }
    let lease = store.try_begin_resolving("chan_hud_1").unwrap().unwrap();
    let snap = tavern_domain::RoomSnapshot {
        room: lease.room.clone(),
        players: seeded_players.clone(),
        pending_actions: vec![],
    };
    let mut updated_room = lease.room.clone();
    updated_room.turn_number = 2;
    updated_room.turn_phase = TurnPhase::Collecting;
    let turn_rec = tavern_domain::TurnRecord {
        id: 0,
        channel_id: "chan_hud_1".into(),
        turn_number: 2,
        batched_input: "林星河：查看柜台".into(),
        scene_title: "独眼酒保的实话".into(),
        narrative: "老雷恩推过来一把黄铜钥匙，门外传来三声轻敲。".into(),
        state_summary: "获得酒窖钥匙".into(),
        location: "余烬酒馆大厅".into(),
        world_time: "21:16".into(),
        created_at: now,
    };
    let whispers = vec![
        tavern_domain::DirectMessageOut {
            user_id: "u_alice".into(),
            player_name: "林星河".into(),
            content: "ALICE_ONLY_BELLTOWER_SECRET".into(),
        },
        tavern_domain::DirectMessageOut {
            user_id: "u_bob".into(),
            player_name: "铁手阿凯".into(),
            content: "BOB_ONLY_CELLAR_SECRET".into(),
        },
    ];
    assert!(
        store
            .commit_turn_cas(
                lease.expected_version,
                &snap,
                &updated_room,
                &seeded_players,
                &turn_rec,
                2,
                &whispers,
            )
            .unwrap()
    );
    // Add a newer Turn 3 whisper for Alice to verify HUD surfaces newest whisper FIRST!
    let lease3 = store.try_begin_resolving("chan_hud_1").unwrap().unwrap();
    let snap3 = tavern_domain::RoomSnapshot {
        room: lease3.room.clone(),
        players: seeded_players.clone(),
        pending_actions: vec![],
    };
    let mut updated_room3 = lease3.room.clone();
    updated_room3.turn_number = 3;
    updated_room3.turn_phase = TurnPhase::Collecting;
    let mut turn_rec3 = turn_rec.clone();
    turn_rec3.turn_number = 3;
    assert!(
        store
            .commit_turn_cas(
                lease3.expected_version,
                &snap3,
                &updated_room3,
                &seeded_players,
                &turn_rec3,
                3,
                &[tavern_domain::DirectMessageOut {
                    user_id: "u_alice".into(),
                    player_name: "林星河".into(),
                    content: "ALICE_NEWER_TURN3_SECRET".into(),
                }],
            )
            .unwrap()
    );

    let alice_tok = tavern_core::hud_token::sign_hud_token(
        &state.hud_secret,
        &room.web_token,
        "chan_hud_1",
        "u_alice",
    );

    // 1. Tampered user_id / raw room.web_token / missing ptoken MUST all get 403 Forbidden!
    for bad in [
        format!("{base}/hud/chan_hud_1?u=u_alice&ptoken=room_tok_hud_1"),
        format!("{base}/api/hud/chan_hud_1?u=u_bob&ptoken={alice_tok}"),
        format!("{base}/api/hud/chan_hud_1?u=u_alice&ptoken=00000000000000000000000000000000"),
        format!("{base}/api/hud/chan_hud_1/events?u=u_bob&ptoken={alice_tok}"),
    ] {
        let r = http.get(&bad).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "expected 403 for {bad}");
    }

    // 2. Valid signed HUD page & API return Alice's whispers, cross-referenced NPC dossier,
    //    spoiler-safe `always_active` glossary, and NEVER Bob's whisper or hidden worldbook secrets!
    let hud_html = http
        .get(format!("{base}/hud/chan_hud_1?u=u_alice&ptoken={alice_tok}"))
        .send()
        .await
        .unwrap();
    assert_eq!(hud_html.status(), StatusCode::OK);
    assert!(hud_html.text().await.unwrap().contains("documentPictureInPicture"));

    let hud_api = http
        .get(format!("{base}/api/hud/chan_hud_1?u=u_alice&ptoken={alice_tok}"))
        .send()
        .await
        .unwrap();
    assert_eq!(hud_api.status(), StatusCode::OK);
    let body = hud_api.text().await.unwrap();
    assert!(body.contains("林星河"));
    assert!(body.contains("铁手阿凯"));
    assert!(body.contains("老雷恩"));
    assert!(body.contains("\"attitude_tier\":\"ally\""));
    assert!(body.contains("ALICE_ONLY_BELLTOWER_SECRET"));
    assert!(body.contains("ALICE_NEWER_TURN3_SECRET"));
    assert!(body.contains("酒馆密会"));
    assert!(body.contains("\"summary_upto_turn\":2"));
    let idx_t3 = body.find("ALICE_NEWER_TURN3_SECRET").unwrap();
    let idx_t2 = body.find("ALICE_ONLY_BELLTOWER_SECRET").unwrap();
    assert!(idx_t3 < idx_t2, "newest whisper MUST appear before older whisper in HUD");
    assert!(body.contains("灰烬港与余烬酒馆常识"));
    // Cross-player & spoiler isolation redlines!
    assert!(!body.contains("BOB_ONLY_CELLAR_SECRET"));
    assert!(!body.contains("艾琳的目标是直接炸毁旧钟楼"));
    assert!(!body.contains("午夜潮汐星仪终局"));
    assert!(!body.contains("u_alice"));
    assert!(!body.contains("u_bob"));

    // 3. HUD SSE stream pushes real-time updates cleanly
    let mut sse = http
        .get(format!("{base}/api/hud/chan_hud_1/events?u=u_alice&ptoken={alice_tok}"))
        .send()
        .await
        .unwrap();
    assert_eq!(sse.status(), StatusCode::OK);
    bus.publish(RoomEvent::RoomUpdated {
        channel_id: "chan_hud_1".into(),
        turn_number: 3,
    });
    let ev = tokio::time::timeout(Duration::from_secs(3), sse.chunk())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&ev).contains("\"turn\":3"));
    drop(sse);

    // 4. Rotating `room.web_token` (as on `/reset`) immediately revokes Alice's old HUD token!
    let mut rotated = room.clone();
    rotated.web_token = "room_tok_hud_rotated".into();
    store.upsert_room(&rotated).unwrap();
    let revoked = http
        .get(format!("{base}/api/hud/chan_hud_1?u=u_alice&ptoken={alice_tok}"))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), StatusCode::FORBIDDEN);

    // 5. Gate 10 P0-1 & P1-8 regressions: `fog_manor` & `midnight_metro` HUD glossaries NEVER spoil
    //    DM-only `always_active` plot twists, AND `"不信任"` is `wary` while `"友好"` is `ally`!
    for (chan, sc_id, expected_public, forbidden_spoiler) in [
        ("ch_fog_hud", "fog_manor", "黑杉庄园共三层", "镜中替身骗进并困在了镜界"),
        ("ch_metro_hud", "midnight_metro", "四号线末班车乘客守则", "被黑笔偷偷篡改的假规则"),
        ("ch_dnd_hud", "sunforge_keep", "晨日核心", "/rest short"),
    ] {
        let mut r = room.clone();
        r.channel_id = chan.into();
        r.scenario_id = sc_id.into();
        r.web_token = format!("tok_{sc_id}");
        r.npc_states = vec![
            NpcState { name: "可疑管家".into(), attitude: "不信任".into(), status: "冷眼旁观".into(), location: "大厅".into() },
            NpcState { name: "善意阿婆".into(), attitude: "友好".into(), status: "递来栗子".into(), location: "车厢".into() },
        ];
        store.upsert_room(&r).unwrap();
        let mut p = seeded_players[0].clone();
        p.channel_id = chan.into();
        store.upsert_player(&p).unwrap();
        let tok = tavern_core::hud_token::sign_hud_token(&state.hud_secret, &r.web_token, chan, "u_alice");
        let resp = http.get(format!("{base}/api/hud/{chan}?u=u_alice&ptoken={tok}")).send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let txt = resp.text().await.unwrap();
        assert!(txt.contains(expected_public), "{sc_id} missing player-visible handout");
        assert!(!txt.contains(forbidden_spoiler), "{sc_id} LEAKED DM-only spoiler to HUD!");
        assert!(txt.contains("\"attitude\":\"不信任\",\"attitude_tier\":\"wary\""));
        assert!(txt.contains("\"attitude\":\"友好\",\"attitude_tier\":\"ally\""));
    }
}
