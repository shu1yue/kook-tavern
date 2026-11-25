use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{ConnectInfo, Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use futures_util::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use tavern_domain::{NpcState, RoomEvent, RoomState, TurnPhase};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::WebState;
use crate::security::{constant_time_eq, security_headers};

const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");

#[derive(Debug, Deserialize)]
pub struct TokenQuery {
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Serialize)]
struct HealthResp {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

/// Strictly sanitized public read-only room view (NEVER exposes `web_token`, `gm_user_id`,
/// `guild_id`, `user_id`, or private `whispers`!).
#[derive(Debug, Serialize)]
pub struct RoomPublicView {
    pub title: String,
    pub scenario_id: String,
    pub location: String,
    pub world_time: String,
    pub turn_number: u32,
    pub turn_phase: &'static str,
    pub chat_mode: &'static str,
    pub narrative_style: &'static str,
    pub active: bool,
    pub summary: String,
    pub summary_upto_turn: u32,
    pub opening_scene: String,
    pub clues: Vec<String>,
    pub npc_states: Vec<NpcState>,
    pub players: Vec<PlayerPublicView>,
    pub turns: Vec<TurnPublicView>,
}

#[derive(Debug, Serialize)]
pub struct PlayerPublicView {
    pub character_name: String,
    pub role_desc: String,
    pub traits: Vec<String>,
    pub hp: i32,
    pub max_hp: i32,
    pub san: i32,
    pub max_san: i32,
    pub inventory: Vec<String>,
    pub status_note: String,
    pub is_afk: bool,
    pub action_ready: bool,
    pub roll_notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TurnPublicView {
    pub turn_number: u32,
    pub scene_title: String,
    pub location: String,
    pub world_time: String,
    pub batched_input: String,
    pub narrative: String,
    pub state_summary: String,
}

pub async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        security_headers(),
        Json(HealthResp {
            status: "ok",
            service: "kook-tavern-dm",
            version: "0.5.0-m2",
        }),
    )
}

pub async fn room_page(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
    Path(channel_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !state.rate_limiter.allow(addr.ip()) {
        return too_many_requests();
    }
    if authorize_room(&state, &channel_id, &q.token).is_none() {
        return forbidden_page();
    }
    (StatusCode::OK, security_headers(), Html(DASHBOARD_HTML)).into_response()
}

pub async fn room_api(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
    Path(channel_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !state.rate_limiter.allow(addr.ip()) {
        return too_many_requests();
    }
    let Some(room) = authorize_room(&state, &channel_id, &q.token) else {
        return (
            StatusCode::FORBIDDEN,
            security_headers(),
            Json(serde_json::json!({ "error": "invalid_or_expired_token" })),
        )
            .into_response();
    };

    let players = match state.store.list_players(&channel_id) {
        Ok(p) => p,
        Err(err) => {
            warn!(channel_id, error = %err, "Web 看板读取玩家列表失败");
            return (StatusCode::INTERNAL_SERVER_ERROR, security_headers(), "internal error")
                .into_response();
        }
    };
    let next_turn = if room.turn_phase == TurnPhase::Resolving {
        room.turn_number + 2
    } else {
        room.turn_number + 1
    };
    let pending = state
        .store
        .list_pending_actions(&channel_id, next_turn)
        .unwrap_or_default();
    let turns = state
        .store
        .list_recent_turns(&channel_id, 30)
        .unwrap_or_default();
    let opening_scene = state
        .scenarios
        .get(&room.scenario_id)
        .map(|s| s.opening_scene.trim().to_string())
        .unwrap_or_default();

    let player_views = players
        .into_iter()
        .map(|p| {
            let p_act = pending.iter().find(|a| a.user_id == p.user_id);
            let action_ready = p_act.is_some_and(|a| !a.action_text.trim().is_empty());
            let roll_notes = p_act.map(|a| a.roll_notes.clone()).unwrap_or_default();
            PlayerPublicView {
                character_name: p.character_name,
                role_desc: p.role_desc,
                traits: p.traits,
                hp: p.hp,
                max_hp: p.max_hp,
                san: p.san,
                max_san: p.max_san,
                inventory: p.inventory,
                status_note: p.status_note,
                is_afk: p.is_afk,
                action_ready,
                roll_notes,
            }
        })
        .collect();

    let turn_views = turns
        .into_iter()
        .map(|t| TurnPublicView {
            turn_number: t.turn_number,
            scene_title: t.scene_title,
            location: t.location,
            world_time: t.world_time,
            batched_input: t.batched_input,
            narrative: t.narrative,
            state_summary: t.state_summary,
        })
        .collect();

    let view = RoomPublicView {
        title: room.title,
        scenario_id: room.scenario_id,
        location: room.location,
        world_time: room.world_time,
        turn_number: room.turn_number,
        turn_phase: room.turn_phase.as_str(),
        chat_mode: room.chat_mode.as_str(),
        narrative_style: room.narrative_style.as_str(),
        active: room.active,
        summary: room.summary,
        summary_upto_turn: room.summary_upto_turn,
        opening_scene,
        clues: room.clues,
        npc_states: room.npc_states,
        players: player_views,
        turns: turn_views,
    };

    (StatusCode::OK, security_headers(), Json(view)).into_response()
}

pub async fn room_events_sse(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
    Path(channel_id): Path<String>,
    Query(q): Query<TokenQuery>,
) -> Response {
    if !state.rate_limiter.allow(addr.ip()) {
        return too_many_requests();
    }
    if authorize_room(&state, &channel_id, &q.token).is_none() {
        return (StatusCode::FORBIDDEN, security_headers(), "forbidden").into_response();
    }

    let Ok(permit) = Arc::clone(&state.sse_permits).try_acquire_owned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            security_headers(),
            "too many live dashboard viewers",
        )
            .into_response();
    };

    let rx = state.events.subscribe();
    let stream = make_sse_stream(rx, channel_id, permit);
    let sse = Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(25))
            .text("keepalive"),
    );
    (security_headers(), sse).into_response()
}

fn make_sse_stream(
    rx: tokio::sync::broadcast::Receiver<RoomEvent>,
    channel_id: String,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> impl Stream<Item = Result<Event, Infallible>> + Send {
    // Once `RoomReset` fires for this room, emit one final `reset` frame then return `None`
    // so the server forcibly terminates the SSE stream and releases the semaphore slot immediately!
    stream::unfold(
        (rx, channel_id, permit, false),
        |(mut rx, chan, permit, closed)| async move {
            if closed {
                return None;
            }
            loop {
                match rx.recv().await {
                    Ok(RoomEvent::RoomUpdated {
                        channel_id,
                        turn_number,
                    }) if channel_id == chan => {
                        let evt = Event::default()
                            .event("update")
                            .data(format!("{{\"turn\":{turn_number}}}"));
                        return Some((Ok(evt), (rx, chan, permit, false)));
                    }
                    Ok(RoomEvent::RoomReset { channel_id }) if channel_id == chan => {
                        let evt = Event::default().event("reset").data("{\"reset\":true}");
                        return Some((Ok(evt), (rx, chan, permit, true)));
                    }
                    Ok(_) => continue,
                    Err(RecvError::Lagged(_)) => {
                        let evt = Event::default().event("update").data("{\"resync\":true}");
                        return Some((Ok(evt), (rx, chan, permit, false)));
                    }
                    Err(RecvError::Closed) => return None,
                }
            }
        },
    )
}

fn authorize_room(state: &WebState, channel_id: &str, token: &str) -> Option<RoomState> {
    if channel_id.is_empty() || token.trim().is_empty() {
        return None;
    }
    let room = state.store.get_room(channel_id).ok()??;
    if room.web_token.is_empty() {
        return None;
    }
    if constant_time_eq(room.web_token.as_bytes(), token.trim().as_bytes()) {
        Some(room)
    } else {
        None
    }
}

fn too_many_requests() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        security_headers(),
        "rate limit exceeded",
    )
        .into_response()
}

fn forbidden_page() -> Response {
    let html = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>访问受限 · AI 群酒馆</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#0d1117;color:#e6edf3;font-family:system-ui,-apple-system,sans-serif;padding:24px}.box{max-width:440px;background:#161b22;border:1px solid #30363d;border-radius:14px;padding:28px;text-align:center}h1{margin:0 0 12px;font-size:20px;color:#f0b429}p{margin:0;color:#8b949e;line-height:1.6}</style></head><body><div class="box"><h1>🔒 只读看板凭证无效或房间已结团</h1><p>请回到 KOOK 跑团频道，点击战局面板里的「🌐 Web 看板」按钮或发送 <code>/web</code> 获取最新有效链接。</p></div></body></html>"#;
    (StatusCode::FORBIDDEN, security_headers(), Html(html)).into_response()
}
