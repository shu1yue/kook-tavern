use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use tavern_core::ResolvingLease;
use tavern_domain::{
    DirectMessageOut, PendingAction, Player, RoomSnapshot, RoomState, TurnPhase, TurnRecord,
};

use crate::mappers::{
    query_pending_actions, query_players, query_room, update_player_dm_state, write_pending_action,
    write_player,
};

pub fn submit_action_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    character_name: &str,
    action_text: &str,
) -> Result<Option<(u32, bool)>> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(None);
    };
    if !room.active {
        return Ok(None);
    }

    let is_resolving = room.turn_phase == TurnPhase::Resolving;
    let target_turn = if is_resolving {
        room.turn_number + 2
    } else {
        room.turn_number + 1
    };

    let existing_rolls: Vec<String> = tx
        .query_row(
            "SELECT roll_notes_json FROM pending_actions WHERE channel_id = ?1 AND user_id = ?2 AND target_turn = ?3",
            params![channel_id, user_id, target_turn],
            |r| r.get::<_, String>(0),
        )
        .optional()?
        .and_then(|j| serde_json::from_str(&j).ok())
        .unwrap_or_default();

    let action = PendingAction {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        target_turn,
        character_name: character_name.to_string(),
        action_text: action_text.to_string(),
        roll_notes: existing_rolls,
        updated_at: Utc::now(),
    };
    write_pending_action(&tx, &action)?;
    tx.commit()?;
    Ok(Some((target_turn, is_resolving)))
}

pub fn append_roll_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    character_name: &str,
    roll_note: &str,
) -> Result<Option<u32>> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(None);
    };
    if !room.active {
        return Ok(None);
    }

    let target_turn = if room.turn_phase == TurnPhase::Resolving {
        room.turn_number + 2
    } else {
        room.turn_number + 1
    };

    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT action_text, roll_notes_json FROM pending_actions WHERE channel_id = ?1 AND user_id = ?2 AND target_turn = ?3",
            params![channel_id, user_id, target_turn],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;

    let (action_text, mut rolls) = match existing {
        Some((text, rolls_json)) => (text, serde_json::from_str(&rolls_json).unwrap_or_default()),
        None => (String::new(), Vec::new()),
    };
    rolls.push(roll_note.to_string());

    let action = PendingAction {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        target_turn,
        character_name: character_name.to_string(),
        action_text,
        roll_notes: rolls,
        updated_at: Utc::now(),
    };
    write_pending_action(&tx, &action)?;
    tx.commit()?;
    Ok(Some(target_turn))
}

pub fn try_begin_resolving_tx(
    conn: &mut Connection,
    channel_id: &str,
) -> Result<Option<ResolvingLease>> {
    let tx = conn.transaction()?;
    let Some(mut room) = query_room(&tx, channel_id)? else {
        return Ok(None);
    };
    if !room.active || room.turn_phase != TurnPhase::Collecting {
        return Ok(None);
    }

    let new_version = room.state_version + 1;
    let updated = tx.execute(
        "UPDATE rooms SET turn_phase = 'resolving', state_version = ?2, updated_at = ?3 WHERE channel_id = ?1 AND state_version = ?4 AND turn_phase = 'collecting'",
        params![
            channel_id,
            new_version as i64,
            Utc::now().to_rfc3339(),
            room.state_version as i64
        ],
    )?;
    if updated == 0 {
        return Ok(None);
    }

    room.state_version = new_version;
    room.turn_phase = TurnPhase::Resolving;
    let target_turn = room.turn_number + 1;
    let players = query_players(&tx, channel_id)?;
    let actions = query_pending_actions(&tx, channel_id, target_turn)?;
    tx.commit()?;

    Ok(Some(ResolvingLease {
        expected_version: new_version,
        target_turn,
        room,
        players,
        actions,
    }))
}

#[allow(clippy::too_many_arguments)]
pub fn commit_turn_cas_tx(
    conn: &mut Connection,
    expected_version: u64,
    snapshot: &RoomSnapshot,
    updated_room: &RoomState,
    updated_players: &[Player],
    turn: &TurnRecord,
    resolved_turn: u32,
    whispers: &[DirectMessageOut],
) -> Result<bool> {
    let tx = conn.transaction()?;

    let clues_json = serde_json::to_string(&updated_room.clues)?;
    let affected = tx.execute(
        r#"
        UPDATE rooms SET
            location = ?3, world_time = ?4, turn_number = ?5, state_version = ?6,
            turn_phase = 'collecting', clues_json = ?7, summary = ?8,
            summary_upto_turn = ?9, updated_at = ?10
        WHERE channel_id = ?1 AND state_version = ?2 AND turn_phase = 'resolving'
        "#,
        params![
            updated_room.channel_id,
            expected_version as i64,
            updated_room.location,
            updated_room.world_time,
            updated_room.turn_number,
            updated_room.state_version as i64,
            clues_json,
            updated_room.summary,
            updated_room.summary_upto_turn,
            updated_room.updated_at.to_rfc3339(),
        ],
    )?;
    if affected == 0 {
        return Ok(false);
    }

    let snap_json = serde_json::to_string(snapshot)?;
    tx.execute(
        "INSERT INTO snapshots (channel_id, turn_number, snapshot_json, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            snapshot.room.channel_id,
            snapshot.room.turn_number,
            snap_json,
            Utc::now().to_rfc3339()
        ],
    )?;
    tx.execute(
        r#"
        DELETE FROM snapshots WHERE channel_id = ?1
          AND id NOT IN (SELECT id FROM snapshots WHERE channel_id = ?1 ORDER BY id DESC LIMIT 15)
        "#,
        params![snapshot.room.channel_id],
    )?;

    // Only update DM-authoritative stats/inventory/status so concurrent `/join` edits survive!
    for p in updated_players {
        update_player_dm_state(&tx, p)?;
    }

    tx.execute(
        r#"
        INSERT INTO turns (
            channel_id, turn_number, batched_input, scene_title,
            narrative, state_summary, location, world_time, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        "#,
        params![
            turn.channel_id,
            turn.turn_number,
            turn.batched_input,
            turn.scene_title,
            turn.narrative,
            turn.state_summary,
            turn.location,
            turn.world_time,
            turn.created_at.to_rfc3339(),
        ],
    )?;

    for w in whispers {
        tx.execute(
            r#"
            INSERT INTO whispers (channel_id, turn_number, user_id, player_name, content, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            "#,
            params![
                updated_room.channel_id,
                turn.turn_number,
                w.user_id,
                w.player_name,
                w.content,
                Utc::now().to_rfc3339(),
            ],
        )?;
    }

    tx.execute(
        "DELETE FROM pending_actions WHERE channel_id = ?1 AND target_turn <= ?2",
        params![updated_room.channel_id, resolved_turn],
    )?;

    tx.commit()?;
    Ok(true)
}

pub fn pop_and_restore_snapshot_tx(
    conn: &mut Connection,
    channel_id: &str,
) -> Result<Option<RoomSnapshot>> {
    let tx = conn.transaction()?;

    let found: Option<(i64, String)> = tx
        .query_row(
            "SELECT id, snapshot_json FROM snapshots WHERE channel_id = ?1 ORDER BY id DESC LIMIT 1",
            params![channel_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((snap_id, snap_json)) = found else {
        return Ok(None);
    };
    let mut snapshot: RoomSnapshot = serde_json::from_str(&snap_json)?;

    let current_version: i64 = tx
        .query_row(
            "SELECT state_version FROM rooms WHERE channel_id = ?1",
            params![channel_id],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or(1);

    let clues_json = serde_json::to_string(&snapshot.room.clues)?;
    tx.execute(
        r#"
        UPDATE rooms SET
            scenario_id = ?2, title = ?3, location = ?4, world_time = ?5,
            turn_number = ?6, state_version = ?7, turn_phase = 'collecting',
            chat_mode = ?8, narrative_style = ?9, clues_json = ?10,
            summary = ?11, summary_upto_turn = ?12, active = ?13, updated_at = ?14
        WHERE channel_id = ?1
        "#,
        params![
            snapshot.room.channel_id,
            snapshot.room.scenario_id,
            snapshot.room.title,
            snapshot.room.location,
            snapshot.room.world_time,
            snapshot.room.turn_number,
            current_version + 1,
            snapshot.room.chat_mode.as_str(),
            snapshot.room.narrative_style.as_str(),
            clues_json,
            snapshot.room.summary,
            snapshot.room.summary_upto_turn,
            if snapshot.room.active { 1 } else { 0 },
            Utc::now().to_rfc3339(),
        ],
    )?;

    tx.execute("DELETE FROM players WHERE channel_id = ?1", params![channel_id])?;
    for p in &snapshot.players {
        write_player(&tx, p)?;
    }

    tx.execute(
        "DELETE FROM pending_actions WHERE channel_id = ?1",
        params![channel_id],
    )?;
    // Normalize legacy M0 snapshots where `target_turn` deserialized as 0
    for a in &mut snapshot.pending_actions {
        if a.target_turn == 0 {
            a.target_turn = snapshot.room.turn_number + 1;
        }
        write_pending_action(&tx, a)?;
    }

    tx.execute(
        "DELETE FROM turns WHERE channel_id = ?1 AND turn_number > ?2",
        params![channel_id, snapshot.room.turn_number],
    )?;
    tx.execute(
        "DELETE FROM whispers WHERE channel_id = ?1 AND turn_number > ?2",
        params![channel_id, snapshot.room.turn_number],
    )?;
    tx.execute("DELETE FROM snapshots WHERE id = ?1", params![snap_id])?;

    tx.commit()?;
    Ok(Some(snapshot))
}
