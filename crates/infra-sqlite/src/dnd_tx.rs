use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, params};
use tavern_core::{
    AbilityUseOutcome, AssistOutcome, CheckCommitOutcome, DndStore, GmUpdateOutcome,
    JoinPlayerOutcome, LevelUpOutcome, RestOutcome, RuleCheckRequest,
};
use tavern_domain::{
    CheckSystem, LevelUpChoice, Player, RestKind, SavedCharacter, StarterCharacterDefaults,
    TurnPhase, apply_level_up, apply_rest_to_players,
};

use crate::char_vault::{list_saved_chars_query, load_char_into_room_tx, save_char_tx};
use crate::dnd_check_tx::{
    append_roll_in_tx, assist_tx, execute_rule_check_tx, has_pending_skill_for_turn,
};
use crate::mappers::{query_player_by_id, query_players, query_room, write_player};
use crate::repo::SqliteRoomStore;

impl DndStore for SqliteRoomStore {
    fn update_check_system(&self, channel_id: &str, system: CheckSystem) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "UPDATE rooms SET check_system = ?2, updated_at = ?3 WHERE channel_id = ?1 AND active = 1 AND ended_id = '' AND turn_phase = 'collecting'",
                params![channel_id, system.as_str(), Utc::now().to_rfc3339()],
            )? > 0)
        })
    }

    fn set_player_inspiration(
        &self,
        channel_id: &str,
        target_name: &str,
        value: bool,
    ) -> Result<GmUpdateOutcome<Player>> {
        self.with_conn(|c| mutate_player_by_name_tx(c, channel_id, target_name, |p| {
            p.dnd.inspiration = value;
        }))
    }

    fn set_player_assist(
        &self,
        channel_id: &str,
        caller_user_id: &str,
        target_name: &str,
        how: &str,
    ) -> Result<AssistOutcome> {
        self.with_conn(|c| assist_tx(c, channel_id, caller_user_id, target_name, how))
    }

    fn execute_rule_check_atomic(
        &self,
        channel_id: &str,
        user_id: &str,
        req: &RuleCheckRequest,
    ) -> Result<CheckCommitOutcome> {
        self.with_conn(|conn| execute_rule_check_tx(conn, channel_id, user_id, req))
    }

    fn use_player_ability(
        &self,
        channel_id: &str,
        user_id: &str,
        query: &str,
        target_note: &str,
    ) -> Result<AbilityUseOutcome> {
        self.with_conn(|conn| use_ability_tx(conn, channel_id, user_id, query, target_note))
    }

    fn apply_party_rest(
        &self,
        channel_id: &str,
        caller_user_id: &str,
        is_gm_or_admin: bool,
        kind: RestKind,
        min_gap_turns: u32,
    ) -> Result<RestOutcome> {
        self.with_conn(|conn| {
            rest_tx(conn, channel_id, caller_user_id, is_gm_or_admin, kind, min_gap_turns)
        })
    }

    fn apply_player_levelup(
        &self,
        channel_id: &str,
        user_id: &str,
        choice: LevelUpChoice,
    ) -> Result<LevelUpOutcome> {
        self.with_conn(|conn| levelup_tx(conn, channel_id, user_id, choice))
    }

    fn save_character(&self, ch: &SavedCharacter) -> Result<()> {
        self.with_conn(|conn| save_char_tx(conn, ch))
    }

    fn list_saved_characters(&self, user_id: &str) -> Result<Vec<SavedCharacter>> {
        self.with_conn(|conn| list_saved_chars_query(conn, user_id))
    }

    fn delete_saved_character(&self, user_id: &str, name: &str) -> Result<bool> {
        self.with_conn(|conn| {
            Ok(conn.execute(
                "DELETE FROM saved_characters WHERE user_id = ?1 AND LOWER(character_name) = LOWER(?2)",
                params![user_id, name.trim()],
            )? > 0)
        })
    }

    fn load_saved_character_into_room(
        &self,
        channel_id: &str,
        user_id: &str,
        saved: &SavedCharacter,
        defaults: &StarterCharacterDefaults,
    ) -> Result<JoinPlayerOutcome> {
        self.with_conn(|c| load_char_into_room_tx(c, channel_id, user_id, saved, defaults))
    }
}

fn mutate_player_by_name_tx(
    conn: &mut Connection,
    channel_id: &str,
    target_name: &str,
    mutate: impl FnOnce(&mut Player),
) -> Result<GmUpdateOutcome<Player>> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(GmUpdateOutcome::NotFound);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(GmUpdateOutcome::NotFound);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(GmUpdateOutcome::RoomResolving);
    }
    let players = query_players(&tx, channel_id)?;
    let needle = target_name.trim();
    let Some(mut target) = players.into_iter().find(|p| {
        p.character_name == needle || p.character_name.eq_ignore_ascii_case(needle)
    }) else {
        return Ok(GmUpdateOutcome::NotFound);
    };
    mutate(&mut target);
    let dnd_json = serde_json::to_string(&target.dnd)?;
    tx.execute(
        "UPDATE players SET dnd_json = ?3 WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, target.user_id, dnd_json],
    )?;
    tx.commit()?;
    Ok(GmUpdateOutcome::Updated(target))
}

fn use_ability_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    query: &str,
    target_note: &str,
) -> Result<AbilityUseOutcome> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(AbilityUseOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(AbilityUseOutcome::RoomUnavailable);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(AbilityUseOutcome::RoomResolving);
    }
    let Some(mut player) = query_player_by_id(&tx, channel_id, user_id)? else {
        return Ok(AbilityUseOutcome::NotAllowed(
            "你尚未入团，请先选职业入团或 `/char load` 载入角色！".into(),
        ));
    };
    if player.is_afk {
        return Ok(AbilityUseOutcome::NotAllowed(
            "你当前处于暂离（AFK）状态，发 `/back` 归队后再施放招牌技能喵！".into(),
        ));
    }
    let q = query.trim();
    let idx = if let Ok(n) = q.parse::<usize>()
        && n >= 1
        && n <= player.dnd.abilities.len()
    {
        Some(n - 1)
    } else {
        let qlow = q.to_ascii_lowercase();
        player.dnd.abilities.iter().position(|a| {
            let nlow = a.name.to_ascii_lowercase();
            nlow == qlow || nlow.contains(&qlow)
        })
    };
    let Some(i) = idx else {
        return Ok(AbilityUseOutcome::NotFound);
    };
    if player.dnd.abilities[i].uses_left == 0 {
        let ab = &player.dnd.abilities[i];
        return Ok(AbilityUseOutcome::Exhausted {
            ability_name: ab.name.clone(),
            max_uses: ab.max_uses,
            recharge: ab.recharge.short_label().to_string(),
        });
    }
    player.dnd.abilities[i].uses_left -= 1;
    let ab_name = player.dnd.abilities[i].name.clone();
    let uses_left = player.dnd.abilities[i].uses_left;
    let max_uses = player.dnd.abilities[i].max_uses;
    let desc = player.dnd.abilities[i].desc.clone();
    let dnd_json = serde_json::to_string(&player.dnd)?;
    tx.execute(
        "UPDATE players SET dnd_json = ?3 WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, user_id, dnd_json],
    )?;

    let target_turn = room.turn_number + 1;
    let target_s = if target_note.trim().is_empty() {
        String::new()
    } else {
        format!("，目标/意图：{}", target_note.trim())
    };
    let roll_note =
        format!("✨【施放招牌技：{ab_name}（剩余 {uses_left}/{max_uses}）】{desc}{target_s}");
    append_roll_in_tx(
        &tx,
        channel_id,
        target_turn,
        user_id,
        &player.character_name,
        &roll_note,
    )?;
    tx.commit()?;
    Ok(AbilityUseOutcome::Used {
        player: Box::new(player),
        ability_name: ab_name,
        uses_left,
        max_uses,
        desc,
        turn_no: target_turn,
        roll_note,
    })
}

fn rest_tx(
    conn: &mut Connection,
    channel_id: &str,
    caller_user_id: &str,
    is_gm_or_admin: bool,
    kind: RestKind,
    min_gap_turns: u32,
) -> Result<RestOutcome> {
    let tx = conn.transaction()?;
    let Some(mut room) = query_room(&tx, channel_id)? else {
        return Ok(RestOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(RestOutcome::RoomUnavailable);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(RestOutcome::RoomResolving);
    }
    if kind == RestKind::Long && !is_gm_or_admin {
        return Ok(RestOutcome::NotAllowed(
            "全队满状态长休（`/rest long`）仅限房主或管理员在安全区发起喵！".into(),
        ));
    }
    let mut players = query_players(&tx, channel_id)?;
    if !is_gm_or_admin {
        let caller_active = players
            .iter()
            .any(|p| p.user_id == caller_user_id && !p.is_afk);
        if !caller_active {
            return Ok(RestOutcome::NotAllowed(
                "只有本局在场队员（非暂离）或房主才能发起小队休整喵！".into(),
            ));
        }
    }
    if !players.iter().any(|p| !p.is_afk) {
        return Ok(RestOutcome::NotAllowed(
            "当前房间暂无在场队员，先选职业入团再营地休整喵！".into(),
        ));
    }
    if room.turn_number == 0 {
        return Ok(RestOutcome::NotAllowed(
            "开局第 0 回合全员刚整装出发，请先推进至少 1 轮剧情后再营地休整喵！".into(),
        ));
    }
    if has_pending_skill_for_turn(&tx, channel_id, room.turn_number + 1, None)? {
        return Ok(RestOutcome::NotAllowed(
            "本轮已有队员施放并绑定了招牌技能，请等本轮 DM 结算完毕后再休整充能喵！".into(),
        ));
    }
    let next_allowed = room.last_rest_turn.saturating_add(min_gap_turns);
    if min_gap_turns > 0 && room.last_rest_turn > 0 && room.turn_number < next_allowed {
        return Ok(RestOutcome::Cooldown {
            wait_turns: next_allowed.saturating_sub(room.turn_number),
        });
    }

    let notes = apply_rest_to_players(&mut players, kind);
    for p in &players {
        write_player(&tx, p)?;
    }
    room.last_rest_turn = room.turn_number;
    if kind == RestKind::Long && !room.world_time.contains("长休后") {
        room.world_time = format!("{}（长休后）", room.world_time.trim());
    }
    tx.execute(
        "UPDATE rooms SET last_rest_turn = ?2, world_time = ?3, updated_at = ?4 WHERE channel_id = ?1",
        params![channel_id, room.last_rest_turn, room.world_time, Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    Ok(RestOutcome::Rested { notes, new_world_time: room.world_time })
}

fn levelup_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    choice: LevelUpChoice,
) -> Result<LevelUpOutcome> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(LevelUpOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(LevelUpOutcome::RoomUnavailable);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(LevelUpOutcome::RoomResolving);
    }
    let Some(mut player) = query_player_by_id(&tx, channel_id, user_id)? else {
        return Ok(LevelUpOutcome::Error("你尚未入团，请先选职业或 `/join`".into()));
    };
    if player.is_afk {
        return Ok(LevelUpOutcome::Error(
            "你当前处于暂离（AFK）状态，发 `/back` 归队后再分配升级点喵！".into(),
        ));
    }
    let summary = match apply_level_up(&mut player, choice) {
        Ok(s) => s,
        Err(msg) => return Ok(LevelUpOutcome::Error(msg)),
    };
    let remaining = player.dnd.pending_levelups;
    write_player(&tx, &player)?;
    tx.commit()?;
    Ok(LevelUpOutcome::Upgraded { player: Box::new(player), summary, remaining })
}
