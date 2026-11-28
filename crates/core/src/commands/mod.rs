pub mod button_roles;
pub mod buttons;
pub mod char_vault;
pub mod check;
pub mod dnd;
pub mod export;
pub mod gm;
pub mod m1;
pub mod ooc;
pub mod party;
pub mod party_helpers;
pub mod room;
pub mod table;
pub mod turn;
pub mod turn_support;
pub mod workshop;
pub mod workshop_ops;

use anyhow::Result;
use tavern_domain::{ChatMode, EngineResponse, IncomingMessage, TurnPhase};
use tracing::debug;

use crate::engine::GameEngine;
pub use ooc::{OocPrecheck, is_plain_roleplay_ooc, precheck_roleplay_ooc};

pub async fn dispatch_message(
    engine: &GameEngine,
    msg: IncomingMessage,
    skip_ooc_gate: bool,
) -> Result<EngineResponse> {
    if msg.is_direct {
        if !engine.cfg.is_dm_allowed(&msg.user_id) {
            debug!(user_id = %msg.user_id, "私聊拦截：白名单开启时仅管理员允许私聊互动");
            return Ok(EngineResponse::Ignore);
        }
    } else {
        let eff_guild = resolve_authoritative_guild(engine, msg.guild_id.as_deref(), &msg.channel_id)?;
        if !table::is_channel_allowed_or_managed(engine, eff_guild.as_deref(), &msg.channel_id) {
            debug!(guild_id = ?eff_guild, channel_id = %msg.channel_id, "白名单拦截：忽略非授权服务器/频道消息");
            return Ok(EngineResponse::Ignore);
        }
    }

    let trimmed = msg.content.trim();
    if trimmed.is_empty() {
        return Ok(EngineResponse::Ignore);
    }

    if !msg.is_direct
        && let Some(gid) = msg.guild_id.as_deref().filter(|s| !s.is_empty())
    {
        let _ = engine.store.backfill_room_guild_id(&msg.channel_id, gid);
    }

    if let Some(rest) = trimmed
        .strip_prefix('>')
        .or_else(|| trimmed.strip_prefix('》'))
    {
        let action = rest.trim();
        if action.is_empty() {
            return Ok(EngineResponse::Ignore);
        }
        return party::cmd_act(engine, &msg, action).await;
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
    if !room_state.active || room_state.ended_id.is_some() {
        return Ok(EngineResponse::Ignore);
    }

    match room_state.chat_mode {
        ChatMode::Strict => Ok(EngineResponse::Ignore),
        ChatMode::Roleplay => {
            if trimmed.starts_with('(') || trimmed.starts_with('（') || trimmed.starts_with("//") {
                return Ok(EngineResponse::Ignore);
            }
            if !skip_ooc_gate
                && !msg.is_direct
                && ooc::evaluate_roleplay_ooc(engine, &room_state, &msg.user_name, trimmed).await
            {
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
        "scenarios" | "list" | "menu" | "大厅" | "剧本" => room::cmd_list_scenarios(engine, &msg.channel_id),
        "table" | "开桌" | "包间" => table::cmd_table(engine, msg, arg)?,
        "tables" | "包间列表" => table::cmd_tables(engine, msg)?,
        "closetable" | "拆桌" | "拆包间" => table::handle_close_table_ask(engine, msg)?,
        "pause" | "等人" | "飙戏" => table::cmd_pause(engine, &msg.channel_id, arg)?,
        "roles" | "职业" => button_roles::cmd_role_menu(engine, &msg.channel_id)?,
        "start" | "open" | "开团" => room::cmd_start(engine, msg, arg)?,
        "join" | "card" | "车卡" | "入团" => party::cmd_join(engine, msg, arg)?,
        "act" | "a" | "行动" => party::cmd_act(engine, msg, arg).await?,
        "roll" | "r" | "掷骰" => party::cmd_roll(engine, msg, arg)?,
        "check" | "ck" | "检定" | "属性" => check::cmd_check(engine, msg, arg)?,
        "rule" | "规则" => dnd::cmd_rule(engine, msg, arg)?,
        "skill" | "sk" | "技能" | "绝活" => dnd::cmd_skill(engine, msg, arg)?,
        "rest" | "休整" | "短休" | "长休" => {
            let eff_arg = if cmd == "长休" && arg.is_empty() { "long" } else if cmd == "短休" && arg.is_empty() { "short" } else { arg };
            dnd::cmd_rest(engine, msg, eff_arg)?
        }
        "inspire" | "激励" => dnd::cmd_inspire(engine, msg, arg)?,
        "assist" | "协助" => dnd::cmd_assist(engine, msg, arg)?,
        "levelup" | "lv" | "升级" | "成长" => dnd::cmd_levelup(engine, msg, arg)?,
        "char" | "角色库" | "车卡库" => char_vault::cmd_char(engine, msg, arg)?,
        "store" | "商店" | "世界书" => workshop::cmd_store(engine, &msg.channel_id, arg),
        "workshop" | "ws" | "工坊" | "创作" => workshop::cmd_workshop(engine, msg, arg).await?,
        "me" | "my" | "我的" | "角色卡" => check::cmd_me(engine, msg)?,
        "endings" | "图鉴" | "成就" | "结局" => check::cmd_endings(engine, msg)?,
        "go" | "resolve" | "next" | "结算" => turn::resolve_turn(engine, &msg.channel_id).await?,
        "status" | "st" | "状态" => room::cmd_status(engine, &msg.channel_id)?,
        "mode" | "模式" => room::cmd_mode(engine, &msg.channel_id, arg)?,
        "style" | "风格" | "档位" => m1::cmd_style(engine, &msg.channel_id, arg)?,
        "afk" | "暂离" | "摸鱼" => m1::cmd_afk(engine, msg).await?,
        "leave" | "quit" | "退团" => m1::cmd_leave(engine, msg)?,
        "whisper" | "纸条" | "秘密" => m1::cmd_whisper(engine, msg)?,
        "cost" | "账单" | "费用" => m1::cmd_cost(engine, &msg.channel_id)?,
        "web" | "看板" | "大屏" => m1::cmd_web(engine, &msg.channel_id)?,
        "hud" | "浮窗" | "悬浮窗" => m1::cmd_hud(engine, msg)?,
        "reload" | "重载" | "重载剧本" => m1::cmd_reload(engine, msg)?,
        "gm" | "房主" => gm::cmd_gm(engine, msg, arg)?,
        "undo" | "回档" => turn::cmd_undo(engine, &msg.channel_id)?,
        "summary" | "recap" | "story" | "前情" | "案情" | "简报" | "剧情" => {
            let caller = (!msg.is_direct).then_some(msg.user_id.as_str());
            room::cmd_summary(engine, &msg.channel_id, caller)?
        }
        "resend" | "补发" | "重发" => turn_support::cmd_resend(engine, &msg.channel_id)?,
        "export" | "log" | "导出" | "战报" => export::cmd_export(engine, &msg.channel_id, msg.is_direct)?,
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
    if let Some(g) = engine.store.get_room(channel_id)?.map(|r| r.guild_id).filter(|s| !s.is_empty()) {
        return Ok(Some(g));
    }
    Ok(engine.store.get_managed_table(channel_id)?.map(|t| t.guild_id).filter(|s| !s.is_empty()))
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
        if !engine.cfg.is_dm_allowed(user_id) { return false; }
    } else {
        let eff_guild = resolve_authoritative_guild(engine, guild_id, channel_id).ok().flatten();
        if !table::is_channel_allowed_or_managed(engine, eff_guild.as_deref(), channel_id) {
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
    if let Some((_, cmd, arg)) = parse_known_or_strict_command(trimmed)
        && matches!(cmd.as_str(), "workshop" | "ws" | "工坊" | "创作")
        && matches!(arg.split_whitespace().next().unwrap_or("").to_ascii_lowercase().as_str(), "forge" | "gen" | "锻造" | "生成")
    {
        return workshop_ops::is_forge_placeholder_ready(engine, user_id);
    }
    let Ok(Some(room)) = engine.store.get_room(channel_id) else {
        return false;
    };
    if !room.active || room.turn_phase == TurnPhase::Resolving || room.ended_id.is_some() {
        return false;
    }

    let mut is_afk_toggle = false;
    if let Some((_, cmd, arg)) = parse_known_or_strict_command(trimmed) {
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

    if !engine.cfg.auto_resolve_all_ready || room.manual_resolve {
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

pub(crate) fn is_known_command_name(cmd: &str) -> bool {
    matches!(
        cmd,
        "help" | "h" | "帮助" | "scenarios" | "list" | "menu" | "大厅" | "剧本"
            | "roles" | "职业" | "start" | "open" | "开团" | "join" | "card" | "车卡" | "入团"
            | "act" | "a" | "行动" | "roll" | "r" | "掷骰" | "check" | "ck" | "检定" | "属性"
            | "rule" | "规则" | "skill" | "sk" | "技能" | "绝活" | "rest" | "休整" | "短休" | "长休"
            | "inspire" | "激励" | "assist" | "协助" | "levelup" | "lv" | "升级" | "成长"
            | "char" | "角色库" | "车卡库" | "store" | "商店" | "世界书"
            | "workshop" | "ws" | "工坊" | "创作"
            | "me" | "my" | "我的" | "角色卡" | "endings" | "图鉴" | "成就" | "结局"
            | "go" | "resolve" | "next" | "结算" | "status" | "st" | "状态" | "mode" | "模式"
            | "style" | "风格" | "档位" | "afk" | "暂离" | "摸鱼" | "leave" | "quit" | "退团"
            | "whisper" | "纸条" | "秘密" | "cost" | "账单" | "费用" | "web" | "看板" | "大屏" | "hud" | "浮窗" | "悬浮窗"
            | "reload" | "重载" | "重载剧本" | "gm" | "房主" | "undo" | "回档"
            | "summary" | "recap" | "story" | "前情" | "案情" | "简报" | "剧情" | "resend" | "补发" | "重发" | "export" | "log" | "导出" | "战报"
            | "table" | "开桌" | "包间" | "tables" | "包间列表" | "closetable" | "拆桌" | "拆包间" | "pause" | "等人" | "飙戏"
            | "reset" | "end" | "结团"
    )
}

/// Returns `Some((strict_slash, lower_cmd, arg))` only when `input` uses a strict `/` prefix
/// OR uses a soft prefix (`!`, `！`, `.`, `。`, `、`) with a recognized command name.
/// Soft-prefixed natural speech (e.g. `...我退后`, `！笑死`) returns `None` so RP/OOC flows uniformly.
pub(crate) fn parse_known_or_strict_command(input: &str) -> Option<(bool, String, &str)> {
    let (strict_slash, body) = strip_command_prefix(input)?;
    let mut parts = body.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let arg = parts.next().unwrap_or("").trim();
    if strict_slash || is_known_command_name(&cmd) {
        Some((strict_slash, cmd, arg))
    } else {
        None
    }
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
