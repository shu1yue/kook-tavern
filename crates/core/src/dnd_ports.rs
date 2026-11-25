use anyhow::Result;
use tavern_domain::{
    CheckSystem, LevelUpChoice, Player, RestKind, RollMode, RuleCheckResult, SavedCharacter,
    StarterCharacterDefaults, StatKind,
};

use crate::ports::{GmUpdateOutcome, JoinPlayerOutcome};

#[derive(Debug, Clone)]
pub struct RuleCheckRequest {
    pub stat: StatKind,
    pub raw_stat_token: String,
    pub mode: RollMode,
    pub dc: Option<i32>,
    pub use_inspiration: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CheckCommitOutcome {
    Committed {
        player: Box<Player>,
        result: Box<RuleCheckResult>,
        turn_no: u32,
        bonus_notes: Vec<String>,
    },
    RoomUnavailable,
    PlayerNotFound,
}

#[derive(Debug, Clone)]
pub enum AssistOutcome {
    Assisted {
        helper: Box<Player>,
        target: Box<Player>,
        turn_no: u32,
    },
    NotAllowed(String),
    TargetNotFound,
    RoomUnavailable,
    RoomResolving,
}

#[derive(Debug, Clone)]
pub enum AbilityUseOutcome {
    Used {
        player: Box<Player>,
        ability_name: String,
        uses_left: u32,
        max_uses: u32,
        desc: String,
        turn_no: u32,
        roll_note: String,
    },
    Exhausted {
        ability_name: String,
        max_uses: u32,
        recharge: String,
    },
    NotAllowed(String),
    NotFound,
    RoomUnavailable,
    RoomResolving,
}

#[derive(Debug, Clone)]
pub enum RestOutcome {
    Rested {
        notes: Vec<String>,
        new_world_time: String,
    },
    Cooldown {
        wait_turns: u32,
    },
    NotAllowed(String),
    RoomUnavailable,
    RoomResolving,
}

#[derive(Debug, Clone)]
pub enum LevelUpOutcome {
    Upgraded {
        player: Box<Player>,
        summary: String,
        remaining: u32,
    },
    Error(String),
    RoomUnavailable,
    RoomResolving,
}

pub trait DndStore: Send + Sync {
    fn update_check_system(&self, channel_id: &str, system: CheckSystem) -> Result<bool>;
    fn set_player_inspiration(
        &self,
        channel_id: &str,
        target_name: &str,
        value: bool,
    ) -> Result<GmUpdateOutcome<Player>>;
    fn set_player_assist(
        &self,
        channel_id: &str,
        caller_user_id: &str,
        target_name: &str,
        how: &str,
    ) -> Result<AssistOutcome>;
    fn execute_rule_check_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        req: &RuleCheckRequest,
    ) -> Result<CheckCommitOutcome>;
    fn use_player_ability(
        &self,
        channel_id: &str,
        user_id: &str,
        query: &str,
        target_note: &str,
    ) -> Result<AbilityUseOutcome>;
    fn apply_party_rest(
        &self,
        channel_id: &str,
        caller_user_id: &str,
        is_gm_or_admin: bool,
        kind: RestKind,
        min_gap_turns: u32,
    ) -> Result<RestOutcome>;
    fn apply_player_levelup(
        &self,
        channel_id: &str,
        user_id: &str,
        choice: LevelUpChoice,
    ) -> Result<LevelUpOutcome>;
    fn save_character(&self, ch: &SavedCharacter) -> Result<()>;
    fn list_saved_characters(&self, user_id: &str) -> Result<Vec<SavedCharacter>>;
    fn delete_saved_character(&self, user_id: &str, name: &str) -> Result<bool>;
    fn load_saved_character_into_room(
        &self,
        channel_id: &str,
        user_id: &str,
        saved: &SavedCharacter,
        defaults: &StarterCharacterDefaults,
    ) -> Result<JoinPlayerOutcome>;
}
