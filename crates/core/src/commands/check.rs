use anyhow::Result;
use tavern_domain::{
    CardTheme, EngineResponse, IncomingMessage, RoomEvent, RoomState, Scenario, StatKind,
    ending_scope_key, perform_stat_check,
};

use crate::commands::party_helpers::ensure_player;
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
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先发送 `/start` 或点绿色开团按钮开一桌，再进行属性检定喵！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    if !room.active {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已归档".to_string(),
            body: "请开启新剧本后再检定。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }
    if let Some(notice) = ended_room_notice(&room) {
        return Ok(notice);
    }

    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 剧本配置缺失".to_string(),
            body: format!("找不到剧本 `{}`。", room.scenario_id),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };

    let Some(player) = ensure_player(engine, msg, &scenario)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间不可操作".to_string(),
            body: "房间已结束或被重置，请重新开团。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    let mut parts = arg.splitn(2, char::is_whitespace);
    let stat_raw = parts.next().unwrap_or("").trim();
    let reason = parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let Some(stat) = StatKind::parse(stat_raw) else {
        return Ok(EngineResponse::Notice {
            title: format!("🎲 {} 的五维属性与检定指南", player.character_name),
            body: format!(
                "**当前五维加值**：{}\n\n**用法**：`/check <属性> [原因]`（走后端确定性 `2d6 + 属性` 三档检定）\n- `体魄`（💪 搏斗/攀爬/抗毒）｜ `敏捷`（🏃 潜行/闪避/开锁）\n- `侦查`（🔍 搜证/察觉/识破）｜ `交涉`（🗣️ 说服/套话/安抚）\n- `灵感`（✨ 直觉/抗惊吓/破译规则）\n\n**三档裁决**：❌ `≤6` 失败 ｜ ⚠️ `7~9` 代价成功 ｜ ✨ `≥10` 完全成功\n例：`/check 侦查 翻找17号酒桶暗格`",
                player.stats.format_inline()
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    let result = perform_stat_check(stat, &player.stats, reason);
    let formatted = result.format_short();
    let Some(turn_no) = engine.store.append_roll_atomic(
        &msg.channel_id,
        &msg.user_id,
        &player.character_name,
        &formatted,
    )?
    else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已结束或归档".to_string(),
            body: "无法再向本局绑定检定，请查看结局图鉴或开启新局。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: turn_no.saturating_sub(1),
    });

    let theme = match result.tier {
        tavern_domain::CheckTier::Success => CardTheme::Success,
        tavern_domain::CheckTier::Partial => CardTheme::Warning,
        tavern_domain::CheckTier::Failure => CardTheme::Danger,
    };

    Ok(EngineResponse::Notice {
        title: format!(
            "{} {} · {}检定",
            stat.icon(),
            player.character_name,
            stat.short_name()
        ),
        body: format!(
            "{formatted}\n\n（已自动绑定到第 **{turn_no}** 轮行动中，接下来直接打字或点 ①②③ 行动按钮，DM 将严格按 **{}** 裁决！）",
            result.tier.rule_hint()
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
