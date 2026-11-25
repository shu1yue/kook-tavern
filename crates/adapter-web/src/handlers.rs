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
use tavern_domain::{RoomEvent, RoomState};
use tokio::sync::broadcast::error::RecvError;
use tracing::warn;

use crate::WebState;
use crate::hud_views::build_player_hud_view;
use crate::security::{constant_time_eq, security_headers};
use crate::views::build_room_public_view;

const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");
const HUD_HTML: &str = include_str!("../assets/hud.html");

#[derive(Debug, Deserialize)]
pub struct TokenQuery {
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct HudQuery {
    #[serde(default)]
    pub u: String,
    #[serde(default)]
    pub ptoken: String,
}

#[derive(Debug, Serialize)]
struct HealthResp {
    status: &'static str,
    service: &'static str,
    version: &'static str,
}

pub async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        security_headers(),
        Json(HealthResp {
            status: "ok",
            service: "kook-tavern-dm",
            version: "1.7.0",
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

    match build_room_public_view(&state, &channel_id, room) {
        Ok(view) => (StatusCode::OK, security_headers(), Json(view)).into_response(),
        Err(err) => {
            warn!(channel_id, error = %err, "Web 看板读取房间快照失败");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                security_headers(),
                "internal error",
            )
                .into_response()
        }
    }
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

pub async fn hud_page(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
    Path(channel_id): Path<String>,
    Query(q): Query<HudQuery>,
) -> Response {
    if !state.rate_limiter.allow(addr.ip()) {
        return too_many_requests();
    }
    if authorize_hud_player(&state, &channel_id, &q.u, &q.ptoken).is_none() {
        return forbidden_hud_page();
    }
    (StatusCode::OK, security_headers(), Html(HUD_HTML)).into_response()
}

pub async fn hud_api(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
    Path(channel_id): Path<String>,
    Query(q): Query<HudQuery>,
) -> Response {
    if !state.rate_limiter.allow(addr.ip()) {
        return too_many_requests();
    }
    let Some(room) = authorize_hud_player(&state, &channel_id, &q.u, &q.ptoken) else {
        return (
            StatusCode::FORBIDDEN,
            security_headers(),
            Json(serde_json::json!({ "error": "invalid_or_expired_hud_token" })),
        )
            .into_response();
    };
    match build_player_hud_view(&state, &channel_id, q.u.trim(), room) {
        Ok(Some(view)) => (StatusCode::OK, security_headers(), Json(view)).into_response(),
        Ok(None) => (
            StatusCode::FORBIDDEN,
            security_headers(),
            Json(serde_json::json!({ "error": "player_not_in_room" })),
        )
            .into_response(),
        Err(err) => {
            warn!(channel_id, error = %err, "个人悬浮 HUD 读取房间快照失败");
            (StatusCode::INTERNAL_SERVER_ERROR, security_headers(), "internal error")
                .into_response()
        }
    }
}

pub async fn hud_events_sse(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    State(state): State<WebState>,
    Path(channel_id): Path<String>,
    Query(q): Query<HudQuery>,
) -> Response {
    if !state.rate_limiter.allow(addr.ip()) {
        return too_many_requests();
    }
    if authorize_hud_player(&state, &channel_id, &q.u, &q.ptoken).is_none() {
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

fn authorize_hud_player(
    state: &WebState,
    channel_id: &str,
    user_id: &str,
    ptoken: &str,
) -> Option<RoomState> {
    let uid = user_id.trim();
    let tok = ptoken.trim();
    if channel_id.is_empty() || uid.is_empty() || tok.is_empty() {
        return None;
    }
    let room = state.store.get_room(channel_id).ok()??;
    if !room.active || room.web_token.is_empty() {
        return None;
    }
    if !tavern_core::hud_token::verify_hud_token(
        &state.hud_secret,
        &room.web_token,
        channel_id,
        uid,
        tok,
    ) {
        return None;
    }
    state.store.get_player(channel_id, uid).ok()?.map(|_| room)
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

fn forbidden_hud_page() -> Response {
    let html = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>悬浮 HUD 已失效 · AI 群酒馆</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#0c0a09;color:#f3ece4;font-family:system-ui,-apple-system,sans-serif;padding:20px}.box{max-width:360px;background:#161210;border:1px solid #382c22;border-radius:14px;padding:24px;text-align:center}h1{margin:0 0 10px;font-size:18px;color:#f0b429}p{margin:0;color:#a69582;line-height:1.6;font-size:13px}</style></head><body><div class="box"><h1>🔒 个人悬浮 HUD 凭证无效或已离桌</h1><p>请回到 KOOK 跑团频道，点击 <b>「🪪 我的角色卡」</b> 或发送 <code>/hud</code> 获取最新专属浮窗链接喵！</p></div></body></html>"#;
    (StatusCode::FORBIDDEN, security_headers(), Html(html)).into_response()
}

fn forbidden_page() -> Response {
    let html = r#"<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>访问受限 · AI 群酒馆</title><style>body{margin:0;min-height:100vh;display:grid;place-items:center;background:#0d1117;color:#e6edf3;font-family:system-ui,-apple-system,sans-serif;padding:24px}.box{max-width:440px;background:#161b22;border:1px solid #30363d;border-radius:14px;padding:28px;text-align:center}h1{margin:0 0 12px;font-size:20px;color:#f0b429}p{margin:0;color:#8b949e;line-height:1.6}</style></head><body><div class="box"><h1>🔒 只读看板凭证无效或房间已结团</h1><p>请回到 KOOK 跑团频道，点击战局面板里的「🌐 Web 看板」按钮或发送 <code>/web</code> 获取最新有效链接。</p></div></body></html>"#;
    (StatusCode::FORBIDDEN, security_headers(), Html(html)).into_response()
}
