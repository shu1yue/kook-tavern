use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use tavern_core::{AssistOutcome, CheckCommitOutcome, RuleCheckRequest};
use tavern_domain::{
    PendingAction, RollMode, TurnPhase, match_proficiency, perform_rule_check,
};

use crate::mappers::{
    query_player_by_id, query_players, query_room, write_pending_action,
};

pub(crate) const SKILL_NOTE_MARKER: &str = "✨【施放招牌技";

/// Appends a roll/skill/assist note to `pending_actions` inside an existing transaction.
pub(crate) fn append_roll_in_tx(
    tx: &Connection,
    channel_id: &str,
    target_turn: u32,
    user_id: &str,
    character_name: &str,
    roll_note: &str,
) -> Result<()> {
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
    write_pending_action(tx, &action)
}

/// Checks whether `pending_actions` for `target_turn` already contains a cast signature ability note.
pub(crate) fn has_pending_skill_for_turn(
    tx: &Connection,
    channel_id: &str,
    target_turn: u32,
    user_id: Option<&str>,
) -> Result<bool> {
    let count: i64 = if let Some(uid) = user_id {
        tx.query_row(
            "SELECT COUNT(*) FROM pending_actions WHERE channel_id = ?1 AND target_turn = ?2 AND user_id = ?3 AND instr(roll_notes_json, ?4) > 0",
            params![channel_id, target_turn, uid, SKILL_NOTE_MARKER],
            |r| r.get(0),
        )?
    } else {
        tx.query_row(
            "SELECT COUNT(*) FROM pending_actions WHERE channel_id = ?1 AND target_turn = ?2 AND instr(roll_notes_json, ?3) > 0",
            params![channel_id, target_turn, SKILL_NOTE_MARKER],
            |r| r.get(0),
        )?
    };
    Ok(count > 0)
}

/// Atomically resolves check bonuses (inspiration + teammate assist), rolls the check,
/// persists consumed bonuses (only when NOT resolving), and binds the roll note to `pending_actions`.
pub(crate) fn execute_rule_check_tx(
    conn: &mut Connection,
    channel_id: &str,
    user_id: &str,
    req: &RuleCheckRequest,
) -> Result<CheckCommitOutcome> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(CheckCommitOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(CheckCommitOutcome::RoomUnavailable);
    }
    let Some(mut player) = query_player_by_id(&tx, channel_id, user_id)? else {
        return Ok(CheckCommitOutcome::PlayerNotFound);
    };

    let is_resolving = room.turn_phase == TurnPhase::Resolving;
    let target_turn = if is_resolving {
        room.turn_number + 2
    } else {
        room.turn_number + 1
    };

    let mut mode = req.mode;
    let mut bonus_notes = Vec::new();
    let mut spent_inspire = false;
    let mut spent_assist = false;

    if is_resolving {
        if req.use_inspiration || !player.dnd.assisted_by.trim().is_empty() {
            bonus_notes.push(
                "⏳ DM 结算中，暂未消耗激励点/队友协助，按指定模式掷骰并绑定至下下轮".to_string(),
            );
        }
    } else {
        if req.use_inspiration {
            if !player.dnd.inspiration {
                bonus_notes.push("ℹ️ 你当前未持有激励点，未触发激励优势".to_string());
            } else if mode == RollMode::Advantage {
                bonus_notes.push("🌟 本次已是优势双骰，为你保留激励点未消耗".to_string());
            } else if mode == RollMode::Disadvantage {
                spent_inspire = true;
                mode = RollMode::Normal;
                bonus_notes.push("🌟 消耗激励点抵消劣势 → **常规掷骰**（优劣相抵）".to_string());
            } else {
                spent_inspire = true;
                mode = RollMode::Advantage;
                bonus_notes.push("🌟 消耗激励点 → **优势双骰**".to_string());
            }
        }
        let helper = player.dnd.assisted_by.trim().to_string();
        if !helper.is_empty() && !spent_inspire {
            if mode == RollMode::Normal {
                spent_assist = true;
                mode = RollMode::Advantage;
                bonus_notes.push(format!("🤝 触发【{helper}】协助 → **优势双骰**"));
            } else if mode == RollMode::Disadvantage {
                spent_assist = true;
                mode = RollMode::Normal;
                bonus_notes.push(format!("🤝 触发【{helper}】协助抵消劣势 → **常规掷骰**"));
            }
        }
        if spent_inspire {
            player.dnd.inspiration = false;
        }
        if spent_assist {
            player.dnd.assisted_by.clear();
        }
        if spent_inspire || spent_assist {
            let dnd_json = serde_json::to_string(&player.dnd)?;
            tx.execute(
                "UPDATE players SET dnd_json = ?3 WHERE channel_id = ?1 AND user_id = ?2",
                params![channel_id, user_id, dnd_json],
            )?;
        }
    }

    let prof_skill = match_proficiency(
        &player.dnd.proficient_skills,
        &req.raw_stat_token,
        req.reason.as_deref().unwrap_or(""),
        req.stat,
    );
    let result = perform_rule_check(
        room.check_system,
        req.stat,
        &player.stats,
        mode,
        req.dc,
        prof_skill,
        req.reason.clone(),
    );
    let formatted = result.format_short();
    append_roll_in_tx(
        &tx,
        channel_id,
        target_turn,
        user_id,
        &player.character_name,
        &formatted,
    )?;
    tx.commit()?;

    Ok(CheckCommitOutcome::Committed {
        player: Box::new(player),
        result: Box::new(result),
        turn_no: target_turn,
        bonus_notes,
    })
}

/// Atomically validates helper & target (both joined, active, non-AFK, distinct), sets `assisted_by`,
/// and records the helper's assist note into `pending_actions` inside a single SQLite transaction.
pub(crate) fn assist_tx(
    conn: &mut Connection,
    channel_id: &str,
    caller_user_id: &str,
    target_name: &str,
    how: &str,
) -> Result<AssistOutcome> {
    let tx = conn.transaction()?;
    let Some(room) = query_room(&tx, channel_id)? else {
        return Ok(AssistOutcome::RoomUnavailable);
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(AssistOutcome::RoomUnavailable);
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(AssistOutcome::RoomResolving);
    }

    let players = query_players(&tx, channel_id)?;
    let Some(helper) = players.iter().find(|p| p.user_id == caller_user_id).cloned() else {
        return Ok(AssistOutcome::NotAllowed(
            "你尚未入团，请先选职业入团或 `/char load` 上桌后再协助队友喵！".into(),
        ));
    };
    if helper.is_afk {
        return Ok(AssistOutcome::NotAllowed(
            "你当前处于暂离（AFK）状态，发 `/back` 归队后再协助队友喵！".into(),
        ));
    }

    let needle = target_name.trim();
    let Some(mut target) = players.into_iter().find(|p| {
        p.character_name == needle || p.character_name.eq_ignore_ascii_case(needle)
    }) else {
        return Ok(AssistOutcome::TargetNotFound);
    };
    if target.user_id == helper.user_id {
        return Ok(AssistOutcome::NotAllowed(
            "不能协助自己哦，去帮同队其他伙伴挂优势吧！".into(),
        ));
    }
    if target.is_afk {
        return Ok(AssistOutcome::NotAllowed(format!(
            "队友【{}】当前处于暂离（AFK）状态，等 TA 归队后再协助吧！",
            target.character_name
        )));
    }

    target.dnd.assisted_by = helper.character_name.clone();
    let dnd_json = serde_json::to_string(&target.dnd)?;
    tx.execute(
        "UPDATE players SET dnd_json = ?3 WHERE channel_id = ?1 AND user_id = ?2",
        params![channel_id, target.user_id, dnd_json],
    )?;

    let target_turn = room.turn_number + 1;
    let desc = if how.trim().is_empty() {
        format!("🤝 协助队友【{}】提供掩护与配合", target.character_name)
    } else {
        format!("🤝 协助队友【{}】：{}", target.character_name, how.trim())
    };
    append_roll_in_tx(
        &tx,
        channel_id,
        target_turn,
        &helper.user_id,
        &helper.character_name,
        &desc,
    )?;
    tx.commit()?;

    Ok(AssistOutcome::Assisted {
        helper: Box::new(helper),
        target: Box::new(target),
        turn_no: target_turn,
    })
}
