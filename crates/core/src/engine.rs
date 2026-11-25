use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::Result;
use tavern_domain::{EngineResponse, IncomingMessage, RoomEvent};

use crate::commands;
use crate::config::AppConfig;
use crate::ports::{
    BackgroundTaskRunner, EventBus, ExportWriter, LlmProvider, NoopBackgroundRunner, NoopEventBus,
    NoopExportWriter, RoomStore, ScenarioCatalog,
};

#[derive(Clone)]
pub struct GameEngine {
    pub(crate) cfg: AppConfig,
    pub(crate) store: Arc<dyn RoomStore>,
    pub(crate) scenarios: Arc<dyn ScenarioCatalog>,
    pub(crate) llm: Arc<dyn LlmProvider>,
    pub(crate) events: Arc<dyn EventBus>,
    pub(crate) exporter: Arc<dyn ExportWriter>,
    pub(crate) bg_runner: Arc<dyn BackgroundTaskRunner>,
    pub(crate) in_flight_llm: Arc<AtomicU64>,
}

impl GameEngine {
    pub fn new(
        cfg: AppConfig,
        store: Arc<dyn RoomStore>,
        scenarios: Arc<dyn ScenarioCatalog>,
        llm: Arc<dyn LlmProvider>,
    ) -> Self {
        Self {
            cfg,
            store,
            scenarios,
            llm,
            events: Arc::new(NoopEventBus),
            exporter: Arc::new(NoopExportWriter),
            bg_runner: Arc::new(NoopBackgroundRunner),
            in_flight_llm: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_ports(
        mut self,
        events: Arc<dyn EventBus>,
        exporter: Arc<dyn ExportWriter>,
        bg_runner: Arc<dyn BackgroundTaskRunner>,
    ) -> Self {
        self.events = events;
        self.exporter = exporter;
        self.bg_runner = bg_runner;
        self
    }

    pub fn emit_event(&self, event: RoomEvent) {
        self.events.publish(event);
    }

    pub fn will_likely_resolve_turn(
        &self,
        guild_id: Option<&str>,
        channel_id: &str,
        user_id: &str,
        content: &str,
        is_direct: bool,
    ) -> bool {
        commands::will_likely_resolve_turn(self, guild_id, channel_id, user_id, content, is_direct)
    }

    pub fn will_button_likely_resolve_turn(
        &self,
        guild_id: Option<&str>,
        channel_id: &str,
        user_id: &str,
        value: &str,
    ) -> bool {
        commands::buttons::will_button_likely_resolve_turn(self, guild_id, channel_id, user_id, value)
    }

    pub async fn handle_message(&self, msg: IncomingMessage) -> Result<EngineResponse> {
        commands::dispatch_message(self, msg).await
    }

    pub async fn handle_button(
        &self,
        channel_id: &str,
        user_id: &str,
        user_name: &str,
        value: &str,
    ) -> Result<EngineResponse> {
        self.handle_button_in_guild(None, channel_id, user_id, user_name, value)
            .await
    }

    pub async fn handle_button_in_guild(
        &self,
        guild_id: Option<&str>,
        channel_id: &str,
        user_id: &str,
        user_name: &str,
        value: &str,
    ) -> Result<EngineResponse> {
        commands::buttons::dispatch_button(self, guild_id, channel_id, user_id, user_name, value)
            .await
    }

    pub async fn resolve_turn(&self, channel_id: &str) -> Result<EngineResponse> {
        commands::turn::resolve_turn(self, channel_id).await
    }
}
