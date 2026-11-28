use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, Row, params};
use tavern_core::TableStore;
use tavern_domain::ManagedTable;

use crate::repo::SqliteRoomStore;

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn map_table_row(r: &Row<'_>) -> rusqlite::Result<ManagedTable> {
    Ok(ManagedTable {
        channel_id: r.get(0)?,
        guild_id: r.get(1)?,
        lobby_channel_id: r.get(2)?,
        scenario_id: r.get(3)?,
        scenario_name: r.get(4)?,
        creator_user_id: r.get(5)?,
        created_at: parse_dt(&r.get::<_, String>(6)?),
    })
}

impl TableStore for SqliteRoomStore {
    fn update_manual_resolve(&self, channel_id: &str, manual: bool) -> Result<bool> {
        self.with_conn(|conn| {
            let affected = conn.execute(
                "UPDATE rooms SET manual_resolve = ?2, updated_at = ?3 WHERE channel_id = ?1",
                params![
                    channel_id,
                    if manual { 1 } else { 0 },
                    Utc::now().to_rfc3339()
                ],
            )?;
            Ok(affected > 0)
        })
    }

    fn register_managed_table(&self, table: &ManagedTable) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO managed_tables (
                    channel_id, guild_id, lobby_channel_id, scenario_id, scenario_name,
                    creator_user_id, created_at, closed_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, '')
                ON CONFLICT(channel_id) DO UPDATE SET
                    guild_id = excluded.guild_id,
                    lobby_channel_id = excluded.lobby_channel_id,
                    scenario_id = excluded.scenario_id,
                    scenario_name = excluded.scenario_name,
                    creator_user_id = excluded.creator_user_id,
                    created_at = excluded.created_at,
                    closed_at = ''
                "#,
                params![
                    table.channel_id,
                    table.guild_id,
                    table.lobby_channel_id,
                    table.scenario_id,
                    table.scenario_name,
                    table.creator_user_id,
                    table.created_at.to_rfc3339(),
                ],
            )?;
            Ok(())
        })
    }

    fn get_managed_table(&self, channel_id: &str) -> Result<Option<ManagedTable>> {
        self.with_conn(|conn| {
            let row = conn
                .query_row(
                    r#"
                    SELECT channel_id, guild_id, lobby_channel_id, scenario_id, scenario_name,
                           creator_user_id, created_at
                    FROM managed_tables
                    WHERE channel_id = ?1 AND closed_at = ''
                    "#,
                    params![channel_id],
                    map_table_row,
                )
                .optional()?;
            Ok(row)
        })
    }

    fn list_guild_managed_tables(&self, guild_id: &str) -> Result<Vec<ManagedTable>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare(
                r#"
                SELECT channel_id, guild_id, lobby_channel_id, scenario_id, scenario_name,
                       creator_user_id, created_at
                FROM managed_tables
                WHERE guild_id = ?1 AND closed_at = ''
                ORDER BY created_at DESC
                "#,
            )?;
            let rows = stmt.query_map(params![guild_id], map_table_row)?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
    }

    fn delete_managed_table(&self, channel_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let affected = conn.execute(
                "UPDATE managed_tables SET closed_at = ?2 WHERE channel_id = ?1 AND closed_at = ''",
                params![channel_id, Utc::now().to_rfc3339()],
            )?;
            Ok(affected > 0)
        })
    }

    fn user_table_cooldown_remaining(&self, user_id: &str, cooldown_secs: u64) -> Result<u64> {
        if cooldown_secs == 0 {
            return Ok(0);
        }
        self.with_conn(|conn| {
            let latest: Option<String> = conn
                .query_row(
                    "SELECT created_at FROM managed_tables WHERE creator_user_id = ?1 ORDER BY created_at DESC LIMIT 1",
                    params![user_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(ts) = latest else {
                return Ok(0);
            };
            let Ok(prev) = DateTime::parse_from_rfc3339(&ts) else {
                return Ok(0);
            };
            let elapsed = Utc::now()
                .signed_duration_since(prev.with_timezone(&Utc))
                .num_seconds()
                .max(0) as u64;
            Ok(cooldown_secs.saturating_sub(elapsed))
        })
    }
}
