use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use anyhow::Result;
use tavern_domain::{EngineResponse, IncomingMessage, LiveStoryHud, RoomEvent};

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

    /// Builds a strictly public live HUD snapshot for in-place KOOK StoryTurn card updates.
    /// NEVER includes private whispers or `personal_secret`.
    pub fn get_live_story_hud(&self, channel_id: &str) -> Result<Option<LiveStoryHud>> {
        let Some(room) = self.store.get_room(channel_id)? else {
            return Ok(None);
        };
        if !room.active || room.ended_id.is_some() {
            return Ok(None);
        }
        let players = self.store.list_players(channel_id)?;
        let pending = self
            .store
            .list_pending_actions(channel_id, room.turn_number + 1)?;

        let party_lines = players
            .iter()
            .take(8)
            .map(|p| {
                let mut tags = String::new();
                if p.is_afk {
                    tags.push_str(" 💤");
                }
                if p.dnd.inspiration {
                    tags.push_str(" 🌟");
                }
                if p.dnd.pending_levelups > 0 {
                    tags.push_str(" ⬆️");
                }
                let stat_hint = if room.turn_number == 0 {
                    format!(" · {}", p.stats.format_inline())
                } else {
                    String::new()
                };
                format!(
                    "**{}**（{} · ❤️{}/{} 🧠{}/{}{stat_hint}）{tags}",
                    p.character_name, p.role_desc, p.hp, p.max_hp, p.san, p.max_san
                )
            })
            .collect();

        let has_any_activity = pending
            .iter()
            .any(|a| !a.action_text.trim().is_empty() || !a.roll_notes.is_empty());
        let mut turn_progress = Vec::new();
        if has_any_activity {
            for p in players.iter().filter(|p| !p.is_afk).take(8) {
                if let Some(act) = pending.iter().find(|a| a.user_id == p.user_id) {
                    let has_act = !act.action_text.trim().is_empty();
                    let last_note = act
                        .roll_notes
                        .last()
                        .map(|s| s.chars().take(110).collect::<String>());
                    match (has_act, last_note) {
                        (true, Some(n)) => turn_progress.push(format!("✅ **{}** 已行动 · {n}", p.character_name)),
                        (true, None) => turn_progress.push(format!("✅ **{}** 已行动", p.character_name)),
                        (false, Some(n)) => turn_progress.push(format!("🎲 **{}**：{n}（待行动）", p.character_name)),
                        (false, None) => turn_progress.push(format!("⏳ **{}** 待行动", p.character_name)),
                    }
                } else {
                    turn_progress.push(format!("⏳ **{}** 待行动", p.character_name));
                }
            }
        }

        let opening_roles = if room.turn_number == 0 {
            self.scenarios
                .get(&room.scenario_id)
                .map(|s| s.preset_roles.into_iter().take(4).collect())
                .unwrap_or_default()
        } else {
            Vec::new()
        };

        Ok(Some(LiveStoryHud {
            turn_number: room.turn_number,
            rule_badge: room.check_system.short_badge().to_string(),
            party_lines,
            turn_progress,
            opening_roles,
        }))
    }
}
