use crate::dnd::{AbilityRecharge, CharacterAbility, CheckSystem, PlayerDndProfile, RestKind};
use crate::models::Player;
use crate::stats::{CharacterStats, CheckTier, StatKind};

/// Formats check tier labels per rule system (`2d6`, `d20`, `d100`).
pub fn check_tier_label_for(tier: CheckTier, sys: CheckSystem) -> &'static str {
    use CheckSystem::*;
    match (sys, tier) {
        (D20, CheckTier::CritSuccess) => "🌟 天然大成功（Nat 20！）",
        (D20, CheckTier::CritFailure) => "💥 天然大失败（Nat 1！）",
        (D20, CheckTier::Success) => "✨ 检定成功",
        (D20, CheckTier::Partial) => "⚠️ 擦边险过（附带代价）",
        (D20, CheckTier::Failure) => "❌ 检定失败",
        (D100, CheckTier::CritSuccess) => "🌟 极难大成功（≤5）",
        (D100, CheckTier::CritFailure) => "💥 致命大失败（≥96）",
        (D100, CheckTier::Success) => "✨ 困难成功",
        (D100, CheckTier::Partial) => "✅ 常规成功",
        (D100, CheckTier::Failure) => "❌ 检定失败",
        (TwoD6, CheckTier::CritSuccess) => "🌟 大成功（≥12！）",
        (TwoD6, CheckTier::Success) => "✨ 完全成功（≥10）",
        (TwoD6, CheckTier::Partial) => "⚠️ 代价成功（7~9）",
        (TwoD6, CheckTier::Failure | CheckTier::CritFailure) => "❌ 失败（≤6）",
    }
}

pub fn check_tier_hint_for(tier: CheckTier, sys: CheckSystem) -> &'static str {
    if sys == CheckSystem::D100 && tier == CheckTier::Partial {
        return "常规成功，顺利达成主要目标";
    }
    match tier {
        CheckTier::CritSuccess => "史诗级大成功！不仅完美达成目标，还附带高光额外收获或关键破局",
        CheckTier::Success => "痛快达成目标，可附带额外线索或有利局势",
        CheckTier::Partial => "达成主要目标，但必须伴随代价、消耗、风险或新麻烦",
        CheckTier::Failure => "行动受挫，局势恶化或触发真实风险/线索受阻",
        CheckTier::CritFailure => "戏剧性大失败！不仅彻底受挫，还立即引发意外危机或尴尬翻车",
    }
}

/// Validates fair point-buy stats for player-authored characters (`-1..=3` each, sum `<= 3`).
pub fn validate_point_buy(stats: &CharacterStats) -> Result<(), String> {
    let vals = [stats.body, stats.agility, stats.investigate, stats.social, stats.insight];
    if vals.iter().any(|&v| !(-1..=3).contains(&v)) {
        return Err("每项属性必须在 -1 到 +3 之间".into());
    }
    let sum: i32 = vals.iter().sum();
    if sum > 3 {
        return Err(format!(
            "五维属性总和为 +{sum}，公平购点上限为 +3（可分配负项换取高项，例如 2,1,1,0,-1）"
        ));
    }
    Ok(())
}

pub fn parse_point_buy_stats(raw: &str) -> Result<CharacterStats, String> {
    let parts: Vec<&str> = raw
        .split([',', '，', '/', ' '])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 5 {
        return Err("五维格式须为 5 个整数（体,敏,侦,交,灵），例如：`2,1,0,-1,1`".into());
    }
    let nums: Result<Vec<i32>, _> = parts.iter().map(|p| p.parse::<i32>()).collect();
    let n = nums.map_err(|_| "五维数值必须为整数（如 `2,1,0,-1,1`）".to_string())?;
    let stats = CharacterStats {
        body: n[0],
        agility: n[1],
        investigate: n[2],
        social: n[3],
        insight: n[4],
    };
    validate_point_buy(&stats)?;
    Ok(stats)
}

pub fn apply_rest_to_players(players: &mut [Player], kind: RestKind) -> Vec<String> {
    let mut notes = Vec::new();
    for p in players.iter_mut().filter(|p| !p.is_afk) {
        let mut items = Vec::new();
        match kind {
            RestKind::Short => {
                let heal = (p.max_hp / 3).max(2);
                let old = p.hp;
                p.hp = (p.hp + heal).clamp(0, p.max_hp);
                if p.hp > old {
                    items.push(format!("HP {}→{}", old, p.hp));
                }
                let mut recharged = 0u32;
                for ab in &mut p.dnd.abilities {
                    if ab.recharge == AbilityRecharge::ShortRest && ab.uses_left < ab.max_uses {
                        ab.uses_left = ab.max_uses;
                        recharged += 1;
                    }
                }
                if recharged > 0 {
                    items.push(format!("恢复 {recharged} 项短休技能"));
                }
            }
            RestKind::Long => {
                if p.hp < p.max_hp {
                    items.push(format!("HP {}→{}", p.hp, p.max_hp));
                    p.hp = p.max_hp;
                }
                if p.san < p.max_san {
                    items.push(format!("SAN {}→{}", p.san, p.max_san));
                    p.san = p.max_san;
                }
                let mut recharged = 0u32;
                for ab in &mut p.dnd.abilities {
                    if ab.uses_left < ab.max_uses {
                        ab.uses_left = ab.max_uses;
                        recharged += 1;
                    }
                }
                if recharged > 0 {
                    items.push(format!("恢复全部 {recharged} 项招牌技"));
                }
            }
        }
        if !items.is_empty() {
            notes.push(format!("**{}**：{}", p.character_name, items.join("，")));
        }
    }
    notes
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LevelUpChoice {
    HpSan,
    Stat(StatKind),
    Skill(String),
    AbilityCharge,
}

pub fn apply_level_up(player: &mut Player, choice: LevelUpChoice) -> Result<String, String> {
    if player.dnd.pending_levelups == 0 {
        return Err("你当前没有待分配的升级成长点（推进到下一幕时全队自动获得）".into());
    }
    let summary = match choice {
        LevelUpChoice::HpSan => {
            player.max_hp = (player.max_hp + 2).min(200);
            player.hp = (player.hp + 2).min(player.max_hp);
            player.max_san = (player.max_san + 2).min(200);
            player.san = (player.san + 2).min(player.max_san);
            format!(
                "❤️ HP/SAN 上限各 +2（现 HP {}/{}，SAN {}/{}）",
                player.hp, player.max_hp, player.san, player.max_san
            )
        }
        LevelUpChoice::Stat(st) => {
            let cur = player.stats.get(st);
            if cur >= 4 {
                return Err(format!(
                    "{}已达成长上限 (+4)，请改选其他属性或生命/技能成长",
                    st.short_name()
                ));
            }
            player.stats.add(st, 1);
            format!("📈 {} {:+} → {:+}", st.display_label(), cur, player.stats.get(st))
        }
        LevelUpChoice::Skill(raw) => {
            let sk: String = raw.trim().chars().take(12).collect();
            if sk.chars().count() < 2 {
                return Err("熟练技能名至少需要 2 个字（如：潜行、奥秘、游说）".into());
            }
            let stat_hits = ["体魄", "敏捷", "侦查", "交涉", "灵感"]
                .iter()
                .filter(|k| sk.contains(**k))
                .count();
            if stat_hits >= 2 {
                return Err("不能把多个属性名拼成一个熟练技能喵，请填具体技能名（如：潜行、开锁、游说）".into());
            }
            if player.dnd.proficient_skills.len() >= 4 {
                return Err("熟练技能已达上限（4 项）".into());
            }
            if player.dnd.proficient_skills.iter().any(|s| s == &sk) {
                return Err(format!("你已经熟练掌握「{sk}」了"));
            }
            player.dnd.proficient_skills.push(sk.clone());
            format!("🎓 习得新熟练技能「{sk}」（对应检定自动享熟练加值）")
        }
        LevelUpChoice::AbilityCharge => {
            let Some(ab) = player.dnd.abilities.first_mut() else {
                return Err("你当前还没有招牌技能，请改选生命或属性/熟练技能成长".into());
            };
            if ab.max_uses >= 6 {
                return Err(format!("招牌技「{}」充能已达上限 (6)", ab.name));
            }
            ab.max_uses += 1;
            ab.uses_left = ab.max_uses;
            format!(
                "✨ 招牌技「{}」充能上限 +1 并回满（现 {}/{}）",
                ab.name, ab.uses_left, ab.max_uses
            )
        }
    };
    player.dnd.pending_levelups -= 1;
    Ok(summary)
}

pub fn apply_inspiration_grants(
    players: &mut [Player],
    names: &[String],
    state_changes: &mut Vec<String>,
) {
    for raw in names.iter().take(4) {
        let target = raw.trim();
        if target.is_empty() {
            continue;
        }
        if let Some(p) = players.iter_mut().find(|p| {
            !p.is_afk
                && (p.character_name == target || p.character_name.eq_ignore_ascii_case(target))
        }) && !p.dnd.inspiration
        {
            p.dnd.inspiration = true;
            state_changes.push(format!("🌟 **{}** 因精彩扮演获得 1 颗【激励点】！", p.character_name));
        }
    }
}

pub fn award_act_milestone(players: &mut [Player], state_changes: &mut Vec<String>) {
    let mut leveled = Vec::new();
    for p in players.iter_mut().filter(|p| !p.is_afk) {
        p.dnd.level = (p.dnd.level + 1).min(20);
        p.dnd.pending_levelups = (p.dnd.pending_levelups + 1).min(5);
        leveled.push(format!("{}(Lv.{})", p.character_name, p.dnd.level));
    }
    if !leveled.is_empty() {
        state_changes.push(format!(
            "⬆️ **过幕成长**：{} 升级啦！发 `/levelup` 任选生命/属性/熟练技能强化",
            leveled.join("、")
        ));
    }
}

pub fn reset_dnd_profile_for_new_room(src: &PlayerDndProfile) -> PlayerDndProfile {
    // Cap archived starter budget: max 2 abilities (max_uses <= 2) & max 3 skills to stop cross-campaign smuggling
    let abilities: Vec<CharacterAbility> = src
        .abilities
        .iter()
        .take(2)
        .cloned()
        .map(|mut a| {
            a.max_uses = a.max_uses.clamp(1, 2);
            a.with_full_uses()
        })
        .collect();
    PlayerDndProfile {
        level: 1,
        pending_levelups: 0,
        inspiration: false,
        assisted_by: String::new(),
        alignment: src.alignment.clone(),
        proficient_skills: src.proficient_skills.iter().take(3).cloned().collect(),
        abilities,
        backstory: src.backstory.clone(),
        personal_secret: src.personal_secret.clone(),
    }
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;
    use crate::dnd::{RollMode, perform_rule_check_with_rng};

    #[test]
    fn d20_d100_and_point_buy_rules_are_consistent() {
        let stats = CharacterStats { body: 2, agility: 1, investigate: 0, social: 0, insight: 0 };
        assert!(validate_point_buy(&stats).is_ok());
        assert!(parse_point_buy_stats("2,1,1,0,-1").is_ok());
        assert!(parse_point_buy_stats("3,3,0,0,0").is_err());

        let mut rng = StdRng::seed_from_u64(19);
        let d20 = perform_rule_check_with_rng(
            CheckSystem::D20, StatKind::Body, &stats, RollMode::Disadvantage,
            Some(15), Some("格斗".into()), None, &mut rng,
        );
        assert_eq!(d20.kept_roll, d20.rolls[0].min(d20.rolls[1]));
        assert_eq!(d20.total, d20.kept_roll + 4);

        let d100 = perform_rule_check_with_rng(
            CheckSystem::D100, StatKind::Body, &stats, RollMode::Normal,
            None, Some("格斗".into()), None, &mut rng,
        );
        assert_eq!(d100.target_dc, 85);
    }
}
