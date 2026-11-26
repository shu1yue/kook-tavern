use tavern_domain::{
    DmStructuredReply, LoreCandidate, PendingAction, Player, RoomState, Scenario, TurnRecord,
    WorldEcho, WorldbookEntry, analyze_spotlight, due_director_beats, format_batched_actions,
    match_dynamic_worldbook, should_trigger_bulletin,
};

pub fn build_lore_corpus(
    room: &RoomState,
    recent_turns: &[TurnRecord],
    actions: &[PendingAction],
) -> String {
    let batched_input = format_batched_actions(actions);
    let mut corpus = format!("{}\n{}", room.location, batched_input);
    for t in recent_turns.iter().rev().take(2) {
        corpus.push('\n');
        corpus.push_str(&t.batched_input);
        corpus.push('\n');
        corpus.push_str(&t.narrative);
    }
    corpus
}

/// Collects non-always-active worldbook entries NOT already hit by deterministic keyword matching
/// so System One semantic recall can complement keywords without duplicating hits.
pub fn collect_unmatched_lore_candidates(scenario: &Scenario, corpus: &str) -> Vec<LoreCandidate> {
    let kw_hits = match_dynamic_worldbook(scenario, corpus, 5);
    let mut unmatched: Vec<_> = scenario
        .worldbook
        .iter()
        .filter(|e| !e.always_active && !kw_hits.iter().any(|h| h.id == e.id))
        .collect();
    unmatched.sort_by_key(|a| std::cmp::Reverse(a.priority));
    unmatched
        .into_iter()
        .take(8)
        .map(|e| LoreCandidate {
            id: e.id.clone(),
            title: e.title.clone(),
            snippet: e.content.trim().chars().take(120).collect(),
        })
        .collect()
}

/// Unions deterministic keyword worldbook hits (100% recall floor) with extra semantic IDs
/// selected by System One, deduplicated and capped at `max_total`.
pub fn merge_dynamic_worldbook<'a>(
    scenario: &'a Scenario,
    corpus: &str,
    extra_ids: &[String],
    max_total: usize,
) -> Vec<&'a WorldbookEntry> {
    let mut out = match_dynamic_worldbook(scenario, corpus, 5);
    for id in extra_ids {
        if out.len() >= max_total {
            break;
        }
        if out.iter().any(|e| e.id == *id) {
            continue;
        }
        if let Some(entry) = scenario
            .worldbook
            .iter()
            .find(|e| !e.always_active && e.id == *id)
        {
            out.push(entry);
        }
    }
    out
}

/// Universal final-act climax safeguard for ALL scenarios (built-in, `/workshop`-forged, and
/// SillyTavern-imported): when the party enters the final act, automatically ensures the top
/// climax/boss/ending worldbook entries are injected even if the current turn didn't utter their
/// exact keywords (so a climax Boss like 白鸦 can NEVER vanish for 36 turns!).
pub fn inject_climax_worldbook<'a>(
    scenario: &'a Scenario,
    room: &RoomState,
    out: &mut Vec<&'a WorldbookEntry>,
    max_total: usize,
) {
    let total_acts = scenario.acts.len() as u32;
    if total_acts == 0 || room.current_act < total_acts || out.len() >= max_total {
        return;
    }
    let mut climax_text = String::new();
    if let Some(last_act) = scenario.acts.last() {
        climax_text.push_str(&last_act.title);
        climax_text.push(' ');
        climax_text.push_str(&last_act.goal);
        climax_text.push(' ');
        climax_text.push_str(&last_act.advance_flags.join(" "));
    }
    for end in &scenario.endings {
        climax_text.push(' ');
        climax_text.push_str(&end.title);
        climax_text.push(' ');
        climax_text.push_str(&end.summary);
    }

    let mut candidates: Vec<(usize, i32, &'a WorldbookEntry)> = scenario
        .worldbook
        .iter()
        .filter(|e| !e.always_active && !out.iter().any(|cur| cur.id == e.id))
        .map(|e| {
            let kw_hits = e
                .keywords
                .iter()
                .filter(|k| !k.trim().is_empty() && climax_text.contains(k.trim()))
                .count();
            let title_bonus = if e.title.contains("终局")
                || e.title.contains("结局")
                || e.title.contains("核心")
                || e.title.contains("真相")
                || e.title.contains("Boss")
                || e.title.contains("主使")
            {
                2
            } else {
                0
            };
            (kw_hits + title_bonus, e.priority, e)
        })
        .filter(|(score, prio, _)| *score > 0 || *prio >= 85)
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));

    for (_, _, entry) in candidates.into_iter().take(3) {
        if out.len() >= max_total {
            break;
        }
        out.push(entry);
    }
}

pub fn summarize_whispers_for_audit(reply: &DmStructuredReply) -> String {
    reply
        .private_whispers
        .iter()
        .filter(|w| !w.secret.trim().is_empty())
        .map(|w| format!("给{}：{}", w.player_name.trim(), w.secret.trim()))
        .collect::<Vec<_>>()
        .join("；")
}

/// Formats scenario-static rules & map reference for the static `system` message
/// (100% cache-stable per scenario for DeepSeek Prefix Caching).
pub fn format_static_rules_and_map(scenario: &Scenario) -> String {
    let mut parts = Vec::new();
    if scenario.rules.has_custom_guidance() {
        parts.push(format!(
            "【剧本外置规则】\n- {}",
            scenario.rules.format_prompt_summary()
        ));
    }
    if !scenario.map_nodes.is_empty() {
        let names: Vec<&str> = scenario.map_nodes.iter().map(|n| n.name.as_str()).collect();
        parts.push(format!(
            "【标准场景地图节点（移动时优先沿用以下规范地名，方便观战地图精准定位）】\n- {}",
            names.join(" / ")
        ));
    }
    parts.join("\n\n")
}

/// Formats dynamic turn-level director directives appended to the final `user` message:
/// spotlight balance, due director outline beats, scheduled faction bulletin window,
/// and guild reincarnation echoes.
pub fn format_dynamic_director_sections(
    scenario: &Scenario,
    room: &RoomState,
    players: &[Player],
    recent_turns: &[TurnRecord],
    actions: &[PendingAction],
    world_echoes: &[WorldEcho],
) -> String {
    let next_turn = room.turn_number + 1;
    let mut sections = Vec::new();

    // 0. Universal solo-play clarity guard (strictly `players.len() <= 1` so an AFK teammate is NEVER leaked to!)
    if players.len() <= 1 {
        sections.push(
            "【单人独行规则】本桌目前仅 1 名玩家，不存在队友信息差：**`private_whispers` 必须输出空数组 `[]`**，所有发现直接清清楚楚写进公开 `narrative`，切勿另开暗线谜题。".to_string(),
        );
    }

    let total_acts = scenario.acts.len() as u32;
    if total_acts > 0 {
        let cur_act = room.current_act.clamp(1, total_acts);
        if cur_act >= total_acts {
            let hard_cap = total_acts.saturating_mul(5).saturating_add(3);
            if next_turn >= hard_cap || room.world_flags.len() >= (total_acts as usize) * 4 {
                sections.push(format!(
                    "【终幕强收束指令（第 {next_turn} 回合）】已进入最终幕（第 {cur_act}/{total_acts} 幕）且本局篇幅已足够长！**严禁再引入任何新敌人、新路人 NPC、新暗道或新机关谜题**；请立刻让终幕核心人物正面现身、用大白话把前因后果彻底摊牌，一旦玩家做出决定性选择（停机/交易/破坏/揭穿/撤离等），务必在 1~2 轮内填写 `trigger_ending_id` 正式结团！"
                ));
            } else {
                sections.push(format!(
                    "【终幕导演提示】当前已进入最终幕（第 {cur_act}/{total_acts} 幕），请尽快让终幕核心人物正面现身与玩家摊牌，把主线真相和选项说清楚，严禁再套娃编造剧本外的新支线或新路人。"
                ));
            }
        } else if next_turn >= cur_act.saturating_mul(7).saturating_add(2) {
            sections.push(format!(
                "【防卡关节奏提醒】当前已在第 {cur_act}/{total_acts} 幕停留较多回合，请在本轮直接给出清晰直白的突破口推进主线；一旦小队达成当前幕目标或进入下一阶段地点，请立刻将 `advance_act` 设为 `true`。"
            ));
        }
    }

    // 1. Spotlight fairness tracker (prevents quieter active players from being sidelined)
    let spotlight = analyze_spotlight(players, recent_turns, actions);
    if let Some(dir) = spotlight.prompt_directive {
        sections.push(dir);
    }

    // 2. Overdue director outline beats (`civ-atlas` outline-guided pacing)
    let beats = due_director_beats(scenario, room, next_turn);
    if !beats.is_empty() {
        let lines = beats
            .iter()
            .map(|b| format!("- 必达节点「{}」：{}", b.flag, b.prompt_hint))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "【导演大纲必达提醒（本幕节奏已到关键节点，请在本轮自然切入以下事件，达成后写入 add_flags）】\n{lines}"
        ));
    }

    // 3. Scheduled undercurrent faction bulletin window
    if should_trigger_bulletin(scenario, room, next_turn) {
        let masthead = scenario.rules.effective_masthead(&scenario.name);
        let factions = scenario
            .rules
            .factions
            .iter()
            .map(|f| {
                if f.escalation.trim().is_empty() {
                    format!("- {}（目标：{}）", f.name, f.goal)
                } else {
                    format!("- {}（目标：{}｜升级手段：{}）", f.name, f.goal, f.escalation)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let prev_heads: Vec<&str> = room
            .bulletins
            .iter()
            .rev()
            .take(3)
            .map(|b| b.headline.as_str())
            .collect();
        let prev_note = if prev_heads.is_empty() {
            String::new()
        } else {
            format!("\n- 此前已发号外（请勿重复）：{}", prev_heads.join("；"))
        };
        sections.push(format!(
            "【幕间暗流号外窗口已开启 — 请在本轮 JSON 中填写 `world_bulletin`】\n- 剧中媒介报头：{masthead}\n- 幕后活跃势力议程：\n{factions}{prev_note}\n- 要求：用大白话挑选 1 个幕后势力推进一次直接相关的暗箱行动，写出简短清楚的 `headline`(8~18字)、`body`(40~80字) 与 `faction`；**严禁凭空编造剧本之外的新路人姓名或新商铺名称**。"
        ));
    }

    // 4. Guild-scoped reincarnation echoes from previous completed runs of this scenario
    if !world_echoes.is_empty() {
        let lines = world_echoes
            .iter()
            .take(3)
            .map(|e| format!("- 《{}》残响：{}", e.ending_title, e.echo_text))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!(
            "【本服上一周目轮回残响（可自然化作壁画刻字、旧物痕迹、NPC 莫名既视感或隐藏风味，切勿直接打破第四面墙剧透）】\n{lines}"
        ));
    }

    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}

pub fn format_player_dnd_tail(p: &Player) -> String {
    let mut parts = Vec::new();
    if p.dnd.level > 1 {
        parts.push(format!("Lv.{}", p.dnd.level));
    }
    if !p.dnd.alignment.trim().is_empty() {
        parts.push(format!("阵营:{}", p.dnd.alignment.trim()));
    }
    if !p.dnd.proficient_skills.is_empty() {
        parts.push(format!("熟练:[{}]", p.dnd.proficient_skills.join("、")));
    }
    if !p.dnd.abilities.is_empty() {
        let abs = p
            .dnd
            .abilities
            .iter()
            .map(|a| format!("{}({}/{})", a.name, a.uses_left, a.max_uses))
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("招牌技:[{abs}]"));
    }
    if p.dnd.inspiration {
        parts.push("🌟持有激励".into());
    }
    if !p.dnd.backstory.trim().is_empty() {
        parts.push(format!("背景:{}", p.dnd.backstory.trim()));
    }
    if !p.dnd.personal_secret.trim().is_empty() {
        parts.push(format!("个人秘密(严禁公屏剧透):{}", p.dnd.personal_secret.trim()));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" | {}", parts.join(" | "))
    }
}

pub fn format_act_progress(scenario: &Scenario, room: &RoomState) -> String {
    let flags = if room.world_flags.is_empty() {
        "无".to_string()
    } else {
        let start = room.world_flags.len().saturating_sub(20);
        room.world_flags[start..].join("、")
    };
    if scenario.acts.is_empty() {
        return format!("自由推进 ｜ 已触发节点：[{flags}]");
    }
    let total = scenario.acts.len();
    let idx = (room.current_act.max(1) as usize)
        .saturating_sub(1)
        .min(total - 1);
    let act = &scenario.acts[idx];
    let mut out = format!(
        "第 {}/{} 幕【{}】｜ 本幕目标：{} ｜ 已触发节点：[{}]",
        idx + 1, total, act.title, act.goal, flags
    );
    if idx + 1 < total {
        let next = &scenario.acts[idx + 1];
        let cond = if act.advance_flags.is_empty() {
            act.goal.clone()
        } else {
            act.advance_flags.join(" / ")
        };
        out.push_str(&format!(
            "\n- 过幕判定条件：[{cond}] → 下一幕预告：第 {}/{} 幕【{}】（目标：{}）。若小队已达成上述目标或已离开本幕核心场景迈向下一阶段，本轮请务必将 JSON 中的 `advance_act` 设为 `true` 以正式转幕并触发全队升级！",
            idx + 2, total, next.title, next.goal
        ));
    }
    out
}
