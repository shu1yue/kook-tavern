use tavern_domain::{EngineResponse, PendingAction, Player};

pub fn format_party_block(players: &[Player], pending_actions: &[PendingAction]) -> String {
    if players.is_empty() {
        return "暂无玩家入团，发送 `/join 角色名 身份 | 特质` 即可加入！".to_string();
    }
    players
        .iter()
        .map(|p| {
            let ready_badge = if p.is_afk {
                "💤 暂离"
            } else if pending_actions
                .iter()
                .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty())
            {
                "✅ 已行动"
            } else {
                "⏳ 待行动"
            };
            let inv = if p.inventory.is_empty() {
                "空".to_string()
            } else {
                p.inventory.join("、")
            };
            format!(
                "**{}**（{}）`{}`\n> 📊 {}\n> ❤️ HP {}/{} ｜ 🧠 SAN {}/{} ｜ 🏷️ {}\n> 🎒 {}",
                p.character_name,
                p.role_desc,
                ready_badge,
                p.stats.format_inline(),
                p.hp,
                p.max_hp,
                p.san,
                p.max_san,
                if p.status_note.is_empty() { "正常" } else { &p.status_note },
                inv
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Complete KMarkdown rendering for KOOK Direct Messages (`include_whispers = true`)
/// and public channel CardMessage failure fallback (`include_whispers = false`, never leaks secrets!).
pub fn render_plain_fallback(resp: &EngineResponse, include_whispers: bool) -> String {
    match resp {
        EngineResponse::Ignore => String::new(),
        EngineResponse::Notice { title, body, .. } => format!("**{title}**\n{body}"),
        EngineResponse::PrivateNotice { title, body, .. } => format!("**{title}**\n{body}"),
        EngineResponse::WorldbookStoreMenu { packs, .. } => {
            let list = packs
                .iter()
                .map(|p| {
                    format!(
                        "- **《{}》** (`{}` · {} · {}幕) — {}",
                        p.name, p.id, p.recommended_system.short_badge(), p.recommended_acts, p.summary
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("**📦 开源世界书商店**\n{list}\n\n发送 `/store <ID>` 查看详情，或 `/workshop wb <ID>` 开写！")
        }
        EngineResponse::WorldbookDetail { pack, .. } => {
            let hooks = pack
                .starter_hooks
                .iter()
                .enumerate()
                .map(|(i, h)| format!("{}. {h}", i + 1))
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "**📖 世界书：《{}》**（{} · {} 条设定）\n{}\n\n**🎯 灵感种子**\n{hooks}\n\n发送 `/workshop wb {}` 开始创作！",
                pack.name, pack.license, pack.entries.len(), pack.summary, pack.id
            )
        }
        EngineResponse::WorkshopDesk { draft, base_pack, .. } => {
            let wb = base_pack.as_deref().map(|p| p.name.as_str()).unwrap_or("空白原创");
            let preview = if draft.forged_summary.is_empty() {
                String::new()
            } else {
                format!("\n\n✨ **锻造预览**\n{}", draft.forged_summary)
            };
            format!(
                "**✍️ 剧本引导创作工坊**\n- 世界书：{wb}\n- 篇幅/规则：{} ｜ {}\n- 脑洞：{}\n- 私房设定：{} 条{preview}",
                draft.length_label(),
                draft.check_system.short_badge(),
                if draft.premise.is_empty() { "（待填写 `/workshop idea ...`）" } else { &draft.premise },
                draft.custom_lore.len()
            )
        }
        EngineResponse::ScenarioMenu { scenarios, .. } => {
            let list = scenarios
                .iter()
                .map(|s| format!("- **{}** (`{}`) — {}", s.name, s.id, s.tagline))
                .collect::<Vec<_>>()
                .join("\n");
            format!("**🍺 可选冒险剧本大厅**\n{list}")
        }
        EngineResponse::RolePicker {
            scenario_name,
            roles,
            ..
        } => {
            let list = roles
                .iter()
                .map(|r| format!("- **{}**（{}）", r.name, r.traits.join("/")))
                .collect::<Vec<_>>()
                .join("\n");
            format!("**🎭 《{scenario_name}》预设职业**\n{list}")
        }
        EngineResponse::ExportReport {
            saved_file,
            turn_count,
            ..
        } => format!("**📜 跑团战报已导出**\n共 {turn_count} 回合：`{saved_file}`"),
        EngineResponse::StoryTurn {
            turn_number,
            scene_title,
            location,
            world_time,
            act_badge,
            narrative,
            state_changes,
            whisper_hints,
            suggested_actions,
            suggested_check,
            world_bulletin,
            ending_reached,
            direct_messages,
            ..
        } => {
            let act_str = act_badge
                .as_deref()
                .map(|a| format!(" ｜ 🎬 {a}"))
                .unwrap_or_default();
            let mut out = format!(
                "**🎭 {scene_title}**\n📍 {location} ｜ 🕒 {world_time} ｜ 🎲 第 {turn_number} 回合{act_str}\n\n{narrative}"
            );
            if let Some(b) = world_bulletin {
                out.push_str(&format!("\n\n📰 **{}** — **【{}】**\n> {}", b.masthead, b.headline, b.body));
            }
            if !state_changes.is_empty() || !whisper_hints.is_empty() {
                out.push_str("\n\n---\n");
                for line in state_changes {
                    out.push_str(&format!("{line}\n"));
                }
                for line in whisper_hints {
                    out.push_str(&format!("{line}\n"));
                }
            }
            if include_whispers {
                for dm in direct_messages {
                    out.push_str(&format!("\n{}\n", dm.content));
                }
            }
            if let Some(end) = ending_reached {
                out.push_str(&format!("\n\n🏆 **达成结局：《{}》**", end.title));
            }
            if let Some(ck) = suggested_check {
                out.push_str(&format!("\n🎲 **推荐检定**：{}（{}）", ck.stat, ck.reason));
            }
            if !suggested_actions.is_empty() {
                out.push_str(&format!("\n💡 **行动灵感**：{}", suggested_actions.join(" / ")));
            }
            out
        }
        EngineResponse::StatusBoard {
            room,
            players,
            pending_actions,
            ..
        } => {
            let mut out = format!(
                "**📊 《{}》当前战局面板**\n📍 {} ｜ 🕒 {} ｜ 🎲 第 {} 回合\n\n{}",
                room.title,
                room.location,
                room.world_time,
                room.turn_number,
                format_party_block(players, pending_actions)
            );
            if !room.clues.is_empty() {
                let clues = room
                    .clues
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n");
                out.push_str(&format!("\n\n🔎 **已掌握线索**\n{clues}"));
            }
            out
        }
    }
}
