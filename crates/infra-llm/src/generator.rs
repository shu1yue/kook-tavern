use anyhow::{Result, bail};
use tavern_core::AppConfig;
use tavern_domain::{
    CharacterStats, PresetRole, Scenario, ScenarioAct, ScenarioEnding, StarterCharacterDefaults,
    WorldbookEntry, normalize_theme, sanitize_scenario_id, validate_scenario,
};

use crate::prompts::ChatMessage;
use crate::transport::OpenAiLlmProvider;

const GEN_SYSTEM_PROMPT: &str = r#"你是专业的轻量文字冒险 / 群酒馆 TRPG 模组设计师。
根据用户给出的题材脑洞，直接输出一个【完整合法的 JSON 对象】（绝不输出任何多余说明文字），严格匹配以下结构：
{
  "id": "全小写英文下划线ID，如 neon_noodle_shop（2~32字符）",
  "name": "中文剧本名（4~14字）",
  "tagline": "一句话悬念卖点（15~36字）",
  "genre": "题材标签，如 赛博悬疑 / 轻量推理",
  "theme": "必须从 ember / fog / metro 中三选一（ember=暖金奇幻，fog=冷蓝悬疑，metro=霓虹都市怪谈）",
  "dm_style": "给 AI DM 的主持文风与秘密纸条运用指导（80~160字）",
  "initial_location": "开局具体地点",
  "initial_time": "开局时间氛围",
  "opening_scene": "极具画面感的开场白（140~260字，分3~4段，结尾留出明显钩子）",
  "opening_actions": ["6~14字开场建议动作1", "6~14字动作2", "6~14字动作3"],
  "player_defaults": {
    "max_hp": 10,
    "max_san": 10,
    "starter_items": ["通用随身物1", "通用随身物2", "通用随身物3"]
  },
  "preset_roles": [
    {
      "name": "职业名（4个差异化职业）",
      "traits": ["特质1", "特质2"],
      "bonus_items": ["专属装备1", "专属装备2"],
      "stats": {"body": 0, "agility": 1, "investigate": 2, "social": -1, "insight": 1}
    }
  ],
  "acts": [
    {"id": "act1", "title": "第一幕标题", "goal": "本幕清晰目标", "advance_flags": ["节点1"]},
    {"id": "act2", "title": "第二幕标题", "goal": "本幕清晰目标", "advance_flags": ["节点2"]},
    {"id": "act3", "title": "第三幕标题", "goal": "终幕决战目标", "advance_flags": ["节点3"]}
  ],
  "endings": [
    {"id": "ending_true", "title": "真结局名", "kind": "true", "summary": "触发条件与结局基调"},
    {"id": "ending_normal", "title": "普通结局名", "kind": "normal", "summary": "触发条件与结局基调"},
    {"id": "ending_bad", "title": "坏结局名", "kind": "bad", "summary": "触发条件与结局基调"}
  ],
  "worldbook": [
    {"id": "wb_1", "title": "常驻世界观底色", "keywords": ["关键词"], "always_active": true, "priority": 100, "content": "80~180字核心设定"},
    {"id": "wb_2", "title": "隐藏线索/NPC秘密（共6~8条）", "keywords": ["触发词1", "触发词2"], "always_active": false, "priority": 90, "content": "具体真相、可给 private_whispers 的暗线、NPC态度转变条件"}
  ]
}
要求：preset_roles 严格 4 个且五维单项在 -1..=2 之间、总和约 +3；acts 严格 3 幕；endings 严格 3 个；worldbook 至少 6 条且第一条 always_active=true。"#;

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

    let mut sc = if mock_llm {
        build_mock_generated_scenario(premise, custom_id, theme_opt)
    } else {
        let provider = OpenAiLlmProvider::from_config(cfg)?.with_max_tokens(3500);
        let user_prompt = format!(
            "请围绕以下脑洞创作一套完整可玩的 v2 多幕多结局剧本：\n「{premise}」\n{}",
            theme_opt
                .map(|t| format!("请将 theme 设为 `{}`。", normalize_theme(t)))
                .unwrap_or_default()
        );
        let raw = provider
            .complete_json(vec![
                ChatMessage {
                    role: "system".into(),
                    content: GEN_SYSTEM_PROMPT.into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user_prompt,
                },
            ])
            .await?;
        parse_scenario_json(&raw)?
    };

    if let Some(id) = custom_id {
        sc.id = sanitize_scenario_id(id);
    } else {
        sc.id = sanitize_scenario_id(&sc.id);
    }
    sc.theme = normalize_theme(theme_opt.unwrap_or(&sc.theme)).to_string();
    clamp_scenario_stats(&mut sc);

    if let Err(errs) = validate_scenario(&sc) {
        bail!("AI 生成的剧本未通过强类型 Schema 校验：{}", errs.join("；"));
    }
    Ok(sc)
}

fn parse_scenario_json(raw: &str) -> Result<Scenario> {
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

fn clamp_scenario_stats(sc: &mut Scenario) {
    sc.player_defaults.max_hp = sc.player_defaults.max_hp.clamp(6, 30);
    sc.player_defaults.max_san = sc.player_defaults.max_san.clamp(6, 30);
    for role in &mut sc.preset_roles {
        role.stats.body = role.stats.body.clamp(-2, 3);
        role.stats.agility = role.stats.agility.clamp(-2, 3);
        role.stats.investigate = role.stats.investigate.clamp(-2, 3);
        role.stats.social = role.stats.social.clamp(-2, 3);
        role.stats.insight = role.stats.insight.clamp(-2, 3);
    }
}

fn build_mock_generated_scenario(
    premise: &str,
    custom_id: Option<&str>,
    theme_opt: Option<&str>,
) -> Scenario {
    let id = custom_id
        .map(sanitize_scenario_id)
        .unwrap_or_else(|| "mock_generated_tale".to_string());
    let theme = normalize_theme(theme_opt.unwrap_or("metro")).to_string();
    let short_name: String = premise.chars().take(12).collect();
    Scenario {
        id,
        name: format!("《{short_name}》"),
        tagline: format!("围绕「{short_name}」展开的三幕悬疑轻冒险"),
        genre: "自定义悬疑 / AI 一键生本".into(),
        theme,
        dm_style: "节奏紧凑、克制有画面感，善用秘密纸条和 NPC 态度变化制造悬疑。".into(),
        initial_location: "故事起点 · 异常现场".into(),
        initial_time: "子夜 00:05".into(),
        opening_scene: format!(
            "夜色像潮水一样漫过四周，关于「{premise}」的传闻此刻正在眼前成真。\n昏黄灯光闪烁三下后，一扇原本紧锁的门缝里递出了一张写满潦草警告的便签。"
        ),
        opening_actions: vec![
            "拾起门缝里的警告便签".into(),
            "仔细检查四周异常痕迹".into(),
            "试探着向门后的人搭话".into(),
        ],
        player_defaults: StarterCharacterDefaults {
            max_hp: 10,
            max_san: 10,
            starter_items: vec!["随身手电".into(), "笔记本".into()],
        },
        preset_roles: vec![
            PresetRole {
                name: "冷静调查员".into(), traits: vec!["细心".into(), "善推理".into()],
                bonus_items: vec!["放大镜".into()],
                stats: CharacterStats { body: 0, agility: 1, investigate: 2, social: 0, insight: 0 },
                alignment: "守序中立".into(), proficient_skills: vec!["调查".into(), "察觉".into()], abilities: vec![],
            },
            PresetRole {
                name: "直觉通灵者".into(), traits: vec!["敏锐".into(), "懂禁忌".into()],
                bonus_items: vec!["护身铜铃".into()],
                stats: CharacterStats { body: -1, agility: 0, investigate: 1, social: 1, insight: 2 },
                alignment: "中立善良".into(), proficient_skills: vec!["奥秘".into(), "神秘".into()], abilities: vec![],
            },
            PresetRole {
                name: "街头交涉手".into(), traits: vec!["嘴甜".into(), "人脉广".into()],
                bonus_items: vec!["老旧通行证".into()],
                stats: CharacterStats { body: 0, agility: 1, investigate: 0, social: 2, insight: 0 },
                alignment: "混乱中立".into(), proficient_skills: vec!["游说".into(), "潜行".into()], abilities: vec![],
            },
            PresetRole {
                name: "可靠护卫".into(), traits: vec!["力大".into(), "抗压".into()],
                bonus_items: vec!["急救包".into()],
                stats: CharacterStats { body: 2, agility: 1, investigate: 0, social: 0, insight: 0 },
                alignment: "守序善良".into(), proficient_skills: vec!["格斗".into(), "医疗".into()], abilities: vec![],
            },
        ],
        acts: vec![
            ScenarioAct {
                id: "act1".into(), title: "异象初现".into(),
                goal: "查清便签来源并锁定核心线索".into(), advance_flags: vec!["发现便签真相".into()],
            },
            ScenarioAct {
                id: "act2".into(), title: "暗流深入".into(),
                goal: "揭露关键 NPC 隐情并集齐破局道具".into(), advance_flags: vec!["取得破局道具".into()],
            },
            ScenarioAct {
                id: "act3".into(), title: "终局抉择".into(),
                goal: "在危机全面失控前做出最终决定".into(), advance_flags: vec!["完成终局行动".into()],
            },
        ],
        endings: vec![
            ScenarioEnding {
                id: "ending_true".into(), title: "破雾天明".into(), kind: "true".into(),
                summary: "集齐真相并妥善收束异象，众人平安脱险。".into(),
            },
            ScenarioEnding {
                id: "ending_normal".into(), title: "惊险撤离".into(), kind: "normal".into(),
                summary: "虽未解开全部秘密，但及时脱身留下悬念。".into(),
            },
            ScenarioEnding {
                id: "ending_bad".into(), title: "夜色吞没".into(), kind: "bad".into(),
                summary: "触发禁忌或理智归零，迷失在异常现场。".into(),
            },
        ],
        worldbook: vec![
            WorldbookEntry {
                id: "wb_core".into(), title: "核心背景设定".into(), keywords: vec!["现场".into()],
                always_active: true, priority: 100,
                content: format!("一切围绕「{premise}」展开，背后藏着一段被刻意尘封的往事。"),
            },
            WorldbookEntry {
                id: "wb_note".into(), title: "门缝便签的隐藏暗码".into(),
                keywords: vec!["便签".into(), "警告".into(), "门".into()],
                always_active: false, priority: 90,
                content: "便签背面在强光照射下会浮现真正的安全暗号，可作为秘密纸条单独告知侦查成功者。".into(),
            },
        ],
        map_nodes: Vec::new(),
        map_edges: Vec::new(),
        rules: Default::default(),
    }
}
