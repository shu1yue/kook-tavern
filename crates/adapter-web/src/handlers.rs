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
use crate::security::{constant_time_eq, security_headers};
use crate::views::build_room_public_view;

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

pub async fn healthz() -> impl IntoResponse {
    (
        StatusCode::OK,
        security_headers(),
        Json(HealthResp {
            status: "ok",
            service: "kook-tavern-dm",
            version: "1.6.3",
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
