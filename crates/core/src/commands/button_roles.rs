use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, Player, RoomEvent};

use crate::commands::party;
use crate::commands::party_helpers::unique_character_name;
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
    if let Some(existing) = engine.store.get_player(&msg.channel_id, &msg.user_id)? {
        if existing.is_afk {
            let _ = engine
                .store
                .set_player_afk(&msg.channel_id, &msg.user_id, false);
        }
        return Ok(EngineResponse::Notice {
            title: format!("⚔️ 你已在队伍中：{}", existing.character_name),
            body: format!(
                "职业：**{}** ｜ ❤️ HP {}/{} ｜ 🧠 SAN {}/{}\n\n想换角色名或特质，随时发 `/join 新名字 新身份 | 特质1,特质2` 即可！",
                existing.role_desc, existing.hp, existing.max_hp, existing.san, existing.max_san
            ),
            theme: CardTheme::Info,
            show_actions: true,
        });
    }
    let existing_players = engine.store.list_players(&msg.channel_id)?;
    let candidate = unique_character_name(&existing_players, &msg.user_name);
    party::cmd_join(engine, msg, &format!("{candidate} 旅途冒险者 | 机警,果断"))
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
    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Ignore);
    };
    let Some(preset) = scenario.preset_roles.get(idx) else {
        return Ok(EngineResponse::Ignore);
    };

    // Existing players keep their authoritative HP/SAN/inventory (prevents item farming!)
    if let Some(existing) = engine.store.get_player(&msg.channel_id, &msg.user_id)? {
        let updated = engine
            .store
            .update_player_profile(
                &msg.channel_id,
                &msg.user_id,
                &existing.character_name,
                &preset.name,
                &preset.traits,
            )?
            .unwrap_or(existing);
        return Ok(EngineResponse::Notice {
            title: format!("🎭 职业已切换：{} · {}", updated.character_name, preset.name),
            body: format!(
                "🏷️ 新特质：{}\n❤️ 当前 HP {}/{} ｜ 🧠 SAN {}/{}（保留原血量与背包，不重复发放初始道具喵！）",
                preset.traits.join(" / "),
                updated.hp,
                updated.max_hp,
                updated.san,
                updated.max_san
            ),
            theme: CardTheme::Success,
            show_actions: true,
        });
    }

    let existing_players = engine.store.list_players(&msg.channel_id)?;
    let character_name = unique_character_name(&existing_players, &msg.user_name);
    let mut inventory = scenario.player_defaults.starter_items.clone();
    for bonus in &preset.bonus_items {
        if !inventory.iter().any(|i| i == bonus) {
            inventory.push(bonus.clone());
        }
    }
    let player = Player {
        channel_id: msg.channel_id.clone(),
        user_id: msg.user_id.clone(),
        character_name: character_name.clone(),
        role_desc: preset.name.clone(),
        traits: preset.traits.clone(),
        hp: scenario.player_defaults.max_hp,
        max_hp: scenario.player_defaults.max_hp,
        san: scenario.player_defaults.max_san,
        max_san: scenario.player_defaults.max_san,
        inventory: inventory.clone(),
        status_note: "精神饱满".to_string(),
        is_afk: false,
    };
    engine.store.upsert_player(&player)?;
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: room.turn_number,
    });

    Ok(EngineResponse::Notice {
        title: format!("⚔️ 一键选卡入团成功：{}（{}）", character_name, preset.name),
        body: format!(
            "🏷️ 特质：**{}**\n❤️ HP {}/{} ｜ 🧠 SAN {}/{}\n🎒 初始行囊（含职业专属装备）：{}\n\n直接点剧情卡上的 ①②③ 按钮，或直接发消息即可开始行动喵！",
            preset.traits.join(" / "),
            player.hp,
            player.max_hp,
            player.san,
            player.max_san,
            inventory.join(" · ")
        ),
        theme: CardTheme::Success,
        show_actions: true,
    })
}
