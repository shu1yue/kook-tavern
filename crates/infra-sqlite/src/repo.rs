use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use rusqlite::{Connection, params};
use tavern_core::{ResolvingLease, RoomStore};
use tavern_domain::{ChatMode, PendingAction, Player, RoomSnapshot, RoomState, TurnRecord};

use crate::mappers::{
    map_turn_row, query_pending_actions, query_player_by_id, query_players, query_room,
    write_player,
};
use crate::migrations::run_migrations;
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

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow!("SQLite 连接锁异常（mutex poisoned）"))
    }
}

impl RoomStore for SqliteRoomStore {
    fn reset_stuck_resolving_on_startup(&self) -> Result<usize> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE rooms SET turn_phase = 'collecting', state_version = state_version + 1 WHERE turn_phase = 'resolving'",
            [],
        )?;
        Ok(affected)
    }

    fn upsert_room(&self, room: &RoomState) -> Result<()> {
        let conn = self.lock()?;
        let clues_json = serde_json::to_string(&room.clues)?;
        conn.execute(
            r#"
            INSERT INTO rooms (
                channel_id, scenario_id, title, location, world_time,
                turn_number, state_version, turn_phase, chat_mode, narrative_style,
                web_token, clues_json, summary, summary_upto_turn,
                active, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)
            ON CONFLICT(channel_id) DO UPDATE SET
                scenario_id = excluded.scenario_id,
                title = excluded.title,
                location = excluded.location,
                world_time = excluded.world_time,
                turn_number = excluded.turn_number,
                state_version = excluded.state_version,
                turn_phase = excluded.turn_phase,
                chat_mode = excluded.chat_mode,
                narrative_style = excluded.narrative_style,
                web_token = excluded.web_token,
                clues_json = excluded.clues_json,
                summary = excluded.summary,
                summary_upto_turn = excluded.summary_upto_turn,
                active = excluded.active,
                updated_at = excluded.updated_at
            "#,
            params![
                room.channel_id,
                room.scenario_id,
                room.title,
                room.location,
                room.world_time,
                room.turn_number,
                room.state_version as i64,
                room.turn_phase.as_str(),
                room.chat_mode.as_str(),
                room.narrative_style.as_str(),
                room.web_token,
                clues_json,
                room.summary,
                room.summary_upto_turn,
                if room.active { 1 } else { 0 },
                room.created_at.to_rfc3339(),
                room.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    fn update_chat_mode(&self, channel_id: &str, mode: ChatMode) -> Result<bool> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE rooms SET chat_mode = ?2, updated_at = ?3 WHERE channel_id = ?1",
            params![channel_id, mode.as_str(), Utc::now().to_rfc3339()],
        )?;
        Ok(affected > 0)
    }

    fn get_room(&self, channel_id: &str) -> Result<Option<RoomState>> {
        let conn = self.lock()?;
        query_room(&conn, channel_id)
    }

    fn delete_room(&self, channel_id: &str) -> Result<bool> {
        let conn = self.lock()?;
        let affected = conn.execute("DELETE FROM rooms WHERE channel_id = ?1", params![channel_id])?;
        Ok(affected > 0)
    }

    fn upsert_player(&self, player: &Player) -> Result<()> {
        let conn = self.lock()?;
        write_player(&conn, player)
    }

    fn get_player(&self, channel_id: &str, user_id: &str) -> Result<Option<Player>> {
        let conn = self.lock()?;
        query_player_by_id(&conn, channel_id, user_id)
    }

    fn list_players(&self, channel_id: &str) -> Result<Vec<Player>> {
        let conn = self.lock()?;
        query_players(&conn, channel_id)
    }

    fn submit_action_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        action_text: &str,
    ) -> Result<Option<(u32, bool)>> {
        let mut conn = self.lock()?;
        submit_action_tx(&mut conn, channel_id, user_id, character_name, action_text)
    }

    fn append_roll_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        character_name: &str,
        roll_note: &str,
    ) -> Result<Option<u32>> {
        let mut conn = self.lock()?;
        append_roll_tx(&mut conn, channel_id, user_id, character_name, roll_note)
    }

    fn list_pending_actions(
        &self,
        channel_id: &str,
        target_turn: u32,
    ) -> Result<Vec<PendingAction>> {
        let conn = self.lock()?;
        query_pending_actions(&conn, channel_id, target_turn)
    }

    fn list_recent_turns(&self, channel_id: &str, limit: usize) -> Result<Vec<TurnRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, channel_id, turn_number, batched_input, scene_title,
                   narrative, state_summary, location, world_time, created_at
            FROM turns WHERE channel_id = ?1
            ORDER BY turn_number DESC, id DESC LIMIT ?2
            "#,
        )?;
        let rows = stmt.query_map(params![channel_id, limit as i64], map_turn_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        out.reverse();
        Ok(out)
    }

    fn list_all_turns(&self, channel_id: &str) -> Result<Vec<TurnRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, channel_id, turn_number, batched_input, scene_title,
                   narrative, state_summary, location, world_time, created_at
            FROM turns WHERE channel_id = ?1
            ORDER BY turn_number ASC, id ASC
            "#,
        )?;
        let rows = stmt.query_map(params![channel_id], map_turn_row)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn list_turns_range(
        &self,
        channel_id: &str,
        from_turn_exclusive: u32,
        to_turn_inclusive: u32,
    ) -> Result<Vec<TurnRecord>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            r#"
            SELECT id, channel_id, turn_number, batched_input, scene_title,
                   narrative, state_summary, location, world_time, created_at
            FROM turns
            WHERE channel_id = ?1 AND turn_number > ?2 AND turn_number <= ?3
            ORDER BY turn_number ASC, id ASC
            "#,
        )?;
        let rows = stmt.query_map(
            params![channel_id, from_turn_exclusive, to_turn_inclusive],
            map_turn_row,
        )?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn try_begin_resolving(&self, channel_id: &str) -> Result<Option<ResolvingLease>> {
        let mut conn = self.lock()?;
        try_begin_resolving_tx(&mut conn, channel_id)
    }

    fn abort_resolving(&self, channel_id: &str, expected_version: u64) -> Result<()> {
        let conn = self.lock()?;
        conn.execute(
            "UPDATE rooms SET turn_phase = 'collecting', state_version = state_version + 1, updated_at = ?3 WHERE channel_id = ?1 AND state_version = ?2 AND turn_phase = 'resolving'",
            params![channel_id, expected_version as i64, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    fn commit_turn_cas(
        &self,
        expected_version: u64,
        snapshot: &RoomSnapshot,
        updated_room: &RoomState,
        updated_players: &[Player],
        turn: &TurnRecord,
        resolved_turn: u32,
    ) -> Result<bool> {
        let mut conn = self.lock()?;
        commit_turn_cas_tx(
            &mut conn,
            expected_version,
            snapshot,
            updated_room,
            updated_players,
            turn,
            resolved_turn,
        )
    }

    fn update_summary_cas(
        &self,
        channel_id: &str,
        expected_version: u64,
        summary: &str,
        summary_upto_turn: u32,
    ) -> Result<bool> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "UPDATE rooms SET summary = ?3, summary_upto_turn = ?4, updated_at = ?5 WHERE channel_id = ?1 AND state_version = ?2",
            params![
                channel_id,
                expected_version as i64,
                summary,
                summary_upto_turn,
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(affected > 0)
    }

    fn pop_and_restore_snapshot(&self, channel_id: &str) -> Result<Option<RoomSnapshot>> {
        let mut conn = self.lock()?;
        pop_and_restore_snapshot_tx(&mut conn, channel_id)
    }
}
