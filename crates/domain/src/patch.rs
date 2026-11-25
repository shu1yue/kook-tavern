use std::collections::BTreeMap;

use crate::models::{NpcState, Player, RoomState};
use crate::protocol::{DirectMessageOut, DmStructuredReply, StatePatch};

#[derive(Debug, Clone, Default)]
pub struct AppliedTurnEffects {
    pub state_changes: Vec<String>,
    pub whisper_hints: Vec<String>,
    pub direct_messages: Vec<DirectMessageOut>,
}

/// Authoritative server-side validator & applier for LLM-proposed state patches.
/// Aggregates multiple patches targeting the same player before applying per-turn `±30` bounds.
pub fn apply_dm_reply(
    room: &mut RoomState,
    players: &mut [Player],
    reply: &DmStructuredReply,
) -> AppliedTurnEffects {
    let mut effects = AppliedTurnEffects::default();

    if let Some(loc) = reply.location.as_deref().map(str::trim)
        && !loc.is_empty()
        && loc != room.location
    {
        effects
            .state_changes
            .push(format!("📍 场景移动：{} → {}", room.location, loc));
        room.location = loc.to_string();
    }

    if let Some(wt) = reply.world_time.as_deref().map(str::trim)
        && !wt.is_empty()
    {
        room.world_time = wt.to_string();
    }

    // Aggregate per player index to prevent multi-patch delta stacking bypass
    let mut aggregated: BTreeMap<usize, StatePatch> = BTreeMap::new();
    for patch in &reply.state_patches {
        let target_name = patch.player_name.trim();
        let Some(idx) = players.iter().position(|p| {
            p.character_name == target_name || p.character_name.eq_ignore_ascii_case(target_name)
        }) else {
            continue;
        };
        let entry = aggregated.entry(idx).or_insert_with(|| StatePatch {
            player_name: players[idx].character_name.clone(),
            ..Default::default()
        });
        entry.hp_delta = entry.hp_delta.saturating_add(patch.hp_delta);
        entry.san_delta = entry.san_delta.saturating_add(patch.san_delta);
        entry.add_items.extend(patch.add_items.iter().cloned());
        entry.remove_items.extend(patch.remove_items.iter().cloned());
        if let Some(ref note) = patch.status_note
            && !note.trim().is_empty()
        {
            entry.status_note = Some(note.clone());
        }
    }

    for (idx, patch) in aggregated {
        let player = &mut players[idx];
        let mut p_notes = Vec::new();

        let hp_delta = patch.hp_delta.clamp(-30, 30);
        if hp_delta != 0 {
            let old = player.hp;
            player.hp = (player.hp + hp_delta).clamp(0, player.max_hp);
            let actual = player.hp - old;
            if actual != 0 {
                let sign = if actual > 0 { "+" } else { "" };
                p_notes.push(format!("❤️ HP {old}→{} ({sign}{actual})", player.hp));
            }
        }

        let san_delta = patch.san_delta.clamp(-30, 30);
        if san_delta != 0 {
            let old = player.san;
            player.san = (player.san + san_delta).clamp(0, player.max_san);
            let actual = player.san - old;
            if actual != 0 {
                let sign = if actual > 0 { "+" } else { "" };
                p_notes.push(format!("🧠 SAN {old}→{} ({sign}{actual})", player.san));
            }
        }

        for item in patch.add_items.iter().take(10) {
            let item = item.trim();
            if !item.is_empty() && !player.inventory.iter().any(|i| i == item) {
                player.inventory.push(item.to_string());
                p_notes.push(format!("🎒 获得「{item}」"));
            }
        }

        for item in patch.remove_items.iter().take(10) {
            let item = item.trim();
            if let Some(pos) = player
                .inventory
                .iter()
                .position(|i| i == item || i.eq_ignore_ascii_case(item))
            {
                let removed = player.inventory.remove(pos);
                p_notes.push(format!("📤 失去「{removed}」"));
            }
        }

        if let Some(note) = patch.status_note.as_deref().map(str::trim)
            && !note.is_empty()
            && note != player.status_note
        {
            player.status_note = note.to_string();
            p_notes.push(format!("🏷️ 状态：{note}"));
        }

        if !p_notes.is_empty() {
            effects
                .state_changes
                .push(format!("**{}**：{}", player.character_name, p_notes.join(" ｜ ")));
        }
    }

    for clue in reply.new_clues.iter().take(10) {
        let clue = clue.trim();
        if !clue.is_empty() && !room.clues.iter().any(|c| c == clue) {
            room.clues.push(clue.to_string());
            effects.state_changes.push(format!("🔎 新线索：{clue}"));
        }
    }

    for npc in reply.npc_updates.iter().take(8) {
        let name: String = npc.name.trim().chars().take(24).collect();
        let attitude: String = npc
            .attitude
            .as_deref()
            .unwrap_or("")
            .trim()
            .chars()
            .take(16)
            .collect();
        let status: String = npc
            .status
            .as_deref()
            .unwrap_or("")
            .trim()
            .chars()
            .take(60)
            .collect();
        if name.is_empty() || (attitude.is_empty() && status.is_empty()) {
            continue;
        }
        if let Some(existing) = room
            .npc_states
            .iter_mut()
            .find(|n| n.name == name || n.name.eq_ignore_ascii_case(&name))
        {
            if !attitude.is_empty() {
                existing.attitude = attitude.clone();
            }
            if !status.is_empty() {
                existing.status = status.clone();
            }
        } else if room.npc_states.len() < 24 {
            room.npc_states.push(NpcState {
                name: name.clone(),
                attitude: if attitude.is_empty() {
                    "中立".to_string()
                } else {
                    attitude.clone()
                },
                status: status.clone(),
            });
        }
        let att_label = if attitude.is_empty() { "动态更新" } else { &attitude };
        let st_suffix = if status.is_empty() {
            String::new()
        } else {
            format!(" — {status}")
        };
        effects
            .state_changes
            .push(format!("🗣️ **{name}**（{att_label}）{st_suffix}"));
    }

    for whisper in &reply.private_whispers {
        let target_name = whisper.player_name.trim();
        let secret = whisper.secret.trim();
        if secret.is_empty() {
            continue;
        }
        if let Some(player) = players.iter().find(|p| {
            p.character_name == target_name || p.character_name.eq_ignore_ascii_case(target_name)
        }) {
            effects.whisper_hints.push(format!(
                "🤫 DM 悄悄给 **{}** 递了一张秘密纸条（已私聊发送）",
                player.character_name
            ));
            effects.direct_messages.push(DirectMessageOut {
                user_id: player.user_id.clone(),
                player_name: player.character_name.clone(),
                content: format!("🤫 **【来自 DM 的秘密纸条 · {}】**\n{}", room.title, secret),
            });
        }
    }

    effects
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::models::{ChatMode, NarrativeStyle, TurnPhase};

    #[test]
    fn aggregates_multiple_patches_and_clamps_bounds() {
        let now = Utc::now();
        let mut room = RoomState {
            channel_id: "c1".into(),
            guild_id: "g1".into(),
            scenario_id: "s1".into(),
            title: "T".into(),
            location: "Loc".into(),
            world_time: "Now".into(),
            turn_number: 1,
            state_version: 1,
            turn_phase: TurnPhase::Collecting,
            chat_mode: ChatMode::Strict,
            narrative_style: NarrativeStyle::Prose,
            gm_user_id: "u1".into(),
            web_token: "secret_tok".into(),
            clues: vec![],
            npc_states: vec![],
            suggested_actions: vec![],
            summary: String::new(),
            summary_upto_turn: 0,
            active: true,
            created_at: now,
            updated_at: now,
        };
        assert!(!format!("{room:?}").contains("secret_tok"));

        let mut players = vec![Player {
            channel_id: "c1".into(),
            user_id: "u1".into(),
            character_name: "陆沉".into(),
            role_desc: "机械师".into(),
            traits: vec![],
            hp: 50,
            max_hp: 50,
            san: 8,
            max_san: 10,
            inventory: vec!["扳手".into()],
            status_note: String::new(),
            is_afk: false,
        }];
        let reply = DmStructuredReply {
            scene_title: "测试".into(),
            narrative: "轰！".into(),
            private_whispers: vec![],
            state_patches: vec![
                StatePatch {
                    player_name: "陆沉".into(),
                    hp_delta: -25,
                    san_delta: 999,
                    add_items: vec!["火把".into()],
                    remove_items: vec!["不存在的物品".into()],
                    status_note: None,
                },
                StatePatch {
                    player_name: "陆沉".into(),
                    hp_delta: -25,
                    san_delta: 0,
                    add_items: vec![],
                    remove_items: vec!["扳手".into()],
                    status_note: Some("重伤".into()),
                },
            ],
            new_clues: vec!["线索A".into(), "线索A".into()],
            npc_updates: vec![],
            world_time: None,
            location: None,
            suggested_actions: vec![],
        };

        let _ = apply_dm_reply(&mut room, &mut players, &reply);
        // Aggregated delta (-50) clamped to -30 => 50 - 30 = 20!
        assert_eq!(players[0].hp, 20);
        assert_eq!(players[0].san, 10);
        assert_eq!(players[0].inventory, vec!["火把"]);
        assert_eq!(room.clues, vec!["线索A"]);
    }
}
