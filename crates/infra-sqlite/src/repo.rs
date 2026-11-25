use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use rusqlite::{Connection, params};
use tavern_core::{GmUpdateOutcome, JoinPlayerOutcome, ResolvingLease, RoomStore};
use tavern_domain::{
    CharacterStats, ChatMode, DailyUsage, DirectMessageOut, EndingUnlock, LlmUsage, NarrativeStyle,
    PendingAction, Player, PresetRole, RoomSnapshot, RoomState, StarterCharacterDefaults,
    TurnRecord, UsageSummary, WhisperRecord, WorldEcho,
};

use crate::endings::{
    list_ending_unlocks_query, list_world_echoes_query, record_ending_unlock_query,
    record_world_echo_query,
};
use crate::m1_ops::{
    get_room_usage_query, get_today_usage_query, gm_add_clue_tx, gm_mutate_player_tx,
    gm_remove_clue_tx, list_player_whispers_query, record_usage_query, remove_player_tx,
    set_player_afk_tx,
};
use crate::mappers::{
    query_pending_actions, query_player_by_id, query_players, query_room,
    update_player_profile_sql, write_player, write_room,
};
use crate::migrations::run_migrations;
use crate::player_tx::{ensure_player_tx, join_player_tx, pick_role_tx};
use crate::turn_queries::{query_all_turns, query_recent_turns, query_turns_range};
use crate::turn_tx::{
    append_roll_tx, commit_turn_cas_tx, pop_and_restore_snapshot_tx, submit_action_tx,
    try_begin_resolving_tx,
};

#[derive(Clone)]
pub struct SqliteRoomStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteRoomStore {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建数据库目录失败: {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("打开 SQLite 数据库失败: {}", path.display()))?;
        Self::from_connection(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    pub fn from_connection(mut conn: Connection) -> Result<Self> {
        run_migrations(&mut conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T>) -> Result<T> {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| anyhow!("SQLite 连接锁异常（mutex poisoned）"))?;
        f(&mut guard)
    }
}

impl RoomStore for SqliteRoomStore {
    fn reset_stuck_resolving_on_startup(&self) -> Result<usize> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE rooms SET turn_phase = 'collecting', state_version = state_version + 1 WHERE turn_phase = 'resolving'",
                [],
            )?)
        })
    }

    fn upsert_room(&self, room: &RoomState) -> Result<()> {
        self.with_conn(|conn| write_room(conn, room))
    }

    fn backfill_room_guild_id(&self, channel_id: &str, guild_id: &str) -> Result<bool> {
        let g = guild_id.trim();
        if g.is_empty() {
            return Ok(false);
        }
        self.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE rooms SET guild_id = ?2 WHERE channel_id = ?1 AND guild_id = ''",
                params![channel_id, g],
            )? > 0)
        })
    }

    fn update_chat_mode(&self, channel_id: &str, mode: ChatMode) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE rooms SET chat_mode = ?2, updated_at = ?3 WHERE channel_id = ?1",
                params![channel_id, mode.as_str(), Utc::now().to_rfc3339()],
            )? > 0)
        })
    }

    fn update_narrative_style(&self, channel_id: &str, style: NarrativeStyle) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE rooms SET narrative_style = ?2, updated_at = ?3 WHERE channel_id = ?1",
                params![channel_id, style.as_str(), Utc::now().to_rfc3339()],
            )? > 0)
        })
    }

    fn get_room(&self, channel_id: &str) -> Result<Option<RoomState>> {
        self.with_conn(|conn| query_room(conn, channel_id))
    }

    fn delete_room(&self, channel_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute("DELETE FROM rooms WHERE channel_id = ?1", params![channel_id])? > 0)
        })
    }

    fn upsert_player(&self, player: &Player) -> Result<()> {
        self.with_conn(|conn| write_player(conn, player))
    }

    fn ensure_player_atomic(
        &self, channel_id: &str, user_id: &str, raw_name: &str, defaults: &StarterCharacterDefaults,
    ) -> Result<Option<Player>> {
        self.with_conn(|c| ensure_player_tx(c, channel_id, user_id, raw_name, defaults))
    }

    fn pick_role_atomic(
        &self, channel_id: &str, user_id: &str, raw_name: &str, preset: &PresetRole,
        defaults: &StarterCharacterDefaults,
    ) -> Result<Option<(Player, bool)>> {
        self.with_conn(|c| pick_role_tx(c, channel_id, user_id, raw_name, preset, defaults))
    }

    fn join_player_atomic(
        &self, channel_id: &str, user_id: &str, character_name: &str, role_desc: &str,
        traits: &[String], defaults: &StarterCharacterDefaults,
    ) -> Result<JoinPlayerOutcome> {
        self.with_conn(|c| join_player_tx(c, channel_id, user_id, character_name, role_desc, traits, defaults))
    }

    fn update_player_profile(
        &self, channel_id: &str, user_id: &str, character_name: &str, role_desc: &str,
        traits: &[String], stats: Option<&CharacterStats>,
    ) -> Result<Option<Player>> {
        self.with_conn(|c| update_player_profile_sql(c, channel_id, user_id, character_name, role_desc, traits, stats))
    }

    fn get_player(&self, channel_id: &str, user_id: &str) -> Result<Option<Player>> {
        self.with_conn(|conn| query_player_by_id(conn, channel_id, user_id))
    }

    fn list_players(&self, channel_id: &str) -> Result<Vec<Player>> {
        self.with_conn(|conn| query_players(conn, channel_id))
    }

    fn set_player_afk(
        &self,
        channel_id: &str,
        user_id: &str,
        is_afk: bool,
    ) -> Result<Option<Player>> {
        self.with_conn(|conn| set_player_afk_tx(conn, channel_id, user_id, is_afk))
    }

    fn remove_player(&self, channel_id: &str, user_id: &str) -> Result<Option<Player>> {
        self.with_conn(|conn| remove_player_tx(conn, channel_id, user_id))
    }

    fn gm_mutate_player(
        &self, channel_id: &str, player_name: &str, hp_delta: i32, san_delta: i32,
        add_item: Option<&str>, remove_item: Option<&str>,
    ) -> Result<GmUpdateOutcome<Player>> {
        self.with_conn(|c| gm_mutate_player_tx(c, channel_id, player_name, hp_delta, san_delta, add_item, remove_item))
    }

    fn gm_add_clue(&self, channel_id: &str, clue: &str) -> Result<GmUpdateOutcome<()>> {
        self.with_conn(|conn| gm_add_clue_tx(conn, channel_id, clue))
    }

    fn gm_remove_clue(&self, channel_id: &str, index_1based: usize) -> Result<GmUpdateOutcome<String>> {
        self.with_conn(|conn| gm_remove_clue_tx(conn, channel_id, index_1based))
    }

    fn submit_action_atomic(
        &self, channel_id: &str, user_id: &str, character_name: &str, action_text: &str,
    ) -> Result<Option<(u32, bool)>> {
        self.with_conn(|c| submit_action_tx(c, channel_id, user_id, character_name, action_text))
    }

    fn append_roll_atomic(
        &self, channel_id: &str, user_id: &str, character_name: &str, roll_note: &str,
    ) -> Result<Option<u32>> {
        self.with_conn(|c| append_roll_tx(c, channel_id, user_id, character_name, roll_note))
    }

    fn list_pending_actions(&self, channel_id: &str, target_turn: u32) -> Result<Vec<PendingAction>> {
        self.with_conn(|conn| query_pending_actions(conn, channel_id, target_turn))
    }

    fn list_recent_turns(&self, channel_id: &str, limit: usize) -> Result<Vec<TurnRecord>> {
        self.with_conn(|conn| query_recent_turns(conn, channel_id, limit))
    }

    fn list_all_turns(&self, channel_id: &str) -> Result<Vec<TurnRecord>> {
        self.with_conn(|conn| query_all_turns(conn, channel_id))
    }

    fn list_turns_range(
        &self,
        channel_id: &str,
        from_turn_exclusive: u32,
        to_turn_inclusive: u32,
    ) -> Result<Vec<TurnRecord>> {
        self.with_conn(|conn| {
            query_turns_range(conn, channel_id, from_turn_exclusive, to_turn_inclusive)
        })
    }

    fn try_begin_resolving(&self, channel_id: &str) -> Result<Option<ResolvingLease>> {
        self.with_conn(|conn| try_begin_resolving_tx(conn, channel_id))
    }

    fn abort_resolving(&self, channel_id: &str, expected_version: u64) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE rooms SET turn_phase = 'collecting', state_version = state_version + 1, updated_at = ?3 WHERE channel_id = ?1 AND state_version = ?2 AND turn_phase = 'resolving'",
                params![channel_id, expected_version as i64, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    fn commit_turn_cas(
        &self,
        expected_version: u64,
        snapshot: &RoomSnapshot,
        updated_room: &RoomState,
        updated_players: &[Player],
        turn: &TurnRecord,
        resolved_turn: u32,
        whispers: &[DirectMessageOut],
    ) -> Result<bool> {
        self.with_conn(|conn| {
            commit_turn_cas_tx(
                conn,
                expected_version,
                snapshot,
                updated_room,
                updated_players,
                turn,
                resolved_turn,
                whispers,
            )
        })
    }

    fn update_summary_cas(
        &self,
        channel_id: &str,
        expected_prev_upto: u32,
        expected_created_at: &str,
        summary: &str,
        summary_upto_turn: u32,
    ) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE rooms SET summary = ?4, summary_upto_turn = ?5, updated_at = ?6 \
                 WHERE channel_id = ?1 AND summary_upto_turn = ?2 AND created_at = ?3 AND turn_number >= ?5",
                params![
                    channel_id,
                    expected_prev_upto,
                    expected_created_at,
                    summary,
                    summary_upto_turn,
                    Utc::now().to_rfc3339(),
                ],
            )? > 0)
        })
    }

    fn pop_and_restore_snapshot(&self, channel_id: &str) -> Result<Option<RoomSnapshot>> {
        self.with_conn(|conn| pop_and_restore_snapshot_tx(conn, channel_id))
    }

    fn list_player_whispers(&self, channel_id: &str, user_id: &str, limit: usize) -> Result<Vec<WhisperRecord>> {
        self.with_conn(|conn| list_player_whispers_query(conn, channel_id, user_id, limit))
    }

    fn list_room_whispers(&self, channel_id: &str, limit: usize) -> Result<Vec<WhisperRecord>> {
        self.with_conn(|conn| crate::m1_ops::list_room_whispers_query(conn, channel_id, limit))
    }

    fn get_active_story_card_msg(&self, channel_id: &str) -> Result<Option<(String, u32)>> {
        self.with_conn(|conn| crate::m1_ops::get_active_story_card_msg_query(conn, channel_id))
    }

    fn set_active_story_card_msg(&self, channel_id: &str, msg_id: Option<(&str, u32)>) -> Result<()> {
        self.with_conn(|conn| crate::m1_ops::set_active_story_card_msg_query(conn, channel_id, msg_id))
    }

    fn record_usage(&self, channel_id: &str, usage: &LlmUsage, estimated_cny: f64) -> Result<()> {
        self.with_conn(|conn| record_usage_query(conn, channel_id, usage, estimated_cny))
    }

    fn get_room_usage(&self, channel_id: &str) -> Result<UsageSummary> {
        self.with_conn(|conn| get_room_usage_query(conn, channel_id))
    }

    fn get_today_usage(&self) -> Result<DailyUsage> {
        self.with_conn(|conn| get_today_usage_query(conn))
    }

    fn record_ending_unlock(&self, unlock: &EndingUnlock) -> Result<bool> {
        self.with_conn(|conn| record_ending_unlock_query(conn, unlock))
    }

    fn list_ending_unlocks(&self, guild_id: &str) -> Result<Vec<EndingUnlock>> {
        self.with_conn(|conn| list_ending_unlocks_query(conn, guild_id))
    }

    fn record_world_echo(&self, echo: &WorldEcho) -> Result<()> {
        self.with_conn(|conn| record_world_echo_query(conn, echo))
    }

    fn list_world_echoes(
        &self,
        scope_key: &str,
        scenario_id: &str,
        before_rfc3339: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WorldEcho>> {
        self.with_conn(|conn| {
            list_world_echoes_query(conn, scope_key, scenario_id, before_rfc3339, limit)
        })
    }
}
