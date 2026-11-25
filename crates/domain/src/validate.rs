use std::collections::HashSet;

use crate::models::Scenario;

pub fn normalize_theme(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fog" => "fog",
        "metro" => "metro",
        _ => "ember",
    }
}

/// Sanitizes an arbitrary string into a safe ASCII scenario ID (`[a-z0-9_-]`, 2..=48 chars),
/// preventing path traversal (`../`) or invalid filenames.
pub fn sanitize_scenario_id(raw: &str) -> String {
    let mut out = String::new();
    let mut prev_sep = false;
    for ch in raw.trim().chars() {
        let low = ch.to_ascii_lowercase();
        if low.is_ascii_lowercase() || low.is_ascii_digit() {
            out.push(low);
            prev_sep = false;
        } else if low == '-' && !out.is_empty() && !prev_sep {
            out.push('-');
            prev_sep = true;
        } else if (matches!(low, '_' | '/' | '\\' | '.') || low.is_whitespace())
            && !out.is_empty()
            && !prev_sep
        {
            out.push('_');
            prev_sep = true;
        }
        if out.len() >= 48 {
            break;
        }
    }
    let trimmed = out.trim_matches(|c| c == '_' || c == '-').to_string();
    if trimmed.len() >= 2 {
        trimmed
    } else {
        "custom_scenario".to_string()
    }
}

/// Strictly validates a parsed or AI-generated `Scenario` before saving or hot-reloading.
/// Rejects malformed IDs, missing core narrative fields, out-of-range stats, and duplicate IDs.
pub fn validate_scenario(sc: &Scenario) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();

    let id = sc.id.as_str();
    if id != id.trim()
        || !(2..=48).contains(&id.len())
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        || !id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    {
        errors.push(format!(
            "剧本 id `{id}` 无效：必须为 2~48 位小写字母/数字开头，仅含 [a-z0-9_-] 且不含空白"
        ));
    }
    if sc.name.trim().is_empty() {
        errors.push("剧本名称 `name` 不能为空".into());
    }
    if sc.tagline.trim().is_empty() {
        errors.push("剧本简介 `tagline` 不能为空".into());
    }
    if sc.genre.trim().is_empty() {
        errors.push("剧本题材 `genre` 不能为空".into());
    }
    if !matches!(
        sc.theme.trim().to_ascii_lowercase().as_str(),
        "" | "ember" | "fog" | "metro"
    ) {
        errors.push(format!(
            "剧本主题 `theme` 无效（{}），仅支持 ember / fog / metro",
            sc.theme
        ));
    }
    if sc.dm_style.trim().is_empty() {
        errors.push("主持风格 `dm_style` 不能为空".into());
    }
    if sc.initial_location.trim().is_empty() {
        errors.push("初始地点 `initial_location` 不能为空".into());
    }
    if sc.initial_time.trim().is_empty() {
        errors.push("初始时间 `initial_time` 不能为空".into());
    }
    if sc.opening_scene.trim().chars().count() < 15 {
        errors.push("开场场景 `opening_scene` 过短或为空（至少 15 字符）".into());
    }
    if !(1..=200).contains(&sc.player_defaults.max_hp) {
        errors.push("`player_defaults.max_hp` 必须在 1~200 之间".into());
    }
    if !(1..=200).contains(&sc.player_defaults.max_san) {
        errors.push("`player_defaults.max_san` 必须在 1~200 之间".into());
    }

    let mut role_names = HashSet::new();
    for (idx, r) in sc.preset_roles.iter().enumerate() {
        let name = r.name.trim();
        if name.is_empty() {
            errors.push(format!("第 {} 个预设职业名称为空", idx + 1));
        } else if !role_names.insert(name.to_string()) {
            errors.push(format!("预设职业名称重复：`{name}`"));
        }
        for v in [
            r.stats.body,
            r.stats.agility,
            r.stats.investigate,
            r.stats.social,
            r.stats.insight,
        ] {
            if !(-5..=5).contains(&v) {
                errors.push(format!("预设职业 `{name}` 五维属性 {v} 超出 -5..=5 范围"));
                break;
            }
        }
    }

    let mut wb_ids = HashSet::new();
    for (idx, wb) in sc.worldbook.iter().enumerate() {
        let wid = wb.id.trim();
        if wid.is_empty() {
            errors.push(format!("第 {} 条世界书缺少 id", idx + 1));
        } else if !wb_ids.insert(wid.to_string()) {
            errors.push(format!("世界书条目 id 重复：`{wid}`"));
        }
        if wb.title.trim().is_empty() || wb.content.trim().is_empty() {
            errors.push(format!("世界书条目 `{wid}` 缺少标题或正文"));
        }
    }

    let mut act_ids = HashSet::new();
    for (idx, act) in sc.acts.iter().enumerate() {
        let aid = act.id.trim();
        if aid.is_empty() || !act_ids.insert(aid.to_string()) {
            errors.push(format!("第 {} 幕 id 为空或重复：`{aid}`", idx + 1));
        }
        if act.title.trim().is_empty() || act.goal.trim().is_empty() {
            errors.push(format!("幕次 `{aid}` 缺少标题或目标"));
        }
    }

    let mut end_ids = HashSet::new();
    for (idx, end) in sc.endings.iter().enumerate() {
        let eid = end.id.trim();
        if eid.is_empty() || !end_ids.insert(eid.to_string()) {
            errors.push(format!("第 {} 个结局 id 为空或重复：`{eid}`", idx + 1));
        }
        if end.title.trim().is_empty() || end.summary.trim().is_empty() {
            errors.push(format!("结局 `{eid}` 缺少标题或说明"));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{StarterCharacterDefaults, WorldbookEntry};

    #[test]
    fn sanitize_id_blocks_path_traversal_and_validates_schema() {
        assert_eq!(sanitize_scenario_id("../../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize_scenario_id("Cyber Neon-2099!"), "cyber_neon-2099");
        assert_eq!(sanitize_scenario_id("中文脑洞"), "custom_scenario");

        let mut sc = Scenario {
            id: "../evil".into(),
            name: "测试本".into(),
            tagline: "简介".into(),
            genre: "悬疑".into(),
            theme: "metro".into(),
            dm_style: "克制".into(),
            initial_location: "车站".into(),
            initial_time: "23:59".into(),
            opening_scene: "这是一段足够长的开场场景描写，暴雨敲打着站台玻璃。".into(),
            opening_actions: vec!["查看站牌".into()],
            player_defaults: StarterCharacterDefaults {
                max_hp: 10,
                max_san: 10,
                starter_items: vec!["手电".into()],
            },
            preset_roles: vec![],
            acts: vec![],
            endings: vec![],
            worldbook: vec![WorldbookEntry {
                id: "wb1".into(),
                title: "规则".into(),
                keywords: vec![],
                always_active: true,
                priority: 100,
                content: "不要回头".into(),
            }],
        };
        assert!(validate_scenario(&sc).is_err());
        sc.id = "valid_demo_01".into();
        assert!(validate_scenario(&sc).is_ok());
    }
}
