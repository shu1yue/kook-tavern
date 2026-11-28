mod handlers;
pub mod hud_views;
mod map_view;
mod security;
pub mod views;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Router;
use axum::routing::get;
use tavern_core::{EventBus, RoomStore, ScenarioCatalog};
use tavern_domain::RoomEvent;
use tokio::net::TcpListener;
use tokio::sync::{Semaphore, broadcast};
use tracing::info;

use crate::security::IpRateLimiter;

const MAX_SSE_CLIENTS: usize = 64;

#[derive(Clone)]
pub struct BroadcastEventBus {
    tx: broadcast::Sender<RoomEvent>,
}

impl BroadcastEventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(32));
        Self { tx }
    }

    pub fn sender(&self) -> broadcast::Sender<RoomEvent> {
        self.tx.clone()
    }
}

impl EventBus for BroadcastEventBus {
    fn publish(&self, event: RoomEvent) {
        let _ = self.tx.send(event);
    }
}

#[derive(Clone)]
pub struct WebState {
    pub store: Arc<dyn RoomStore>,
    pub scenarios: Arc<dyn ScenarioCatalog>,
    pub events: broadcast::Sender<RoomEvent>,
    pub hud_secret: [u8; 32],
    pub(crate) rate_limiter: IpRateLimiter,
    pub(crate) sse_permits: Arc<Semaphore>,
}

impl WebState {
    pub fn new(
        store: Arc<dyn RoomStore>,
        scenarios: Arc<dyn ScenarioCatalog>,
        events: broadcast::Sender<RoomEvent>,
    ) -> Self {
        Self {
            store,
            scenarios,
            events,
            hud_secret: tavern_core::hud_token::derive_hud_secret(None, None, None),
            rate_limiter: IpRateLimiter::default(),
            sse_permits: Arc::new(Semaphore::new(MAX_SSE_CLIENTS)),
        }
    }

    pub fn with_hud_secret(mut self, secret: [u8; 32]) -> Self {
        self.hud_secret = secret;
        self
    }

    pub fn available_sse_permits(&self) -> usize {
        self.sse_permits.available_permits()
    }
}

pub fn build_router(state: WebState) -> Router {
    Router::new()
        .route("/", get(handlers::landing_page))
        .route("/healthz", get(handlers::healthz))
        .route("/room/{channel_id}", get(handlers::room_page))
        .route("/api/room/{channel_id}", get(handlers::room_api))
        .route(
            "/api/room/{channel_id}/events",
            get(handlers::room_events_sse),
        )
        .route("/hud/{channel_id}", get(handlers::hud_page))
        .route("/api/hud/{channel_id}", get(handlers::hud_api))
        .route(
            "/api/hud/{channel_id}/events",
            get(handlers::hud_events_sse),
        )
        .with_state(state)
}

const RESERVED_COEXISTENCE_PORTS: [u16; 3] = [8080, 3000, 6099];

pub fn validate_bind_addr(bind_addr: &str) -> Result<()> {
    let port: u16 = if let Ok(sa) = bind_addr.parse::<SocketAddr>() {
        sa.port()
    } else if let Some((_, p)) = bind_addr.rsplit_once(':') {
        p.parse::<u16>()
            .with_context(|| format!("Web 监听地址端口格式无效: {bind_addr}"))?
    } else {
        bail!("Web 监听地址缺少端口号（格式如 0.0.0.0:8787）: {bind_addr}");
    };
    if RESERVED_COEXISTENCE_PORTS.contains(&port) {
        bail!(
            "安全红线拦截：端口 {port} 为同机保留端口（8080/3000/6099），严禁 Web 看板占用，请改用 8787 等空闲端口！"
        );
    }
    Ok(())
}

pub async fn run_web_server(
    bind_addr: &str,
    state: WebState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    validate_bind_addr(bind_addr)?;
    let listener = TcpListener::bind(bind_addr)
        .await
        .with_context(|| format!("Web 看板监听地址绑定失败: {bind_addr}"))?;
    let local_addr = listener.local_addr()?;
    info!(%local_addr, "只读伴生 Web 看板大屏已启动");

    let app = build_router(state).into_make_service_with_connect_info::<SocketAddr>();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .context("Web 看板服务运行异常")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_reserved_coexistence_ports_and_allows_8787() {
        assert!(validate_bind_addr("0.0.0.0:8787").is_ok());
        assert!(validate_bind_addr("127.0.0.1:0").is_ok());
        assert!(validate_bind_addr("0.0.0.0:8080").is_err());
        assert!(validate_bind_addr("127.0.0.1:3000").is_err());
        assert!(validate_bind_addr("[::]:6099").is_err());
        assert!(validate_bind_addr("localhost:8080").is_err());
    }
}
