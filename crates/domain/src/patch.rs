use std::collections::BTreeMap;

use crate::models::{NpcState, Player, RoomState, Scenario};
use crate::protocol::{DirectMessageOut, DmStructuredReply, StatePatch};
use crate::stats::ScenarioEnding;
use crate::world::WorldBulletin;

#[derive(Debug, Clone, Default)]
pub struct AppliedTurnEffects {
    pub state_changes: Vec<String>,
    pub whisper_hints: Vec<String>,
    pub direct_messages: Vec<DirectMessageOut>,
    pub world_bulletin: Option<WorldBulletin>,
}

/// Authoritative server-side validator & applier for LLM-proposed state patches.
/// Aggregates multiple patches targeting the same player before applying per-turn `±30` bounds.
pub fn apply_dm_reply(
    room: &mut RoomState,
    players: &mut [Player],
    reply: &DmStructuredReply,
) -> AppliedTurnEffects {
    let mut effects = AppliedTurnEffects::default();

    let prev_room_loc = room.location.clone();
    if let Some(raw_loc) = reply.location.as_deref().map(str::trim)
        && !raw_loc.is_empty()
    {
        let loc: String = raw_loc.chars().take(48).collect();
        if loc != room.location {
            effects
                .state_changes
                .push(format!("📍 场景移动：{} → {}", room.location, loc));
            room.location = loc.clone();
            for p in players.iter_mut() {
                if !p.is_afk && (p.location.trim().is_empty() || p.location == prev_room_loc) {
                    p.location = loc.clone();
                }
            }
        }
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
        if let Some(ref ploc) = patch.location
            && !ploc.trim().is_empty()
        {
            entry.location = Some(ploc.clone());
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

        if let Some(ploc) = patch.location.as_deref().map(str::trim)
            && !ploc.is_empty()
            && ploc != player.location
        {
            player.location = ploc.chars().take(48).collect();
            p_notes.push(format!("🧭 位置：{}", player.location));
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

    let trim_opt = |opt: &Option<String>, lim: usize| -> String {
        opt.as_deref().unwrap_or("").trim().chars().take(lim).collect()
    };
    for npc in reply.npc_updates.iter().take(8) {
        let name: String = npc.name.trim().chars().take(24).collect();
        let attitude = trim_opt(&npc.attitude, 16);
        let status = trim_opt(&npc.status, 60);
        let nloc = trim_opt(&npc.location, 48);
        if name.is_empty() || (attitude.is_empty() && status.is_empty() && nloc.is_empty()) {
            continue;
        }
        if let Some(existing) = room
            .npc_states
            .iter_mut()
            .find(|n| n.name == name || n.name.eq_ignore_ascii_case(&name))
        {
            if !attitude.is_empty() { existing.attitude = attitude.clone(); }
            if !status.is_empty() { existing.status = status.clone(); }
            if !nloc.is_empty() { existing.location = nloc.clone(); }
        } else if room.npc_states.len() < 24 {
            room.npc_states.push(NpcState {
                name: name.clone(),
                attitude: if attitude.is_empty() { "中立".to_string() } else { attitude.clone() },
                status: status.clone(),
                location: nloc.clone(),
            });
        }
        let att_label = if attitude.is_empty() { "动态更新" } else { &attitude };
        let st_suffix = if status.is_empty() { String::new() } else { format!(" — {status}") };
        effects.state_changes.push(format!("🗣️ **{name}**（{att_label}）{st_suffix}"));
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

    crate::dnd_ops::apply_inspiration_grants(
        players,
        &reply.grant_inspiration,
        &mut effects.state_changes,
    );
    effects
}

/// Authoritative server-side applier for scenario progression:
/// world flags, act transitions (bounded to `scenario.acts`), milestone level-up, and validated endings.
pub fn apply_story_progression(
    room: &mut RoomState,
    players: &mut [Player],
    scenario: &Scenario,
    reply: &DmStructuredReply,
    resolving_turn: u32,
    effects: &mut AppliedTurnEffects,
) -> Option<ScenarioEnding> {
    for raw in reply.add_flags.iter().take(5) {
        let flag: String = raw.trim().chars().take(36).collect();
        if !flag.is_empty() && !room.world_flags.iter().any(|f| f == &flag) {
            if room.world_flags.len() >= 60 {
                room.world_flags.remove(0);
            }
            room.world_flags.push(flag.clone());
            effects.state_changes.push(format!("🚩 关键进展：{flag}"));
        }
    }

    let total_acts = scenario.acts.len() as u32;
    let mut target_act = room.current_act.max(1);
    if reply.advance_act && total_acts > 1 && target_act < total_acts {
        target_act += 1;
    }
    if total_acts > 1
        && target_act < total_acts
        && let Some(node_id) = crate::world::resolve_map_node_id(&scenario.map_nodes, &room.location)
        && let Some((idx, _)) = scenario.acts.iter().enumerate().find(|(i, a)| {
            (*i as u32 + 1) > target_act && a.enter_nodes.iter().any(|n| n == &node_id)
        })
    {
        target_act = (idx as u32) + 1;
    }
    while room.current_act < target_act && room.current_act < total_acts {
        room.current_act += 1;
        if let Some(act) = scenario.acts.get((room.current_act - 1) as usize) {
            effects.state_changes.push(format!(
                "🎬 剧情推进至第 {}/{} 幕 · **{}**（目标：{}）",
                room.current_act, total_acts, act.title, act.goal
            ));
        }
        crate::dnd_ops::award_act_milestone(players, &mut effects.state_changes);
    }

    if let Some(ref draft) = reply.world_bulletin {
        let headline: String = draft.headline.trim().chars().take(48).collect();
        let body: String = draft.body.trim().chars().take(240).collect();
        let faction: String = draft.faction.trim().chars().take(32).collect();
        if !headline.is_empty() && !body.is_empty() {
            let b = WorldBulletin {
                turn_number: resolving_turn,
                masthead: scenario.rules.effective_masthead(&scenario.name),
                headline, body, faction,
            };
            if room.bulletins.len() >= 12 { room.bulletins.remove(0); }
            room.bulletins.push(b.clone());
            effects.world_bulletin = Some(b);
        }
    }

    let raw_end = reply.trigger_ending_id.as_deref().map(str::trim).unwrap_or("");
    if raw_end.is_empty() || room.ended_id.is_some() { return None; }
    let matched = scenario.endings.iter()
        .find(|e| e.id == raw_end || e.id.eq_ignore_ascii_case(raw_end))?.clone();

    let active_players: Vec<&Player> = players.iter().filter(|p| !p.is_afk).collect();
    let party_down =
        !active_players.is_empty() && active_players.iter().all(|p| p.hp <= 0 || p.san <= 0);
    // Hard guard on Turns 1..=2: block premature endings unless all active players wiped on a bad ending!
    if resolving_turn <= 2 {
        if !(party_down && matched.kind.eq_ignore_ascii_case("bad")) {
            return None;
        }
    } else if party_down && !matched.kind.eq_ignore_ascii_case("bad") {
        // Even on Turn 3+, a wiped party cannot unlock non-bad endings via LLM hallucination
        return None;
    }

    room.ended_id = Some(matched.id.clone());
    room.ended_title = Some(matched.title.clone());
    Some(matched)
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
            channel_id: "c1".into(), guild_id: "g1".into(), scenario_id: "s1".into(),
            title: "T".into(), location: "Loc".into(), world_time: "Now".into(),
            turn_number: 1, state_version: 1, turn_phase: TurnPhase::Collecting,
            chat_mode: ChatMode::Strict, narrative_style: NarrativeStyle::Prose,
            gm_user_id: "u1".into(), web_token: "secret_tok".into(),
            clues: vec![], npc_states: vec![], suggested_actions: vec![],
            current_act: 1, world_flags: vec![], ended_id: None, ended_title: None,
            bulletins: vec![], check_system: Default::default(), last_rest_turn: 0,
            summary: String::new(), summary_upto_turn: 0, active: true,
            created_at: now, updated_at: now,
        };
        assert!(!format!("{room:?}").contains("secret_tok"));
        let mut players = vec![Player {
            channel_id: "c1".into(), user_id: "u1".into(), character_name: "陆沉".into(),
            role_desc: "机械师".into(), traits: vec![], hp: 50, max_hp: 50, san: 8, max_san: 10,
            inventory: vec!["扳手".into()], status_note: String::new(), is_afk: false,
            stats: Default::default(), location: String::new(), dnd: Default::default(),
        }];
        let reply = DmStructuredReply {
            scene_title: "测试".into(), narrative: "轰！".into(), private_whispers: vec![],
            state_patches: vec![
                StatePatch { player_name: "陆沉".into(), hp_delta: -25, san_delta: 999, add_items: vec!["火把".into()], remove_items: vec!["不存在的物品".into()], status_note: None, location: Some("地下酒窖".into()) },
                StatePatch { player_name: "陆沉".into(), hp_delta: -25, san_delta: 0, add_items: vec![], remove_items: vec!["扳手".into()], status_note: Some("重伤".into()), location: None },
            ],
            new_clues: vec!["线索A".into(), "线索A".into()], npc_updates: vec![],
            world_time: None, location: None, suggested_actions: vec![],
            suggested_check: None, add_flags: vec![], advance_act: false, trigger_ending_id: None,
            world_bulletin: None, reincarnation_echo: None, grant_inspiration: vec!["陆沉".into()],
        };
        let _ = apply_dm_reply(&mut room, &mut players, &reply);
        assert_eq!((players[0].hp, players[0].san), (20, 10));
        assert!(players[0].dnd.inspiration);
        assert_eq!((players[0].inventory.as_slice(), players[0].location.as_str(), room.clues.as_slice()), (&["火把".to_string()][..], "地下酒窖", &["线索A".to_string()][..]));
    }
}
