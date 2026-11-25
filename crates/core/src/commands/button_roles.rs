use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, RoomEvent};

use crate::commands::check::ended_room_notice;
use crate::engine::GameEngine;

pub fn cmd_role_menu(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 还没开团呢".to_string(),
            body: "先点绿色按钮开启剧本，再挑选心仪职业上桌喵！".to_string(),
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
    if let Some(notice) = ended_room_notice(&room) {
        return Ok(notice);
    }
    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 找不到剧本配置".to_string(),
            body: "剧本文件缺失，请通知房主检查。".to_string(),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };
    if scenario.preset_roles.is_empty() {
        return Ok(EngineResponse::Notice {
            title: "🎭 当前剧本未配置预设职业".to_string(),
            body: "直接发送 `/join 角色名 职业 | 特质1,特质2` 自由捏人即可喵！".to_string(),
            theme: CardTheme::Info,
            show_actions: true,
        });
    }
    Ok(EngineResponse::RolePicker {
        channel_id: channel_id.to_string(),
        scenario_name: scenario.name,
        roles: scenario.preset_roles,
    })
}

pub fn handle_quick_join(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
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
    if let Some(notice) = ended_room_notice(&room) {
        return Ok(notice);
    }
    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Ignore);
    };
    let Some(player) = engine.store.ensure_player_atomic(
        &msg.channel_id,
        &msg.user_id,
        &msg.user_name,
        &scenario.player_defaults,
    )?
    else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已结束或归档".to_string(),
            body: "请重新开启新冒险后再入团。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: room.turn_number,
    });
    Ok(EngineResponse::Notice {
        title: format!("⚔️ 已就位：{}", player.character_name),
        body: format!(
            "职业：**{}** ｜ 📊 {}\n❤️ HP {}/{} ｜ 🧠 SAN {}/{}\n\n想挑带专属装备的预设职业，点面板里的「🎭 选职业入团」即可；直接打人话或点 ①②③ 即可行动！",
            player.role_desc,
            player.stats.format_inline(),
            player.hp,
            player.max_hp,
            player.san,
            player.max_san
        ),
        theme: CardTheme::Success,
        show_actions: true,
    })
}

pub fn handle_pick_role(
    engine: &GameEngine,
    msg: &IncomingMessage,
    idx: usize,
) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 还没开团呢".to_string(),
            body: "请先开启剧本再选职业喵！".to_string(),
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
    if let Some(notice) = ended_room_notice(&room) {
        return Ok(notice);
    }
    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Ignore);
    };
    let Some(preset) = scenario.preset_roles.get(idx) else {
        return Ok(EngineResponse::Ignore);
    };

    let Some((player, switched)) = engine.store.pick_role_atomic(
        &msg.channel_id,
        &msg.user_id,
        &msg.user_name,
        preset,
        &scenario.player_defaults,
    )?
    else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已结束或归档".to_string(),
            body: "请开启新冒险后再选职业。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: room.turn_number,
    });

    if switched {
        return Ok(EngineResponse::Notice {
            title: format!("🎭 职业已切换：{} · {}", player.character_name, preset.name),
            body: format!(
                "🏷️ 新特质：{}\n📊 五维属性：{}\n❤️ 当前 HP {}/{} ｜ 🧠 SAN {}/{}（保留原血量与背包，不重复发放初始道具喵！）",
                preset.traits.join(" / "),
                player.stats.format_inline(),
                player.hp,
                player.max_hp,
                player.san,
                player.max_san
            ),
            theme: CardTheme::Success,
            show_actions: true,
        });
    }

    Ok(EngineResponse::Notice {
        title: format!("⚔️ 一键选卡入团成功：{}（{}）", player.character_name, preset.name),
        body: format!(
            "🏷️ 特质：**{}**\n📊 五维属性：{}\n❤️ HP {}/{} ｜ 🧠 SAN {}/{}\n🎒 初始行囊（含职业专属装备）：{}\n\n直接点剧情卡上的 ①②③ 按钮，或直接发消息即可开始行动喵！",
            preset.traits.join(" / "),
            player.stats.format_inline(),
            player.hp,
            player.max_hp,
            player.san,
            player.max_san,
            player.inventory.join(" · ")
        ),
        theme: CardTheme::Success,
        show_actions: true,
    })
}
