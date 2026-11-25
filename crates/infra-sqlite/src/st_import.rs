use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tavern_domain::{
    CharacterStats, PresetRole, Scenario, ScenarioAct, ScenarioEnding, StarterCharacterDefaults,
    WorldbookEntry, normalize_theme, sanitize_scenario_id, validate_scenario,
};

use crate::st_png::{MAX_IMPORT_BYTES, PNG_SIGNATURE, extract_png_chara_json};

/// Imports a SillyTavern character card PNG (`chara`/`ccv3` tEXt/iTXt chunk),
/// Character Card V1/V2/V3 JSON, or standalone World Info / Lorebook JSON into a validated `Scenario`.
pub fn import_sillytavern_file(
    input_path: &Path,
    custom_id: Option<&str>,
    theme_opt: Option<&str>,
) -> Result<Scenario> {
    let meta = fs::metadata(input_path)
        .with_context(|| format!("无法访问导入文件: {}", input_path.display()))?;
    if !meta.is_file() {
        bail!("导入路径必须是普通文件（拒绝目录/FIFO/设备文件）");
    }
    if meta.len() > MAX_IMPORT_BYTES {
        bail!("导入文件超过 16 MiB 安全上限，已拒绝加载");
    }

    let mut bytes = Vec::new();
    File::open(input_path)
        .with_context(|| format!("打开导入文件失败: {}", input_path.display()))?
        .take(MAX_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("读取导入文件失败: {}", input_path.display()))?;
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        bail!("导入文件实际读取超过 16 MiB 安全上限，已拒绝加载");
    }

    let stem = input_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("st_imported");
    parse_sillytavern_bytes(&bytes, stem, custom_id, theme_opt)
}

/// Parses raw SillyTavern `.png` or `.json` bytes into a validated `Scenario` without disk I/O.
pub fn parse_sillytavern_bytes(
    bytes: &[u8],
    fallback_stem: &str,
    custom_id: Option<&str>,
    theme_opt: Option<&str>,
) -> Result<Scenario> {
    if bytes.len() as u64 > MAX_IMPORT_BYTES {
        bail!("导入数据超过 16 MiB 安全上限，已拒绝加载");
    }
    let json_text = if bytes.starts_with(&PNG_SIGNATURE) {
        extract_png_chara_json(bytes)?
    } else {
        std::str::from_utf8(bytes)
            .context("JSON 文件不是有效的 UTF-8 文本")?
            .to_string()
    };
    let root: Value = serde_json::from_str(&json_text).context("解析 SillyTavern JSON 失败")?;
    convert_st_json(&root, fallback_stem, custom_id, theme_opt)
}

fn convert_st_json(
    root: &Value,
    fallback_stem: &str,
    custom_id: Option<&str>,
    theme_opt: Option<&str>,
) -> Result<Scenario> {
    let card = root.get("data").filter(|v| v.is_object()).unwrap_or(root);
    let is_lorebook_only = card.get("entries").is_some() && card.get("name").is_none();

    let name = str_field(card, "name")
        .or_else(|| str_field(root, "name"))
        .unwrap_or_else(|| format!("导入剧本 · {fallback_stem}"));
    let id = custom_id
        .map(sanitize_scenario_id)
        .unwrap_or_else(|| sanitize_scenario_id(fallback_stem));
    let theme = normalize_theme(theme_opt.unwrap_or("ember")).to_string();

    let mut worldbook = Vec::new();
    let desc = str_field(card, "description").unwrap_or_default();
    let personality = str_field(card, "personality").unwrap_or_default();
    let scenario_text = str_field(card, "scenario").unwrap_or_default();
    let sys_prompt = str_field(card, "system_prompt").unwrap_or_default();

    let mut base_lore = Vec::new();
    if !desc.is_empty() {
        base_lore.push(format!("【核心设定与人物】\n{desc}"));
    }
    if !personality.is_empty() {
        base_lore.push(format!("【性格与行事逻辑】\n{personality}"));
    }
    if !scenario_text.is_empty() {
        base_lore.push(format!("【当前场景背景】\n{scenario_text}"));
    }
    if !base_lore.is_empty() {
        worldbook.push(WorldbookEntry {
            id: "wb_st_core".into(),
            title: format!("{name} · 核心设定"),
            keywords: vec![name.clone()],
            always_active: true,
            priority: 100,
            content: base_lore.join("\n\n"),
        });
    }

    let entries_val = card
        .get("character_book")
        .and_then(|b| b.get("entries"))
        .or_else(|| card.get("entries"));
    if let Some(ev) = entries_val {
        collect_lore_entries(ev, &mut worldbook);
    }
    if worldbook.is_empty() {
        worldbook.push(WorldbookEntry {
            id: "wb_st_default".into(),
            title: format!("{name} · 背景底色"),
            keywords: vec![],
            always_active: true,
            priority: 100,
            content: format!("由 SillyTavern 导入的冒险剧本《{name}》。"),
        });
    }

    let first_mes = str_field(card, "first_mes").unwrap_or_default();
    let opening_scene = if first_mes.chars().count() >= 15 {
        first_mes
    } else if scenario_text.chars().count() >= 15 {
        scenario_text.clone()
    } else {
        format!("夜幕低垂，你们踏入了《{name}》的世界。空气里弥漫着未知的紧张感，眼前的景象正等待着你们的第一步试探……")
    };

    let dm_style = if !sys_prompt.is_empty() {
        sys_prompt
    } else {
        format!("担任《{name}》的群酒馆文字冒险 DM，紧扣导入的角色与世界书设定，克制自然地推进互动与秘密线索。")
    };

    let sc = Scenario {
        id,
        name: name.clone(),
        tagline: if is_lorebook_only {
            format!("由 SillyTavern 世界书转换的多人冒险模组（含 {} 条设定）", worldbook.len())
        } else {
            format!("由 SillyTavern 角色卡导入的互动剧本（含 {} 条世界书）", worldbook.len())
        },
        genre: "SillyTavern 导入 / 自由探索冒险".into(),
        theme,
        dm_style,
        initial_location: format!("{name} · 起始场景"),
        initial_time: "序幕开启".into(),
        opening_scene,
        opening_actions: vec![
            "仔细观察四周环境细节".into(),
            "主动向核心人物试探搭话".into(),
            "翻找附近可疑的线索物件".into(),
        ],
        player_defaults: StarterCharacterDefaults {
            max_hp: 10,
            max_san: 10,
            starter_items: vec!["随身行囊".into(), "记事本".into()],
        },
        preset_roles: default_imported_roles(),
        acts: vec![
            ScenarioAct {
                id: "act1".into(), title: "初入异境".into(),
                goal: "结识关键人物，摸清当前局势与隐藏规则".into(), advance_flags: vec![],
            },
            ScenarioAct {
                id: "act2".into(), title: "深入迷局".into(),
                goal: "追查世界书中的核心秘密并建立同盟".into(), advance_flags: vec![],
            },
            ScenarioAct {
                id: "act3".into(), title: "命运收束".into(),
                goal: "直面核心冲突并迎来最终结局".into(), advance_flags: vec![],
            },
        ],
        endings: vec![
            ScenarioEnding {
                id: "ending_true".into(), title: "拨云见日".into(), kind: "true".into(),
                summary: "揭开核心真相并圆满化解危机。".into(),
            },
            ScenarioEnding {
                id: "ending_normal".into(), title: "余波未平".into(), kind: "normal".into(),
                summary: "平安度过风波，但仍有隐秘留在暗处。".into(),
            },
            ScenarioEnding {
                id: "ending_bad".into(), title: "陷入深渊".into(), kind: "bad".into(),
                summary: "局势失控或全员力竭，故事在阴霾中落幕。".into(),
            },
        ],
        worldbook,
        map_nodes: Vec::new(),
        map_edges: Vec::new(),
        rules: Default::default(),
    };

    if let Err(errs) = validate_scenario(&sc) {
        bail!("导入转换后的剧本校验失败：{}", errs.join("；"));
    }
    Ok(sc)
}

fn collect_lore_entries(entries: &Value, out: &mut Vec<WorldbookEntry>) {
    let list: Vec<&Value> = match entries {
        Value::Array(arr) => arr.iter().collect(),
        Value::Object(map) => map.values().collect(),
        _ => return,
    };
    for (idx, item) in list.into_iter().enumerate().take(40) {
        if item.get("enabled").and_then(Value::as_bool) == Some(false)
            || item.get("disable").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let content = str_field(item, "content").unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let title = str_field(item, "comment")
            .or_else(|| str_field(item, "name"))
            .unwrap_or_else(|| format!("世界书条目 #{}", idx + 1));
        let keywords: Vec<String> = item
            .get("keys")
            .or_else(|| item.get("key"))
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let always_active = item.get("constant").and_then(Value::as_bool).unwrap_or(false);
        let priority = item
            .get("insertion_order")
            .or_else(|| item.get("order"))
            .and_then(Value::as_i64)
            .unwrap_or(50)
            .clamp(1, 999) as i32;
        out.push(WorldbookEntry {
            id: format!("wb_st_{}", idx + 1),
            title,
            keywords,
            always_active,
            priority,
            content,
        });
    }
}

fn str_field(obj: &Value, key: &str) -> Option<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(4000).collect())
}

fn default_imported_roles() -> Vec<PresetRole> {
    vec![
        PresetRole {
            name: "敏锐调查员".into(), traits: vec!["观察力强".into()], bonus_items: vec!["袖珍放大镜".into()],
            stats: CharacterStats { body: 0, agility: 1, investigate: 2, social: 0, insight: 0 },
            alignment: "守序中立".into(), proficient_skills: vec!["调查".into(), "察觉".into()], abilities: vec![],
        },
        PresetRole {
            name: "圆滑交涉者".into(), traits: vec!["善解人意".into()], bonus_items: vec!["精致名片夹".into()],
            stats: CharacterStats { body: -1, agility: 0, investigate: 1, social: 2, insight: 1 },
            alignment: "混乱中立".into(), proficient_skills: vec!["游说".into()], abilities: vec![],
        },
        PresetRole {
            name: "神秘学者".into(), traits: vec!["直觉敏锐".into()], bonus_items: vec!["旧笔记残页".into()],
            stats: CharacterStats { body: -1, agility: 0, investigate: 1, social: 1, insight: 2 },
            alignment: "中立善良".into(), proficient_skills: vec!["奥秘".into(), "神秘".into()], abilities: vec![],
        },
        PresetRole {
            name: "前排护卫".into(), traits: vec!["临危不乱".into()], bonus_items: vec!["便携急救包".into()],
            stats: CharacterStats { body: 2, agility: 1, investigate: 0, social: 0, insight: 0 },
            alignment: "守序善良".into(), proficient_skills: vec!["格斗".into(), "医疗".into()], abilities: vec![],
        },
    ]
}
