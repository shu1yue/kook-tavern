use anyhow::Result;
use tavern_domain::{
    CardTheme, EngineResponse, format_clock_bar, format_full_brief_markdown, init_room_clocks,
};

use crate::engine::GameEngine;

pub fn cmd_summary(
    engine: &GameEngine,
    channel_id: &str,
    caller_user_id: Option<&str>,
) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先 `/start` 或在 `/menu` 点「🚪 开独立包间」开桌吧！".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    let scenario = engine.scenarios.get(&room.scenario_id);
    let mut sections = Vec::new();

    // 1. Complete spoiler-free Player Brief (world stage + mission + cast + ending paths)
    if let Some(sc) = &scenario {
        let brief_md = format_full_brief_markdown(sc);
        if !brief_md.is_empty() {
            sections.push(format!("### 📜 桌前无剧透任务书\n{brief_md}"));
        }
    }
    sections.push(format!(
        "📍 **当前坐标**：**{}**（{} · 第 {} 回合{}）",
        room.location,
        room.world_time,
        room.turn_number,
        if room.manual_resolve { " · ⏸️ 手动结算/自由飙戏中" } else { "" }
    ));

    // 2. Who are the key people we've met?
    let npc_block = if room.npc_states.is_empty() {
        "（暂未记录到已互动的关键人物，可先跟在场人物搭话了解情况）".to_string()
    } else {
        room.npc_states
            .iter()
            .take(8)
            .map(|n| {
                let loc = if n.location.trim().is_empty() {
                    String::new()
                } else {
                    format!(" @{}", n.location.trim())
                };
                format!("- **{}**（{}{}）：{}", n.name, n.attitude, loc, n.status)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    sections.push(format!("### 🗣️ 已接触关键人物动态\n{npc_block}"));

    // 3. Current act goal + pacing clocks + open threads + latest 6 clues
    let mut todo_lines = Vec::new();
    if let Some(sc) = &scenario
        && !sc.acts.is_empty()
    {
        let total = sc.acts.len();
        let idx = (room.current_act.max(1) as usize)
            .saturating_sub(1)
            .min(total - 1);
        let cur = &sc.acts[idx];
        todo_lines.push(format!(
            "🎯 **当前主线（第 {}/{total} 幕 · {}）**：{}",
            idx + 1,
            cur.title,
            cur.goal
        ));
    }

    let active_clocks = if !room.clocks.is_empty() {
        room.clocks.clone()
    } else if let Some(sc) = &scenario {
        init_room_clocks(sc)
    } else {
        Vec::new()
    };
    if !active_clocks.is_empty() {
        let bars = active_clocks
            .iter()
            .map(format_clock_bar)
            .collect::<Vec<_>>()
            .join("\n");
        todo_lines.push(format!("⏳ **局势进度钟**：\n{bars}"));
    }

    let trimmed_sum = room.summary.trim();
    if let Some((_, threads)) = trimmed_sum.rsplit_once("【契约与未解伏笔便签】")
        && !threads.trim().is_empty()
    {
        let clean_threads: Vec<&str> = threads
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .take(5)
            .collect();
        if !clean_threads.is_empty() {
            todo_lines.push(format!("📌 **未解关键事项**：\n{}", clean_threads.join("\n")));
        }
    }

    if !room.clues.is_empty() {
        let start = room.clues.len().saturating_sub(6);
        let recent_clues = room.clues[start..]
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n");
        todo_lines.push(format!(
            "🔎 **最近关键线索**（共 {} 条，展示最新 {} 条）：\n{}",
            room.clues.len(),
            room.clues.len() - start,
            recent_clues
        ));
    }
    if !todo_lines.is_empty() {
        sections.push(format!("### 🧭 当前战局与手头线索\n{}", todo_lines.join("\n\n")));
    }

    // 4. Compact latest chapter recap
    if !trimmed_sum.is_empty() {
        let chapters_part = trimmed_sum
            .rsplit_once("【契约与未解伏笔便签】")
            .map(|(l, _)| l.trim())
            .unwrap_or(trimmed_sum)
            .strip_prefix("【编年史封卷（永不覆写）】")
            .unwrap_or(trimmed_sum)
            .trim();
        let chs: Vec<&str> = chapters_part
            .split("\n### ")
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if !chs.is_empty() {
            let take_from = chs.len().saturating_sub(2);
            let digest = chs[take_from..]
                .iter()
                .map(|c| {
                    if c.starts_with("### ") {
                        c.to_string()
                    } else {
                        format!("### {c}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            sections.push(format!(
                "### 📖 最近前情纪要（截至第 {} 轮）\n{digest}",
                room.summary_upto_turn
            ));
        }
    }

    let title = format!("📖 《{}》桌前任务书与案情简报", room.title);
    let body = sections.join("\n\n");
    if let Some(uid) = caller_user_id.filter(|s| !s.is_empty()) {
        Ok(EngineResponse::PrivateNotice {
            user_id: uid.to_string(),
            title,
            body,
            actions: Vec::new(),
        })
    } else {
        Ok(EngineResponse::Notice {
            title,
            body,
            theme: CardTheme::Info,
            show_actions: true,
        })
    }
}
