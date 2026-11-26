use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatKind {
    Body,
    Agility,
    Investigate,
    Social,
    Insight,
}

impl StatKind {
    pub const ALL: [StatKind; 5] = [
        StatKind::Body,
        StatKind::Agility,
        StatKind::Investigate,
        StatKind::Social,
        StatKind::Insight,
    ];

    pub fn key(self) -> &'static str {
        match self {
            StatKind::Body => "body",
            StatKind::Agility => "agility",
            StatKind::Investigate => "investigate",
            StatKind::Social => "social",
            StatKind::Insight => "insight",
        }
    }

    pub fn short_name(self) -> &'static str {
        match self {
            StatKind::Body => "体魄",
            StatKind::Agility => "敏捷",
            StatKind::Investigate => "侦查",
            StatKind::Social => "交涉",
            StatKind::Insight => "灵感",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            StatKind::Body => "💪",
            StatKind::Agility => "🏃",
            StatKind::Investigate => "🔍",
            StatKind::Social => "🗣️",
            StatKind::Insight => "✨",
        }
    }

    pub fn display_label(self) -> String {
        format!("{} {}", self.icon(), self.short_name())
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let s = raw.trim().to_ascii_lowercase();
        match s.as_str() {
            "body" | "str" | "con" | "体魄" | "力量" | "体质" | "搏斗" => Some(StatKind::Body),
            "agility" | "agi" | "dex" | "敏捷" | "身手" | "潜行" | "闪避" | "开锁" => {
                Some(StatKind::Agility)
            }
            "investigate" | "inv" | "det" | "侦查" | "观察" | "调查" | "搜查" | "洞察" => {
                Some(StatKind::Investigate)
            }
            "social" | "soc" | "cha" | "交涉" | "话术" | "说服" | "魅力" | "安抚" => {
                Some(StatKind::Social)
            }
            "insight" | "ins" | "wit" | "灵感" | "直觉" | "神秘" | "意志" | "学识" => {
                Some(StatKind::Insight)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterStats {
    #[serde(default)]
    pub body: i32,
    #[serde(default)]
    pub agility: i32,
    #[serde(default)]
    pub investigate: i32,
    #[serde(default)]
    pub social: i32,
    #[serde(default)]
    pub insight: i32,
}

impl CharacterStats {
    pub fn get(&self, kind: StatKind) -> i32 {
        let raw = match kind {
            StatKind::Body => self.body,
            StatKind::Agility => self.agility,
            StatKind::Investigate => self.investigate,
            StatKind::Social => self.social,
            StatKind::Insight => self.insight,
        };
        raw.clamp(-3, 5)
    }

    pub fn add(&mut self, kind: StatKind, delta: i32) {
        let slot = match kind {
            StatKind::Body => &mut self.body,
            StatKind::Agility => &mut self.agility,
            StatKind::Investigate => &mut self.investigate,
            StatKind::Social => &mut self.social,
            StatKind::Insight => &mut self.insight,
        };
        *slot = (*slot + delta).clamp(-3, 5);
    }

    pub fn format_inline(&self) -> String {
        format!(
            "💪{} 🏃{} 🔍{} 🗣️{} ✨{}",
            fmt_mod(self.body),
            fmt_mod(self.agility),
            fmt_mod(self.investigate),
            fmt_mod(self.social),
            fmt_mod(self.insight),
        )
    }

    pub fn format_prompt(&self) -> String {
        format!(
            "体魄{} / 敏捷{} / 侦查{} / 交涉{} / 灵感{}",
            fmt_mod(self.body),
            fmt_mod(self.agility),
            fmt_mod(self.investigate),
            fmt_mod(self.social),
            fmt_mod(self.insight),
        )
    }
}

fn fmt_mod(v: i32) -> String {
    let v = v.clamp(-3, 5);
    if v >= 0 { format!("+{v}") } else { format!("{v}") }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckTier {
    CritFailure,
    Failure,
    Partial,
    Success,
    CritSuccess,
}

impl CheckTier {
    pub fn from_total(total: i32) -> Self {
        if total >= 10 {
            CheckTier::Success
        } else if total >= 7 {
            CheckTier::Partial
        } else {
            CheckTier::Failure
        }
    }

    pub fn label(self) -> &'static str {
        crate::dnd_ops::check_tier_label_for(self, crate::dnd::CheckSystem::TwoD6)
    }

    pub fn label_for(self, sys: crate::dnd::CheckSystem) -> &'static str {
        crate::dnd_ops::check_tier_label_for(self, sys)
    }

    pub fn rule_hint(self) -> &'static str {
        crate::dnd_ops::check_tier_hint_for(self, crate::dnd::CheckSystem::TwoD6)
    }

    pub fn rule_hint_for(self, sys: crate::dnd::CheckSystem) -> &'static str {
        crate::dnd_ops::check_tier_hint_for(self, sys)
    }
}

#[derive(Debug, Clone)]
pub struct StatCheckResult {
    pub stat: StatKind,
    pub rolls: [i32; 2],
    pub modifier: i32,
    pub total: i32,
    pub tier: CheckTier,
    pub reason: Option<String>,
}

impl StatCheckResult {
    pub fn format_short(&self) -> String {
        let reason_tag = self
            .reason
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|r| format!(" · {}", r.trim()))
            .unwrap_or_default();
        let mod_str = if self.modifier > 0 {
            format!(" + {}", self.modifier)
        } else if self.modifier < 0 {
            format!(" - {}", -self.modifier)
        } else {
            String::new()
        };
        format!(
            "🎲【{}检定{reason_tag}】2d6{} = [{}, {}]{mod_str} → **{}** ｜ {}",
            self.stat.short_name(),
            fmt_mod(self.modifier),
            self.rolls[0],
            self.rolls[1],
            self.total,
            self.tier.label()
        )
    }
}

pub fn perform_stat_check(
    stat: StatKind,
    stats: &CharacterStats,
    reason: Option<String>,
) -> StatCheckResult {
    let mut rng = rand::thread_rng();
    perform_stat_check_with_rng(stat, stats, reason, &mut rng)
}

pub fn perform_stat_check_with_rng<R: Rng + ?Sized>(
    stat: StatKind,
    stats: &CharacterStats,
    reason: Option<String>,
    rng: &mut R,
) -> StatCheckResult {
    let a = rng.gen_range(1..=6);
    let b = rng.gen_range(1..=6);
    let modifier = stats.get(stat);
    let total = a + b + modifier;
    let tier = CheckTier::from_total(total);
    StatCheckResult {
        stat,
        rolls: [a, b],
        modifier,
        total,
        tier,
        reason,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SuggestedCheck {
    pub stat: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub dc: Option<i32>,
}

impl SuggestedCheck {
    pub fn parsed_stat(&self) -> Option<StatKind> {
        StatKind::parse(&self.stat)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioAct {
    pub id: String,
    pub title: String,
    pub goal: String,
    #[serde(default)] pub advance_flags: Vec<String>,
    /// Optional map node IDs whose entry deterministically advances the campaign to this Act
    /// even if the LLM omits `"advance_act": true`.
    #[serde(default)] pub enter_nodes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioEnding {
    pub id: String,
    pub title: String,
    #[serde(default = "default_ending_kind")]
    pub kind: String,
    pub summary: String,
}

fn default_ending_kind() -> String {
    "normal".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndingUnlock {
    pub guild_id: String,
    pub scenario_id: String,
    pub ending_id: String,
    pub ending_title: String,
    pub ending_kind: String,
    pub turn_count: u32,
    pub party_names: String,
    pub unlocked_at: DateTime<Utc>,
}

/// Canonical isolation key for ending achievements:
/// - Guild rooms use their authoritative `guild_id`;
/// - Direct messages or guildless legacy rooms are strictly scoped per `channel_id`
///   (`chan:<channel_id>`) so players and guilds NEVER cross-leak ending records.
pub fn ending_scope_key(guild_id: Option<&str>, channel_id: &str) -> String {
    if let Some(g) = guild_id.map(str::trim).filter(|s| !s.is_empty()) {
        g.to_string()
    } else {
        format!("chan:{}", channel_id.trim())
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    #[test]
    fn stat_parse_and_three_tier_check_work() {
        assert_eq!(StatKind::parse("侦查"), Some(StatKind::Investigate));
        assert_eq!(StatKind::parse("body"), Some(StatKind::Body));
        assert_eq!(StatKind::parse("神秘"), Some(StatKind::Insight));
        assert_eq!(StatKind::parse("unknown"), None);

        let stats = CharacterStats {
            body: 1,
            agility: 2,
            investigate: 3,
            social: -1,
            insight: 0,
        };
        let mut rng = StdRng::seed_from_u64(7);
        let res = perform_stat_check_with_rng(
            StatKind::Investigate,
            &stats,
            Some("翻找酒桶夹层".into()),
            &mut rng,
        );
        assert_eq!(res.modifier, 3);
        assert_eq!(res.total, res.rolls[0] + res.rolls[1] + 3);
        assert_eq!(res.tier, CheckTier::from_total(res.total));
        assert!(res.format_short().contains("侦查检定"));

        assert_eq!(CheckTier::from_total(6), CheckTier::Failure);
        assert_eq!(CheckTier::from_total(7), CheckTier::Partial);
        assert_eq!(CheckTier::from_total(9), CheckTier::Partial);
        assert_eq!(CheckTier::from_total(10), CheckTier::Success);
    }
}
