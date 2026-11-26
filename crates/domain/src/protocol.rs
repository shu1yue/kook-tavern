use serde::{Deserialize, Serialize};

use crate::models::{PendingAction, Player, PresetRole, RoomState};
use crate::stats::SuggestedCheck;
use crate::workshop::{PrivateButton, WorldbookPack, WorldbookPackSummary, WorkshopDraft};
use crate::world::WorldBulletin;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StatePatch {
    pub player_name: String,
    #[serde(default)]
    pub hp_delta: i32,
    #[serde(default)]
    pub san_delta: i32,
    #[serde(default)]
    pub add_items: Vec<String>,
    #[serde(default)]
    pub remove_items: Vec<String>,
    #[serde(default)]
    pub status_note: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateWhisper {
    pub player_name: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NpcUpdate {
    pub name: String,
    #[serde(default)]
    pub attitude: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldBulletinDraft {
    #[serde(default)]
    pub headline: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub faction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClockUpdate {
    pub id: String,
    #[serde(default)]
    pub delta: i8,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmStructuredReply {
    #[serde(default)]
    pub scene_title: String,
    /// Plan-first plain-Chinese summary (15-35 chars) committed BEFORE `narrative`
    /// so the LLM decides concrete plot progression before writing prose.
    #[serde(default)]
    pub turn_outcome: String,
    pub narrative: String,
    #[serde(default)]
    pub private_whispers: Vec<PrivateWhisper>,
    #[serde(default)]
    pub state_patches: Vec<StatePatch>,
    #[serde(default)]
    pub new_clues: Vec<String>,
    #[serde(default)]
    pub npc_updates: Vec<NpcUpdate>,
    #[serde(default)]
    pub world_time: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
    #[serde(default)]
    pub suggested_check: Option<SuggestedCheck>,
    #[serde(default)]
    pub add_flags: Vec<String>,
    #[serde(default)]
    pub advance_act: bool,
    #[serde(default)]
    pub trigger_ending_id: Option<String>,
    #[serde(default)]
    pub world_bulletin: Option<WorldBulletinDraft>,
    #[serde(default)]
    pub reincarnation_echo: Option<String>,
    #[serde(default)]
    pub grant_inspiration: Vec<String>,
    #[serde(default)]
    pub clock_updates: Vec<ClockUpdate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
}

#[derive(Debug, Clone)]
pub struct DmTurnOutput {
    pub reply: DmStructuredReply,
    pub usage: LlmUsage,
    pub extra_usages: Vec<LlmUsage>,
}

#[derive(Debug, Clone)]
pub struct SummaryOutput {
    pub summary: String,
    pub usage: LlmUsage,
}

#[derive(Debug, Clone)]
pub struct IncomingMessage {
    pub guild_id: Option<String>,
    pub channel_id: String,
    pub user_id: String,
    pub user_name: String,
    pub content: String,
    pub is_direct: bool,
}

#[derive(Debug, Clone)]
pub struct DirectMessageOut {
    pub user_id: String,
    pub player_name: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub enum RoomEvent {
    RoomUpdated { channel_id: String, turn_number: u32 },
    RoomReset { channel_id: String },
}

#[derive(Debug, Clone)]
pub struct ScenarioSummary {
    pub id: String,
    pub name: String,
    pub genre: String,
    pub tagline: String,
    pub worldbook_count: usize,
}

#[derive(Debug, Clone)]
pub struct WallReport {
    pub target_channel_id: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub struct EndingBanner {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub newly_unlocked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LiveStoryHud {
    pub turn_number: u32,
    pub rule_badge: String,
    pub current_goal: String,
    pub clock_lines: Vec<String>,
    pub party_lines: Vec<String>,
    pub turn_progress: Vec<String>,
    pub opening_roles: Vec<PresetRole>,
}

#[derive(Debug, Clone)]
pub enum EngineResponse {
    Ignore,
    Notice {
        title: String,
        body: String,
        theme: CardTheme,
        show_actions: bool,
    },
    /// Interactive scenario lobby card with clickable start buttons.
    ScenarioMenu {
        channel_id: String,
        scenarios: Vec<ScenarioSummary>,
    },
    /// Interactive preset role picker card for one-click character creation.
    RolePicker {
        channel_id: String,
        scenario_name: String,
        roles: Vec<PresetRole>,
    },
    /// Export confirmation plus optional highlight card published to the report wall channel.
    ExportReport {
        saved_file: String,
        turn_count: usize,
        wall_report: Option<WallReport>,
    },
    /// Delivered privately to a single user (DM or KOOK `temp_target_id`), NEVER public!
    PrivateNotice {
        user_id: String,
        title: String,
        body: String,
        actions: Vec<PrivateButton>,
    },
    /// Open-source Worldbook & Setting Store menu card.
    WorldbookStoreMenu {
        channel_id: String,
        packs: Vec<WorldbookPackSummary>,
    },
    /// Detailed view of a single Worldbook Pack with clickable starter hooks.
    WorldbookDetail {
        channel_id: String,
        pack: Box<WorldbookPack>,
    },
    /// Guided 5-step interactive Scenario Creation Workshop desk.
    WorkshopDesk {
        channel_id: String,
        user_id: String,
        draft: Box<WorkshopDraft>,
        base_pack: Option<Box<WorldbookPack>>,
    },
    StoryTurn {
        channel_id: String,
        turn_number: u32,
        scene_title: String,
        location: String,
        world_time: String,
        act_badge: Option<String>,
        narrative: String,
        state_changes: Vec<String>,
        whisper_hints: Vec<String>,
        suggested_actions: Vec<String>,
        suggested_check: Option<SuggestedCheck>,
        world_bulletin: Option<WorldBulletin>,
        ending_reached: Option<EndingBanner>,
        direct_messages: Vec<DirectMessageOut>,
    },
    StatusBoard {
        channel_id: String,
        room: RoomState,
        act_label: Option<String>,
        players: Vec<Player>,
        pending_actions: Vec<PendingAction>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTheme {
    Primary,
    Success,
    Warning,
    Danger,
    Info,
    Secondary,
}

impl CardTheme {
    pub fn kook_str(self) -> &'static str {
        match self {
            CardTheme::Primary => "primary",
            CardTheme::Success => "success",
            CardTheme::Warning => "warning",
            CardTheme::Danger => "danger",
            CardTheme::Info => "info",
            CardTheme::Secondary => "secondary",
        }
    }
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
