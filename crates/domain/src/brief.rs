use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::Scenario;

/// Spoiler-free table handout shown to players at Turn 0, `/story`, Floating HUD, and Web Dashboard.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerBrief {
    /// 2-3 sentence world stage & atmosphere setup (zero DM-only secrets).
    #[serde(default)]
    pub world_stage: String,
    /// Clear opening mission / immediate stake for the party.
    #[serde(default)]
    pub mission: String,
    /// Brief spoiler-free cast or faction pointers players know at the start.
    #[serde(default)]
    pub cast_intro: Vec<String>,
    /// Spoiler-free preview of major fate branches (direction & stakes ONLY, never twists/solutions).
    #[serde(default)]
    pub ending_paths: Vec<String>,
}

impl PlayerBrief {
    pub fn is_empty(&self) -> bool {
        self.world_stage.trim().is_empty()
            && self.mission.trim().is_empty()
            && self.cast_intro.is_empty()
            && self.ending_paths.is_empty()
    }
}

/// Persistent registry entry for an isolated text channel dynamically created by the bot.
/// ONLY channels present and open in this table may ever be deleted via "拆掉包间".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManagedTable {
    pub channel_id: String,
    pub guild_id: String,
    pub lobby_channel_id: String,
    pub scenario_id: String,
    pub scenario_name: String,
    pub creator_user_id: String,
    pub created_at: DateTime<Utc>,
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let prefix: String = trimmed.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{prefix}…")
}

/// Returns the scenario's player brief, safely synthesizing spoiler-free fallbacks
/// for legacy or SillyTavern-imported scenarios that did not author `player_brief`.
pub fn effective_player_brief(sc: &Scenario) -> PlayerBrief {
    let raw = &sc.player_brief;

    let world_stage = if !raw.world_stage.trim().is_empty() {
        raw.world_stage.trim().to_string()
    } else if let Some(handout) = sc.worldbook.iter().find(|e| e.player_visible) {
        truncate_chars(&handout.content, 120)
    } else {
        format!(
            "{} · {}（起始地：{}｜{}）",
            sc.genre.trim(),
            sc.tagline.trim(),
            sc.initial_location.trim(),
            sc.initial_time.trim()
        )
    };

    let mission = if !raw.mission.trim().is_empty() {
        raw.mission.trim().to_string()
    } else if let Some(act1) = sc.acts.first().filter(|a| !a.goal.trim().is_empty()) {
        truncate_chars(&act1.goal, 96)
    } else {
        truncate_chars(&sc.tagline, 96)
    };

    let cast_intro = if !raw.cast_intro.is_empty() {
        raw.cast_intro
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .take(5)
            .collect()
    } else {
        sc.preset_roles
            .iter()
            .take(4)
            .map(|r| {
                if r.traits.is_empty() {
                    format!("可选身份·{}", r.name)
                } else {
                    format!("可选身份·{}（{}）", r.name, r.traits.join(" / "))
                }
            })
            .collect()
    };

    let ending_paths = if !raw.ending_paths.is_empty() {
        raw.ending_paths
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .take(4)
            .collect()
    } else if !sc.endings.is_empty() {
        let mut true_cnt = 0usize;
        let mut normal_cnt = 0usize;
        let mut bad_cnt = 0usize;
        let mut secret_cnt = 0usize;
        for e in &sc.endings {
            match e.kind.trim().to_ascii_lowercase().as_str() {
                "true" => true_cnt += 1,
                "bad" => bad_cnt += 1,
                "secret" => secret_cnt += 1,
                _ => normal_cnt += 1,
            }
        }
        let mut tags = Vec::new();
        if true_cnt > 0 {
            tags.push(format!("真结局×{true_cnt}"));
        }
        if normal_cnt > 0 {
            tags.push(format!("常规结局×{normal_cnt}"));
        }
        if bad_cnt > 0 {
            tags.push(format!("危机/坏结局×{bad_cnt}"));
        }
        if secret_cnt > 0 {
            tags.push(format!("隐藏分歧×{secret_cnt}"));
        }
        vec![format!(
            "本局共埋有 {} 条命运收束（{}），调查深度、阵营站队与关键抉择将决定最终走向。",
            sc.endings.len(),
            tags.join(" / ")
        )]
    } else {
        Vec::new()
    };

    PlayerBrief {
        world_stage,
        mission,
        cast_intro,
        ending_paths,
    }
}

/// Compact 3-line player brief block embedded on Turn 0 opening cards.
pub fn format_opening_brief_note(sc: &Scenario) -> Vec<String> {
    let brief = effective_player_brief(sc);
    let mut lines = Vec::new();
    if !brief.world_stage.is_empty() {
        lines.push(format!("🌍 世界舞台：{}", truncate_chars(&brief.world_stage, 84)));
    }
    if !brief.mission.is_empty() {
        lines.push(format!("🎯 开局任务书：{}", truncate_chars(&brief.mission, 84)));
    }
    if !brief.ending_paths.is_empty() {
        lines.push(format!(
            "🔀 命运分歧：{}条线索走向（发 `/story` 可私密查看完整无剧透手册）",
            brief.ending_paths.len()
        ));
    }
    lines
}

/// Full spoiler-free handbook markdown for `/story` private overlay.
pub fn format_full_brief_markdown(sc: &Scenario) -> String {
    let brief = effective_player_brief(sc);
    let mut lines = Vec::new();
    if !brief.world_stage.is_empty() {
        lines.push(format!("🌍 **世界舞台**\n{}", brief.world_stage));
    }
    if !brief.mission.is_empty() {
        lines.push(format!("🎯 **当前任务书**\n{}", brief.mission));
    }
    if !brief.cast_intro.is_empty() {
        let cast = brief
            .cast_intro
            .iter()
            .map(|c| format!("• {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        lines.push(format!("🎭 **公开人物 / 势力名录**\n{cast}"));
    }
    if !brief.ending_paths.is_empty() {
        let paths = brief
            .ending_paths
            .iter()
            .map(|p| format!("• {p}"))
            .collect::<Vec<_>>()
            .join("\n");
        lines.push(format!("🔀 **命运分歧预告（无剧透）**\n{paths}"));
    }
    lines.join("\n\n")
}
