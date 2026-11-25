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

    // 1. /healthz returns 200 OK with strict security headers
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
        gm_user_id: "secret_gm_uid_777".into(),
        web_token: "tok_secret_abc123".into(),
        clues: vec!["03-17 断钥匙".into()],
        npc_states: vec![NpcState {
            name: "老雷恩".into(),
            attitude: "信任".into(),
            status: "递出酒窖钥匙".into(),
        }],
        suggested_actions: vec![],
        current_act: 2,
        world_flags: vec!["取得老雷恩信任".into()],
        ended_id: None,
        ended_title: None,
        summary: String::new(),
        summary_upto_turn: 0,
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
    assert!(page.text().await.unwrap().contains("实况只读看板"));

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
