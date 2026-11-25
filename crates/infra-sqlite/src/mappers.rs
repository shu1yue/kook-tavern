use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use tavern_domain::{
    ChatMode, NarrativeStyle, PendingAction, Player, RoomState, TurnPhase, TurnRecord,
};

pub fn query_room(conn: &Connection, channel_id: &str) -> Result<Option<RoomState>> {
    let row = conn
        .query_row(
            r#"
            SELECT channel_id, guild_id, scenario_id, title, location, world_time,
                   turn_number, state_version, turn_phase, chat_mode, narrative_style,
                   gm_user_id, web_token, clues_json, suggested_actions_json, summary,
                   summary_upto_turn, active, created_at, updated_at
            FROM rooms WHERE channel_id = ?1
            "#,
            params![channel_id],
            |r| {
                Ok(RoomState {
                    channel_id: r.get(0)?,
                    guild_id: r.get(1)?,
                    scenario_id: r.get(2)?,
                    title: r.get(3)?,
                    location: r.get(4)?,
                    world_time: r.get(5)?,
                    turn_number: r.get(6)?,
                    state_version: r.get::<_, i64>(7)?.max(1) as u64,
                    turn_phase: TurnPhase::parse(&r.get::<_, String>(8)?),
                    chat_mode: ChatMode::parse(&r.get::<_, String>(9)?).unwrap_or(ChatMode::Strict),
                    narrative_style: NarrativeStyle::parse(&r.get::<_, String>(10)?)
                        .unwrap_or(NarrativeStyle::Prose),
                    gm_user_id: r.get(11)?,
                    web_token: r.get(12)?,
                    clues: serde_json::from_str(&r.get::<_, String>(13)?).unwrap_or_default(),
                    suggested_actions: serde_json::from_str(&r.get::<_, String>(14)?)
                        .unwrap_or_default(),
                    summary: r.get(15)?,
                    summary_upto_turn: r.get(16)?,
                    active: r.get::<_, i32>(17)? != 0,
                    created_at: parse_dt(&r.get::<_, String>(18)?),
                    updated_at: parse_dt(&r.get::<_, String>(19)?),
                })
            },
        )
        .optional()?;
    Ok(row)
}

pub fn write_room(conn: &Connection, room: &RoomState) -> Result<()> {
    let clues_json = serde_json::to_string(&room.clues)?;
    let suggested_json = serde_json::to_string(&room.suggested_actions)?;
    conn.execute(
        r#"
        INSERT INTO rooms (
            channel_id, guild_id, scenario_id, title, location, world_time,
            turn_number, state_version, turn_phase, chat_mode, narrative_style,
            gm_user_id, web_token, clues_json, suggested_actions_json, summary,
            summary_upto_turn, active, created_at, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
        ON CONFLICT(channel_id) DO UPDATE SET
            guild_id = excluded.guild_id, scenario_id = excluded.scenario_id, title = excluded.title,
            location = excluded.location, world_time = excluded.world_time,
            turn_number = excluded.turn_number, state_version = excluded.state_version,
            turn_phase = excluded.turn_phase, chat_mode = excluded.chat_mode,
            narrative_style = excluded.narrative_style, gm_user_id = excluded.gm_user_id,
            web_token = excluded.web_token, clues_json = excluded.clues_json,
            suggested_actions_json = excluded.suggested_actions_json,
            summary = excluded.summary, summary_upto_turn = excluded.summary_upto_turn,
            active = excluded.active, updated_at = excluded.updated_at
        "#,
        params![
            room.channel_id, room.guild_id, room.scenario_id, room.title,
            room.location, room.world_time, room.turn_number, room.state_version as i64,
            room.turn_phase.as_str(), room.chat_mode.as_str(), room.narrative_style.as_str(),
            room.gm_user_id, room.web_token, clues_json, suggested_json, room.summary,
            room.summary_upto_turn, if room.active { 1 } else { 0 },
            room.created_at.to_rfc3339(), room.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn write_player(conn: &Connection, player: &Player) -> Result<()> {
    let traits_json = serde_json::to_string(&player.traits)?;
    let inv_json = serde_json::to_string(&player.inventory)?;
    conn.execute(
        r#"
        INSERT INTO players (
            channel_id, user_id, character_name, role_desc, traits_json,
            hp, max_hp, san, max_san, inventory_json, status_note, is_afk
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        ON CONFLICT(channel_id, user_id) DO UPDATE SET
            character_name = excluded.character_name,
            role_desc = excluded.role_desc,
            traits_json = excluded.traits_json,
            hp = excluded.hp,
            max_hp = excluded.max_hp,
            san = excluded.san,
            max_san = excluded.max_san,
            inventory_json = excluded.inventory_json,
            status_note = excluded.status_note,
            is_afk = excluded.is_afk
        "#,
        params![
            player.channel_id,
            player.user_id,
            player.character_name,
            player.role_desc,
            traits_json,
            player.hp,
            player.max_hp,
            player.san,
            player.max_san,
            inv_json,
            player.status_note,
            if player.is_afk { 1 } else { 0 },
        ],
    )?;
    Ok(())
}

/// Updates ONLY player-authored profile fields (`character_name`, `role_desc`, `traits`, `is_afk = 0`)
/// so re-running `/join` never clobbers DM-authoritative HP, SAN, inventory, or status_note.
pub fn update_player_profile_sql(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
    character_name: &str,
    role_desc: &str,
    traits: &[String],
) -> Result<Option<Player>> {
    let traits_json = serde_json::to_string(traits)?;
    let affected = conn.execute(
        r#"
        UPDATE players SET
            character_name = ?3,
            role_desc = ?4,
            traits_json = ?5,
            is_afk = 0
        WHERE channel_id = ?1 AND user_id = ?2
        "#,
        params![channel_id, user_id, character_name, role_desc, traits_json],
    )?;
    if affected == 0 {
        return Ok(None);
    }
    query_player_by_id(conn, channel_id, user_id)
}

/// Updates ONLY DM-authoritative runtime state columns during `commit_turn_cas`,
/// never overwriting concurrent `/join` edits to `character_name`, `role_desc`, `traits`, or `is_afk`.
pub fn update_player_dm_state(conn: &Connection, player: &Player) -> Result<()> {
    let inv_json = serde_json::to_string(&player.inventory)?;
    conn.execute(
        r#"
        UPDATE players SET
            hp = ?3,
            san = ?4,
            inventory_json = ?5,
            status_note = ?6
        WHERE channel_id = ?1 AND user_id = ?2
        "#,
        params![
            player.channel_id,
            player.user_id,
            player.hp,
            player.san,
            inv_json,
            player.status_note,
        ],
    )?;
    Ok(())
}

pub fn query_player_by_id(
    conn: &Connection,
    channel_id: &str,
    user_id: &str,
) -> Result<Option<Player>> {
    let row = conn
        .query_row(
            r#"
            SELECT channel_id, user_id, character_name, role_desc, traits_json,
                   hp, max_hp, san, max_san, inventory_json, status_note, is_afk
            FROM players WHERE channel_id = ?1 AND user_id = ?2
            "#,
            params![channel_id, user_id],
            map_player_row,
        )
        .optional()?;
    Ok(row)
}

pub fn query_players(conn: &Connection, channel_id: &str) -> Result<Vec<Player>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT channel_id, user_id, character_name, role_desc, traits_json,
               hp, max_hp, san, max_san, inventory_json, status_note, is_afk
        FROM players WHERE channel_id = ?1 ORDER BY rowid ASC
        "#,
    )?;
    let rows = stmt.query_map(params![channel_id], map_player_row)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn map_player_row(r: &Row<'_>) -> rusqlite::Result<Player> {
    Ok(Player {
        channel_id: r.get(0)?,
        user_id: r.get(1)?,
        character_name: r.get(2)?,
        role_desc: r.get(3)?,
        traits: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
        hp: r.get(5)?,
        max_hp: r.get(6)?,
        san: r.get(7)?,
        max_san: r.get(8)?,
        inventory: serde_json::from_str(&r.get::<_, String>(9)?).unwrap_or_default(),
        status_note: r.get(10)?,
        is_afk: r.get::<_, i32>(11)? != 0,
    })
}

pub fn write_pending_action(conn: &Connection, action: &PendingAction) -> Result<()> {
    let rolls_json = serde_json::to_string(&action.roll_notes)?;
    conn.execute(
        r#"
        INSERT INTO pending_actions (
            channel_id, user_id, target_turn, character_name, action_text, roll_notes_json, updated_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        ON CONFLICT(channel_id, user_id, target_turn) DO UPDATE SET
            character_name = excluded.character_name,
            action_text = excluded.action_text,
            roll_notes_json = excluded.roll_notes_json,
            updated_at = excluded.updated_at
        "#,
        params![
            action.channel_id,
            action.user_id,
            action.target_turn,
            action.character_name,
            action.action_text,
            rolls_json,
            action.updated_at.to_rfc3339(),
        ],
    )?;
    Ok(())
}

pub fn query_pending_actions(
    conn: &Connection,
    channel_id: &str,
    target_turn: u32,
) -> Result<Vec<PendingAction>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT channel_id, user_id, target_turn, character_name, action_text, roll_notes_json, updated_at
        FROM pending_actions
        WHERE channel_id = ?1 AND target_turn = ?2
        ORDER BY updated_at ASC
        "#,
    )?;
    let rows = stmt.query_map(params![channel_id, target_turn], |r| {
        Ok(PendingAction {
            channel_id: r.get(0)?,
            user_id: r.get(1)?,
            target_turn: r.get(2)?,
            character_name: r.get(3)?,
            action_text: r.get(4)?,
            roll_notes: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
            updated_at: parse_dt(&r.get::<_, String>(6)?),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub fn query_recent_turns(conn: &Connection, channel_id: &str, limit: usize) -> Result<Vec<TurnRecord>> {
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
        created_at: parse_dt(&r.get::<_, String>(9)?),
    })
}

fn parse_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}
