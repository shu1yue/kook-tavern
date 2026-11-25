use anyhow::Result;
use tavern_domain::{
    CardTheme, CheckSystem, CheckTier, EngineResponse, IncomingMessage, PrivateButton, RoomEvent,
    RoomState, Scenario, ending_scope_key,
};

use crate::commands::dnd::{format_dnd_me_sections, parse_check_args};
use crate::commands::party_helpers::ensure_player;
use crate::dnd_ports::{CheckCommitOutcome, RuleCheckRequest};
use crate::engine::GameEngine;

/// Single authoritative guard for ended rooms (`ended_id.is_some()`).
/// Always returns `show_actions: false` so ended rooms never re-emit live turn buttons.
pub fn ended_room_notice(room: &RoomState) -> Option<EngineResponse> {
    let end_id = room.ended_id.as_deref()?;
    let end_title = room.ended_title.as_deref().unwrap_or(end_id);
    Some(EngineResponse::Notice {
        title: format!("🏆 本局已达成结局：{end_title}"),
        body: "故事已完整收束！可点「📜 发战报墙」留念、「🏆 结局图鉴」看收集进度、「↩️ 回档上轮」改写命运，或由房主点「📚 换剧本」开启新局。".to_string(),
        theme: CardTheme::Info,
        show_actions: false,
    })
}

pub fn format_act_badge(scenario: Option<&Scenario>, current_act: u32) -> Option<String> {
    let sc = scenario?;
    let total = sc.acts.len();
    if total == 0 {
        return None;
    }
    let idx = (current_act.max(1) as usize)
        .saturating_sub(1)
        .min(total - 1);
    let act = &sc.acts[idx];
    Some(format!("第 {}/{} 幕 · {}", idx + 1, total, act.title))
}

pub fn cmd_check(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".into(),
            body: "先发送 `/start` 或点绿色开团按钮开一桌，再进行属性检定喵！".into(),
            theme: CardTheme::Warning, show_actions: false,
        });
    };
    if !room.active {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已归档".into(), body: "请开启新剧本后再检定。".into(),
            theme: CardTheme::Warning, show_actions: false,
        });
    }
    if let Some(notice) = ended_room_notice(&room) { return Ok(notice); }
    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 剧本配置缺失".into(), body: format!("找不到剧本 `{}`。", room.scenario_id),
            theme: CardTheme::Danger, show_actions: false,
        });
    };
    let Some(player) = ensure_player(engine, msg, &scenario)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间不可操作".into(), body: "房间已结束或被重置，请重新开团。".into(),
            theme: CardTheme::Warning, show_actions: false,
        });
    };

    let Some(parsed) = parse_check_args(arg) else {
        return Ok(EngineResponse::Notice {
            title: format!("🎲 {} 的属性检定指南（{}）", player.character_name, room.check_system.short_badge()),
            body: format!(
                "**五维加值**：{}\n**当前体系**：{}\n\n**用法**：`/check <属性> [adv|dis|inspire] [dc15] [原因]`\n- `体魄`(STR) ｜ `敏捷`(DEX) ｜ `侦查`(INV) ｜ `交涉`(CHA) ｜ `灵感`(WIS)\n例：`/check 侦查 dc15 翻找暗格` 或 `/check 敏捷 adv 飞跃断桥`",
                player.stats.format_inline(),
                room.check_system.prompt_guide()
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    let req = RuleCheckRequest {
        stat: parsed.stat,
        raw_stat_token: parsed.raw_stat_token,
        mode: parsed.mode,
        dc: parsed.dc,
        use_inspiration: parsed.use_inspiration,
        reason: parsed.reason,
    };
    let (player, result, turn_no, bonus_notes) =
        match engine
            .store
            .execute_rule_check_atomic(&msg.channel_id, &msg.user_id, &req)?
        {
            CheckCommitOutcome::Committed { player, result, turn_no, bonus_notes } => {
                (player, result, turn_no, bonus_notes)
            }
            CheckCommitOutcome::RoomUnavailable | CheckCommitOutcome::PlayerNotFound => {
                return Ok(EngineResponse::Notice {
                    title: "ℹ️ 当前房间已结束或归档".to_string(),
                    body: "无法再向本局绑定检定，请查看结局图鉴或开启新局。".to_string(),
                    theme: CardTheme::Info,
                    show_actions: false,
                });
            }
        };

    let formatted = result.format_short();
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: turn_no.saturating_sub(1),
    });
    let theme = match (result.system, result.tier) {
        (_, CheckTier::CritSuccess | CheckTier::Success)
        | (CheckSystem::D100, CheckTier::Partial) => CardTheme::Success,
        (_, CheckTier::Partial) => CardTheme::Warning,
        (_, CheckTier::Failure | CheckTier::CritFailure) => CardTheme::Danger,
    };
    let bonus_line = if bonus_notes.is_empty() {
        String::new()
    } else {
        format!("{}\n", bonus_notes.join(" ｜ "))
    };
    Ok(EngineResponse::Notice {
        title: format!(
            "{} {} · {}检定",
            req.stat.icon(),
            player.character_name,
            req.stat.short_name()
        ),
        body: format!(
            "{bonus_line}{formatted}\n\n（已绑定至第 **{turn_no}** 轮行动，DM 将按 **{}** 裁决！）",
            result.tier.rule_hint_for(result.system)
        ),
        theme,
        show_actions: false,
    })
}

pub fn cmd_endings(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let room_guild = engine
        .store
        .get_room(&msg.channel_id)
        .ok()
        .flatten()
        .map(|r| r.guild_id)
        .filter(|s| !s.is_empty());
    let effective_guild = if msg.is_direct {
        None
    } else {
        room_guild
            .as_deref()
            .or_else(|| msg.guild_id.as_deref().filter(|s| !s.is_empty()))
    };
    let scope_key = ending_scope_key(effective_guild, &msg.channel_id);

    let unlocks = engine.store.list_ending_unlocks(&scope_key)?;
    let scenarios = engine.scenarios.list();
    let mut total_endings = 0usize;
    let mut total_unlocked = 0usize;
    let mut blocks = Vec::new();

    for sc in &scenarios {
        if sc.endings.is_empty() {
            continue;
        }
        let sc_total = sc.endings.len();
        total_endings += sc_total;
        let mut lines = Vec::new();
        let mut sc_got = 0usize;
        for end in &sc.endings {
            if let Some(u) = unlocks
                .iter()
                .find(|u| u.scenario_id == sc.id && u.ending_id == end.id)
            {
                sc_got += 1;
                total_unlocked += 1;
                let badge = ending_kind_badge(&end.kind);
                let party = if u.party_names.is_empty() {
                    "冒险小队"
                } else {
                    &u.party_names
                };
                lines.push(format!(
                    "- ✅ {badge} **{}**（历时 {} 轮 · {}）",
                    end.title, u.turn_count, party
                ));
            } else {
                lines.push(format!("- 🔒 {} `未解锁`", ending_kind_badge(&end.kind)));
            }
        }
        blocks.push(format!(
            "**《{}》**（已收集 **{sc_got}/{sc_total}**）\n{}",
            sc.name,
            lines.join("\n")
        ));
    }

    let body = if blocks.is_empty() {
        "当前剧本库尚未配置多结局条目。".to_string()
    } else {
        let scope_label = if msg.is_direct { "本私聊频道" } else { "本服" };
        format!(
            "{scope_label}累计解锁结局：**{total_unlocked} / {total_endings}**\n\n{}",
            blocks.join("\n\n")
        )
    };

    Ok(EngineResponse::Notice {
        title: "🏆 酒馆结局图鉴与通关成就".to_string(),
        body,
        theme: CardTheme::Primary,
        show_actions: false,
    })
}

pub fn ending_kind_badge(kind: &str) -> &'static str {
    match kind.trim().to_ascii_lowercase().as_str() {
        "true" => "🌟 真结局",
        "good" => "✨ 佳结局",
        "bad" => "💀 坏结局",
        "secret" => "🔮 隐藏结局",
        _ => "📜 普通结局",
    }
}

/// Private personal character sheet (`/me` or `🪪 我的角色卡` button).
/// Always returned as `EngineResponse::PrivateNotice` so teammates NEVER see private whispers!
pub fn cmd_me(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🪪 个人角色卡".to_string(),
            body: "当前频道尚未开团，点置顶卡里的绿色「▶️ 开启剧本」按钮先开一桌吧！".to_string(),
            actions: Vec::new(),
        });
    };
    let Some(p) = engine.store.get_player(&msg.channel_id, &msg.user_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: format!("🪪 《{}》尚未入团", room.title),
            body: "你还没有在本局创建角色卡！点「🎭 选职业入团」或发 `/char load <名字>` 即可上桌。".into(),
            actions: Vec::new(),
        });
    };
    let next_turn = if room.turn_phase == tavern_domain::TurnPhase::Resolving { room.turn_number + 2 } else { room.turn_number + 1 };
    let pending = engine.store.list_pending_actions(&msg.channel_id, next_turn)?;
    let my_act = pending.iter().find(|a| a.user_id == msg.user_id);
    let act_status = if room.ended_id.is_some() { "🏆 本局已收束" }
        else if p.is_afk { "💤 暂离中（自动结算跳过）" }
        else if my_act.is_some_and(|a| !a.action_text.trim().is_empty()) { "✅ 本轮已提交行动" }
        else { "⏳ 等待你行动" };
    let hp_bar = mini_meter(p.hp, p.max_hp, "🟥", "⬜");
    let san_bar = mini_meter(p.san, p.max_san, "🟦", "⬜");
    let traits = if p.traits.is_empty() { "无".into() } else { p.traits.join(" / ") };
    let inv = if p.inventory.is_empty() { "空".into() } else { p.inventory.join(" · ") };
    let status_txt = if p.status_note.trim().is_empty() { "正常" } else { p.status_note.trim() };
    let loc_txt = if p.location.trim().is_empty() { room.location.as_str() } else { p.location.trim() };
    let mut sections = vec![
        format!(
            "🎭 **《{}》** ｜ 第 **{}** 回合 ｜ `{act_status}`\n📍 **你当前所在**：{loc_txt}",
            room.title, room.turn_number
        ),
        format!(
            "❤️ **HP**：{hp_bar}\n🧠 **SAN**：{san_bar}\n🏷️ **当前状态**：{status_txt}\n✦ **特质**：{traits}"
        ),
        format!(
            "📊 **五维属性（`/check <属性>`）**\n{}",
            p.stats.format_inline()
        ),
        format!("🎒 **背包行囊**：{inv}"),
    ];
    sections.extend(format_dnd_me_sections(&p, room.check_system));

    if let Some(a) = my_act {
        let mut cur_lines = Vec::new();
        if !a.action_text.trim().is_empty() {
            cur_lines.push(format!("- 已提交行动：{}", a.action_text.trim()));
        }
        for r in &a.roll_notes {
            cur_lines.push(format!("- 已绑定检定/骰子：{r}"));
        }
        if !cur_lines.is_empty() {
            sections.push(format!("📝 **第 {next_turn} 轮准备中**\n{}", cur_lines.join("\n")));
        }
    }

    let whispers = engine.store.list_player_whispers(&msg.channel_id, &msg.user_id, 5)?;
    if whispers.is_empty() {
        sections.push("🤫 **我的专属秘密纸条**：暂无（单独察觉暗线情报时会自动收进这里，队友不可见）".to_string());
    } else {
        let w_lines = whispers.iter().rev()
            .map(|w| format!("- **第 {} 回合**：{}", w.turn_number, w.content.replace('\n', " ")))
            .collect::<Vec<_>>().join("\n");
        sections.push(format!("🤫 **我的专属秘密纸条（仅你可见，最近 {} 条）**\n{w_lines}", whispers.len()));
    }

    let mut actions = Vec::new();
    if !msg.is_direct && room.active && room.ended_id.is_none() && !p.is_afk {
        for (i, ab) in p.dnd.abilities.iter().enumerate() {
            if ab.uses_left > 0 && actions.len() < 2 {
                let short: String = ab.name.chars().take(8).collect();
                actions.push(PrivateButton {
                    label: format!("✨ {short}({}/{})", ab.uses_left, ab.max_uses),
                    value: format!("tavern:skill:{}:{}", msg.channel_id, i + 1),
                    theme: CardTheme::Primary,
                });
            }
        }
        if p.dnd.pending_levelups > 0 {
            actions.push(PrivateButton {
                label: "⬆️ 升级 +2血/理智".into(),
                value: format!("tavern:lv:{}:hp", msg.channel_id),
                theme: CardTheme::Success,
            });
            if !p.dnd.abilities.is_empty() && actions.len() < 4 {
                actions.push(PrivateButton {
                    label: "⬆️ 招牌技+1充能".into(),
                    value: format!("tavern:lv:{}:charge", msg.channel_id),
                    theme: CardTheme::Warning,
                });
            }
        }
    }

    if let Some(hud_url) = crate::hud_token::build_player_hud_url(&engine.cfg, &room, &msg.user_id) {
        if actions.len() < 4 {
            actions.push(PrivateButton { label: "🪟 悬浮角色卡 HUD".into(), value: hud_url.clone(), theme: CardTheme::Info });
        }
        sections.push(format!("🪟 **个人实时悬浮 HUD**（含角色、人物关系图、线索与仅你可见的密语，1 秒自动同步）：\n{hud_url}"));
    }

    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("🪪 个人角色卡 · {}（{}）", p.character_name, p.role_desc),
        body: sections.join("\n\n"),
        actions,
    })
}

fn mini_meter(cur: i32, max: i32, filled: &str, empty: &str) -> String {
    let max = max.max(1);
    let clamped = cur.clamp(0, max);
    let slots = 5usize;
    let lit = (((clamped as f32 / max as f32) * slots as f32).round() as usize).min(slots);
    format!("{}{} `{cur}/{max}`", filled.repeat(lit), empty.repeat(slots - lit))
}
