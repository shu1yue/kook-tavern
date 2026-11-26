use tavern_domain::{
    DmStructuredReply, LoreCandidate, PendingAction, Player, RoomState, Scenario, TurnRecord,
    WorldbookEntry, format_batched_actions, match_dynamic_worldbook,
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

pub fn format_clocks_snapshot(room: &RoomState) -> String {
    if room.clocks.is_empty() {
        return "（暂未启用进度钟）".to_string();
    }
    room.clocks
        .iter()
        .map(|c| {
            let auto_hint = if c.auto_tick_every > 0 {
                format!("，每{}轮自动+1", c.auto_tick_every)
            } else {
                String::new()
            };
            let full_tag = if c.filled >= c.segments {
                "【已满·必须兑现！】"
            } else {
                ""
            };
            format!(
                "- `{}`（{} · {}）：{}/{}{}{} ｜ 满格后果：{}",
                c.id,
                c.name,
                c.kind.label(),
                c.filled,
                c.segments,
                auto_hint,
                full_tag,
                c.on_full
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Solo-player clarity guard: strictly when total enrolled players `<= 1` (never when an
/// AFK teammate exists!) and no whisper redaction was explicitly triggered, folds stray
/// `private_whispers` cleanly into `narrative` so solo play stays in ONE cohesive card.
pub(crate) fn fold_solo_whispers(
    players: &[Player],
    did_whisper_redact: bool,
    reply: &mut DmStructuredReply,
) {
    if players.len() > 1 || did_whisper_redact || reply.private_whispers.is_empty() {
        return;
    }
    let mut extras = Vec::new();
    for w in reply.private_whispers.drain(..) {
        let s = w.secret.trim();
        if !s.is_empty() && !reply.narrative.contains(s) {
            extras.push(s.to_string());
        }
    }
    if !extras.is_empty() {
        if !reply.narrative.trim().is_empty() {
            reply.narrative.push_str("\n\n");
        }
        reply.narrative.push_str(&extras.join("\n"));
    }
}
