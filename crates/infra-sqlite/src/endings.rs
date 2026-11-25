use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use tavern_domain::{EndingUnlock, WorldEcho};

/// Records an ending achievement scoped to `unlock.guild_id` (fail-closed if empty).
/// Returns `true` if this is the first time unlocked in this scope.
pub fn record_ending_unlock_query(conn: &Connection, unlock: &EndingUnlock) -> Result<bool> {
    let scope_key = unlock.guild_id.trim();
    if scope_key.is_empty() {
        return Ok(false);
    }
    let inserted = conn.execute(
        r#"
        INSERT OR IGNORE INTO ending_unlocks (
            guild_id, scenario_id, ending_id, ending_title,
            ending_kind, turn_count, party_names, unlocked_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            scope_key,
            unlock.scenario_id,
            unlock.ending_id,
            unlock.ending_title,
            unlock.ending_kind,
            unlock.turn_count,
            unlock.party_names,
            unlock.unlocked_at.to_rfc3339(),
        ],
    )?;
    Ok(inserted > 0)
}

pub fn list_ending_unlocks_query(conn: &Connection, scope_key: &str) -> Result<Vec<EndingUnlock>> {
    let scope_key = scope_key.trim();
    if scope_key.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        r#"
        SELECT guild_id, scenario_id, ending_id, ending_title,
               ending_kind, turn_count, party_names, unlocked_at
        FROM ending_unlocks
        WHERE guild_id = ?1
        ORDER BY unlocked_at DESC
        LIMIT 500
        "#,
    )?;
    let rows = stmt.query_map(params![scope_key], |r| {
        let dt_str: String = r.get(7)?;
        let unlocked_at = DateTime::parse_from_rfc3339(&dt_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Ok(EndingUnlock {
            guild_id: r.get(0)?,
            scenario_id: r.get(1)?,
            ending_id: r.get(2)?,
            ending_title: r.get(3)?,
            ending_kind: r.get(4)?,
            turn_count: r.get::<_, i64>(5)?.max(0) as u32,
            party_names: r.get(6)?,
            unlocked_at,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Persists a worldline reincarnation echo scoped to `echo.guild_id` (fail-closed if empty),
/// deduplicates same `(guild, scenario, source_channel, ending_id)` so `/undo` retries never flood,
/// and keeps at most the latest 12 echoes per `(guild_id, scenario_id)`.
pub fn record_world_echo_query(conn: &Connection, echo: &WorldEcho) -> Result<()> {
    let scope_key = echo.guild_id.trim();
    if scope_key.is_empty() || echo.echo_text.trim().is_empty() {
        return Ok(());
    }
    if !echo.source_channel_id.trim().is_empty() {
        conn.execute(
            r#"
            DELETE FROM world_echoes
            WHERE guild_id = ?1 AND scenario_id = ?2 AND source_channel_id = ?3 AND ending_id = ?4
            "#,
            params![scope_key, echo.scenario_id, echo.source_channel_id, echo.ending_id],
        )?;
    }
    conn.execute(
        r#"
        INSERT INTO world_echoes (
            guild_id, scenario_id, source_channel_id, source_turn,
            ending_id, ending_title, ending_kind, party_names, echo_text, turn_count, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
        "#,
        params![
            scope_key,
            echo.scenario_id,
            echo.source_channel_id,
            echo.source_turn,
            echo.ending_id,
            echo.ending_title,
            echo.ending_kind,
            echo.party_names,
            echo.echo_text,
            echo.turn_count,
            echo.created_at.to_rfc3339(),
        ],
    )?;
    conn.execute(
        r#"
        DELETE FROM world_echoes
        WHERE guild_id = ?1 AND scenario_id = ?2
          AND id NOT IN (
              SELECT id FROM world_echoes
              WHERE guild_id = ?1 AND scenario_id = ?2
              ORDER BY id DESC LIMIT 12
          )
        "#,
        params![scope_key, echo.scenario_id],
    )?;
    Ok(())
}

pub fn list_world_echoes_query(
    conn: &Connection,
    scope_key: &str,
    scenario_id: &str,
    before_rfc3339: Option<&str>,
    limit: usize,
) -> Result<Vec<WorldEcho>> {
    let scope_key = scope_key.trim();
    if scope_key.is_empty() || scenario_id.trim().is_empty() || limit == 0 {
        return Ok(Vec::new());
    }
    let lim = limit.min(12) as i64;
    let before = before_rfc3339.map(str::trim).unwrap_or("");
    let mut stmt = conn.prepare(
        r#"
        SELECT id, guild_id, scenario_id, source_channel_id, source_turn,
               ending_id, ending_title, ending_kind, party_names, echo_text, turn_count, created_at
        FROM world_echoes
        WHERE guild_id = ?1 AND scenario_id = ?2
          AND (?3 = '' OR created_at < ?3)
        ORDER BY id DESC
        LIMIT ?4
        "#,
    )?;
    let rows = stmt.query_map(params![scope_key, scenario_id.trim(), before, lim], |r| {
        let dt_str: String = r.get(11)?;
        let created_at = DateTime::parse_from_rfc3339(&dt_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());
        Ok(WorldEcho {
            id: r.get(0)?,
            guild_id: r.get(1)?,
            scenario_id: r.get(2)?,
            source_channel_id: r.get(3)?,
            source_turn: r.get::<_, i64>(4)?.max(0) as u32,
            ending_id: r.get(5)?,
            ending_title: r.get(6)?,
            ending_kind: r.get(7)?,
            party_names: r.get(8)?,
            echo_text: r.get(9)?,
            turn_count: r.get::<_, i64>(10)?.max(0) as u32,
            created_at,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}
