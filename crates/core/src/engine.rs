use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::Result;
use tavern_domain::{EngineResponse, IncomingMessage, RoomEvent};

use crate::commands;
use crate::config::AppConfig;
use crate::ports::{
    BackgroundTaskRunner, DecisionGate, EventBus, ExportWriter, LlmProvider, NoopBackgroundRunner,
    NoopDecisionGate, NoopEventBus, NoopExportWriter, RoomStore, ScenarioCatalog,
};
use crate::workshop_ports::{
    EmptyWorldbookCatalog, NoopStImporter, StRemoteImporter, WorldbookCatalog,
};

#[derive(Clone)]
pub struct GameEngine {
    pub(crate) cfg: AppConfig,
    pub(crate) store: Arc<dyn RoomStore>,
    pub(crate) scenarios: Arc<dyn ScenarioCatalog>,
    pub(crate) worldbooks: Arc<dyn WorldbookCatalog>,
    pub(crate) st_importer: Arc<dyn StRemoteImporter>,
    pub(crate) llm: Arc<dyn LlmProvider>,
    pub(crate) decision: Arc<dyn DecisionGate>,
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
            worldbooks: Arc::new(EmptyWorldbookCatalog),
            st_importer: Arc::new(NoopStImporter),
            llm,
            decision: Arc::new(NoopDecisionGate),
            events: Arc::new(NoopEventBus),
            exporter: Arc::new(NoopExportWriter),
            bg_runner: Arc::new(NoopBackgroundRunner),
            in_flight_llm: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn with_worldbooks(mut self, worldbooks: Arc<dyn WorldbookCatalog>) -> Self {
        self.worldbooks = worldbooks;
        self
    }

    pub fn with_st_importer(mut self, importer: Arc<dyn StRemoteImporter>) -> Self {
        self.st_importer = importer;
        self
    }

    pub fn with_decision_gate(mut self, decision: Arc<dyn DecisionGate>) -> Self {
        self.decision = decision;
        self
    }

    pub fn has_decision_gate(&self) -> bool {
        self.decision.is_enabled()
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
        commands::dispatch_message(self, msg, false).await
    }

    pub async fn handle_message_prechecked(
        &self,
        msg: IncomingMessage,
        skip_ooc_gate: bool,
    ) -> Result<EngineResponse> {
        commands::dispatch_message(self, msg, skip_ooc_gate).await
    }

    pub async fn precheck_roleplay_ooc(
        &self,
        guild_id: Option<&str>,
        channel_id: &str,
        user_id: &str,
        user_name: &str,
        content: &str,
        is_direct: bool,
    ) -> commands::OocPrecheck {
        commands::precheck_roleplay_ooc(self, guild_id, channel_id, user_id, user_name, content, is_direct)
            .await
    }

    pub async fn is_plain_roleplay_ooc(
        &self,
        guild_id: Option<&str>,
        channel_id: &str,
        user_id: &str,
        user_name: &str,
        content: &str,
        is_direct: bool,
    ) -> bool {
        commands::is_plain_roleplay_ooc(self, guild_id, channel_id, user_id, user_name, content, is_direct)
            .await
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
