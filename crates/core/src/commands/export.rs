use anyhow::Result;
use chrono::Utc;
use tavern_domain::{CardTheme, EngineResponse, Player, RoomState, TurnRecord, WallReport};

use crate::engine::GameEngine;

pub fn cmd_export(
    engine: &GameEngine,
    channel_id: &str,
    is_direct: bool,
) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "当前没有可导出的跑团记录。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    let players = engine.store.list_players(channel_id)?;
    let turns = engine.store.list_all_turns(channel_id)?;
    let scenario = engine.scenarios.get(&room.scenario_id);

    let safe_scenario = sanitize_filename_segment(&room.scenario_id);
    let safe_channel = sanitize_filename_segment(channel_id);
    let filename = format!(
        "{}_{}_{}.md",
        safe_scenario,
        safe_channel,
        Utc::now().format("%Y%m%d_%H%M%S")
    );

    let mut md = String::new();
    md.push_str(&format!("# 🎭 跑团战报：《{}》\n\n", room.title));
    if let Some(ref end_title) = room.ended_title {
        md.push_str(&format!("> 🏆 **达成结局**：{}\n\n", end_title));
    }
    md.push_str(&format!(
        "- **频道**：`{}`\n- **总回合数**：{}\n- **最新位置**：{}\n- **最新时间**：{}\n\n",
        room.channel_id, room.turn_number, room.location, room.world_time
    ));

    md.push_str("## 🪪 参团角色\n\n");
    for p in &players {
        md.push_str(&format!(
            "- **{}**（{}）｜HP {}/{}｜SAN {}/{}｜背包：{}\n",
            p.character_name,
            p.role_desc,
            p.hp,
            p.max_hp,
            p.san,
            p.max_san,
            if p.inventory.is_empty() {
                "空".to_string()
            } else {
                p.inventory.join("、")
            }
        ));
    }
    md.push('\n');

    if !room.npc_states.is_empty() {
        md.push_str("## 🗣️ 关键 NPC 动向\n\n");
        for npc in &room.npc_states {
            md.push_str(&format!(
                "- **{}**（{}）：{}\n",
                npc.name, npc.attitude, npc.status
            ));
        }
        md.push('\n');
    }

    if let Some(sc) = scenario {
        md.push_str("## 🕯️ 序幕\n\n");
        md.push_str(sc.opening_scene.trim());
        md.push_str("\n\n");
    }

    for t in &turns {
        md.push_str(&format!(
            "## 第 {} 回合 · {}（{} / {}）\n\n",
            t.turn_number, t.scene_title, t.location, t.world_time
        ));
        md.push_str("### 玩家行动\n\n");
        for line in t.batched_input.lines() {
            md.push_str(&format!("> {line}\n"));
        }
        md.push_str("\n### DM 叙事\n\n");
        md.push_str(t.narrative.trim());
        md.push_str("\n\n");
        if !t.state_summary.trim().is_empty() {
            md.push_str(&format!("*状态变化：{}*\n\n", t.state_summary));
        }
    }

    let saved_file = engine.exporter.write_markdown(&filename, &md)?;
    let wall_report = if !is_direct && !turns.is_empty() {
        engine.cfg.export_channel_id.as_ref().map(|wall_chan| {
            build_wall_report(wall_chan, &room, &players, &turns)
        })
    } else {
        None
    };

    Ok(EngineResponse::ExportReport {
        saved_file,
        turn_count: turns.len(),
        wall_report,
    })
}

fn build_wall_report(
    target_channel_id: &str,
    room: &RoomState,
    players: &[Player],
    turns: &[TurnRecord],
) -> WallReport {
    let roster = if players.is_empty() {
        "神秘冒险者小队".to_string()
    } else {
        players
            .iter()
            .map(|p| {
                format!(
                    "**{}**（{}｜❤️{}/{} 🧠{}/{}）",
                    p.character_name, p.role_desc, p.hp, p.max_hp, p.san, p.max_san
                )
            })
            .collect::<Vec<_>>()
            .join("\n- ")
    };

    let mut sections = Vec::new();
    if let Some(ref end_title) = room.ended_title {
        sections.push(format!("🏆 **达成结局**：**{end_title}**"));
    }
    sections.push(format!(
        "📍 **定格场景**：{} ｜ 🕒 {} ｜ 🎲 共历 **{}** 回合",
        room.location, room.world_time, turns.len()
    ));
    sections.push(format!("**⚔️ 冒险阵容**\n- {roster}"));

    if !room.clues.is_empty() {
        let top_clues = room
            .clues
            .iter()
            .take(6)
            .enumerate()
            .map(|(i, c)| format!("{}. {c}", i + 1))
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("**🔎 关键线索**\n{top_clues}"));
    }

    if !room.npc_states.is_empty() {
        let npcs = room
            .npc_states
            .iter()
            .take(6)
            .map(|n| {
                if n.status.trim().is_empty() {
                    format!("- **{}**（{}）", n.name, n.attitude)
                } else {
                    format!("- **{}**（{}）— {}", n.name, n.attitude, n.status)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        sections.push(format!("**🗣️ 江湖传闻与 NPC 结局**\n{npcs}"));
    }

    if let Some(last) = turns.last() {
        let excerpt: String = last.narrative.trim().chars().take(260).collect();
        sections.push(format!(
            "**🎬 最新一幕 · {}**\n> {}",
            last.scene_title,
            excerpt.replace('\n', "\n> ")
        ));
    }

    WallReport {
        target_channel_id: target_channel_id.to_string(),
        title: format!("🏆 冒险战报 · 《{}》", room.title),
        body: sections.join("\n\n"),
    }
}

fn sanitize_filename_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if cleaned.is_empty() || cleaned.chars().all(|c| c == '_') {
        "session".to_string()
    } else {
        cleaned
    }
}
