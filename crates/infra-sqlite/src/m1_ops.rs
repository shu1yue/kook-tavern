use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use tavern_core::GmUpdateOutcome;
use tavern_domain::{DailyUsage, LlmUsage, Player, TurnPhase, UsageSummary, WhisperRecord};

use crate::mappers::{query_player_by_id, query_players, query_room, update_player_dm_state};

pub fn set_player_afk_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    is_afk: bool,
) -> Result<Option<Player>> {
    let tx = conn.transaction()?;
    let affected = tx.execute(
        "UPDATE players SET is_afk = ?3 WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, user_id, if is_afk { 1 } else { 0 }],
    )?;
    if affected == 0 {
        return Ok(None);
    }
    let player = query_player_by_id(&tx, channel_id, user_id)?;
    tx.commit()?;
    Ok(player)
}

pub fn remove_player_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
) -> Result<Option<Player>> {
    let tx = conn.transaction()?;
    let Some(player) = query_player_by_id(&tx, channel_id, user_id)? else {
        return Ok(None);
    };
    tx.execute(
        "DELETE FROM pending_actions WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, user_id],
    )?;
    tx.execute(
        "DELETE FROM players WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, user_id],
    )?;
    tx.commit()?;
    Ok(Some(player))
}

pub fn gm_mutate_player_tx(
    conn: &mut Connection,
    channel_id: &str,
    player_name: &str,
    hp_delta: i32,
    san_delta: i32,
    add_item: Option<&str>,
    remove_item: Option<&str>,
) -> Result<GmUpdateOutcome<Player>> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(GmUpdateOutcome::NotFound);
    };
    if !room.active {
        return Ok(GmUpdateOutcome::NotFound);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(GmUpdateOutcome::RoomResolving);
    }

    let target = player_name.trim();
    let players = query_players(&tx, channel_id)?;
    let Some(mut player) = players
        .into_iter()
        .find(|p| p.character_name == target || p.character_name.eq_ignore_ascii_case(target))
    else {
        return Ok(GmUpdateOutcome::NotFound);
    };

    if hp_delta != 0 {
        player.hp = (player.hp + hp_delta).clamp(0, player.max_hp);
    }
    if san_delta != 0 {
        player.san = (player.san + san_delta).clamp(0, player.max_san);
    }
    if let Some(item) = add_item.map(str::trim).filter(|s| !s.is_empty())
        && !player.inventory.iter().any(|i| i == item)
    {
        player.inventory.push(item.to_string());
    }
    if let Some(item) = remove_item.map(str::trim).filter(|s| !s.is_empty())
        && let Some(pos) = player
            .inventory
            .iter()
            .position(|i| i == item || i.eq_ignore_ascii_case(item))
    {
        player.inventory.remove(pos);
    }

    update_player_dm_state(&tx, &player)?;
    tx.commit()?;
    Ok(GmUpdateOutcome::Updated(player))
}

pub fn gm_add_clue_tx(
    conn: &mut Connection,
    channel_id: &str,
    clue: &str,
) -> Result<GmUpdateOutcome<()>> {
    let tx = conn.transaction()?;
    let Some(mut room) = query_room(&tx, channel_id)? else {
        return Ok(GmUpdateOutcome::NotFound);
    };
    if !room.active {
        return Ok(GmUpdateOutcome::NotFound);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(GmUpdateOutcome::RoomResolving);
    }
    let clue = clue.trim();
    if clue.is_empty() {
        return Ok(GmUpdateOutcome::NotFound);
    }
    if !room.clues.iter().any(|c| c == clue) {
        room.clues.push(clue.to_string());
        let clues_json = serde_json::to_string(&room.clues)?;
        tx.execute(
            "UPDATE rooms SET clues_json = ?2, updated_at = ?3 WHERE channel_id = ?1",
            params![channel_id, clues_json, Utc::now().to_rfc3339()],
        )?;
    }
    tx.commit()?;
    Ok(GmUpdateOutcome::Updated(()))
}

pub fn gm_remove_clue_tx(
    conn: &mut Connection,
    channel_id: &str,
    index_1based: usize,
) -> Result<GmUpdateOutcome<String>> {
    let tx = conn.transaction()?;
    let Some(mut room) = query_room(&tx, channel_id)? else {
        return Ok(GmUpdateOutcome::NotFound);
    };
    if !room.active {
        return Ok(GmUpdateOutcome::NotFound);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(GmUpdateOutcome::RoomResolving);
    }
    if index_1based == 0 || index_1based > room.clues.len() {
        return Ok(GmUpdateOutcome::NotFound);
    }
    let removed = room.clues.remove(index_1based - 1);
    let clues_json = serde_json::to_string(&room.clues)?;
    tx.execute(
        "UPDATE rooms SET clues_json = ?2, updated_at = ?3 WHERE channel_id = ?1",
        params![channel_id, clues_json, Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(GmUpdateOutcome::Updated(removed))
}

pub fn list_player_whispers_query(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
    limit: usize,
) -> Result<Vec<WhisperRecord>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT id, channel_id, turn_number, user_id, player_name, content, created_at
        FROM whispers
        WHERE channel_id = ?1 AND user_id = ?2
        ORDER BY id DESC
        LIMIT ?3
        "#,
    )?;
    let rows = stmt.query_map(params![channel_id, user_id, limit as i64], |r| {
        let created_str: String = r.get(6)?;
        Ok(WhisperRecord {
            id: r.get(0)?,
            channel_id: r.get(1)?,
            turn_number: r.get(2)?,
            user_id: r.get(3)?,
            player_name: r.get(4)?,
            content: r.get(5)?,
            created_at: DateTime::parse_from_rfc3339(&created_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    out.reverse();
    Ok(out)
}

pub fn record_usage_query(
    conn: &Connection,
    channel_id: &str,
    usage: &LlmUsage,
    estimated_cny: f64,
) -> Result<()> {
    let now = Utc::now();
    let day_utc = now.format("%Y-%m-%d").to_string();
    conn.execute(
        r#"
        INSERT INTO usage_logs (
            channel_id, day_utc, prompt_tokens, completion_tokens,
            cache_hit_tokens, cache_miss_tokens, estimated_cny, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            channel_id,
            day_utc,
            usage.prompt_tokens as i64,
            usage.completion_tokens as i64,
            usage.cache_hit_tokens as i64,
            usage.cache_miss_tokens as i64,
            estimated_cny,
            now.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn get_room_usage_query(conn: &Connection, channel_id: &str) -> Result<UsageSummary> {
    let summary = conn.query_row(
        r#"
        SELECT COUNT(*),
               COALESCE(SUM(prompt_tokens), 0),
               COALESCE(SUM(completion_tokens), 0),
               COALESCE(SUM(cache_hit_tokens), 0),
               COALESCE(SUM(cache_miss_tokens), 0),
               COALESCE(SUM(estimated_cny), 0.0)
        FROM usage_logs
        WHERE channel_id = ?1
        "#,
        params![channel_id],
        |r| {
            Ok(UsageSummary {
                total_calls: r.get::<_, i64>(0)?.max(0) as u64,
                prompt_tokens: r.get::<_, i64>(1)?.max(0) as u64,
                completion_tokens: r.get::<_, i64>(2)?.max(0) as u64,
                cache_hit_tokens: r.get::<_, i64>(3)?.max(0) as u64,
                cache_miss_tokens: r.get::<_, i64>(4)?.max(0) as u64,
                estimated_cny: r.get::<_, f64>(5)?,
            })
        },
    )?;
    Ok(summary)
}

pub fn get_today_usage_query(conn: &Connection) -> Result<DailyUsage> {
    let day_utc = Utc::now().format("%Y-%m-%d").to_string();
    let (calls, estimated_cny) = conn.query_row(
        r#"
        SELECT COUNT(*), COALESCE(SUM(estimated_cny), 0.0)
        FROM usage_logs
        WHERE day_utc = ?1
        "#,
        params![&day_utc],
        |r| Ok((r.get::<_, i64>(0)?.max(0) as u64, r.get::<_, f64>(1)?)),
    )?;
    Ok(DailyUsage {
        day_utc,
        calls,
        estimated_cny,
    })
}
