use crate::stats::StatKind;

/// Matches a player's proficient skills against a check's raw stat token & reason text.
/// Guards against multi-stat-name abuse, single-character false positives, and cross-stat misfires.
pub fn match_proficiency(
    skills: &[String],
    raw_stat_token: &str,
    reason: &str,
    stat: StatKind,
) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let query = format!("{} {}", raw_stat_token.trim(), reason.trim())
        .trim()
        .to_ascii_lowercase();
    let has_reason = !reason.trim().is_empty()
        || (!raw_stat_token.trim().is_empty()
            && !matches!(
                raw_stat_token.trim().to_ascii_lowercase().as_str(),
                "body" | "agility" | "investigate" | "social" | "insight"
                    | "str" | "con" | "dex" | "inv" | "soc" | "cha" | "ins" | "wit"
                    | "体魄" | "敏捷" | "侦查" | "交涉" | "灵感"
            ));

    for sk in skills {
        let s = sk.trim();
        if s.chars().count() < 2 {
            continue;
        }
        let low = s.to_ascii_lowercase();
        if !query.is_empty() && query.contains(&low) {
            return Some(s.to_string());
        }
        if has_reason && skill_matches_stat_reason(s, &query, stat) {
            return Some(s.to_string());
        }
        if !has_reason && canonical_skill_matches_stat(s, stat) {
            return Some(s.to_string());
        }
    }
    None
}

fn canonical_skill_matches_stat(skill: &str, stat: StatKind) -> bool {
    match stat {
        StatKind::Body => matches!(skill, "格斗" | "搏斗" | "近战" | "医疗" | "急救"),
        StatKind::Agility => matches!(skill, "潜行" | "隐匿" | "开锁" | "机关" | "射击"),
        StatKind::Investigate => matches!(skill, "察觉" | "调查" | "观察" | "机关"),
        StatKind::Social => matches!(skill, "游说" | "说服" | "威吓" | "欺瞒"),
        StatKind::Insight => matches!(skill, "奥秘" | "神秘" | "魔法" | "通灵"),
    }
}

fn skill_matches_stat_reason(skill: &str, q: &str, stat: StatKind) -> bool {
    let any = |words: &[&str]| words.iter().any(|w| q.contains(w));
    match stat {
        StatKind::Body => {
            (skill.contains("格斗") || skill.contains("搏斗") || skill.contains("近战"))
                && any(&["挥砍", "劈砍", "猛劈", "肉搏", "格斗", "搏斗", "重击", "撞门", "破门", "近战"])
                || (skill.contains("医疗") || skill.contains("急救"))
                    && any(&["治疗", "包扎", "急救", "止血", "救治"])
        }
        StatKind::Agility => {
            (skill.contains("潜行") || skill.contains("隐匿"))
                && any(&["潜行", "隐匿", "偷袭", "潜入", "屏息", "悄悄", "藏身"])
                || (skill.contains("开锁") || skill.contains("机关"))
                    && any(&["开锁", "撬锁", "拆除", "机关", "陷阱", "暗锁"])
                || skill.contains("射击") && any(&["射击", "拉弓", "瞄准", "狙击", "箭矢"])
        }
        StatKind::Investigate => {
            (skill.contains("察觉") || skill.contains("调查") || skill.contains("观察"))
                && any(&["调查", "观察", "搜查", "搜寻", "翻找", "检查", "痕迹", "细查", "察觉", "监听", "环顾"])
                || (skill.contains("开锁") || skill.contains("机关"))
                    && any(&["机关", "暗格", "陷阱", "锁芯", "结构"])
                || skill.contains("奥秘") && any(&["符文", "法阵", "魔纹", "咒文", "典籍"])
        }
        StatKind::Social => {
            (skill.contains("游说") || skill.contains("说服") || skill.contains("威吓") || skill.contains("欺瞒"))
                && any(&["劝说", "说服", "游说", "套话", "交涉", "谈判", "威吓", "唬住", "欺骗", "安抚", "打听"])
        }
        StatKind::Insight => {
            (skill.contains("奥秘") || skill.contains("神秘") || skill.contains("魔法"))
                && any(&["奥秘", "神秘", "法术", "咒语", "符文", "仪式", "魔力", "感应", "通灵", "直觉"])
        }
    }
}
