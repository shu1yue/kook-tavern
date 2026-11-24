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
            SELECT channel_id, scenario_id, title, location, world_time,
                   turn_number, state_version, turn_phase, chat_mode, narrative_style,
                   web_token, clues_json, summary, summary_upto_turn,
                   active, created_at, updated_at
            FROM rooms WHERE channel_id = ?1
            "#,
            params![channel_id],
            |r| {
                Ok(RoomState {
                    channel_id: r.get(0)?,
                    scenario_id: r.get(1)?,
                    title: r.get(2)?,
                    location: r.get(3)?,
                    world_time: r.get(4)?,
                    turn_number: r.get(5)?,
                    state_version: r.get::<_, i64>(6)?.max(1) as u64,
                    turn_phase: TurnPhase::parse(&r.get::<_, String>(7)?),
                    chat_mode: ChatMode::parse(&r.get::<_, String>(8)?).unwrap_or(ChatMode::Strict),
                    narrative_style: NarrativeStyle::parse(&r.get::<_, String>(9)?)
                        .unwrap_or(NarrativeStyle::Prose),
                    web_token: r.get(10)?,
                    clues: serde_json::from_str(&r.get::<_, String>(11)?).unwrap_or_default(),
                    summary: r.get(12)?,
                    summary_upto_turn: r.get(13)?,
                    active: r.get::<_, i32>(14)? != 0,
                    created_at: parse_dt(&r.get::<_, String>(15)?),
                    updated_at: parse_dt(&r.get::<_, String>(16)?),
                })
            },
        )
        .optional()?;
    Ok(row)
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
