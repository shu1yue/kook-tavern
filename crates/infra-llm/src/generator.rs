use std::collections::HashSet;

use anyhow::{Result, bail};
use tavern_core::AppConfig;
use tavern_domain::{
    AbilityRecharge, CharacterAbility, CheckSystem, ForgeScenarioRequest, Scenario,
    WorldbookEntry, WorkshopDraft, normalize_theme, sanitize_scenario_id, validate_scenario,
};

use crate::generator_helpers::{
    build_mock_forged_scenario, normalize_role_stats, sanitize_map_and_beats, short_fnv1a,
};
use crate::prompts::ChatMessage;
use crate::transport::OpenAiLlmProvider;

pub(crate) use crate::generator_helpers::build_mock_forged_scenario as mock_forge_scenario;

const FORGE_SYSTEM_PROMPT: &str = r#"你是专业的线上多人 TRPG / 群酒馆模组设计师（支持 D&D d20、2d6 三档与 CoC d100 体系）。
根据创作者提供的世界观底座、规则体系、目标幕数与主线脑洞，直接输出一个【完整合法的 JSON 对象】（严禁输出多余说明文字），严格匹配以下结构：
{
  "id": "全小写英文下划线ID，如 crimson_harbor_oath（4~36字符）",
  "name": "中文剧本名（4~16字）",
  "tagline": "一句话悬念卖点（15~40字）",
  "genre": "题材标签（如 古典奇幻 / D&D 城邦调查）",
  "theme": "ember / fog / metro 三选一",
  "dm_style": "给 AI DM 的主持文风与秘密纸条运用指导（80~160字）",
  "initial_location": "开局具体地点（须与首个 map_nodes.name 一致）",
  "initial_time": "开局时间氛围",
  "opening_scene": "画面感极强的开场白（140~260字，分3~4段，结尾留出清晰悬念钩子）",
  "opening_actions": ["6~14字开场建议动作1", "6~14字动作2", "6~14字动作3"],
  "player_defaults": {
    "max_hp": 14,
    "max_san": 12,
    "starter_items": ["通用随身物1", "通用随身物2", "通用随身物3"]
  },
  "preset_roles": [
    {
      "name": "职业名（严格4个差异化职业）",
      "traits": ["特质1", "特质2"],
      "bonus_items": ["专属装备1", "专属装备2"],
      "stats": {"body": 2, "agility": 0, "investigate": 0, "social": 1, "insight": 0},
      "alignment": "守序善良",
      "proficient_skills": ["格斗", "游说"],
      "abilities": [
        {"name": "招牌技名", "max_uses": 1, "recharge": "short_rest", "desc": "20~40字效果说明"}
      ]
    }
  ],
  "acts": [
    {"id": "act1", "title": "第一幕标题", "goal": "本幕清晰目标", "advance_flags": ["节点1"]}
  ],
  "endings": [
    {"id": "ending_true", "title": "真结局名", "kind": "true", "summary": "触发条件与结局基调"},
    {"id": "ending_good", "title": "佳结局名", "kind": "good", "summary": "触发条件与结局基调"},
    {"id": "ending_bad", "title": "坏结局名", "kind": "bad", "summary": "触发条件与结局基调"}
  ],
  "map_nodes": [
    {"id": "start_hub", "name": "起始地点名", "aliases": ["别名"], "x": 20, "y": 50, "kind": "hub"}
  ],
  "map_edges": [
    {"from": "start_hub", "to": "second_node", "label": "通道"}
  ],
  "worldbook": [
    {"id": "wb_plot_1", "title": "本局专属关键秘密/NPC真相（生成4~6条）", "keywords": ["关键词1", "关键词2"], "always_active": false, "priority": 92, "content": "具体真相、可给 private_whispers 的暗线、NPC态度转变条件"}
  ]
}
要求：
1. preset_roles 严格 4 个，五维单项在 -1..=3 且总和 ≤ 3，每个职业带 2 项熟练技能与 1~2 项充能招牌技（max_uses 仅 1 或 2）；
2. acts 严格等于用户要求的幕数（3、5 或 6 幕），层层递进；endings 为 3~4 个（含 true 与 bad）；
3. map_nodes 提供 5~8 个合理坐标（0~100）的地点并用 map_edges 连通，node id 仅含小写英文数字下划线；
4. 若用户已提供基础世界书，worldbook 只需补写本局专属的暗线秘密、关键 NPC 真相与Boss机制（不要重复照抄基础世界书）。"#;

pub fn build_forge_messages(req: &ForgeScenarioRequest) -> Vec<ChatMessage> {
    let d = &req.draft;
    let act_count = d.act_count.clamp(3, 6);
    let mut parts = vec![format!(
        "【创作需求】\n- 目标规则体系：{}\n- 目标篇幅：严格 **{} 幕**（{}）\n- 氛围主题：`{}`",
        d.check_system.short_badge(),
        act_count,
        d.length_label(),
        normalize_theme(&d.theme),
    )];
    if !d.title.trim().is_empty() {
        parts.push(format!("- 指定剧本名：《{}》", d.title.trim()));
    }
    parts.push(format!("- 核心主线脑洞/开场钩子：\n{}", d.premise.trim()));

    if let Some(pack) = &req.base_pack {
        let lore_brief = pack
            .entries
            .iter()
            .take(8)
            .map(|e| format!("  - {}：{}", e.title, e.content))
            .collect::<Vec<_>>()
            .join("\n");
        let facs = pack
            .factions
            .iter()
            .map(|f| format!("{}（{}）", f.name, f.goal))
            .collect::<Vec<_>>()
            .join("；");
        parts.push(format!(
            "【选用的开源世界书底座：《{}》】\n简介：{}\n势力：{}\n核心设定：\n{}",
            pack.name, pack.summary, facs, lore_brief
        ));
    }
    if !d.custom_lore.is_empty() {
        let custom = d
            .custom_lore
            .iter()
            .map(|e| format!("  - {}（关键词：{}）：{}", e.title, e.keywords.join("/"), e.content))
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("【创作者追加的私房设定（必须融入剧情）】\n{custom}"));
    }

    vec![
        ChatMessage {
            role: "system".into(),
            content: FORGE_SYSTEM_PROMPT.into(),
        },
        ChatMessage {
            role: "user".into(),
            content: parts.join("\n\n"),
        },
    ]
}

pub async fn generate_scenario_with_llm(
    cfg: &AppConfig,
    premise: &str,
    custom_id: Option<&str>,
    theme_opt: Option<&str>,
    mock_llm: bool,
) -> Result<Scenario> {
    let premise = premise.trim();
    if premise.is_empty() {
        bail!("请提供非空的剧本脑洞描述，例如：`深夜大学宿舍规则怪谈`");
    }
    let mut draft = WorkshopDraft::new("cli");
    draft.premise = premise.to_string();
    draft.act_count = 3;
    draft.check_system = CheckSystem::TwoD6;
    draft.theme = normalize_theme(theme_opt.unwrap_or("metro")).to_string();
    let req = ForgeScenarioRequest { draft, base_pack: None };

    let mut sc = if mock_llm {
        build_mock_forged_scenario(&req)
    } else {
        let provider = OpenAiLlmProvider::from_config(cfg)?.with_max_tokens(4096);
        let raw = provider.complete_json(build_forge_messages(&req)).await?;
        parse_scenario_json(&raw)?
    };
    if let Some(id) = custom_id {
        sc.id = sanitize_scenario_id(id);
    }
    finalize_forged_scenario(sc, &req, custom_id)
}

pub fn parse_scenario_json(raw: &str) -> Result<Scenario> {
    let trimmed = raw.trim();
    if let Ok(sc) = serde_json::from_str::<Scenario>(trimmed) {
        return Ok(sc);
    }
    if let Some(start) = trimmed.find('{')
        && let Some(end) = trimmed.rfind('}')
        && start < end
        && let Ok(sc) = serde_json::from_str::<Scenario>(&trimmed[start..=end])
    {
        return Ok(sc);
    }
    bail!("模型返回内容无法解析为合法 Scenario JSON")
}

pub fn finalize_forged_scenario(
    mut sc: Scenario,
    req: &ForgeScenarioRequest,
    forced_id: Option<&str>,
) -> Result<Scenario> {
    let d = &req.draft;
    if !d.title.trim().is_empty() {
        sc.name = d.title.trim().chars().take(24).collect();
    }
    sc.id = if let Some(cid) = forced_id {
        sanitize_scenario_id(cid)
    } else {
        let cleaned = sanitize_scenario_id(&sc.id);
        if cleaned == "custom_scenario" {
            format!("ws_{:x}", short_fnv1a(&format!("{}:{}", sc.name, d.premise)))
        } else {
            cleaned
        }
    };
    sc.theme = normalize_theme(&d.theme).to_string();
    sc.rules.check_system = d.check_system;
    sc.player_defaults.max_hp = sc.player_defaults.max_hp.clamp(8, 30);
    sc.player_defaults.max_san = sc.player_defaults.max_san.clamp(8, 30);

    sc.preset_roles.retain(|r| !r.name.trim().is_empty());
    sc.preset_roles.truncate(4);
    for role in &mut sc.preset_roles {
        normalize_role_stats(&mut role.stats);
        role.proficient_skills.truncate(3);
        role.abilities.retain(|a| !a.name.trim().is_empty());
        role.abilities.truncate(2);
        for ab in &mut role.abilities {
            ab.max_uses = ab.max_uses.clamp(1, 2);
            ab.uses_left = ab.max_uses;
        }
        if d.check_system == CheckSystem::D20 && role.abilities.is_empty() {
            role.abilities.push(CharacterAbility {
                name: "绝境爆发".into(),
                max_uses: 1,
                uses_left: 1,
                recharge: AbilityRecharge::ShortRest,
                desc: "在关键时刻集中意志扭转局势或打出高光一击".into(),
            });
        }
    }
    if sc.preset_roles.is_empty() {
        bail!("锻造结果缺少预设职业");
    }
    sc.acts.truncate(8);
    if sc.acts.is_empty() {
        bail!("锻造结果缺少幕次结构（acts 为空）");
    }
    sc.endings.truncate(6);
    if sc.endings.is_empty() {
        bail!("锻造结果缺少结局配置（endings 为空）");
    }

    let ai_lore = std::mem::take(&mut sc.worldbook);
    let mut merged = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut push_unique = |mut e: WorldbookEntry, prefix: &str| {
        if merged.len() >= 24 || e.title.trim().is_empty() || e.content.trim().is_empty() {
            return;
        }
        let mut eid = sanitize_scenario_id(&e.id);
        let mut seq = merged.len() + 1;
        while seen_ids.contains(&eid) {
            eid = format!("{prefix}_{seq}");
            seq += 1;
        }
        seen_ids.insert(eid.clone());
        e.id = eid;
        merged.push(e);
    };
    if let Some(pack) = &req.base_pack {
        for e in &pack.entries {
            push_unique(e.clone(), "wb_base");
        }
        if sc.rules.factions.is_empty() {
            sc.rules.factions = pack.factions.iter().take(6).cloned().collect();
        }
    }
    for e in &d.custom_lore {
        push_unique(e.clone(), "wb_user");
    }
    for e in ai_lore {
        push_unique(e, "wb_ai");
    }
    if !merged.iter().any(|e| e.always_active) {
        if let Some(first) = merged.first_mut() {
            first.always_active = true;
            first.priority = first.priority.max(100);
        } else {
            merged.push(WorldbookEntry {
                id: "wb_core_base".into(),
                title: format!("{} · 世界观底色", sc.name),
                keywords: vec![sc.name.clone()],
                always_active: true,
                player_visible: true,
                priority: 100,
                content: format!("围绕「{}」展开的冒险，危机正在步步逼近。", d.premise.trim()),
            });
        }
    }
    sc.worldbook = merged;

    sanitize_map_and_beats(&mut sc);
    if let Err(errs) = validate_scenario(&sc) {
        bail!("锻造的剧本未通过 Schema 校验：{}", errs.join("；"));
    }
    Ok(sc)
}
