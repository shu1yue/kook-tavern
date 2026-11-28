use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::models::{RoomState, Scenario};
use crate::protocol::ClockUpdate;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ClockKind {
    #[default]
    Threat,
    Progress,
}

impl ClockKind {
    pub fn label(self) -> &'static str {
        match self {
            ClockKind::Threat => "威胁钟",
            ClockKind::Progress => "进度钟",
        }
    }
}

fn default_clock_segments() -> u8 {
    6
}

/// Blades-in-the-Dark style pacing clock definition declared on a Scenario.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioClock {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: ClockKind,
    #[serde(default = "default_clock_segments")]
    pub segments: u8,
    /// Deterministic server auto-tick every N turns (0 = ticks only on DM/player progress).
    #[serde(default)]
    pub auto_tick_every: u8,
    /// Concrete narrative consequence enforced the moment this clock reaches full.
    #[serde(default)]
    pub on_full: String,
}

/// Live runtime state of a pacing clock persisted in `RoomState`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClockState {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: ClockKind,
    #[serde(default)]
    pub filled: u8,
    #[serde(default = "default_clock_segments")]
    pub segments: u8,
    #[serde(default)]
    pub auto_tick_every: u8,
    #[serde(default)]
    pub on_full: String,
    #[serde(default)]
    pub triggered: bool,
}

/// Builds initial room clocks from scenario YAML, falling back to a universal 2-clock pair
/// (Threat + Progress) so legacy, SillyTavern, and custom scenarios all get hard pacing pressure.
pub fn init_room_clocks(scenario: &Scenario) -> Vec<ClockState> {
    let mut out: Vec<ClockState> = scenario
        .clocks
        .iter()
        .filter(|c| !c.id.trim().is_empty() && !c.name.trim().is_empty())
        .take(4)
        .map(|c| {
            let on_full = if c.on_full.trim().is_empty() {
                match c.kind {
                    ClockKind::Threat => {
                        "危机彻底爆发并直接找上门来，必须当场迎战或承受惨痛代价！".to_string()
                    }
                    ClockKind::Progress => {
                        "核心真相与破局缺口彻底敞开，立即进入终局摊牌与最终抉择！".to_string()
                    }
                }
            } else {
                c.on_full.trim().to_string()
            };
            ClockState {
                id: c.id.trim().to_string(),
                name: c.name.trim().to_string(),
                kind: c.kind,
                filled: 0,
                segments: c.segments.clamp(3, 12),
                auto_tick_every: c.auto_tick_every.min(6),
                on_full,
                triggered: false,
            }
        })
        .collect();

    if out.is_empty() {
        out.push(ClockState {
            id: "crisis_countdown".to_string(),
            name: "幕后危机倒计时".to_string(),
            kind: ClockKind::Threat,
            filled: 0,
            segments: 6,
            auto_tick_every: 2,
            on_full: format!(
                "《{}》的幕后危机彻底失控并直接逼到玩家眼前，局势恶化进入生死摊牌，严禁继续拖延！",
                scenario.name
            ),
            triggered: false,
        });
        out.push(ClockState {
            id: "truth_breakthrough".to_string(),
            name: "核心破局进度".to_string(),
            kind: ClockKind::Progress,
            filled: 0,
            segments: 4,
            auto_tick_every: 0,
            on_full: format!(
                "《{}》的关键真相与幕后黑手彻底暴露，破局条件全部齐备，立即推进最终决战与结局收束！",
                scenario.name
            ),
            triggered: false,
        });
    }
    out
}

/// Formats a single pacing clock as a compact visual bar for KOOK cards and `/story`.
pub fn format_clock_bar(c: &ClockState) -> String {
    let total = c.segments.clamp(1, 12) as usize;
    let filled = (c.filled as usize).min(total);
    let bar = format!("{}{}", "●".repeat(filled), "○".repeat(total - filled));
    let icon = match c.kind {
        ClockKind::Threat => "🔴 ⏳",
        ClockKind::Progress => "🟢 🔑",
    };
    let suffix = if filled >= total {
        " **【已满·爆发！】**"
    } else {
        ""
    };
    format!("{icon} **{}** {bar} `{filled}/{total}`{suffix}", c.name)
}

/// Authoritative server-side clock settlement:
/// aggregates LLM `clock_updates` (clamped `-2..=2` per clock), runs deterministic `auto_tick_every`
/// without double-ticking the same clock on the same turn, and fires one-shot full-clock alerts.
pub fn apply_clock_progression(
    room: &mut RoomState,
    scenario: &Scenario,
    updates: &[ClockUpdate],
    resolving_turn: u32,
    state_changes: &mut Vec<String>,
) {
    if room.clocks.is_empty() {
        room.clocks = init_room_clocks(scenario);
    }
    let mut agg_clocks: BTreeMap<usize, (i16, String)> = BTreeMap::new();
    for upd in updates.iter().take(6) {
        let key = upd.id.trim();
        if key.is_empty() || upd.delta == 0 {
            continue;
        }
        let Some(idx) = room
            .clocks
            .iter()
            .position(|c| c.id == key || c.id.eq_ignore_ascii_case(key) || c.name == key)
        else {
            continue;
        };
        let entry = agg_clocks.entry(idx).or_insert((0, String::new()));
        entry.0 = entry.0.saturating_add(i16::from(upd.delta));
        let reason = upd.reason.trim();
        if !reason.is_empty() && entry.1.is_empty() {
            entry.1 = reason.chars().take(28).collect();
        }
    }

    let mut llm_positively_ticked: BTreeSet<usize> = BTreeSet::new();
    for (idx, (raw_delta, reason)) in agg_clocks {
        let delta = raw_delta.clamp(-2, 2);
        if delta == 0 {
            continue;
        }
        let clock = &mut room.clocks[idx];
        let old = clock.filled;
        let max_seg = clock.segments.clamp(1, 12);
        let new_val = (i16::from(old) + delta).clamp(0, i16::from(max_seg)) as u8;
        if delta > 0 {
            llm_positively_ticked.insert(idx);
        }
        if new_val != old {
            clock.filled = new_val;
            let actual = i16::from(new_val) - i16::from(old);
            let sign = if actual > 0 { "+" } else { "" };
            let icon = match clock.kind {
                ClockKind::Threat => "🔴 ⏳",
                ClockKind::Progress => "🟢 🔑",
            };
            let why = if reason.is_empty() {
                String::new()
            } else {
                format!(" · {reason}")
            };
            state_changes.push(format!(
                "{icon} **{}** {old}→{new_val}/{max_seg} ({sign}{actual}{why})",
                clock.name
            ));
        }
    }

    for (idx, clock) in room.clocks.iter_mut().enumerate() {
        let max_seg = clock.segments.clamp(1, 12);
        if clock.auto_tick_every > 0
            && resolving_turn > 0
            && resolving_turn.is_multiple_of(u32::from(clock.auto_tick_every))
            && !clock.triggered
            && clock.filled < max_seg
            && !llm_positively_ticked.contains(&idx)
        {
            let old = clock.filled;
            clock.filled = (clock.filled + 1).min(max_seg);
            let icon = match clock.kind {
                ClockKind::Threat => "🔴 ⏳",
                ClockKind::Progress => "🟢 🔑",
            };
            state_changes.push(format!(
                "{icon} **{}** {old}→{}/{max_seg} (+1 · 时限逼近自动推进)",
                clock.name, clock.filled
            ));
        }
    }

    for clock in &mut room.clocks {
        let max_seg = clock.segments.clamp(1, 12);
        if !clock.triggered && clock.filled >= max_seg {
            clock.triggered = true;
            let banner = match clock.kind {
                ClockKind::Threat => format!(
                    "🚨 **威胁钟已满 · 【{}】({}/{})！** {}",
                    clock.name, clock.filled, max_seg, clock.on_full
                ),
                ClockKind::Progress => format!(
                    "🎉 **进度钟已满 · 【{}】({}/{})！** {}",
                    clock.name, clock.filled, max_seg, clock.on_full
                ),
            };
            state_changes.push(banner);
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::models::{ChatMode, NarrativeStyle, Player, RoomState, TurnPhase};
    use crate::patch::apply_dm_reply;
    use crate::protocol::{DmStructuredReply, StatePatch};

    #[test]
    fn aggregates_multiple_patches_and_clamps_bounds() {
        let now = Utc::now();
        let mut room = RoomState {
            channel_id: "c1".into(), guild_id: "g1".into(), scenario_id: "s1".into(),
            title: "T".into(), location: "Loc".into(), world_time: "Now".into(),
            turn_number: 1, state_version: 1, turn_phase: TurnPhase::Collecting,
            chat_mode: ChatMode::Strict, narrative_style: NarrativeStyle::Prose, manual_resolve: false,
            gm_user_id: "u1".into(), web_token: "secret_tok".into(),
            clues: vec![], npc_states: vec![], suggested_actions: vec![],
            current_act: 1, world_flags: vec![], ended_id: None, ended_title: None,
            bulletins: vec![], clocks: vec![], check_system: Default::default(), last_rest_turn: 0,
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
            scene_title: "测试".into(), turn_outcome: "拿到火把并移动".into(), narrative: "轰！".into(), private_whispers: vec![],
            state_patches: vec![
                StatePatch { player_name: "陆沉".into(), hp_delta: -25, san_delta: 999, add_items: vec!["火把".into()], remove_items: vec!["不存在的物品".into()], status_note: None, location: Some("地下酒窖".into()) },
                StatePatch { player_name: "陆沉".into(), hp_delta: -25, san_delta: 0, add_items: vec![], remove_items: vec!["扳手".into()], status_note: Some("重伤".into()), location: None },
            ],
            new_clues: vec!["线索A".into(), "线索A".into()], npc_updates: vec![],
            world_time: None, location: None, suggested_actions: vec![],
            suggested_check: None, add_flags: vec![], advance_act: false, trigger_ending_id: None,
            world_bulletin: None, reincarnation_echo: None, grant_inspiration: vec!["陆沉".into()],
            clock_updates: vec![],
        };
        let _ = apply_dm_reply(&mut room, &mut players, &reply);
        assert_eq!((players[0].hp, players[0].san), (20, 10));
        assert!(players[0].dnd.inspiration);
        assert_eq!((players[0].inventory.as_slice(), players[0].location.as_str(), room.clues.as_slice()), (&["火把".to_string()][..], "地下酒窖", &["线索A".to_string()][..]));
    }
}
