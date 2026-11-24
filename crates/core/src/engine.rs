use std::sync::Arc;

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

    pub async fn handle_message(&self, msg: IncomingMessage) -> Result<EngineResponse> {
        commands::dispatch_message(self, msg).await
    }

    pub async fn handle_button(
        &self,
        channel_id: &str,
        _user_id: &str,
        _user_name: &str,
        value: &str,
    ) -> Result<EngineResponse> {
        commands::dispatch_button(self, channel_id, value).await
    }

    pub async fn resolve_turn(&self, channel_id: &str) -> Result<EngineResponse> {
        commands::turn::resolve_turn(self, channel_id).await
    }
}
