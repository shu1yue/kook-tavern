use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::dnd::{CharacterAbility, CheckSystem, PlayerDndProfile};
use crate::stats::{CharacterStats, ScenarioAct, ScenarioEnding};
use crate::world::{ScenarioMapEdge, ScenarioMapNode, ScenarioRules, WorldBulletin};

#[derive(Debug, Error)]
pub enum DomainError {
    #[error("无效的骰子表达式：{0}")]
    InvalidDice(String),
    #[error("状态冲突：房间版本已变更（并发结算或已回档）")]
    VersionConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChatMode {
    #[default]
    Strict,
    Roleplay,
}

impl ChatMode {
    pub fn as_str(self) -> &'static str {
        match self { ChatMode::Strict => "strict", ChatMode::Roleplay => "roleplay" }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "strict" | "严格" | "指令" => Some(ChatMode::Strict),
            "roleplay" | "rp" | "自由" | "沉浸" => Some(ChatMode::Roleplay),
            _ => None,
        }
    }
    pub fn display_label(self) -> &'static str {
        match self {
            ChatMode::Strict => "严格模式（仅 /act 或 > 计入行动，日常吹水自动屏蔽）",
            ChatMode::Roleplay => "沉浸模式（直接发言即行动，带 ( 或 （ 或 // 视为场外 OOC）",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NarrativeStyle {
    #[default]
    Prose,
    Voice,
}

impl NarrativeStyle {
    pub fn as_str(self) -> &'static str {
        match self { NarrativeStyle::Prose => "prose", NarrativeStyle::Voice => "voice" }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "prose" | "text" | "文字" | "沉浸" => Some(NarrativeStyle::Prose),
            "voice" | "brief" | "语音" | "开黑" | "极简" => Some(NarrativeStyle::Voice),
            _ => None,
        }
    }
    pub fn display_label(self) -> &'static str {
        match self {
            NarrativeStyle::Prose => "纯文字沉浸档（200~420字画面描写）",
            NarrativeStyle::Voice => "语音开黑极简档（100~180字快扫卡片）",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    #[default]
    Collecting,
    Resolving,
}

impl TurnPhase {
    pub fn as_str(self) -> &'static str {
        match self { TurnPhase::Collecting => "collecting", TurnPhase::Resolving => "resolving" }
    }
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "resolving" => TurnPhase::Resolving,
            _ => TurnPhase::Collecting,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StarterCharacterDefaults {
    pub max_hp: i32,
    pub max_san: i32,
    #[serde(default)]
    pub starter_items: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldbookEntry {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub always_active: bool,
    #[serde(default = "default_priority")]
    pub priority: i32,
    pub content: String,
}

fn default_priority() -> i32 { 50 }
fn default_state_version() -> u64 { 1 }
fn default_current_act() -> u32 { 1 }
fn default_theme() -> String { "ember".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetRole {
    pub name: String,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default)]
    pub bonus_items: Vec<String>,
    #[serde(default)]
    pub stats: CharacterStats,
    #[serde(default)]
    pub alignment: String,
    #[serde(default)]
    pub proficient_skills: Vec<String>,
    #[serde(default)]
    pub abilities: Vec<CharacterAbility>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NpcState {
    pub name: String,
    #[serde(default)]
    pub attitude: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub genre: String,
    #[serde(default = "default_theme")]
    pub theme: String,
    pub dm_style: String,
    pub initial_location: String,
    pub initial_time: String,
    pub opening_scene: String,
    #[serde(default)]
    pub opening_actions: Vec<String>,
    pub player_defaults: StarterCharacterDefaults,
    #[serde(default)]
    pub preset_roles: Vec<PresetRole>,
    #[serde(default)]
    pub acts: Vec<ScenarioAct>,
    #[serde(default)]
    pub endings: Vec<ScenarioEnding>,
    #[serde(default)]
    pub worldbook: Vec<WorldbookEntry>,
    #[serde(default)]
    pub map_nodes: Vec<ScenarioMapNode>,
    #[serde(default)]
    pub map_edges: Vec<ScenarioMapEdge>,
    #[serde(default)]
    pub rules: ScenarioRules,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoomState {
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: String,
    pub scenario_id: String,
    pub title: String,
    pub location: String,
    pub world_time: String,
    pub turn_number: u32,
    #[serde(default = "default_state_version")]
    pub state_version: u64,
    #[serde(default)]
    pub turn_phase: TurnPhase,
    #[serde(default)]
    pub chat_mode: ChatMode,
    #[serde(default)]
    pub narrative_style: NarrativeStyle,
    #[serde(default)]
    pub gm_user_id: String,
    #[serde(default)]
    pub web_token: String,
    #[serde(default)]
    pub clues: Vec<String>,
    #[serde(default)]
    pub npc_states: Vec<NpcState>,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
    #[serde(default = "default_current_act")]
    pub current_act: u32,
    #[serde(default)]
    pub world_flags: Vec<String>,
    #[serde(default)]
    pub ended_id: Option<String>,
    #[serde(default)]
    pub ended_title: Option<String>,
    #[serde(default)]
    pub bulletins: Vec<WorldBulletin>,
    #[serde(default)]
    pub check_system: CheckSystem,
    #[serde(default)]
    pub last_rest_turn: u32,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub summary_upto_turn: u32,
    pub active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl fmt::Debug for RoomState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RoomState")
            .field("channel_id", &self.channel_id)
            .field("scenario_id", &self.scenario_id)
            .field("location", &self.location)
            .field("turn_number", &self.turn_number)
            .field("state_version", &self.state_version)
            .field("turn_phase", &self.turn_phase)
            .field("web_token", &"***REDACTED***")
            .field("clues_len", &self.clues.len())
            .field("active", &self.active)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub channel_id: String,
    pub user_id: String,
    pub character_name: String,
    pub role_desc: String,
    #[serde(default)]
    pub traits: Vec<String>,
    pub hp: i32,
    pub max_hp: i32,
    pub san: i32,
    pub max_san: i32,
    #[serde(default)]
    pub inventory: Vec<String>,
    #[serde(default)]
    pub status_note: String,
    #[serde(default)]
    pub is_afk: bool,
    #[serde(default)]
    pub stats: CharacterStats,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub dnd: PlayerDndProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingAction {
    pub channel_id: String,
    pub user_id: String,
    #[serde(default)]
    pub target_turn: u32,
    pub character_name: String,
    #[serde(default)]
    pub action_text: String,
    #[serde(default)]
    pub roll_notes: Vec<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnRecord {
    pub id: i64,
    pub channel_id: String,
    pub turn_number: u32,
    pub batched_input: String,
    pub scene_title: String,
    pub narrative: String,
    pub state_summary: String,
    pub location: String,
    pub world_time: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub room: RoomState,
    pub players: Vec<Player>,
    pub pending_actions: Vec<PendingAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperRecord {
    pub id: i64,
    pub channel_id: String,
    pub turn_number: u32,
    pub user_id: String,
    pub player_name: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    pub estimated_cny: f64,
}

impl UsageSummary {
    pub fn cache_hit_ratio_pct(&self) -> f64 {
        let total = self.cache_hit_tokens + self.cache_miss_tokens;
        let denom = if total > 0 { total } else { self.prompt_tokens };
        if denom == 0 { 0.0 } else { (self.cache_hit_tokens as f64 / denom as f64) * 100.0 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DailyUsage { pub day_utc: String, pub calls: u64, pub estimated_cny: f64 }

pub fn format_batched_actions(actions: &[PendingAction]) -> String {
    actions
        .iter()
        .map(|a| {
            let mut e = format!("【{}】行动：{}", a.character_name, a.action_text.trim());
            if !a.roll_notes.is_empty() {
                e.push_str(&format!("\n  ↳ 已验证后端掷骰：{}", a.roll_notes.join("；")));
            }
            e
        })
        .collect::<Vec<_>>()
        .join("\n")
}
