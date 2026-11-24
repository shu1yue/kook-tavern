use anyhow::Result;
use chrono::Utc;
use tavern_domain::{CardTheme, EngineResponse};

use crate::engine::GameEngine;

pub fn cmd_export(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
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

    let saved_name = engine.exporter.write_markdown(&filename, &md)?;

    Ok(EngineResponse::Notice {
        title: "📜 跑团战报已导出".to_string(),
        body: format!(
            "已导出共 **{} 个回合** 的完整 Markdown 战报：`{}`",
            turns.len(),
            saved_name
        ),
        theme: CardTheme::Success,
        show_actions: false,
    })
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
