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
            "【幕间暗流号外窗口已开启 — 请在本轮 JSON 中填写 `world_bulletin`】\n- 剧中媒介报头：{masthead}\n- 幕后活跃势力议程：\n{factions}{prev_note}\n- 要求：挑选 1 个幕后势力推进一次独立暗箱行动，写出简短带钩子的 `headline`(8~18字)、`body`(40~90字) 与 `faction`。"
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

pub fn format_act_progress(scenario: &Scenario, room: &RoomState) -> String {
    let flags = if room.world_flags.is_empty() {
        "无".to_string()
    } else {
        room.world_flags.join("、")
    };
    if scenario.acts.is_empty() {
        return format!("自由推进 ｜ 已触发节点：[{flags}]");
    }
    let total = scenario.acts.len();
    let idx = (room.current_act.max(1) as usize)
        .saturating_sub(1)
        .min(total - 1);
    let act = &scenario.acts[idx];
    format!(
        "第 {}/{} 幕【{}】｜ 本幕目标：{} ｜ 已触发节点：[{}]",
        idx + 1,
        total,
        act.title,
        act.goal,
        flags
    )
}
