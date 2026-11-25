use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use tavern_domain::EndingUnlock;

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
