use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use tavern_core::JoinPlayerOutcome;
use tavern_domain::{
    Player, SavedCharacter, StarterCharacterDefaults, TurnPhase, reset_dnd_profile_for_new_room,
    validate_point_buy,
};

use crate::dnd_check_tx::has_pending_skill_for_turn;
use crate::mappers::{query_player_by_id, query_players, query_room, write_player};

const MAX_SAVED_CHARS_PER_USER: i64 = 8;

pub(crate) fn save_char_tx(conn: &mut Connection, ch: &SavedCharacter) -> Result<()> {
    let tx = conn.transaction()?;
    let exists: i64 = tx.query_row(
        "SELECT COUNT(*) FROM saved_characters WHERE user_id = ?1 AND LOWER(character_name) = LOWER(?2)",
        params![ch.user_id, ch.character_name],
        |r| r.get(0),
    )?;
    if exists == 0 {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM saved_characters WHERE user_id = ?1",
            params![ch.user_id],
            |r| r.get(0),
        )?;
        if count >= MAX_SAVED_CHARS_PER_USER {
            bail!(
                "个人角色库已满（上限 {MAX_SAVED_CHARS_PER_USER} 个），请先用 `/char del <名字>` 删除旧角色"
            );
        }
    }
    tx.execute(
        "DELETE FROM saved_characters WHERE user_id = ?1 AND LOWER(character_name) = LOWER(?2)",
        params![ch.user_id, ch.character_name],
    )?;
    tx.execute(
        r#"
        INSERT INTO saved_characters (user_id, character_name, role_desc, traits_json, stats_json, dnd_json, updated_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            ch.user_id,
            ch.character_name,
            ch.role_desc,
            serde_json::to_string(&ch.traits)?,
            serde_json::to_string(&ch.stats)?,
            serde_json::to_string(&ch.dnd)?,
            ch.updated_at.to_rfc3339(),
        ],
    )?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn list_saved_chars_query(conn: &Connection, user_id: &str) -> Result<Vec<SavedCharacter>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, character_name, role_desc, traits_json, stats_json, dnd_json, updated_at FROM saved_characters WHERE user_id = ?1 ORDER BY updated_at DESC",
    )?;
    let rows = stmt.query_map(params![user_id], |r| {
        Ok(SavedCharacter {
            user_id: r.get(0)?,
            character_name: r.get(1)?,
            role_desc: r.get(2)?,
            traits: serde_json::from_str(&r.get::<_, String>(3)?).unwrap_or_default(),
            stats: serde_json::from_str(&r.get::<_, String>(4)?).unwrap_or_default(),
            dnd: serde_json::from_str(&r.get::<_, String>(5)?).unwrap_or_default(),
            updated_at: DateTime::parse_from_rfc3339(&r.get::<_, String>(6)?)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now()),
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

pub(crate) fn load_char_into_room_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    saved: &SavedCharacter,
    defaults: &StarterCharacterDefaults,
) -> Result<JoinPlayerOutcome> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(JoinPlayerOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() || room.turn_phase == TurnPhase::Resolving {
        return Ok(JoinPlayerOutcome::RoomUnavailable);
    }
    let all = query_players(&tx, channel_id)?;
    if all.iter().any(|p| {
        p.user_id != user_id && p.character_name.eq_ignore_ascii_case(&saved.character_name)
    }) {
        return Ok(JoinPlayerOutcome::DuplicateName);
    }
    let safe_stats = if validate_point_buy(&saved.stats).is_ok() {
        saved.stats
    } else {
        Default::default()
    };
    let fresh_dnd = reset_dnd_profile_for_new_room(&saved.dnd);
    let player = if let Some(mut prev) = query_player_by_id(&tx, channel_id, user_id)? {
        prev.character_name = saved.character_name.clone();
        prev.role_desc = saved.role_desc.clone();
        prev.traits = saved.traits.clone();
        prev.is_afk = false;
        prev.dnd.alignment = fresh_dnd.alignment;
        prev.dnd.backstory = fresh_dnd.backstory;
        prev.dnd.personal_secret = fresh_dnd.personal_secret;
        if room.turn_number == 0 && !has_pending_skill_for_turn(&tx, channel_id, 1, Some(user_id))? {
            prev.stats = safe_stats;
            prev.dnd.proficient_skills = fresh_dnd.proficient_skills;
            prev.dnd.abilities = fresh_dnd.abilities;
        }
        prev
    } else {
        Player {
            channel_id: channel_id.to_string(),
            user_id: user_id.to_string(),
            character_name: saved.character_name.clone(),
            role_desc: saved.role_desc.clone(),
            traits: saved.traits.clone(),
            hp: defaults.max_hp,
            max_hp: defaults.max_hp,
            san: defaults.max_san,
            max_san: defaults.max_san,
            inventory: defaults.starter_items.clone(),
            status_note: "精神饱满".to_string(),
            is_afk: false,
            stats: safe_stats,
            location: room.location.clone(),
            dnd: fresh_dnd,
        }
    };
    write_player(&tx, &player)?;
    tx.commit()?;
    Ok(JoinPlayerOutcome::Joined(Box::new(player)))
}
