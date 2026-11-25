use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use tavern_domain::{
    CharacterStats, ChatMode, DailyUsage, DirectMessageOut, DmTurnOutput, EndingUnlock, LlmUsage,
    NarrativeStyle, PendingAction, Player, PresetRole, RoomEvent, RoomSnapshot, RoomState,
    Scenario, StarterCharacterDefaults, SummaryOutput, TurnRecord, UsageSummary, WhisperRecord,
    WorldEcho,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub enum GmUpdateOutcome<T> {
    Updated(T),
    NotFound,
    RoomResolving,
}

#[derive(Debug, Clone)]
pub enum JoinPlayerOutcome {
    RoomUnavailable,
    DuplicateName,
    Joined(Box<Player>),
}

#[derive(Debug, Clone)]
pub struct ResolvingLease {
    pub expected_version: u64,
    pub target_turn: u32,
    pub room: RoomState,
    pub players: Vec<Player>,
    pub actions: Vec<PendingAction>,
}

pub trait RoomStore: Send + Sync {
    fn reset_stuck_resolving_on_startup(&self) -> Result<usize>;
    fn upsert_room(&self, room: &RoomState) -> Result<()>;
    fn backfill_room_guild_id(&self, channel_id: &str, guild_id: &str) -> Result<bool>;
    fn update_chat_mode(&self, channel_id: &str, mode: ChatMode) -> Result<bool>;
    fn update_narrative_style(&self, channel_id: &str, style: NarrativeStyle) -> Result<bool>;
    fn get_room(&self, channel_id: &str) -> Result<Option<RoomState>>;
    fn delete_room(&self, channel_id: &str) -> Result<bool>;

    fn upsert_player(&self, player: &Player) -> Result<()>;
    fn ensure_player_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        raw_name: &str,
        defaults: &StarterCharacterDefaults,
    ) -> Result<Option<Player>>;
    fn pick_role_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        raw_name: &str,
        preset: &PresetRole,
        defaults: &StarterCharacterDefaults,
    ) -> Result<Option<(Player, bool)>>;
    fn join_player_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        role_desc: &str,
        traits: &[String],
        defaults: &StarterCharacterDefaults,
    ) -> Result<JoinPlayerOutcome>;
    fn update_player_profile(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        role_desc: &str,
        traits: &[String],
        stats: Option<&CharacterStats>,
    ) -> Result<Option<Player>>;
    fn get_player(&self, channel_id: &str, user_id: &str) -> Result<Option<Player>>;
    fn list_players(&self, channel_id: &str) -> Result<Vec<Player>>;
    fn set_player_afk(&self, channel_id: &str, user_id: &str, is_afk: bool)
        -> Result<Option<Player>>;
    fn remove_player(&self, channel_id: &str, user_id: &str) -> Result<Option<Player>>;

    fn gm_mutate_player(
        &self,
        channel_id: &str,
        player_name: &str,
        hp_delta: i32,
        san_delta: i32,
        add_item: Option<&str>,
        remove_item: Option<&str>,
    ) -> Result<GmUpdateOutcome<Player>>;
    fn gm_add_clue(&self, channel_id: &str, clue: &str) -> Result<GmUpdateOutcome<()>>;
    fn gm_remove_clue(
        &self,
        channel_id: &str,
        index_1based: usize,
    ) -> Result<GmUpdateOutcome<String>>;

    fn submit_action_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        action_text: &str,
    ) -> Result<Option<(u32, bool)>>;

    fn append_roll_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        roll_note: &str,
    ) -> Result<Option<u32>>;

    fn list_pending_actions(&self, channel_id: &str, target_turn: u32)
        -> Result<Vec<PendingAction>>;

    fn list_recent_turns(&self, channel_id: &str, limit: usize) -> Result<Vec<TurnRecord>>;
    fn list_all_turns(&self, channel_id: &str) -> Result<Vec<TurnRecord>>;
    fn list_turns_range(
        &self,
        channel_id: &str,
        from_turn_exclusive: u32,
        to_turn_inclusive: u32,
    ) -> Result<Vec<TurnRecord>>;

    fn try_begin_resolving(&self, channel_id: &str) -> Result<Option<ResolvingLease>>;
    fn abort_resolving(&self, channel_id: &str, expected_version: u64) -> Result<()>;

    #[allow(clippy::too_many_arguments)]
    fn commit_turn_cas(
        &self,
        expected_version: u64,
        snapshot: &RoomSnapshot,
        updated_room: &RoomState,
        updated_players: &[Player],
        turn: &TurnRecord,
        resolved_turn: u32,
        whispers: &[DirectMessageOut],
    ) -> Result<bool>;

    fn update_summary_cas(
        &self,
        channel_id: &str,
        expected_version: u64,
        summary: &str,
        summary_upto_turn: u32,
    ) -> Result<bool>;

    fn pop_and_restore_snapshot(&self, channel_id: &str) -> Result<Option<RoomSnapshot>>;

    fn list_player_whispers(
        &self,
        channel_id: &str,
        user_id: &str,
        limit: usize,
    ) -> Result<Vec<WhisperRecord>>;

    fn record_usage(&self, channel_id: &str, usage: &LlmUsage, estimated_cny: f64) -> Result<()>;
    fn get_room_usage(&self, channel_id: &str) -> Result<UsageSummary>;
    fn get_today_usage(&self) -> Result<DailyUsage>;

    fn record_ending_unlock(&self, unlock: &EndingUnlock) -> Result<bool>;
    fn list_ending_unlocks(&self, guild_id: &str) -> Result<Vec<EndingUnlock>>;

    fn record_world_echo(&self, echo: &WorldEcho) -> Result<()>;
    fn list_world_echoes(
        &self,
        scope_key: &str,
        scenario_id: &str,
        before_rfc3339: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorldEcho>>;
}

pub trait LlmProvider: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn generate_turn<'a>(
        &'a self,
        scenario: &'a Scenario,
        room: &'a RoomState,
        players: &'a [Player],
        recent_turns: &'a [TurnRecord],
        actions: &'a [PendingAction],
        world_echoes: &'a [WorldEcho],
        allow_nudge: bool,
    ) -> BoxFuture<'a, Result<DmTurnOutput>>;

    fn summarize_turns<'a>(
        &'a self,
        scenario: &'a Scenario,
        existing_summary: &'a str,
        turns: &'a [TurnRecord],
    ) -> BoxFuture<'a, Result<SummaryOutput>>;
}

pub trait ScenarioCatalog: Send + Sync {
    fn list(&self) -> Vec<Scenario>;
    fn get(&self, id: &str) -> Option<Scenario>;
    fn default_scenario(&self) -> Option<Scenario>;
    fn reload(&self) -> Result<usize> {
        Ok(self.list().len())
    }
}

pub trait EventBus: Send + Sync {
    fn publish(&self, event: RoomEvent);
}

#[derive(Debug, Default)]
pub struct NoopEventBus;

impl EventBus for NoopEventBus {
    fn publish(&self, _event: RoomEvent) {}
}

pub trait ExportWriter: Send + Sync {
    fn write_markdown(&self, safe_filename: &str, content: &str) -> Result<String>;
}

#[derive(Debug, Default)]
pub struct NoopExportWriter;

impl ExportWriter for NoopExportWriter {
    fn write_markdown(&self, safe_filename: &str, _content: &str) -> Result<String> {
        Ok(safe_filename.to_string())
    }
}

pub trait BackgroundTaskRunner: Send + Sync {
    fn spawn(&self, fut: BoxFuture<'static, ()>);
}

#[derive(Debug, Default)]
pub struct NoopBackgroundRunner;

impl BackgroundTaskRunner for NoopBackgroundRunner {
    fn spawn(&self, _fut: BoxFuture<'static, ()>) {}
}
