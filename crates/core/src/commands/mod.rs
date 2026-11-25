pub mod button_roles;
pub mod buttons;
pub mod export;
pub mod gm;
pub mod m1;
pub mod party;
pub mod party_helpers;
pub mod room;
pub mod turn;
pub mod turn_support;

use anyhow::Result;
use tavern_domain::{ChatMode, EngineResponse, IncomingMessage, TurnPhase};
use tracing::debug;

use crate::engine::GameEngine;

pub async fn dispatch_message(engine: &GameEngine, msg: IncomingMessage) -> Result<EngineResponse> {
    if msg.is_direct {
        if !engine.cfg.is_dm_allowed(&msg.user_id) {
            debug!(user_id = %msg.user_id, "私聊拦截：白名单开启时仅管理员允许私聊互动");
            return Ok(EngineResponse::Ignore);
        }
    } else if !engine
        .cfg
        .is_scope_allowed(msg.guild_id.as_deref(), &msg.channel_id)
    {
        debug!(
            guild_id = ?msg.guild_id,
            channel_id = %msg.channel_id,
            "白名单拦截：忽略非授权服务器/频道消息"
        );
        return Ok(EngineResponse::Ignore);
    }

    let trimmed = msg.content.trim();
    if trimmed.is_empty() {
        return Ok(EngineResponse::Ignore);
    }

    if let Some(rest) = trimmed
        .strip_prefix('>')
        .or_else(|| trimmed.strip_prefix('》'))
    {
        let action = rest.trim();
        if !action.is_empty() {
            return party::cmd_act(engine, &msg, action).await;
        }
    }

    if let Some((strict_slash, cmd_body)) = strip_command_prefix(trimmed) {
        let mut parts = cmd_body.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("").trim().to_ascii_lowercase();
        let arg = parts.next().unwrap_or("").trim();

        if let Some(resp) = match_known_command(engine, &msg, &cmd, arg).await? {
            return Ok(resp);
        }
        if strict_slash {
            return Ok(EngineResponse::Ignore);
        }
    }

    let Some(room_state) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Ignore);
    };
    if !room_state.active {
        return Ok(EngineResponse::Ignore);
    }

    match room_state.chat_mode {
        ChatMode::Strict => Ok(EngineResponse::Ignore),
        ChatMode::Roleplay => {
            if trimmed.starts_with('(') || trimmed.starts_with('（') || trimmed.starts_with("//") {
                return Ok(EngineResponse::Ignore);
            }
            party::cmd_act(engine, &msg, trimmed).await
        }
    }
}

async fn match_known_command(
    engine: &GameEngine,
    msg: &IncomingMessage,
    cmd: &str,
    arg: &str,
) -> Result<Option<EngineResponse>> {
    let resp = match cmd {
        "help" | "h" | "帮助" => room::cmd_help(),
        "scenarios" | "list" | "menu" | "大厅" | "剧本" => {
            room::cmd_list_scenarios(engine, &msg.channel_id)
        }
        "roles" | "职业" => button_roles::cmd_role_menu(engine, &msg.channel_id)?,
        "start" | "open" | "开团" => room::cmd_start(engine, msg, arg)?,
        "join" | "card" | "车卡" | "入团" => party::cmd_join(engine, msg, arg)?,
        "act" | "a" | "行动" => party::cmd_act(engine, msg, arg).await?,
        "roll" | "r" | "掷骰" => party::cmd_roll(engine, msg, arg)?,
        "go" | "resolve" | "next" | "结算" => turn::resolve_turn(engine, &msg.channel_id).await?,
        "status" | "st" | "状态" => room::cmd_status(engine, &msg.channel_id)?,
        "mode" | "模式" => room::cmd_mode(engine, &msg.channel_id, arg)?,
        "style" | "风格" | "档位" => m1::cmd_style(engine, &msg.channel_id, arg)?,
        "afk" | "暂离" | "摸鱼" => m1::cmd_afk(engine, msg).await?,
        "leave" | "quit" | "退团" => m1::cmd_leave(engine, msg)?,
        "whisper" | "纸条" | "秘密" => m1::cmd_whisper(engine, msg)?,
        "cost" | "账单" | "费用" => m1::cmd_cost(engine, &msg.channel_id)?,
        "web" | "看板" | "大屏" => m1::cmd_web(engine, &msg.channel_id)?,
        "gm" | "房主" => gm::cmd_gm(engine, msg, arg)?,
        "undo" | "回档" => turn::cmd_undo(engine, &msg.channel_id)?,
        "summary" | "recap" | "前情" => room::cmd_summary(engine, &msg.channel_id)?,
        "export" | "log" | "导出" | "战报" => {
            export::cmd_export(engine, &msg.channel_id, msg.is_direct)?
        }
        "reset" | "end" | "结团" => room::cmd_reset(engine, msg)?,
        _ => return Ok(None),
    };
    Ok(Some(resp))
}

pub(crate) fn resolve_authoritative_guild(
    engine: &GameEngine,
    provided_guild: Option<&str>,
    channel_id: &str,
) -> Result<Option<String>> {
    if let Some(g) = provided_guild.filter(|s| !s.is_empty()) {
        return Ok(Some(g.to_string()));
    }
    Ok(engine
        .store
        .get_room(channel_id)?
        .map(|r| r.guild_id)
        .filter(|s| !s.is_empty()))
}

pub fn will_likely_resolve_turn(
    engine: &GameEngine,
    guild_id: Option<&str>,
    channel_id: &str,
    user_id: &str,
    content: &str,
    is_direct: bool,
) -> bool {
    if is_direct {
        if !engine.cfg.is_dm_allowed(user_id) {
            return false;
        }
    } else {
        let eff_guild = resolve_authoritative_guild(engine, guild_id, channel_id)
            .ok()
            .flatten();
        if !engine
            .cfg
            .is_scope_allowed(eff_guild.as_deref(), channel_id)
        {
            return false;
        }
    }

    if turn_support::is_daily_budget_exhausted(engine) {
        return false;
    }

    let trimmed = content.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(Some(room)) = engine.store.get_room(channel_id) else {
        return false;
    };
    if !room.active || room.turn_phase == TurnPhase::Resolving {
        return false;
    }

    let mut is_afk_toggle = false;
    if let Some((_, body)) = strip_command_prefix(trimmed) {
        let mut parts = body.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
        let arg = parts.next().unwrap_or("").trim();
        if matches!(cmd.as_str(), "go" | "resolve" | "next" | "结算") {
            return engine
                .store
                .list_pending_actions(channel_id, room.turn_number + 1)
                .map(|v| !v.is_empty())
                .unwrap_or(false);
        }
        if matches!(cmd.as_str(), "afk" | "暂离" | "摸鱼") {
            is_afk_toggle = true;
        } else if matches!(cmd.as_str(), "act" | "a" | "行动") {
            if arg.is_empty() {
                return false;
            }
        } else {
            return false;
        }
    } else if let Some(rest) = trimmed
        .strip_prefix('>')
        .or_else(|| trimmed.strip_prefix('》'))
    {
        if rest.trim().is_empty() {
            return false;
        }
    } else if room.chat_mode != ChatMode::Roleplay
        || trimmed.starts_with('(')
        || trimmed.starts_with('（')
        || trimmed.starts_with("//")
    {
        return false;
    }

    if !engine.cfg.auto_resolve_all_ready {
        return false;
    }
    let Ok(players) = engine.store.list_players(channel_id) else {
        return false;
    };
    if is_afk_toggle {
        // Only triggers auto-resolve if caller is currently non-AFK and all other active players are ready
        let caller_active = players
            .iter()
            .any(|p| p.user_id == user_id && !p.is_afk);
        if !caller_active {
            return false;
        }
    }
    let active_others: Vec<_> = players
        .iter()
        .filter(|p| !p.is_afk && p.user_id != user_id)
        .collect();
    if active_others.is_empty() {
        return !is_afk_toggle;
    }
    let Ok(pending) = engine
        .store
        .list_pending_actions(channel_id, room.turn_number + 1)
    else {
        return false;
    };
    active_others.iter().all(|p| {
        pending
            .iter()
            .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty())
    })
}

pub fn strip_command_prefix(input: &str) -> Option<(bool, &str)> {
    if let Some(rest) = input
        .strip_prefix('/')
        .or_else(|| input.strip_prefix('／'))
    {
        let body = rest.trim();
        if !body.is_empty() && !body.starts_with('/') {
            return Some((true, body));
        }
        return None;
    }

    input
        .strip_prefix('、')
        .or_else(|| input.strip_prefix('!'))
        .or_else(|| input.strip_prefix('！'))
        .or_else(|| input.strip_prefix('.'))
        .or_else(|| input.strip_prefix('。'))
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| (false, s))
}
