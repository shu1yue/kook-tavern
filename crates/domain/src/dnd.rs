use chrono::{DateTime, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};

use crate::dnd_ops::check_tier_label_for;
use crate::stats::{CharacterStats, CheckTier, StatKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckSystem {
    #[default]
    #[serde(rename = "2d6", alias = "two_d6", alias = "pbta")]
    TwoD6,
    #[serde(rename = "d20", alias = "dnd", alias = "dnd5e")]
    D20,
    #[serde(rename = "d100", alias = "coc")]
    D100,
}

impl CheckSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            CheckSystem::TwoD6 => "2d6",
            CheckSystem::D20 => "d20",
            CheckSystem::D100 => "d100",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "2d6" | "pbta" | "轻量" | "三档" => Some(CheckSystem::TwoD6),
            "d20" | "dnd" | "dnd5e" | "龙与地下城" => Some(CheckSystem::D20),
            "d100" | "coc" | "克苏鲁" | "百分" => Some(CheckSystem::D100),
            _ => None,
        }
    }

    pub fn short_badge(self) -> &'static str {
        match self {
            CheckSystem::TwoD6 => "🎲 2d6 三档",
            CheckSystem::D20 => "🐉 D&D d20",
            CheckSystem::D100 => "🐙 CoC d100",
        }
    }

    pub fn prompt_guide(self) -> &'static str {
        match self {
            CheckSystem::TwoD6 => "2d6+属性三档检定（≤6失败 / 7~9代价成功 / ≥10完全成功）",
            CheckSystem::D20 => "经典 D&D d20+属性+熟练(+2) 对抗 DC（默认DC12，简单10/困难15/史诗20；Nat 20天然大成功、Nat 1天然大失败，支持优势/劣势）",
            CheckSystem::D100 => "经典 d100 技能检定（目标值=50+属性×10+熟练15%；≤5大成功、≤半数困难成功、≥96大失败）",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RollMode {
    #[default]
    Normal,
    Advantage,
    Disadvantage,
}

impl RollMode {
    pub fn label(self) -> &'static str {
        match self {
            RollMode::Normal => "",
            RollMode::Advantage => "【优势】",
            RollMode::Disadvantage => "【劣势】",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AbilityRecharge {
    #[default]
    #[serde(alias = "short")]
    ShortRest,
    #[serde(alias = "long")]
    LongRest,
}

impl AbilityRecharge {
    pub fn short_label(self) -> &'static str {
        match self {
            AbilityRecharge::ShortRest => "短休恢复",
            AbilityRecharge::LongRest => "长休恢复",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterAbility {
    pub name: String,
    #[serde(default = "default_ability_uses")]
    pub max_uses: u32,
    #[serde(default = "default_ability_uses")]
    pub uses_left: u32,
    #[serde(default)]
    pub recharge: AbilityRecharge,
    #[serde(default)]
    pub desc: String,
}

fn default_ability_uses() -> u32 { 1 }
fn default_level() -> u32 { 1 }

impl CharacterAbility {
    pub fn normalized(mut self) -> Self {
        self.max_uses = self.max_uses.clamp(1, 6);
        self.uses_left = self.uses_left.min(self.max_uses);
        self
    }

    pub fn with_full_uses(mut self) -> Self {
        self.max_uses = self.max_uses.clamp(1, 6);
        self.uses_left = self.max_uses;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerDndProfile {
    #[serde(default = "default_level")]
    pub level: u32,
    #[serde(default)]
    pub pending_levelups: u32,
    #[serde(default)]
    pub inspiration: bool,
    #[serde(default)]
    pub assisted_by: String,
    #[serde(default)]
    pub alignment: String,
    #[serde(default)]
    pub proficient_skills: Vec<String>,
    #[serde(default)]
    pub abilities: Vec<CharacterAbility>,
    #[serde(default)]
    pub backstory: String,
    #[serde(default)]
    pub personal_secret: String,
}

impl Default for PlayerDndProfile {
    fn default() -> Self {
        Self {
            level: 1,
            pending_levelups: 0,
            inspiration: false,
            assisted_by: String::new(),
            alignment: String::new(),
            proficient_skills: Vec::new(),
            abilities: Vec::new(),
            backstory: String::new(),
            personal_secret: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedCharacter {
    pub user_id: String,
    pub character_name: String,
    pub role_desc: String,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default)]
    pub stats: CharacterStats,
    #[serde(default)]
    pub dnd: PlayerDndProfile,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct RuleCheckResult {
    pub system: CheckSystem,
    pub stat: StatKind,
    pub mode: RollMode,
    pub rolls: Vec<i32>,
    pub kept_roll: i32,
    pub stat_mod: i32,
    pub prof_bonus: i32,
    pub prof_skill: Option<String>,
    pub total: i32,
    pub target_dc: i32,
    pub tier: CheckTier,
    pub reason: Option<String>,
}

impl RuleCheckResult {
    pub fn format_short(&self) -> String {
        let reason_tag = self
            .reason
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|r| format!(" · {}", r.trim()))
            .unwrap_or_default();
        let mode_tag = self.mode.label();
        let prof_tag = match &self.prof_skill {
            Some(sk) if self.prof_bonus > 0 => format!(" + 熟练({sk} +{})", self.prof_bonus),
            _ => String::new(),
        };
        let rolls_str = self.rolls.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ");
        match self.system {
            CheckSystem::D20 => format!(
                "🐉【D&D {}检定{reason_tag}】{mode_tag}d20=[{rolls_str}]→{} ({:+}{prof_tag}) = **{}** vs DC {} ｜ {}",
                self.stat.short_name(), self.kept_roll, self.stat_mod, self.total, self.target_dc, check_tier_label_for(self.tier, self.system)
            ),
            CheckSystem::D100 => {
                let d100_prof = match &self.prof_skill {
                    Some(sk) if self.prof_bonus > 0 => format!("（含熟练 {sk} +{}%）", self.prof_bonus),
                    _ => String::new(),
                };
                format!(
                    "🐙【d100 {}检定{reason_tag}】{mode_tag}[{rolls_str}]→**{}** / 目标值 {}%{d100_prof} ｜ {}",
                    self.stat.short_name(), self.kept_roll, self.target_dc, check_tier_label_for(self.tier, self.system)
                )
            }
            CheckSystem::TwoD6 => {
                let kept_tag = if self.mode == RollMode::Normal { String::new() } else { format!("→{}", self.kept_roll) };
                format!(
                    "🎲【{}检定{reason_tag}】{mode_tag}2d6{:+} = [{rolls_str}]{kept_tag}{prof_tag} → **{}** ｜ {}",
                    self.stat.short_name(), self.stat_mod, self.total, check_tier_label_for(self.tier, self.system)
                )
            }
        }
    }
}

pub fn perform_rule_check(
    system: CheckSystem, stat: StatKind, stats: &CharacterStats, mode: RollMode,
    dc: Option<i32>, prof_skill: Option<String>, reason: Option<String>,
) -> RuleCheckResult {
    let mut rng = rand::thread_rng();
    perform_rule_check_with_rng(system, stat, stats, mode, dc, prof_skill, reason, &mut rng)
}

#[allow(clippy::too_many_arguments)]
pub fn perform_rule_check_with_rng<R: Rng + ?Sized>(
    system: CheckSystem, stat: StatKind, stats: &CharacterStats, mode: RollMode,
    dc: Option<i32>, prof_skill: Option<String>, reason: Option<String>, rng: &mut R,
) -> RuleCheckResult {
    let stat_mod = stats.get(stat);
    let has_prof = prof_skill.is_some();
    match system {
        CheckSystem::D20 => {
            let target_dc = dc.unwrap_or(12).clamp(5, 25);
            let prof_bonus = if has_prof { 2 } else { 0 };
            let (rolls, kept) = match mode {
                RollMode::Normal => { let a = rng.gen_range(1..=20); (vec![a], a) }
                RollMode::Advantage => { let (a, b) = (rng.gen_range(1..=20), rng.gen_range(1..=20)); (vec![a, b], a.max(b)) }
                RollMode::Disadvantage => { let (a, b) = (rng.gen_range(1..=20), rng.gen_range(1..=20)); (vec![a, b], a.min(b)) }
            };
            let total = kept + stat_mod + prof_bonus;
            let tier = if kept == 20 { CheckTier::CritSuccess }
                else if kept == 1 { CheckTier::CritFailure }
                else if total >= target_dc { CheckTier::Success }
                else if total >= target_dc - 2 { CheckTier::Partial }
                else { CheckTier::Failure };
            RuleCheckResult { system, stat, mode, rolls, kept_roll: kept, stat_mod, prof_bonus, prof_skill, total, target_dc, tier, reason }
        }
        CheckSystem::D100 => {
            let prof_bonus = if has_prof { 15 } else { 0 };
            let target = (50 + stat_mod * 10 + prof_bonus).clamp(15, 90);
            let (rolls, kept) = match mode {
                RollMode::Normal => { let a = rng.gen_range(1..=100); (vec![a], a) }
                RollMode::Advantage => { let (a, b) = (rng.gen_range(1..=100), rng.gen_range(1..=100)); (vec![a, b], a.min(b)) }
                RollMode::Disadvantage => { let (a, b) = (rng.gen_range(1..=100), rng.gen_range(1..=100)); (vec![a, b], a.max(b)) }
            };
            let tier = if kept <= 5 { CheckTier::CritSuccess }
                else if kept >= 96 { CheckTier::CritFailure }
                else if kept <= target / 2 { CheckTier::Success }
                else if kept <= target { CheckTier::Partial }
                else { CheckTier::Failure };
            RuleCheckResult { system, stat, mode, rolls, kept_roll: kept, stat_mod, prof_bonus, prof_skill, total: kept, target_dc: target, tier, reason }
        }
        CheckSystem::TwoD6 => {
            let prof_bonus = if has_prof { 1 } else { 0 };
            let (rolls, kept) = match mode {
                RollMode::Normal => { let (a, b) = (rng.gen_range(1..=6), rng.gen_range(1..=6)); (vec![a, b], a + b) }
                RollMode::Advantage => { let mut v = [rng.gen_range(1..=6), rng.gen_range(1..=6), rng.gen_range(1..=6)]; v.sort_unstable(); (v.to_vec(), v[1] + v[2]) }
                RollMode::Disadvantage => { let mut v = [rng.gen_range(1..=6), rng.gen_range(1..=6), rng.gen_range(1..=6)]; v.sort_unstable(); (v.to_vec(), v[0] + v[1]) }
            };
            let total = kept + stat_mod + prof_bonus;
            let tier = if total >= 12 { CheckTier::CritSuccess } else { CheckTier::from_total(total) };
            RuleCheckResult { system, stat, mode, rolls, kept_roll: kept, stat_mod, prof_bonus, prof_skill, total, target_dc: 10, tier, reason }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestKind { Short, Long }
