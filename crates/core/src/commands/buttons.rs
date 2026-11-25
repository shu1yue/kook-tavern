use anyhow::Result;
use tavern_domain::{CardTheme, ChatMode, EngineResponse, IncomingMessage, NarrativeStyle, TurnPhase};

use crate::commands::{m1, party, resolve_authoritative_guild, room, turn, will_likely_resolve_turn};
use crate::engine::GameEngine;

pub async fn dispatch_button(
    engine: &GameEngine,
    guild_id: Option<&str>,
    channel_id: &str,
    user_id: &str,
    user_name: &str,
    value: &str,
) -> Result<EngineResponse> {
    let effective_guild = resolve_authoritative_guild(engine, guild_id, channel_id)?;
    if !engine
        .cfg
        .is_scope_allowed(effective_guild.as_deref(), channel_id)
    {
        return Ok(EngineResponse::Ignore);
    }

    let parts: Vec<&str> = value.splitn(5, ':').collect();
    if parts.len() < 3 || parts[0] != "tavern" {
        return Ok(EngineResponse::Ignore);
    }
    let action = parts[1];
    let payload_chan = parts[2];
    if payload_chan.is_empty() || payload_chan != channel_id {
        return Ok(EngineResponse::Ignore);
    }

    let mk_msg = |content: String| IncomingMessage {
        guild_id: effective_guild.clone(),
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        user_name: user_name.to_string(),
        content,
        is_direct: false,
    };

    match action {
        "resolve" => turn::resolve_turn(engine, channel_id).await,
        "status" => room::cmd_status(engine, channel_id),
        "undo" => turn::cmd_undo(engine, channel_id),
        "cost" => m1::cmd_cost(engine, channel_id),
        "help" => Ok(room::cmd_help()),
        "menu" => Ok(room::cmd_list_scenarios(engine, channel_id)),
        "start" => {
            let scenario_id = parts.get(3).copied().unwrap_or("").trim();
            let msg = mk_msg(format!("/start {scenario_id}"));
            room::cmd_start(engine, &msg, scenario_id)
        }
        "quickjoin" => handle_quick_join(engine, &mk_msg("/join".into())),
        "whisper" => m1::cmd_whisper(engine, &mk_msg("/whisper".into())),
        "afk" => m1::cmd_afk(engine, &mk_msg("/afk".into())).await,
        "roll" => {
            let expr = parts.get(3).copied().unwrap_or("");
            if !matches!(expr, "2d6" | "1d100" | "1d20") {
                return Ok(EngineResponse::Ignore);
            }
            party::cmd_roll(engine, &mk_msg(format!("/roll {expr}")), expr)
        }
        "pick" => {
            let Some(expected_turn) = parts.get(3).and_then(|s| s.parse::<u32>().ok()) else {
                return Ok(EngineResponse::Ignore);
            };
            let Some(idx) = parts.get(4).and_then(|s| s.parse::<usize>().ok()) else {
                return Ok(EngineResponse::Ignore);
            };
            handle_pick_action(engine, &mk_msg(String::new()), expected_turn, idx).await
        }
        "toggle_mode" => handle_toggle_mode(engine, channel_id),
        "toggle_style" => handle_toggle_style(engine, channel_id),
        _ => Ok(EngineResponse::Ignore),
    }
}

pub fn will_button_likely_resolve_turn(
    engine: &GameEngine,
    guild_id: Option<&str>,
    channel_id: &str,
    user_id: &str,
    value: &str,
) -> bool {
    let parts: Vec<&str> = value.splitn(5, ':').collect();
    if parts.len() < 3 || parts[0] != "tavern" || parts[2] != channel_id {
        return false;
    }
    match parts[1] {
        "resolve" => will_likely_resolve_turn(engine, guild_id, channel_id, user_id, "/go", false),
        "afk" => will_likely_resolve_turn(engine, guild_id, channel_id, user_id, "/afk", false),
        "pick" => {
            let Some(expected_turn) = parts.get(3).and_then(|s| s.parse::<u32>().ok()) else {
                return false;
            };
            let Some(idx) = parts.get(4).and_then(|s| s.parse::<usize>().ok()) else {
                return false;
            };
            let Ok(Some(room)) = engine.store.get_room(channel_id) else {
                return false;
            };
            if !room.active
                || room.turn_phase == TurnPhase::Resolving
                || room.turn_number != expected_turn
                || idx >= room.suggested_actions.len()
            {
                return false;
            }
            will_likely_resolve_turn(engine, guild_id, channel_id, user_id, "/act pick", false)
        }
        _ => false,
    }
}

fn handle_quick_join(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 还没开团呢".to_string(),
            body: "先点上方的「▶️ 开启剧本」按钮开一桌，再点快速入团喵！".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    if !room.active {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已归档".to_string(),
            body: "请点「📚 换剧本」开启新冒险喵！".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }

    if let Some(existing) = engine.store.get_player(&msg.channel_id, &msg.user_id)? {
        if existing.is_afk {
            let _ = engine
                .store
                .set_player_afk(&msg.channel_id, &msg.user_id, false);
        }
        return Ok(EngineResponse::Notice {
            title: format!("⚔️ 你已在队伍中：{}", existing.character_name),
            body: format!(
                "职业：**{}** ｜ ❤️ HP {}/{} ｜ 🧠 SAN {}/{}\n\n想换角色名或特质，随时发 `/join 新名字 新身份 | 特质1,特质2` 即可（不会清零血量和背包喵）！",
                existing.role_desc, existing.hp, existing.max_hp, existing.san, existing.max_san
            ),
            theme: CardTheme::Info,
            show_actions: true,
        });
    }

    let existing_players = engine.store.list_players(&msg.channel_id)?;
    let base_name: String = msg
        .user_name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|' && *c != '｜')
        .take(16)
        .collect();
    let base = if base_name.is_empty() {
        "冒险者".to_string()
    } else {
        base_name
    };
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while existing_players
        .iter()
        .any(|p| p.character_name.eq_ignore_ascii_case(&candidate))
    {
        candidate = format!("{base}#{suffix}");
        suffix += 1;
    }

    let join_arg = format!("{candidate} 旅途冒险者 | 机警,果断");
    party::cmd_join(engine, msg, &join_arg)
}

async fn handle_pick_action(
    engine: &GameEngine,
    msg: &IncomingMessage,
    expected_turn: u32,
    idx: usize,
) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先点「📚 选剧本开团」开启冒险吧喵！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    if !room.active {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已归档".to_string(),
            body: "请重新开启新剧本喵！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }
    if room.turn_phase == TurnPhase::Resolving {
        return Ok(EngineResponse::Notice {
            title: "⏳ DM 正在推演本轮剧情".to_string(),
            body: "稍等几秒，等新剧情卡出来后再点新选项（也可以直接打字暂存下一轮行动）喵！"
                .to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }
    if room.turn_number != expected_turn {
        return Ok(EngineResponse::Notice {
            title: "🕒 这是上一轮的旧选项按钮".to_string(),
            body: format!(
                "当前已经推进到 **第 {} 回合** 啦，请翻到最下面点最新剧情卡上的选项按钮（或直接打字行动）喵！",
                room.turn_number
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }
    let Some(action_text) = room.suggested_actions.get(idx).cloned() else {
        return Ok(EngineResponse::Notice {
            title: "⚠️ 选项已失效".to_string(),
            body: "该快捷行动选项已更新，请直接发消息行动或点最新剧情卡喵。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    party::cmd_act(engine, msg, &action_text).await
}

fn handle_toggle_mode(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 还没开团".to_string(),
            body: "请先开团后再切换说话模式喵。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    let next = match room.chat_mode {
        ChatMode::Roleplay => "strict",
        ChatMode::Strict => "rp",
    };
    room::cmd_mode(engine, channel_id, next)
}

fn handle_toggle_style(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 还没开团".to_string(),
            body: "请先开团后再切换叙事档位喵。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    let next = match room.narrative_style {
        NarrativeStyle::Voice => "prose",
        NarrativeStyle::Prose => "voice",
    };
    m1::cmd_style(engine, channel_id, next)
}
