use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use tavern_domain::{
    ChatMode, DmTurnOutput, PendingAction, Player, RoomEvent, RoomSnapshot, RoomState, Scenario,
    TurnRecord,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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
    fn update_chat_mode(&self, channel_id: &str, mode: ChatMode) -> Result<bool>;
    fn get_room(&self, channel_id: &str) -> Result<Option<RoomState>>;
    fn delete_room(&self, channel_id: &str) -> Result<bool>;

    fn upsert_player(&self, player: &Player) -> Result<()>;
    fn get_player(&self, channel_id: &str, user_id: &str) -> Result<Option<Player>>;
    fn list_players(&self, channel_id: &str) -> Result<Vec<Player>>;

    /// Atomically reads room phase inside a write transaction and stores the action in either
    /// `turn_number + 1` (Collecting) or `turn_number + 2` (Resolving), preserving existing rolls.
    /// Returns `Ok(None)` if room does not exist or is inactive.
    fn submit_action_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        action_text: &str,
    ) -> Result<Option<(u32, bool)>>;

    /// Atomically appends a verified dice roll to the current target turn inside a write transaction.
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

    fn commit_turn_cas(
        &self,
        expected_version: u64,
        snapshot: &RoomSnapshot,
        updated_room: &RoomState,
        updated_players: &[Player],
        turn: &TurnRecord,
        resolved_turn: u32,
    ) -> Result<bool>;

    fn update_summary_cas(
        &self,
        channel_id: &str,
        expected_version: u64,
        summary: &str,
        summary_upto_turn: u32,
    ) -> Result<bool>;

    fn pop_and_restore_snapshot(&self, channel_id: &str) -> Result<Option<RoomSnapshot>>;
}

pub trait LlmProvider: Send + Sync {
    fn generate_turn<'a>(
        &'a self,
        scenario: &'a Scenario,
        room: &'a RoomState,
        players: &'a [Player],
        recent_turns: &'a [TurnRecord],
        actions: &'a [PendingAction],
    ) -> BoxFuture<'a, Result<DmTurnOutput>>;

    fn summarize_turns<'a>(
        &'a self,
        scenario: &'a Scenario,
        existing_summary: &'a str,
        turns: &'a [TurnRecord],
    ) -> BoxFuture<'a, Result<String>>;
}

pub trait ScenarioCatalog: Send + Sync {
    fn list(&self) -> Vec<Scenario>;
    fn get(&self, id: &str) -> Option<Scenario>;
    fn default_scenario(&self) -> Option<Scenario>;
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
    /// Persists Markdown report under safe filename and returns display-safe filename only.
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
