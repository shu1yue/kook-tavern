use anyhow::Result;
use tavern_domain::{
    CardTheme, ChatMode, EngineResponse, IncomingMessage, NarrativeStyle, PrivateButton, StatKind,
    TurnPhase,
};

use crate::commands::{
    button_roles, check, dnd, export, m1, party, resolve_authoritative_guild, room, turn,
    will_likely_resolve_turn, workshop,
};
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
    if let Some(gid) = effective_guild.as_deref().filter(|s| !s.is_empty()) {
        let _ = engine.store.backfill_room_guild_id(channel_id, gid);
    }

    let parts: Vec<&str> = value.splitn(5, ':').collect();
    if parts.len() < 2 || parts[0] != "tavern" {
        return Ok(EngineResponse::Ignore);
    }
    let action = parts[1];
    let mk_msg = |content: String| IncomingMessage {
        guild_id: effective_guild.clone(),
        channel_id: channel_id.to_string(),
        user_id: user_id.to_string(),
        user_name: user_name.to_string(),
        content,
        is_direct: false,
    };
    if parts.len() == 2 {
        return match action {
            "menu" => Ok(room::cmd_list_scenarios(engine, channel_id)),
            "endings" => check::cmd_endings(engine, &mk_msg("/endings".into())),
            "help" => Ok(room::cmd_help()),
            "store" => Ok(workshop::cmd_store(engine, channel_id, "")),
            "workshop" => workshop::cmd_workshop(engine, &mk_msg("/workshop".into()), "").await,
            _ => Ok(EngineResponse::Ignore),
        };
    }
    let payload_chan = parts[2];
    if payload_chan.is_empty() || payload_chan != channel_id {
        return Ok(EngineResponse::Ignore);
    }

    match action {
        "resolve" => turn::resolve_turn(engine, channel_id).await,
        "status" => room::cmd_status(engine, channel_id),
        "undo" => turn::cmd_undo(engine, channel_id),
        "cost" => m1::cmd_cost(engine, channel_id),
        "web" => m1::cmd_web(engine, channel_id),
        "me" => check::cmd_me(engine, &mk_msg("/me".into())),
        "endings" => check::cmd_endings(engine, &mk_msg("/endings".into())),
        "help" => Ok(room::cmd_help()),
        "menu" => Ok(room::cmd_list_scenarios(engine, channel_id)),
        "rolemenu" => button_roles::cmd_role_menu(engine, channel_id),
        "export" => export::cmd_export(engine, channel_id, false),
        "start" => {
            let scenario_id = parts.get(3).copied().unwrap_or("").trim();
            room::cmd_start(engine, &mk_msg(format!("/start {scenario_id}")), scenario_id)
        }
        "quickjoin" => button_roles::handle_quick_join(engine, &mk_msg("/join".into())),
        "pickrole" => {
            let Some(idx) = parts.get(3).and_then(|s| s.parse::<usize>().ok()) else {
                return Ok(EngineResponse::Ignore);
            };
            button_roles::handle_pick_role(engine, &mk_msg("/join".into()), idx)
        }
        "whisper" => m1::cmd_whisper(engine, &mk_msg("/whisper".into())),
        "afk" => m1::cmd_afk(engine, &mk_msg("/afk".into())).await,
        "leave" => m1::cmd_leave(engine, &mk_msg("/leave".into())),
        "reset_ask" => handle_reset_ask(engine, &mk_msg("/reset".into())),
        "reset_confirm" => room::cmd_reset(engine, &mk_msg("/reset".into())),
        "roll" => {
            let expr = parts.get(3).copied().unwrap_or("");
            if !matches!(expr, "2d6" | "1d100" | "1d20") {
                return Ok(EngineResponse::Ignore);
            }
            party::cmd_roll(engine, &mk_msg(format!("/roll {expr}")), expr)
        }
        "check" => {
            let stat_raw = parts.get(3).copied().unwrap_or("");
            let Some(stat) = StatKind::parse(stat_raw) else {
                return Ok(EngineResponse::Ignore);
            };
            let dc_arg = parts
                .get(4)
                .and_then(|s| s.parse::<i32>().ok())
                .map(|d| format!(" dc{}", d.clamp(5, 25)))
                .unwrap_or_default();
            let arg_str = format!("{}{dc_arg}", stat.key());
            check::cmd_check(engine, &mk_msg(format!("/check {arg_str}")), &arg_str)
        }
        "rest" => {
            let kind = parts.get(3).copied().unwrap_or("short");
            dnd::cmd_rest(engine, &mk_msg(format!("/rest {kind}")), kind)
        }
        "skill" => {
            let idx = parts.get(3).copied().unwrap_or("").trim();
            dnd::cmd_skill(engine, &mk_msg(format!("/skill {idx}")), idx)
        }
        "lv" => {
            let choice = parts.get(3).copied().unwrap_or("").trim();
            if !matches!(choice, "hp" | "charge") {
                return Ok(EngineResponse::Ignore);
            }
            dnd::cmd_levelup(engine, &mk_msg(format!("/levelup {choice}")), choice)
        }
        "store" => {
            let pack_id = parts.get(3).copied().unwrap_or("").trim();
            Ok(workshop::cmd_store(engine, channel_id, pack_id))
        }
        "ws" => {
            let sub = parts.get(3).copied().unwrap_or("desk").trim();
            let extra = parts.get(4).copied().unwrap_or("").trim();
            let arg = if extra.is_empty() { sub.to_string() } else { format!("{sub} {extra}") };
            workshop::cmd_workshop(engine, &mk_msg(format!("/workshop {arg}")), &arg).await
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
        "ws" if matches!(parts.get(3).copied(), Some("forge" | "gen" | "锻造" | "生成")) => {
            will_likely_resolve_turn(engine, guild_id, channel_id, user_id, "/workshop forge", false)
        }
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
                || room.ended_id.is_some()
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

async fn handle_pick_action(
    engine: &GameEngine,
    msg: &IncomingMessage,
    expected_turn: u32,
    idx: usize,
) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先点「📚 换剧本」开启冒险吧喵！".to_string(),
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
    if let Some(notice) = check::ended_room_notice(&room) {
        return Ok(notice);
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

fn handle_reset_ask(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "ℹ️ 当前频道没有进行中的剧本".into(),
            body: "房间本来就是空的，随时可以点「📚 换剧本」或发 `/start` 开新桌喵！".into(),
            actions: vec![PrivateButton {
                label: "📚 挑选新剧本".into(),
                value: format!("tavern:menu:{}", msg.channel_id),
                theme: CardTheme::Success,
            }],
        });
    };
    if !engine.cfg.is_gm_or_admin(&msg.user_id, &room.gm_user_id) {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🔒 仅房主或管理员可结束整局".into(),
            body: format!(
                "《{}》正在进行中，结束整局清空进度需要由开团房主或管理员操作。\n如果你本人有事想先撤，可以点下方按钮单独退团离桌，不影响队友继续玩喵！",
                room.title
            ),
            actions: vec![PrivateButton {
                label: "🚪 仅自己退团离桌".into(),
                value: format!("tavern:leave:{}", msg.channel_id),
                theme: CardTheme::Warning,
            }],
        });
    }
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("⚠️ 确认结束《{}》并清桌？", room.title),
        body: format!(
            "当前进度：第 **{}** 回合。\n确认后将清空本频道的当前冒险进度并撤掉旧卡按钮，随后可直接重新选本开局喵！（若想留纪念，可先点「📜 先发战报墙」）",
            room.turn_number
        ),
        actions: vec![
            PrivateButton {
                label: "🛑 确认结束并清桌".into(),
                value: format!("tavern:reset_confirm:{}", msg.channel_id),
                theme: CardTheme::Danger,
            },
            PrivateButton {
                label: "📜 先发战报墙".into(),
                value: format!("tavern:export:{}", msg.channel_id),
                theme: CardTheme::Success,
            },
        ],
    })
}
