use serde::{Deserialize, Serialize};

use crate::models::{PendingAction, Player, RoomState};

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivateWhisper {
    pub player_name: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DmStructuredReply {
    #[serde(default)]
    pub scene_title: String,
    pub narrative: String,
    #[serde(default)]
    pub private_whispers: Vec<PrivateWhisper>,
    #[serde(default)]
    pub state_patches: Vec<StatePatch>,
    #[serde(default)]
    pub new_clues: Vec<String>,
    #[serde(default)]
    pub world_time: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub suggested_actions: Vec<String>,
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
pub enum EngineResponse {
    Ignore,
    Notice {
        title: String,
        body: String,
        theme: CardTheme,
        show_actions: bool,
    },
    StoryTurn {
        channel_id: String,
        turn_number: u32,
        scene_title: String,
        location: String,
        world_time: String,
        narrative: String,
        state_changes: Vec<String>,
        whisper_hints: Vec<String>,
        suggested_actions: Vec<String>,
        direct_messages: Vec<DirectMessageOut>,
    },
    StatusBoard {
        channel_id: String,
        room: RoomState,
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
