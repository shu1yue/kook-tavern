use std::collections::HashMap;

use anyhow::Result;
use tavern_core::GameEngine;
use tavern_domain::{EngineResponse, IncomingMessage};
use tokio::io::{self, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn run_local_cli(engine: GameEngine, channel_id: String, mock_mode: bool) -> Result<()> {
    let mut stdout = io::stdout();
    let banner = format!(
        r#"
============================================================
🍺 KOOK AI 群酒馆 / 文字冒险引擎 · 本地多人模拟器
   频道ID: {channel_id} | 模式: {mode}
============================================================
使用说明：
  1. 用 `@昵称 内容` 模拟不同群友发言，例如：
       @小明 /start
       @小明 /join 陆沉 流浪机械师 | 眼尖,手巧
       @小红 /join 薇拉 吟游诗人 | 嘴甜,胆大
       @小明 .r 2d6+1 检查信使
       @小明 /act 蹲下检查信使手中的断钥匙
       @小红 /act 安抚信使并环顾酒馆里的可疑眼神
  2. 不写 `@昵称` 则沿用当前发言人（默认：小明）
  3. 模拟点击 KOOK 卡片按钮：`:btn resolve` / `:btn status` / `:btn undo`
  4. 输入 `:quit` 退出
============================================================
"#,
        mode = if mock_mode {
            "Mock DM（离线零 API 成本演练）"
        } else {
            "真实大模型 API"
        }
    );
    stdout.write_all(banner.as_bytes()).await?;
    stdout.flush().await?;

    let stdin = BufReader::new(io::stdin());
    let mut lines = stdin.lines();
    let mut current_speaker = "小明".to_string();
    let mut user_ids: HashMap<String, String> = HashMap::new();
    user_ids.insert("小明".to_string(), "local_u1".to_string());

    loop {
        let prompt = format!("[{current_speaker}] > ");
        stdout.write_all(prompt.as_bytes()).await?;
        stdout.flush().await?;

        let Some(line) = lines.next_line().await? else {
            break;
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if matches!(trimmed, ":quit" | ":exit" | ":q" | "：quit" | "：exit" | "：q") {
            stdout.write_all("👋 已退出本地模拟器。\n".as_bytes()).await?;
            stdout.flush().await?;
            break;
        }

        if let Some(btn_arg) = trimmed
            .strip_prefix(":btn ")
            .or_else(|| trimmed.strip_prefix("：btn "))
        {
            let mut parts = btn_arg.split_whitespace();
            let action = parts.next().unwrap_or("");
            let extra: Vec<&str> = parts.collect();
            let uid = get_or_assign_uid(&mut user_ids, &current_speaker);
            let value = if extra.is_empty() {
                format!("tavern:{action}:{channel_id}")
            } else {
                format!("tavern:{action}:{channel_id}:{}", extra.join(":"))
            };
            if !mock_mode && action == "resolve" {
                stdout
                    .write_all("⏳ DM 正在结算本轮剧情，请稍候...\n".as_bytes())
                    .await?;
                stdout.flush().await?;
            }
            match engine
                .handle_button_in_guild(Some("local_guild"), &channel_id, &uid, &current_speaker, &value)
                .await
            {
                Ok(resp) => print_response(&mut stdout, &resp).await?,
                Err(err) => {
                    let msg = format!("\n❌ 按钮执行报错：{err}\n\n");
                    stdout.write_all(msg.as_bytes()).await?;
                    stdout.flush().await?;
                }
            }
            continue;
        }

        let (speaker, content) = parse_local_input(trimmed, &current_speaker);
        current_speaker = speaker.clone();
        let uid = get_or_assign_uid(&mut user_ids, &speaker);

        let incoming = IncomingMessage {
            guild_id: Some("local_guild".to_string()),
            channel_id: channel_id.clone(),
            user_id: uid,
            user_name: speaker,
            content: content.clone(),
            is_direct: false,
        };

        if !mock_mode && might_trigger_llm(&content) {
            stdout
                .write_all("⏳ 正在处理并等待 AI DM 响应...\n".as_bytes())
                .await?;
            stdout.flush().await?;
        }

        match engine.handle_message(incoming).await {
            Ok(resp) => print_response(&mut stdout, &resp).await?,
            Err(err) => {
                let msg = format!("\n❌ 引擎报错：{err}\n\n");
                stdout.write_all(msg.as_bytes()).await?;
                stdout.flush().await?;
            }
        }
    }

    Ok(())
}

fn get_or_assign_uid(map: &mut HashMap<String, String>, name: &str) -> String {
    if let Some(existing) = map.get(name) {
        return existing.clone();
    }
    let next_id = format!("local_u{}", map.len() + 1);
    map.insert(name.to_string(), next_id.clone());
    next_id
}

fn parse_local_input(line: &str, default_speaker: &str) -> (String, String) {
    if let Some(rest) = line.strip_prefix('@').or_else(|| line.strip_prefix('＠')) {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let who = parts.next().unwrap_or(default_speaker).trim();
        let content = parts.next().unwrap_or("").trim();
        if !who.is_empty() && !content.is_empty() {
            return (who.to_string(), content.to_string());
        }
    }
    (default_speaker.to_string(), line.to_string())
}

fn might_trigger_llm(content: &str) -> bool {
    let t = content.trim();
    t.starts_with('>')
        || t.starts_with('》')
        || t.starts_with("/act")
        || t.starts_with("/a ")
        || t.starts_with("/go")
        || t.starts_with("/结算")
        || t.starts_with("／act")
        || t.starts_with("、act")
        || t.starts_with("／go")
        || t.starts_with("、go")
}

async fn print_response(stdout: &mut io::Stdout, resp: &EngineResponse) -> Result<()> {
    match resp {
        EngineResponse::Ignore => {
            stdout
                .write_all(
                    "  (🔇 未触发指令/已按场外 OOC 忽略。开团请发 /start，提交行动请发 /act 内容 或 > 内容，查看指令发 /help)\n"
                        .as_bytes(),
                )
                .await?;
        }
        EngineResponse::Notice { title, body, .. } => {
            let out = format!(
                "\n┌──────────────────────────────────────────────────────────\n│ {title}\n├──────────────────────────────────────────────────────────\n{}\n└──────────────────────────────────────────────────────────\n\n",
                indent_lines(body)
            );
            stdout.write_all(out.as_bytes()).await?;
        }
        EngineResponse::ScenarioMenu { scenarios, .. } => {
            let mut buf =
                String::from("\n┌────────────────── 🍺 可选冒险剧本大厅 ──────────────────\n");
            for s in scenarios {
                buf.push_str(&format!(
                    "│ ▶️ {} (`{}`) — {}\n│    {}（世界书 {} 条）\n",
                    s.name, s.id, s.genre, s.tagline, s.worldbook_count
                ));
            }
            buf.push_str("└──────────────────────────────────────────────────────────\n\n");
            stdout.write_all(buf.as_bytes()).await?;
        }
        EngineResponse::RolePicker {
            scenario_name,
            roles,
            ..
        } => {
            let mut buf = format!("\n┌────────── 🎭 《{scenario_name}》可选职业 ──────────\n");
            for (i, r) in roles.iter().enumerate() {
                buf.push_str(&format!(
                    "│ {}. {}（{}）｜装备：{}\n",
                    i + 1,
                    r.name,
                    r.traits.join("/"),
                    r.bonus_items.join("、")
                ));
            }
            buf.push_str("└──────────────────────────────────────────────────────────\n\n");
            stdout.write_all(buf.as_bytes()).await?;
        }
        EngineResponse::ExportReport {
            saved_file,
            turn_count,
            ..
        } => {
            let out = format!(
                "\n📜 已导出 {turn_count} 回合战报：`{saved_file}`\n\n"
            );
            stdout.write_all(out.as_bytes()).await?;
        }
        EngineResponse::PrivateNotice {
            user_id,
            title,
            body,
        } => {
            let out = format!(
                "\n┌── 🔒 仅对 {user_id} 可见 ─────────────────────────────────\n│ {title}\n├──────────────────────────────────────────────────────────\n{}\n└──────────────────────────────────────────────────────────\n\n",
                indent_lines(body)
            );
            stdout.write_all(out.as_bytes()).await?;
        }
        EngineResponse::StoryTurn {
            turn_number, scene_title, location, world_time, act_badge, narrative,
            state_changes, whisper_hints, suggested_actions, suggested_check, world_bulletin,
            ending_reached, direct_messages, ..
        } => {
            let act_str = act_badge.as_deref().map(|a| format!(" ｜ 🎬 {a}")).unwrap_or_default();
            let mut buf = format!(
                "\n╔══════════════════════════════════════════════════════════\n║ 🎭 {scene_title}\n║ 📍 {location} ｜ 🕒 {world_time} ｜ 🎲 第 {turn_number} 回合{act_str}\n╠══════════════════════════════════════════════════════════\n{}\n",
                indent_lines(narrative)
            );
            if let Some(b) = world_bulletin {
                buf.push_str(&format!("╟──────────────────────────────────────────────────────────\n║ 📰 {} · 【{}】\n║    {}\n", b.masthead, b.headline, b.body));
            }
            if !state_changes.is_empty() || !whisper_hints.is_empty() {
                buf.push_str("╟──────────────────────────────────────────────────────────\n");
                for line in state_changes { buf.push_str(&format!("║ {line}\n")); }
                for line in whisper_hints { buf.push_str(&format!("║ {line}\n")); }
            }
            if let Some(end) = ending_reached {
                let tag = if end.newly_unlocked { " [🆕 首次解锁]" } else { "" };
                buf.push_str(&format!("╟──────────────────────────────────────────────────────────\n║ 🏆 达成结局：《{}》{tag}\n", end.title));
            }
            if let Some(ck) = suggested_check {
                buf.push_str(&format!("║ 🎲 推荐检定：{} ({})\n", ck.stat, ck.reason));
            }
            if !suggested_actions.is_empty() {
                buf.push_str("╟──────────────────────────────────────────────────────────\n");
                buf.push_str(&format!("║ 💡 建议行动：{}\n", suggested_actions.join(" / ")));
            }
            buf.push_str("╚══════════════════════════════════════════════════════════\n");

            for dm in direct_messages {
                buf.push_str(&format!(
                    "  [私信 -> {} ({})]\n{}\n",
                    dm.player_name,
                    dm.user_id,
                    indent_lines(&dm.content)
                ));
            }
            buf.push('\n');
            stdout.write_all(buf.as_bytes()).await?;
        }
        EngineResponse::StatusBoard {
            room,
            players,
            pending_actions,
            ..
        } => {
            let mut buf = format!(
                "\n┌────────────────── 📊 《{}》战局面板 ──────────────────\n│ 📍 {} ｜ 🕒 {} ｜ 🎲 第 {} 回合 ｜ 模式: {} ｜ 档位: {}\n├──────────────────────────────────────────────────────────\n",
                room.title,
                room.location,
                room.world_time,
                room.turn_number,
                room.chat_mode.as_str(),
                room.narrative_style.as_str()
            );
            if players.is_empty() {
                buf.push_str("│ （暂无已入团角色）\n");
            } else {
                for p in players {
                    let badge = if p.is_afk {
                        "💤 暂离"
                    } else if pending_actions
                        .iter()
                        .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty())
                    {
                        "✅ 已行动"
                    } else {
                        "⏳ 等待中"
                    };
                    buf.push_str(&format!(
                        "│ {}（{}） [{}] ｜ {}\n│   ❤️ HP {}/{} ｜ 🧠 SAN {}/{} ｜ 🏷️ {}\n│   🎒 {}\n",
                        p.character_name, p.role_desc, badge, p.stats.format_inline(),
                        p.hp, p.max_hp, p.san, p.max_san,
                        if p.status_note.is_empty() { "正常" } else { &p.status_note },
                        if p.inventory.is_empty() { "空".to_string() } else { p.inventory.join("、") }
                    ));
                }
            }
            if !room.clues.is_empty() {
                buf.push_str("├──────────────────────────────────────────────────────────\n│ 🔎 已掌握线索：\n");
                for (i, c) in room.clues.iter().enumerate() {
                    buf.push_str(&format!("│   {}. {c}\n", i + 1));
                }
            }
            if !room.npc_states.is_empty() {
                buf.push_str("├──────────────────────────────────────────────────────────\n│ 🗣️ 关键 NPC：\n");
                for n in &room.npc_states {
                    buf.push_str(&format!("│   - {} [{}]：{}\n", n.name, n.attitude, n.status));
                }
            }
            buf.push_str("└──────────────────────────────────────────────────────────\n\n");
            stdout.write_all(buf.as_bytes()).await?;
        }
    }
    stdout.flush().await?;
    Ok(())
}

fn indent_lines(text: &str) -> String {
    text.lines()
        .map(|l| format!("│ {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
