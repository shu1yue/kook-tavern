use std::collections::HashSet;

use tavern_domain::{
    AbilityRecharge, CharacterAbility, CharacterStats, ClockKind, ForgeScenarioRequest, PresetRole,
    Scenario, ScenarioAct, ScenarioClock, ScenarioEnding, ScenarioMapEdge, ScenarioMapNode,
    StarterCharacterDefaults, WorldbookEntry, normalize_theme, sanitize_scenario_id,
};

/// Clamps raw stat fields directly into fair point-buy bounds (`-1..=3`, sum `<= 3`)
/// in strictly O(1) bounded steps, immune to `i32::MIN`/`<= -4` infinite loops.
pub(crate) fn normalize_role_stats(st: &mut CharacterStats) {
    st.body = st.body.clamp(-1, 3);
    st.agility = st.agility.clamp(-1, 3);
    st.investigate = st.investigate.clamp(-1, 3);
    st.social = st.social.clamp(-1, 3);
    st.insight = st.insight.clamp(-1, 3);
    for _ in 0..20 {
        let sum = st.body + st.agility + st.investigate + st.social + st.insight;
        if sum <= 3 {
            break;
        }
        let slots = [
            &mut st.body,
            &mut st.agility,
            &mut st.investigate,
            &mut st.social,
            &mut st.insight,
        ];
        if let Some(top) = slots.into_iter().max_by_key(|v| **v) {
            *top = (*top - 1).max(-1);
        }
    }
}

pub(crate) fn sanitize_map_and_beats(sc: &mut Scenario) {
    let mut valid_nodes = HashSet::new();
    let mut clean_nodes = Vec::new();
    for (i, mut n) in std::mem::take(&mut sc.map_nodes).into_iter().take(12).enumerate() {
        if n.name.trim().is_empty() {
            continue;
        }
        let base = sanitize_scenario_id(&n.id);
        let mut nid = if base.len() >= 2 && !valid_nodes.contains(&base) {
            base
        } else {
            format!("node_{}", i + 1)
        };
        let mut seq = i + 2;
        while valid_nodes.contains(&nid) {
            nid = format!("node_{seq}");
            seq += 1;
        }
        n.id = nid.clone();
        n.name = n.name.trim().chars().take(20).collect();
        n.x = n.x.clamp(5.0, 95.0);
        n.y = n.y.clamp(5.0, 95.0);
        if !matches!(n.kind.as_str(), "hub" | "mystery" | "danger" | "secret") {
            n.kind = "mystery".into();
        }
        valid_nodes.insert(nid);
        clean_nodes.push(n);
    }
    sc.map_nodes = clean_nodes;

    let mut edge_keys = HashSet::new();
    sc.map_edges.retain(|e| {
        if e.from == e.to || !valid_nodes.contains(&e.from) || !valid_nodes.contains(&e.to) {
            return false;
        }
        let pair = if e.from < e.to {
            (e.from.clone(), e.to.clone())
        } else {
            (e.to.clone(), e.from.clone())
        };
        edge_keys.insert(pair)
    });
    sc.map_edges.truncate(24);

    if !matches!(sc.rules.tension_pace.as_str(), "" | "steady" | "brisk" | "intense") {
        sc.rules.tension_pace = "steady".into();
    }
    if !matches!(sc.rules.lethality.as_str(), "" | "forgiving" | "balanced" | "harsh") {
        sc.rules.lethality = "balanced".into();
    }
    if !matches!(sc.rules.clue_generosity.as_str(), "" | "strict" | "balanced" | "generous") {
        sc.rules.clue_generosity = "balanced".into();
    }
    sc.rules.bulletin_interval = if sc.rules.bulletin_interval == 0 {
        0
    } else {
        sc.rules.bulletin_interval.clamp(5, 20)
    };
    sc.rules.factions.retain(|f| !f.name.trim().is_empty() && !f.goal.trim().is_empty());
    sc.rules.factions.truncate(8);

    // 1. Validate act.enter_nodes against valid map_nodes and auto-wire fallback progression nodes
    //    for Acts 2..=N so forged scenarios NEVER get permanently stuck on Act 1!
    for act in &mut sc.acts {
        act.enter_nodes.retain(|nid| valid_nodes.contains(nid));
    }
    let act_count = sc.acts.len();
    if act_count >= 2 && sc.map_nodes.len() >= 2 {
        let non_hub_ids: Vec<String> = sc
            .map_nodes
            .iter()
            .skip(1)
            .map(|n| n.id.clone())
            .collect();
        if !non_hub_ids.is_empty() {
            let later_acts = act_count - 1;
            for (offset, act) in sc.acts.iter_mut().skip(1).enumerate() {
                if !act.enter_nodes.is_empty() {
                    continue;
                }
                let start = (offset * non_hub_ids.len()) / later_acts;
                let end = (((offset + 1) * non_hub_ids.len()) / later_acts).max(start + 1);
                for nid in &non_hub_ids[start..end.min(non_hub_ids.len())] {
                    if !act.enter_nodes.contains(nid) {
                        act.enter_nodes.push(nid.clone());
                    }
                }
            }
        }
    }

    // 2. Ensure at least one always-active entry is player_visible for spoiler-safe story briefings,
    //    and attach the final map node's name to the top hidden climax entry so the Boss NEVER vanishes!
    if !sc.worldbook.iter().any(|w| w.player_visible)
        && let Some(first_always) = sc.worldbook.iter_mut().find(|w| w.always_active)
    {
        first_always.player_visible = true;
    }
    if let Some(last_node) = sc.map_nodes.last() {
        let final_loc = last_node.name.trim().to_string();
        if !final_loc.is_empty()
            && let Some(top_hidden) = sc
                .worldbook
                .iter_mut()
                .filter(|w| !w.always_active)
                .max_by_key(|w| w.priority)
            && !top_hidden.keywords.iter().any(|k| k.trim() == final_loc)
        {
            top_hidden.keywords.push(final_loc);
        }
    }

    // 3. Ensure `must_hit_beats` includes a final-act showdown beat so climax never drags forever
    let max_act = sc.acts.len().max(1) as u32;
    sc.rules.must_hit_beats.retain(|b| {
        b.act >= 1
            && b.act <= max_act
            && b.by_turn >= 1
            && !b.flag.trim().is_empty()
            && !b.prompt_hint.trim().is_empty()
    });
    if max_act >= 2 && !sc.rules.must_hit_beats.iter().any(|b| b.act == max_act) {
        let final_goal = sc
            .acts
            .last()
            .map(|a| a.goal.trim())
            .filter(|g| !g.is_empty())
            .unwrap_or("面对终幕核心人物并做出最终抉择");
        sc.rules.must_hit_beats.push(tavern_domain::DirectorBeat {
            act: max_act,
            by_turn: max_act.saturating_mul(4),
            flag: "终幕核心摊牌".into(),
            prompt_hint: format!(
                "终幕核心人物必须正面现身与玩家摊牌，用大白话说清真相并围绕「{final_goal}」推进最终结局收束。"
            ),
        });
    }
    sc.rules.must_hit_beats.truncate(12);

    // 4. Sanitize and guarantee Blades-in-the-Dark pacing clocks (at least 1 Threat + 1 Progress)
    let mut seen_clocks = HashSet::new();
    let mut clean_clocks = Vec::new();
    for (i, mut c) in std::mem::take(&mut sc.clocks).into_iter().take(4).enumerate() {
        if c.name.trim().is_empty() {
            continue;
        }
        let base = sanitize_scenario_id(&c.id);
        let mut cid = if base.len() >= 2 && base != "custom_scenario" && !seen_clocks.contains(&base) {
            base
        } else {
            format!("clock_{}", i + 1)
        };
        let mut seq = i + 2;
        while seen_clocks.contains(&cid) {
            cid = format!("clock_{seq}");
            seq += 1;
        }
        c.id = cid.clone();
        c.name = c.name.trim().chars().take(16).collect();
        c.segments = c.segments.clamp(3, 12);
        c.auto_tick_every = c.auto_tick_every.min(6);
        if c.on_full.trim().is_empty() {
            c.on_full = match c.kind {
                ClockKind::Threat => format!("《{}》的核心危机彻底失控并直接逼近眼前，立刻进入终局正面对决！", sc.name),
                ClockKind::Progress => format!("《{}》的关键真相与破局缺口彻底敞开，立刻推进最终结局收束！", sc.name),
            };
        }
        seen_clocks.insert(cid);
        clean_clocks.push(c);
    }
    let long_campaign = sc.acts.len() >= 5;
    if !clean_clocks.iter().any(|c| c.kind == ClockKind::Threat) {
        clean_clocks.insert(
            0,
            ScenarioClock {
                id: "crisis_countdown".into(),
                name: "幕后危机倒计时".into(),
                kind: ClockKind::Threat,
                segments: if long_campaign { 8 } else { 6 },
                auto_tick_every: if long_campaign { 3 } else { 2 },
                on_full: format!("《{}》的幕后危机彻底失控，反派与危险正面找上门来，必须当场做出终局抉择！", sc.name),
            },
        );
    }
    if !clean_clocks.iter().any(|c| c.kind == ClockKind::Progress) {
        clean_clocks.push(ScenarioClock {
            id: "truth_breakthrough".into(),
            name: "核心破局进度".into(),
            kind: ClockKind::Progress,
            segments: if long_campaign { 6 } else { 4 },
            auto_tick_every: 0,
            on_full: format!("《{}》的核心真相与破局条件全部就位，幕后黑手无处遁形，立即迎来终局摊牌！", sc.name),
        });
    }
    clean_clocks.truncate(4);
    sc.clocks = clean_clocks;
}

pub(crate) fn short_fnv1a(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

pub(crate) fn build_mock_forged_scenario(req: &ForgeScenarioRequest) -> Scenario {
    let d = &req.draft;
    let act_count = d.act_count.clamp(3, 6) as usize;
    let short_name: String = if !d.title.trim().is_empty() {
        d.title.trim().chars().take(16).collect()
    } else {
        d.premise.trim().chars().take(12).collect()
    };
    let slug = format!("ws_{:x}", short_fnv1a(&format!("{}:{}", short_name, d.premise)));
    let act_titles = ["异象初现", "暗流摸排", "阵营博弈", "禁区潜入", "真相揭晓", "终局决战"];
    let acts = (0..act_count)
        .map(|i| ScenarioAct {
            id: format!("act{}", i + 1),
            title: act_titles[i].into(),
            goal: format!("围绕「{short_name}」完成第 {} 幕关键突破", i + 1),
            advance_flags: vec![format!("第{}幕突破", i + 1)],
            enter_nodes: Vec::new(),
        })
        .collect();
    let genre = req
        .base_pack
        .as_ref()
        .map(|p| format!("{} / 工坊共创", p.genre))
        .unwrap_or_else(|| "工坊原创 / 多幕冒险".into());
    let mk_ab = |name: &str, desc: &str| CharacterAbility {
        name: name.into(), max_uses: 1, uses_left: 1, recharge: AbilityRecharge::ShortRest, desc: desc.into(),
    };
    Scenario {
        id: slug,
        name: short_name.clone(),
        tagline: format!("围绕「{}」锻造的 {} 幕 {} 冒险", short_name, act_count, d.check_system.short_badge()),
        genre,
        theme: normalize_theme(&d.theme).into(),
        dm_style: "节奏紧凑、画面感鲜明，善用秘密纸条、势力暗流与多规则检定推动沉浸扮演。".into(),
        initial_location: "起始据点".into(),
        initial_time: "序幕之夜".into(),
        opening_scene: format!(
            "夜色低垂，关于「{}」的危机正在你们眼前铺开。\n线索就藏在近在咫尺的暗流之中，而各方势力已经开始落子，等待你们踏出第一步。",
            d.premise.trim()
        ),
        opening_actions: vec![
            "异常现场：透着说不清的违和，可细查物证、拍照或寻找暗格".into(),
            "在场关键人物：神色紧绷且知晓内情，可套话、交易或施压".into(),
            "周边出入口与势力动向：暗流正在逼近，可先探路或分工戒备".into(),
        ],
        player_defaults: StarterCharacterDefaults {
            max_hp: 14, max_san: 12, starter_items: vec!["随身行囊".into(), "火折/手电".into()],
        },
        preset_roles: vec![
            PresetRole {
                name: "先锋守卫".into(), traits: vec!["沉稳".into(), "抗压".into()], bonus_items: vec!["精钢盾牌".into()],
                stats: CharacterStats { body: 2, agility: 1, investigate: 0, social: 0, insight: 0 },
                alignment: "守序善良".into(), proficient_skills: vec!["格斗".into(), "医疗".into()],
                abilities: vec![mk_ab("盾墙庇护", "为自己或队友硬抗一次致命打击")],
            },
            PresetRole {
                name: "暗影侦探".into(), traits: vec!["敏锐".into(), "巧手".into()], bonus_items: vec!["万能撬锁具".into()],
                stats: CharacterStats { body: 0, agility: 2, investigate: 2, social: 0, insight: -1 },
                alignment: "混乱善良".into(), proficient_skills: vec!["潜行".into(), "察觉".into()],
                abilities: vec![mk_ab("无声潜伏", "隐入阴影并精准找出隐藏暗格")],
            },
            PresetRole {
                name: "秘法学者".into(), traits: vec!["博学".into(), "直觉强".into()], bonus_items: vec!["符文手札".into()],
                stats: CharacterStats { body: -1, agility: 0, investigate: 1, social: 1, insight: 2 },
                alignment: "中立善良".into(), proficient_skills: vec!["奥秘".into(), "调查".into()],
                abilities: vec![mk_ab("灵视洞悉", "看穿法阵/异常现象背后的真实弱点")],
            },
            PresetRole {
                name: "纵横游侠".into(), traits: vec!["健谈".into(), "人脉广".into()], bonus_items: vec!["加密通行证".into()],
                stats: CharacterStats { body: 0, agility: 1, investigate: 0, social: 2, insight: 0 },
                alignment: "混乱中立".into(), proficient_skills: vec!["游说".into(), "开锁".into()],
                abilities: vec![mk_ab("巧舌破局", "用惊人话术瞬间扭转 NPC 的戒心")],
            },
        ],
        acts,
        endings: vec![
            ScenarioEnding { id: "ending_true".into(), title: "破雾天明".into(), kind: "true".into(), summary: "揭开全部真相并完美化解危机。".into() },
            ScenarioEnding { id: "ending_good".into(), title: "险中求存".into(), kind: "good".into(), summary: "稳住局势带着核心收获凯旋。".into() },
            ScenarioEnding { id: "ending_bad".into(), title: "暗流吞没".into(), kind: "bad".into(), summary: "关键抉择失误或全员力竭导致局势失控。".into() },
        ],
        worldbook: vec![
            WorldbookEntry {
                id: "wb_plot_core".into(), title: "故事核心背景".into(),
                keywords: vec![short_name.clone()], always_active: req.base_pack.is_none(), player_visible: true, priority: 99,
                content: format!("本局围绕「{}」展开，危机层层升级。", d.premise.trim()),
            },
            WorldbookEntry {
                id: "wb_plot_secret".into(), title: "主线隐藏真相".into(),
                keywords: vec!["真相".into(), "秘密".into(), "线索".into()],
                always_active: false, player_visible: false, priority: 92,
                content: format!("幕后主使正借「{}」掩盖真正的图谋，侦查成功者可获秘密纸条。", short_name),
            },
        ],
        map_nodes: vec![
            ScenarioMapNode { id: "hub_start".into(), name: "起始据点".into(), aliases: vec!["起点".into()], x: 20.0, y: 50.0, kind: "hub".into() },
            ScenarioMapNode { id: "clue_hall".into(), name: "线索回廊".into(), aliases: vec!["回廊".into()], x: 50.0, y: 35.0, kind: "mystery".into() },
            ScenarioMapNode { id: "core_vault".into(), name: "终局核心区".into(), aliases: vec!["核心".into()], x: 82.0, y: 52.0, kind: "danger".into() },
        ],
        map_edges: vec![
            ScenarioMapEdge { from: "hub_start".into(), to: "clue_hall".into(), label: "主路".into(), locked: false },
            ScenarioMapEdge { from: "clue_hall".into(), to: "core_vault".into(), label: "暗道".into(), locked: false },
        ],
        rules: Default::default(),
        clocks: Vec::new(),
        player_brief: Default::default(),
    }
}
