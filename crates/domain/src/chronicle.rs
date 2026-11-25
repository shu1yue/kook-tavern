use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::TurnRecord;
use crate::stats::ScenarioEnding;
use crate::world::WorldBulletin;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorldEcho {
    #[serde(default)]
    pub id: i64,
    pub guild_id: String,
    pub scenario_id: String,
    #[serde(default)]
    pub source_channel_id: String,
    #[serde(default)]
    pub source_turn: u32,
    pub ending_id: String,
    pub ending_title: String,
    pub ending_kind: String,
    pub party_names: String,
    pub echo_text: String,
    pub turn_count: u32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChronicleEntry {
    pub turn_number: u32,
    pub importance: u8,
    pub tag: String,
    pub location: String,
    pub world_time: String,
    pub headline: String,
    pub detail: String,
}

/// Builds a `civ-atlas`-style importance-graded chronicle (`1`=normal, `2`=notable, `3`=milestone).
pub fn build_chronicle(
    turns: &[TurnRecord],
    bulletins: &[WorldBulletin],
    ended_title: Option<&str>,
) -> Vec<ChronicleEntry> {
    let mut entries = Vec::with_capacity(turns.len() + bulletins.len());
    let last_turn_num = turns.last().map(|t| t.turn_number).unwrap_or(0);

    for t in turns {
        let sum = t.state_summary.as_str();
        let is_ending = ended_title.is_some() && t.turn_number == last_turn_num;
        let (importance, tag) = if is_ending {
            (3, "终局")
        } else if sum.contains("🎬 剧情推进") {
            (3, "转幕")
        } else if (sum.contains("HP") && sum.contains("(-"))
            || (sum.contains("SAN") && sum.contains("(-"))
        {
            (2, "危情")
        } else if sum.contains("🚩 关键进展") || sum.contains("🔎 新线索") {
            (2, "线索")
        } else if sum.contains("📍 场景移动") {
            (2, "移动")
        } else {
            (1, "推进")
        };
        let detail = if sum.trim().is_empty() {
            t.narrative.chars().take(72).collect::<String>()
        } else {
            sum.chars().take(120).collect::<String>()
        };
        entries.push(ChronicleEntry {
            turn_number: t.turn_number,
            importance,
            tag: tag.to_string(),
            location: t.location.clone(),
            world_time: t.world_time.clone(),
            headline: t.scene_title.clone(),
            detail,
        });
    }

    for b in bulletins {
        entries.push(ChronicleEntry {
            turn_number: b.turn_number,
            importance: 2,
            tag: "号外".to_string(),
            location: b.masthead.clone(),
            world_time: format!("第 {} 回合暗流", b.turn_number),
            headline: b.headline.clone(),
            detail: b.body.clone(),
        });
    }

    entries.sort_by_key(|e| (e.turn_number, if e.tag == "号外" { 1 } else { 0 }));
    entries
}

/// Distills a concise reincarnation worldline echo when a room reaches an ending.
pub fn distill_echo_text(
    ending: &ScenarioEnding,
    dm_echo: Option<&str>,
    party_names: &str,
    world_flags: &[String],
) -> String {
    if let Some(raw) = dm_echo.map(str::trim).filter(|s| !s.is_empty()) {
        return raw.chars().take(140).collect();
    }
    let crew = if party_names.trim().is_empty() {
        "上一队无名旅人"
    } else {
        party_names.trim()
    };
    let flag_hint = world_flags
        .iter()
        .rev()
        .take(2)
        .cloned()
        .collect::<Vec<_>>()
        .join("、");
    if flag_hint.is_empty() {
        format!("上一周目（{crew}）曾在此迎来《{}》：{}", ending.title, ending.summary)
            .chars()
            .take(140)
            .collect()
    } else {
        format!(
            "上一周目（{crew}）留下「{flag_hint}」的痕迹，最终走向《{}》。",
            ending.title
        )
        .chars()
        .take(140)
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_graded_chronicle_with_bulletins_and_endings() {
        let now = Utc::now();
        let turns = vec![
            TurnRecord {
                id: 1, channel_id: "c1".into(), turn_number: 1, batched_input: "".into(),
                scene_title: "开局试探".into(), narrative: "环顾四周".into(),
                state_summary: String::new(), location: "大厅".into(), world_time: "21:00".into(),
                created_at: now,
            },
            TurnRecord {
                id: 2, channel_id: "c1".into(), turn_number: 2, batched_input: "".into(),
                scene_title: "暗格开启".into(), narrative: "拿到钥匙".into(),
                state_summary: "🎬 剧情推进至第 2/3 幕".into(), location: "酒窖".into(),
                world_time: "21:20".into(), created_at: now,
            },
        ];
        let bulletins = vec![WorldBulletin {
            turn_number: 2, masthead: "《余烬号外》".into(), headline: "修女现身码头".into(),
            body: "教会封锁了东栈桥。".into(), faction: "圣光教会".into(),
        }];
        let c = build_chronicle(&turns, &bulletins, None);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0].importance, 1);
        assert_eq!(c[1].importance, 3);
        assert_eq!(c[2].tag, "号外");
    }
}
