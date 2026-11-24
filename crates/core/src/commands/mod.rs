pub mod export;
pub mod party;
pub mod party_helpers;
pub mod room;
pub mod turn;

use anyhow::Result;
use tavern_domain::{ChatMode, EngineResponse, IncomingMessage};

use crate::engine::GameEngine;

pub async fn dispatch_message(engine: &GameEngine, msg: IncomingMessage) -> Result<EngineResponse> {
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
        // Only hard-ignore unknown commands starting with `/` or `／`;
        // in Roleplay mode, sentences starting with `...` / `！` / `。` / `、` fall through to IC!
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
        "scenarios" | "list" | "剧本" => room::cmd_list_scenarios(engine),
        "start" | "open" | "开团" => room::cmd_start(engine, msg, arg)?,
        "join" | "card" | "车卡" | "入团" => party::cmd_join(engine, msg, arg)?,
        "act" | "a" | "行动" => party::cmd_act(engine, msg, arg).await?,
        "roll" | "r" | "掷骰" => party::cmd_roll(engine, msg, arg)?,
        "go" | "resolve" | "next" | "结算" => turn::resolve_turn(engine, &msg.channel_id).await?,
        "status" | "st" | "状态" => room::cmd_status(engine, &msg.channel_id)?,
        "mode" | "模式" => room::cmd_mode(engine, &msg.channel_id, arg)?,
        "undo" | "回档" => turn::cmd_undo(engine, &msg.channel_id)?,
        "summary" | "recap" | "前情" => room::cmd_summary(engine, &msg.channel_id)?,
        "export" | "log" | "导出" => export::cmd_export(engine, &msg.channel_id)?,
        "reset" | "end" | "结团" => room::cmd_reset(engine, &msg.channel_id)?,
        _ => return Ok(None),
    };
    Ok(Some(resp))
}

pub async fn dispatch_button(
    engine: &GameEngine,
    channel_id: &str,
    value: &str,
) -> Result<EngineResponse> {
    let parts: Vec<&str> = value.splitn(3, ':').collect();
    if parts.len() < 2 || parts[0] != "tavern" {
        return Ok(EngineResponse::Ignore);
    }
    // Never allow forged button payloads from another channel to mutate a different channel's room!
    if let Some(payload_chan) = parts.get(2)
        && !payload_chan.is_empty()
        && *payload_chan != channel_id
    {
        return Ok(EngineResponse::Ignore);
    }

    match parts[1] {
        "resolve" => turn::resolve_turn(engine, channel_id).await,
        "status" => room::cmd_status(engine, channel_id),
        "undo" => turn::cmd_undo(engine, channel_id),
        _ => Ok(EngineResponse::Ignore),
    }
}

/// Returns `(is_strict_slash, remaining_body)`.
pub fn strip_command_prefix(input: &str) -> Option<(bool, &str)> {
    if let Some(rest) = input
        .strip_prefix('/')
        .or_else(|| input.strip_prefix('／'))
    {
        let body = rest.trim();
        // Keep `//` as OOC comment rather than empty slash command
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
