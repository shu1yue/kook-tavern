use anyhow::Result;
use rusqlite::{Connection, params};
use tavern_core::JoinPlayerOutcome;
use tavern_domain::{
    CharacterAbility, Player, PlayerDndProfile, PresetRole, StarterCharacterDefaults, TurnPhase,
};

use crate::dnd_check_tx::has_pending_skill_for_turn;
use crate::mappers::{query_player_by_id, query_players, query_room, write_player};

fn pick_unique_name(existing: &[Player], raw_name: &str) -> String {
    let base_name: String = raw_name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|' && *c != '｜')
        .take(16)
        .collect();
    let base = if base_name.is_empty() {
        "冒险者".to_string()
    } else {
        base_name
    };
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while existing
        .iter()
        .any(|p| p.character_name.eq_ignore_ascii_case(&candidate))
    {
        candidate = format!("{base}#{suffix}");
        suffix += 1;
    }
    candidate
}

/// Atomically ensures a player exists in an active, non-ended room without ever overwriting
/// an existing player's role, stats, HP/SAN, or inventory.
pub fn ensure_player_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    raw_name: &str,
    defaults: &StarterCharacterDefaults,
) -> Result<Option<Player>> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(None);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(None);
    }
    if let Some(mut existing) = query_player_by_id(&tx, channel_id, user_id)? {
        if existing.is_afk {
            tx.execute(
                "UPDATE players SET is_afk = 0 WHERE channel_id = ?1 AND user_id = ?2",
                params![channel_id, user_id],
            )?;
            existing.is_afk = false;
        }
        tx.commit()?;
        return Ok(Some(existing));
    }
    let all = query_players(&tx, channel_id)?;
    let character_name = pick_unique_name(&all, raw_name);
    let p = Player {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        character_name,
        role_desc: "酒馆旅客".to_string(),
        traits: Vec::new(),
        hp: defaults.max_hp,
        max_hp: defaults.max_hp,
        san: defaults.max_san,
        max_san: defaults.max_san,
        inventory: defaults.starter_items.clone(),
        status_note: "正常".to_string(),
        is_afk: false,
        stats: Default::default(),
        location: room.location.clone(),
        dnd: Default::default(),
    };
    write_player(&tx, &p)?;
    tx.commit()?;
    Ok(Some(p))
}

/// Atomically joins with a preset role (new player gets starter + bonus items + stats + D&D abilities)
/// or switches an existing player's role/traits/stats WITHOUT touching their `character_name`,
/// HP, SAN, inventory, or mid-game ability charges!
pub fn pick_role_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    raw_name: &str,
    preset: &PresetRole,
    defaults: &StarterCharacterDefaults,
) -> Result<Option<(Player, bool)>> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(None);
    };
    if !room.active || room.ended_id.is_some() || room.turn_phase == TurnPhase::Resolving {
        return Ok(None);
    }
    if let Some(mut prev) = query_player_by_id(&tx, channel_id, user_id)? {
        let traits_json = serde_json::to_string(&preset.traits)?;
        if !preset.alignment.trim().is_empty() {
            prev.dnd.alignment = preset.alignment.clone();
        }
        // Anti-farm & levelup-preservation guard:
        // Only respec stats/proficiencies/abilities on Turn 0 (before casting a signature skill)
        // or once for a blank quick-join "酒馆旅客" who hasn't picked a build yet.
        let can_respec = room.turn_number == 0
            && !has_pending_skill_for_turn(&tx, channel_id, 1, Some(user_id))?;
        let is_blank_traveler = prev.role_desc == "酒馆旅客"
            && prev.dnd.abilities.is_empty()
            && prev.dnd.proficient_skills.is_empty()
            && prev.stats == Default::default();
        if can_respec || is_blank_traveler {
            prev.stats = preset.stats;
            prev.dnd.proficient_skills = preset.proficient_skills.iter().take(4).cloned().collect();
            prev.dnd.abilities = preset
                .abilities
                .iter()
                .take(4)
                .cloned()
                .map(CharacterAbility::with_full_uses)
                .collect();
        }
        let stats_json = serde_json::to_string(&prev.stats)?;
        let dnd_json = serde_json::to_string(&prev.dnd)?;
        tx.execute(
            r#"
            UPDATE players SET role_desc = ?3, traits_json = ?4, stats_json = ?5, dnd_json = ?6, is_afk = 0
            WHERE channel_id = ?1 AND user_id = ?2
            "#,
            params![channel_id, user_id, preset.name, traits_json, stats_json, dnd_json],
        )?;
        let updated = query_player_by_id(&tx, channel_id, user_id)?;
        tx.commit()?;
        return Ok(updated.map(|p| (p, true)));
    }
    let all = query_players(&tx, channel_id)?;
    let character_name = pick_unique_name(&all, raw_name);
    let mut inventory = defaults.starter_items.clone();
    for bonus in &preset.bonus_items {
        if !inventory.iter().any(|i| i == bonus) {
            inventory.push(bonus.clone());
        }
    }
    let dnd = PlayerDndProfile {
        alignment: preset.alignment.clone(),
        proficient_skills: preset.proficient_skills.iter().take(4).cloned().collect(),
        abilities: preset
            .abilities
            .iter()
            .take(4)
            .cloned()
            .map(CharacterAbility::with_full_uses)
            .collect(),
        ..PlayerDndProfile::default()
    };
    let p = Player {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        character_name,
        role_desc: preset.name.clone(),
        traits: preset.traits.clone(),
        hp: defaults.max_hp,
        max_hp: defaults.max_hp,
        san: defaults.max_san,
        max_san: defaults.max_san,
        inventory,
        status_note: "精神饱满".to_string(),
        is_afk: false,
        stats: preset.stats,
        location: room.location.clone(),
        dnd,
    };
    write_player(&tx, &p)?;
    tx.commit()?;
    Ok(Some((p, false)))
}

/// Atomically handles `/join` inside a single SQLite transaction: enforces room availability,
/// case-insensitive unique character_name, and preserves existing HP/SAN/inventory/stats on edit.
pub fn join_player_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    character_name: &str,
    role_desc: &str,
    traits: &[String],
    defaults: &StarterCharacterDefaults,
) -> Result<JoinPlayerOutcome> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(JoinPlayerOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(JoinPlayerOutcome::RoomUnavailable);
    }
    let all = query_players(&tx, channel_id)?;
    if all.iter().any(|p| {
        p.user_id != user_id && p.character_name.eq_ignore_ascii_case(character_name)
    }) {
        return Ok(JoinPlayerOutcome::DuplicateName);
    }
    if let Some(prev) = query_player_by_id(&tx, channel_id, user_id)? {
        let next_role = if role_desc.is_empty() {
            prev.role_desc.as_str()
        } else {
            role_desc
        };
        let next_traits = if traits.is_empty() {
            prev.traits.as_slice()
        } else {
            traits
        };
        let traits_json = serde_json::to_string(next_traits)?;
        tx.execute(
            r#"
            UPDATE players SET character_name = ?3, role_desc = ?4, traits_json = ?5, is_afk = 0
            WHERE channel_id = ?1 AND user_id = ?2
            "#,
            params![channel_id, user_id, character_name, next_role, traits_json],
        )?;
        let updated = query_player_by_id(&tx, channel_id, user_id)?
            .unwrap_or(prev);
        tx.commit()?;
        return Ok(JoinPlayerOutcome::Joined(Box::new(updated)));
    }
    let p = Player {
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        character_name: character_name.to_string(),
        role_desc: if role_desc.is_empty() {
            "初来乍到的冒险者".to_string()
        } else {
            role_desc.to_string()
        },
        traits: traits.to_vec(),
        hp: defaults.max_hp,
        max_hp: defaults.max_hp,
        san: defaults.max_san,
        max_san: defaults.max_san,
        inventory: defaults.starter_items.clone(),
        status_note: "精神饱满".to_string(),
        is_afk: false,
        stats: Default::default(),
        location: room.location.clone(),
        dnd: Default::default(),
    };
    write_player(&tx, &p)?;
    tx.commit()?;
    Ok(JoinPlayerOutcome::Joined(Box::new(p)))
}
