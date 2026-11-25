use serde::{Deserialize, Serialize};

use crate::decision::ReplyAuditVerdict;
use crate::models::{PendingAction, Player, RoomState, Scenario, TurnRecord};
use crate::protocol::{DmStructuredReply, StatePatch, WorldBulletinDraft};
use crate::world::resolve_map_node_id;

/// Spotlight fairness analysis inspired by multi-agent narrative pacing lessons:
/// ensures quieter active players aren't starved of camera focus by chatty teammates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpotlightGuidance {
    pub sidelined_players: Vec<String>,
    pub prompt_directive: Option<String>,
}

pub fn analyze_spotlight(
    players: &[Player],
    recent_turns: &[TurnRecord],
    current_actions: &[PendingAction],
) -> SpotlightGuidance {
    let active: Vec<&Player> = players.iter().filter(|p| !p.is_afk).collect();
    if active.len() < 2 || recent_turns.len() < 2 {
        return SpotlightGuidance::default();
    }

    let window: Vec<&TurnRecord> = recent_turns.iter().rev().take(3).collect();
    let mut sidelined = Vec::new();

    for p in &active {
        let name = p.character_name.trim();
        if name.is_empty() {
            continue;
        }
        let mut neglected = 0usize;
        for turn in &window {
            if turn.narrative.contains(name) {
                break;
            }
            neglected += 1;
        }
        let acted_now = current_actions
            .iter()
            .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty());
        if neglected >= 2 || (neglected >= 1 && acted_now && window.len() >= 2) {
            sidelined.push(name.to_string());
        }
    }

    if sidelined.is_empty() {
        return SpotlightGuidance::default();
    }

    let names = sidelined.join("、");
    let prompt_directive = format!(
        "【聚光灯均衡提醒】以下活跃角色最近几轮主镜头偏少：{names}。本轮描写请主动给他们安排专属环境细节、NPC 搭话或发现线索的契机，确保镜头均衡、不冷落任何在线队友（仍严禁替其做决定）。"
    );
    SpotlightGuidance {
        sidelined_players: sidelined,
        prompt_directive: Some(prompt_directive),
    }
}

/// Strictly scoped plan describing WHICH exact omitted fields the one-shot repair may touch.
/// Prevents double-applying HP/SAN deltas or leaking `private_whispers` into public clues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsistencyNudgePlan {
    pub prompt: String,
    pub want_vitals: bool,
    pub want_items: bool,
    pub want_location: bool,
    pub want_bulletin: bool,
    pub want_narrative_redact: bool,
}

fn null_as_empty_vec<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<T>>::deserialize(d)?.unwrap_or_default())
}

/// Strictly scoped JSON-only repair delta returned by the one-shot consistency nudge.
/// `deny_unknown_fields` prevents stray non-patch `{...}` objects from silently matching.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DmPatchDelta {
    #[serde(default, deserialize_with = "null_as_empty_vec")]
    pub state_patches: Vec<StatePatch>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub world_bulletin: Option<WorldBulletinDraft>,
    #[serde(default)]
    pub redacted_narrative: Option<String>,
}

/// Detects clear mismatches where DM narrative describes concrete state changes omitted from JSON.
/// Falls back to deterministic Chinese cue heuristics when no System One audit verdict is supplied.
pub fn detect_consistency_nudge(
    scenario: &Scenario,
    room: &RoomState,
    players: &[Player],
    reply: &DmStructuredReply,
    bulletin_expected: bool,
) -> Option<ConsistencyNudgePlan> {
    detect_consistency_nudge_with_audit(scenario, room, players, reply, bulletin_expected, None, 0.80)
}

/// Semantic-audit-enhanced consistency detector:
/// - When `audit` is `Some`, uses System One calibrated probabilities for vitals, item gains,
///   and secret whisper leaks (eliminating brittle keyword false positives like "幸好没受伤");
/// - Map relocation & due-bulletin checks remain 100% deterministic Rust.
pub fn detect_consistency_nudge_with_audit(
    scenario: &Scenario,
    room: &RoomState,
    players: &[Player],
    reply: &DmStructuredReply,
    bulletin_expected: bool,
    audit: Option<&ReplyAuditVerdict>,
    audit_threshold: f64,
) -> Option<ConsistencyNudgePlan> {
    if players.is_empty() || reply.narrative.trim().is_empty() {
        return None;
    }

    let mut issues = Vec::new();
    let text = reply.narrative.as_str();
    let text_low = text.to_ascii_lowercase();
    let thr = audit_threshold.clamp(0.50, 0.98);

    let mut want_vitals = false;
    let mut want_items = false;
    let mut want_location = false;
    let mut want_bulletin = false;
    let mut want_narrative_redact = false;

    let has_hp_or_san_patch = reply.state_patches.iter().any(|p| p.hp_delta != 0 || p.san_delta != 0);
    let has_add_item = reply.state_patches.iter().any(|p| !p.add_items.is_empty());

    if let Some(a) = audit {
        if !has_hp_or_san_patch && a.prob_missed_vitals >= thr {
            want_vitals = true;
            issues.push("语义质检发现正文描写了实际受伤或理智受创，但 `state_patches` 未填 `hp_delta`/`san_delta`，请补全".into());
        }
        if !has_add_item && a.prob_missed_item >= thr {
            want_items = true;
            issues.push("语义质检发现正文描写了角色拿到新道具，但 `state_patches.add_items` 为空，请仅补全对应 `add_items`".into());
        }
        let has_whispers = reply
            .private_whispers
            .iter()
            .any(|w| !w.secret.trim().is_empty());
        if has_whispers && a.prob_whisper_leak >= thr {
            want_narrative_redact = true;
            issues.push("语义质检警告：公开 `narrative` 可能泄露了 `private_whispers` 私密情报！若确有泄密请在 `redacted_narrative` 输出完整重写后的干净公开正文；若核查无泄密请填 null（严禁输出'同原文'等说明）".into());
        }
    } else {
        let damage_cues = [
            "受了伤", "鲜血渗出", "划开一道口子", "重重摔", "一阵剧痛", "灼伤",
            "理智一震", "头晕目眩", "san值", "一阵恶寒直冲脑海", "咳出血", "击中肩膀",
        ];
        if !has_hp_or_san_patch && damage_cues.iter().any(|k| text_low.contains(k)) {
            want_vitals = true;
            issues.push("正文描写了角色受伤或理智受创，但 `state_patches` 中未填写任何 `hp_delta`/`san_delta`（若确实未造成数值变化可留空，否则请补全）".into());
        }
        let gain_verbs = ["递给你", "交给了", "塞进你手", "拾起了", "摸出了", "收入囊中", "揣进怀里", "拿到了"];
        let item_nouns = ["钥匙", "怀表", "药剂", "抗雾剂", "铜哨", "手电", "简图", "银盾", "纸条", "密信", "短刃", "手术刀", "平安符"];
        if !has_add_item && gain_verbs.iter().any(|v| text.contains(v)) && item_nouns.iter().any(|n| text.contains(n)) {
            want_items = true;
            issues.push("正文描写了角色拿到/获赠关键道具，但 `state_patches.add_items` 为空，请仅补全对应角色的 `add_items`（勿重复已扣过的 HP/SAN）".into());
        }
    }

    if !scenario.map_nodes.is_empty() {
        let cur_node = resolve_map_node_id(&scenario.map_nodes, &room.location);
        let reply_loc = reply.location.as_deref().unwrap_or(&room.location);
        let reply_node = resolve_map_node_id(&scenario.map_nodes, reply_loc);
        if reply_node == cur_node {
            let move_verbs = ["来到", "步入", "推门进入", "走下", "踏入", "抵达", "钻进", "穿过连接门进入"];
            if move_verbs.iter().any(|v| text.contains(v)) {
                for node in &scenario.map_nodes {
                    if Some(node.id.as_str()) == cur_node.as_deref() { continue; }
                    let moved_there = move_verbs.iter().any(|v| {
                        text.contains(&format!("{v}{}", node.name))
                            || node.aliases.iter().any(|a| a.chars().count() >= 2 && text.contains(&format!("{v}{a}")))
                    });
                    if moved_there {
                        want_location = true;
                        issues.push(format!("正文已将小队带入新地点「{}」，但 `location` 仍停留在「{}」，请更新 `location`", node.name, reply_loc));
                        break;
                    }
                }
            }
        }
    }

    if bulletin_expected && reply.world_bulletin.is_none() {
        want_bulletin = true;
        issues.push("本轮已触发幕间暗流号外窗口，但未返回 `world_bulletin`，请补写一条简短的幕后势力号外（严禁泄露任何 private_whispers 内容）".into());
    }

    if issues.is_empty() {
        return None;
    }

    let player_names = players.iter().map(|p| p.character_name.as_str()).collect::<Vec<_>>().join("、");
    let schema = if want_narrative_redact {
        "{\"state_patches\":[],\"location\":null,\"world_bulletin\":null,\"redacted_narrative\":\"删去私密剧透后的公开正文\"}"
    } else {
        "{\"state_patches\":[],\"location\":null,\"world_bulletin\":null,\"redacted_narrative\":null}"
    };
    let prompt = format!(
        "【状态一致性补齐校验】检测到正文与结构化字段可能存在遗漏：\n- {}\n\n当前角色名单：[{}]。\n请**仅输出一个合法 JSON 补丁对象**（只填本次遗漏项，严禁重复首轮已扣过的 HP/SAN，严禁泄露任何私密纸条），结构严格限定为：\n{}\n无需改动的字段保持空数组或 null。",
        issues.join("\n- "),
        player_names,
        schema
    );
    Some(ConsistencyNudgePlan {
        prompt,
        want_vitals,
        want_items,
        want_location,
        want_bulletin,
        want_narrative_redact,
    })
}

/// Safely merges ONLY the fields explicitly requested by `plan`, preventing double-applied
/// HP/SAN deltas or unintended state mutations.
pub fn merge_patch_delta(
    reply: &mut DmStructuredReply,
    plan: &ConsistencyNudgePlan,
    delta: DmPatchDelta,
) {
    if plan.want_vitals || plan.want_items {
        for mut p in delta.state_patches {
            if !plan.want_vitals { p.hp_delta = 0; p.san_delta = 0; }
            if !plan.want_items { p.add_items.clear(); }
            p.remove_items.clear();
            p.status_note = None;
            p.location = None;
            if p.hp_delta != 0 || p.san_delta != 0 || !p.add_items.is_empty() {
                reply.state_patches.push(p);
            }
        }
    }
    if plan.want_location
        && let Some(loc) = delta.location.map(|s| s.trim().to_string())
        && !loc.is_empty()
    {
        reply.location = Some(loc);
    }
    if plan.want_bulletin && reply.world_bulletin.is_none() && delta.world_bulletin.is_some() {
        reply.world_bulletin = delta.world_bulletin;
    }
    if plan.want_narrative_redact
        && let Some(redacted) = delta.redacted_narrative.map(|s| s.trim().to_string())
        && !redacted.starts_with('{')
        && !redacted.contains("```")
    {
        let orig_len = reply.narrative.chars().count();
        let new_len = redacted.chars().count();
        let min_len = if orig_len >= 80 { (orig_len / 3).max(30) } else { 20 };
        if new_len >= min_len {
            reply.narrative = redacted;
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use super::*;
    use crate::models::{ChatMode, NarrativeStyle, StarterCharacterDefaults, TurnPhase};
    use crate::world::ScenarioMapNode;

    fn mk_player(uid: &str, name: &str) -> Player {
        Player {
            channel_id: "c1".into(), user_id: uid.into(), character_name: name.into(),
            role_desc: "旅人".into(), traits: vec![], hp: 10, max_hp: 10, san: 10, max_san: 10,
            inventory: vec![], status_note: String::new(), is_afk: false,
            stats: Default::default(), location: String::new(), dnd: Default::default(),
        }
    }

    #[test]
    fn spotlight_and_scoped_merge_prevent_double_damage() {
        let now = Utc::now();
        let players = vec![mk_player("u1", "阿明"), mk_player("u2", "小红")];
        let mk_turn = |id, tn, nar: &str| TurnRecord {
            id, channel_id: "c1".into(), turn_number: tn, batched_input: "".into(),
            scene_title: format!("{tn}"), narrative: nar.into(), state_summary: "".into(),
            location: "大厅".into(), world_time: "21:00".into(), created_at: now,
        };
        let turns = vec![
            mk_turn(1, 1, "阿明和老雷恩聊得很起劲。"),
            mk_turn(2, 2, "阿明推开了吧台后的暗门。"),
        ];
        assert_eq!(analyze_spotlight(&players, &turns, &[]).sidelined_players, vec!["小红"]);
        assert!(analyze_spotlight(&players[..1], &turns, &[]).sidelined_players.is_empty());

        let sc = Scenario {
            id: "s1".into(), name: "S".into(), tagline: "t".into(), genre: "g".into(),
            theme: "ember".into(), dm_style: "d".into(), initial_location: "酒馆大厅".into(),
            initial_time: "21:00".into(), opening_scene: "开场足够长一段场景文字描写。".into(),
            opening_actions: vec![], player_defaults: StarterCharacterDefaults { max_hp: 10, max_san: 10, starter_items: vec![] },
            preset_roles: vec![], acts: vec![], endings: vec![], worldbook: vec![],
            map_nodes: vec![
                ScenarioMapNode { id: "hall".into(), name: "酒馆大厅".into(), x: 20.0, y: 50.0, aliases: vec!["大厅".into()], kind: "hub".into() },
                ScenarioMapNode { id: "cellar".into(), name: "地下酒窖".into(), x: 40.0, y: 80.0, aliases: vec!["酒窖".into()], kind: "mystery".into() },
            ],
            map_edges: vec![], rules: Default::default(),
        };
        let room = RoomState {
            channel_id: "c1".into(), guild_id: "g1".into(), scenario_id: "s1".into(),
            title: "S".into(), location: "酒馆大厅".into(), world_time: "21:00".into(),
            turn_number: 1, state_version: 1, turn_phase: TurnPhase::Collecting,
            chat_mode: ChatMode::Strict, narrative_style: NarrativeStyle::Prose,
            gm_user_id: "u1".into(), web_token: "tok".into(), clues: vec![], npc_states: vec![],
            suggested_actions: vec![], current_act: 1, world_flags: vec![],
            ended_id: None, ended_title: None, bulletins: vec![], check_system: Default::default(),
            last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
            created_at: now, updated_at: now,
        };
        let mut reply = DmStructuredReply {
            scene_title: "深入地窖".into(),
            narrative: "老雷恩把铜钥匙递给你，一阵恶寒让你的SAN值剧震，你走下地下酒窖。".into(),
            private_whispers: vec![],
            state_patches: vec![StatePatch { player_name: "阿明".into(), hp_delta: -3, ..Default::default() }],
            new_clues: vec![], npc_updates: vec![], world_time: None,
            location: Some("酒馆大厅".into()), suggested_actions: vec![],
            suggested_check: None, add_flags: vec![], advance_act: false,
            trigger_ending_id: None, world_bulletin: None, reincarnation_echo: None,
            grant_inspiration: vec![],
        };
        let plan = detect_consistency_nudge(&sc, &room, &players, &reply, true).unwrap();
        assert!(!plan.want_vitals && plan.want_items && plan.want_location && plan.want_bulletin);

        merge_patch_delta(&mut reply, &plan, DmPatchDelta {
            state_patches: vec![StatePatch { player_name: "阿明".into(), hp_delta: -3, add_items: vec!["铜钥匙".into()], ..Default::default() }],
            location: Some("地下酒窖".into()), world_bulletin: None, redacted_narrative: None,
        });
        assert_eq!(reply.state_patches.iter().map(|p| p.hp_delta).sum::<i32>(), -3);
        assert_eq!(reply.location.as_deref(), Some("地下酒窖"));
    }
}
