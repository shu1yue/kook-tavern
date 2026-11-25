use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, Row, params};
use tavern_domain::TurnRecord;

pub fn query_recent_turns(
    conn: &Connection,
    channel_id: &str,
    limit: usize,
) -> Result<Vec<TurnRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel_id, turn_number, batched_input, scene_title, narrative, state_summary, location, world_time, created_at FROM turns WHERE channel_id = ?1 ORDER BY turn_number DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![channel_id, limit as i64], map_turn_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    out.reverse();
    Ok(out)
}

pub fn query_all_turns(conn: &Connection, channel_id: &str) -> Result<Vec<TurnRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel_id, turn_number, batched_input, scene_title, narrative, state_summary, location, world_time, created_at FROM turns WHERE channel_id = ?1 ORDER BY turn_number ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![channel_id], map_turn_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn query_turns_range(
    conn: &Connection,
    channel_id: &str,
    from_turn_exclusive: u32,
    to_turn_inclusive: u32,
) -> Result<Vec<TurnRecord>> {
    let mut stmt = conn.prepare(
        "SELECT id, channel_id, turn_number, batched_input, scene_title, narrative, state_summary, location, world_time, created_at FROM turns WHERE channel_id = ?1 AND turn_number > ?2 AND turn_number <= ?3 ORDER BY turn_number ASC, id ASC",
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

pub fn map_turn_row(r: &Row<'_>) -> rusqlite::Result<TurnRecord> {
    let created_str: String = r.get(9)?;
    Ok(TurnRecord {
        id: r.get(0)?,
        channel_id: r.get(1)?,
        turn_number: r.get(2)?,
        batched_input: r.get(3)?,
        scene_title: r.get(4)?,
        narrative: r.get(5)?,
        state_summary: r.get(6)?,
        location: r.get(7)?,
        world_time: r.get(8)?,
        created_at: DateTime::parse_from_rfc3339(&created_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
    })
}
