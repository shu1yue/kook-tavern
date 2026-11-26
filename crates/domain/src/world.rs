use serde::{Deserialize, Serialize};

use crate::dnd::CheckSystem;
use crate::models::{RoomState, Scenario};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScenarioMapNode {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default = "default_node_kind")]
    pub kind: String,
}

fn default_node_kind() -> String {
    "hub".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioMapEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FactionAgenda {
    pub name: String,
    pub goal: String,
    #[serde(default)]
    pub escalation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DirectorBeat {
    pub act: u32,
    pub by_turn: u32,
    pub flag: String,
    pub prompt_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScenarioRules {
    #[serde(default)]
    pub check_system: CheckSystem,
    #[serde(default = "default_pace")]
    pub tension_pace: String,
    #[serde(default = "default_balanced")]
    pub lethality: String,
    #[serde(default = "default_balanced")]
    pub clue_generosity: String,
    #[serde(default)]
    pub dm_weight_note: String,
    #[serde(default)]
    pub bulletin_name: String,
    #[serde(default = "default_bulletin_interval")]
    pub bulletin_interval: u32,
    #[serde(default)]
    pub factions: Vec<FactionAgenda>,
    #[serde(default)]
    pub must_hit_beats: Vec<DirectorBeat>,
}

fn default_pace() -> String {
    "brisk".to_string()
}
fn default_balanced() -> String {
    "balanced".to_string()
}
fn default_bulletin_interval() -> u32 {
    6
}

impl Default for ScenarioRules {
    fn default() -> Self {
        Self {
            check_system: CheckSystem::default(),
            tension_pace: default_pace(),
            lethality: default_balanced(),
            clue_generosity: default_balanced(),
            dm_weight_note: String::new(),
            bulletin_name: String::new(),
            bulletin_interval: default_bulletin_interval(),
            factions: Vec::new(),
            must_hit_beats: Vec::new(),
        }
    }
}

impl ScenarioRules {
    pub fn has_custom_guidance(&self) -> bool {
        self.check_system != CheckSystem::TwoD6
            || !self.dm_weight_note.trim().is_empty()
            || !self.factions.is_empty()
            || !self.must_hit_beats.is_empty()
            || self.tension_pace != "brisk"
            || self.lethality != "balanced"
            || self.clue_generosity != "balanced"
    }

    pub fn effective_masthead(&self, scenario_name: &str) -> String {
        let m = self.bulletin_name.trim();
        if m.is_empty() {
            format!("《{} · 暗流号外》", scenario_name)
        } else {
            m.to_string()
        }
    }

    pub fn format_prompt_summary(&self) -> String {
        let pace = match self.tension_pace.trim().to_ascii_lowercase().as_str() {
            "steady" => "舒缓铺垫（留足调查与对话呼吸感）",
            "intense" => "高压紧迫（每轮施压并加快倒计时）",
            _ => "紧凑明快（调查与冲突交替推进）",
        };
        let lethal = match self.lethality.trim().to_ascii_lowercase().as_str() {
            "forgiving" => "宽容（侧重线索与戏剧冲突，少致命重创）",
            "harsh" => "冷酷（莽撞或大失败将带来严厉 HP/SAN 代价）",
            _ => "平衡（智取给回报，冒进给真实代价）",
        };
        let clues = match self.clue_generosity.trim().to_ascii_lowercase().as_str() {
            "strict" => "克制（核心线索需精确勘查或检定）",
            "generous" => "慷慨（鼓励大胆尝试，主动给足推进线索）",
            _ => "适中（有据可循，避免卡关）",
        };
        let mut s = format!(
            "默认检定体系：{} ｜ 节奏：{pace} ｜ 致命度：{lethal} ｜ 线索投放：{clues}",
            self.check_system.prompt_guide()
        );
        if !self.dm_weight_note.trim().is_empty() {
            s.push_str(&format!("\n- DM 裁决权重补充：{}", self.dm_weight_note.trim()));
        }
        s
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldBulletin {
    pub turn_number: u32,
    pub masthead: String,
    pub headline: String,
    pub body: String,
    #[serde(default)]
    pub faction: String,
}

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

/// Resolves a free-form location string onto the best matching scenario map node ID.
pub fn resolve_map_node_id(nodes: &[ScenarioMapNode], location_text: &str) -> Option<String> {
    let text = location_text.trim();
    if text.is_empty() || nodes.is_empty() {
        return None;
    }
    let low = text.to_ascii_lowercase();
    if let Some(n) = nodes
        .iter()
        .find(|n| n.id.eq_ignore_ascii_case(text) || n.name.trim() == text)
    {
        return Some(n.id.clone());
    }
    let mut best: Option<(&ScenarioMapNode, usize)> = None;
    for node in nodes {
        let mut candidates = vec![node.name.as_str()];
        candidates.extend(node.aliases.iter().map(String::as_str));
        for cand in candidates {
            let c = cand.trim();
            if c.is_empty() {
                continue;
            }
            let c_low = c.to_ascii_lowercase();
            if text.contains(c) || low.contains(&c_low) {
                let score = c.chars().count();
                if best.is_none_or(|(_, prev)| score > prev) {
                    best = Some((node, score));
                }
            }
        }
    }
    best.map(|(n, _)| n.id.clone())
}

/// Decides whether the upcoming turn should trigger an undercurrent faction bulletin.
/// Automatically silences street bulletins during the final climax act so boss showdowns
/// are never distracted by unrelated background rumors.
pub fn should_trigger_bulletin(scenario: &Scenario, room: &RoomState, next_turn: u32) -> bool {
    let interval = scenario.rules.bulletin_interval;
    if interval == 0 || scenario.rules.factions.is_empty() || next_turn < 2 {
        return false;
    }
    if !scenario.acts.is_empty() && room.current_act >= scenario.acts.len() as u32 {
        return false;
    }
    let last_turn = room.bulletins.last().map(|b| b.turn_number).unwrap_or(0);
    next_turn.saturating_sub(last_turn) >= interval
}

/// Returns director outline beats that are overdue in the current act and not yet flagged.
pub fn due_director_beats<'a>(
    scenario: &'a Scenario,
    room: &RoomState,
    next_turn: u32,
) -> Vec<&'a DirectorBeat> {
    scenario
        .rules
        .must_hit_beats
        .iter()
        .filter(|b| {
            let flag = b.flag.trim();
            !flag.is_empty()
                && b.act == room.current_act
                && next_turn >= b.by_turn
                && !room.world_flags.iter().any(|f| f.contains(flag))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_map_nodes_by_name_and_alias() {
        let nodes = vec![
            ScenarioMapNode {
                id: "hall".into(), name: "余烬酒馆大厅".into(), x: 20.0, y: 50.0,
                aliases: vec!["酒馆".into(), "柜台".into()], kind: "hub".into(),
            },
            ScenarioMapNode {
                id: "cellar".into(), name: "地下酿酒窖".into(), x: 35.0, y: 78.0,
                aliases: vec!["酒窖".into(), "17号".into()], kind: "mystery".into(),
            },
        ];
        assert_eq!(resolve_map_node_id(&nodes, "灰烬港 · 地下酒窖 17号桶旁").as_deref(), Some("cellar"));
        assert_eq!(resolve_map_node_id(&nodes, "余烬酒馆柜台").as_deref(), Some("hall"));
        assert_eq!(resolve_map_node_id(&nodes, "未知荒野"), None);
    }
}
