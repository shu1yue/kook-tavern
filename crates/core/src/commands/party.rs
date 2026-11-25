use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, Player, RoomEvent, roll_dice};

use crate::commands::party_helpers::{ensure_player, format_party_progress, parse_join_args};
use crate::commands::turn;
use crate::engine::GameEngine;

pub fn cmd_join(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "请先发送 `/start` 开启一场冒险，再建卡入团喵！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    if !room.active {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已归档".to_string(),
            body: "请发送 `/start` 开启新冒险。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }

    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 剧本配置缺失".to_string(),
            body: format!("找不到剧本 `{}` 的 YAML 定义。", room.scenario_id),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };

    let (character_name, role_desc, traits) = parse_join_args(arg, &msg.user_name);

    // Reject duplicate character_name owned by a different user to avoid whisper/patch misrouting
    let all_players = engine.store.list_players(&msg.channel_id)?;
    if all_players.iter().any(|p| {
        p.user_id != msg.user_id && p.character_name.eq_ignore_ascii_case(&character_name)
    }) {
        return Ok(EngineResponse::Notice {
            title: "⚠️ 角色名已被队友占用".to_string(),
            body: format!(
                "房间里已经有队友叫 **{character_name}** 啦，换个独特点的名字防止 DM 递错纸条喵！"
            ),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }

    let existing = engine.store.get_player(&msg.channel_id, &msg.user_id)?;
    let player = if let Some(prev) = existing {
        let next_role = if role_desc.is_empty() {
            prev.role_desc
        } else {
            role_desc
        };
        let next_traits = if traits.is_empty() {
            prev.traits
        } else {
            traits
        };
        // Targeted profile update ONLY — never clobbers HP/SAN/inventory committed concurrently!
        engine
            .store
            .update_player_profile(
                &msg.channel_id,
                &msg.user_id,
                &character_name,
                &next_role,
                &next_traits,
            )?
            .unwrap_or(Player {
                character_name,
                role_desc: next_role,
                traits: next_traits,
                is_afk: false,
                ..prev
            })
    } else {
        let p = Player {
            channel_id: msg.channel_id.clone(),
            user_id: msg.user_id.clone(),
            character_name,
            role_desc: if role_desc.is_empty() {
                "初来乍到的冒险者".to_string()
            } else {
                role_desc
            },
            traits,
            hp: scenario.player_defaults.max_hp,
            max_hp: scenario.player_defaults.max_hp,
            san: scenario.player_defaults.max_san,
            max_san: scenario.player_defaults.max_san,
            inventory: scenario.player_defaults.starter_items,
            status_note: "精神饱满".to_string(),
            is_afk: false,
        };
        engine.store.upsert_player(&p)?;
        p
    };
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: room.turn_number,
    });

    let traits_text = if player.traits.is_empty() {
        "无".to_string()
    } else {
        player.traits.join("、")
    };
    let inv_text = if player.inventory.is_empty() {
        "空".to_string()
    } else {
        player.inventory.join("、")
    };

    Ok(EngineResponse::Notice {
        title: format!("🪪 角色卡已就绪：{}", player.character_name),
        body: format!(
            "- **身份**：{}\n- **特质**：{}\n- **状态**：❤️ HP {}/{} ｜ 🧠 SAN {}/{}\n- **初始背包**：{}\n\n现在可以发送 `/act 你的行动` 或 `> 你的行动` 开始演出了！",
            player.role_desc,
            traits_text,
            player.hp,
            player.max_hp,
            player.san,
            player.max_san,
            inv_text
        ),
        theme: CardTheme::Success,
        show_actions: true,
    })
}

pub async fn cmd_act(
    engine: &GameEngine,
    msg: &IncomingMessage,
    action_text: &str,
) -> Result<EngineResponse> {
    let action_text = action_text.trim();
    if action_text.is_empty() {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 请输入行动内容".to_string(),
            body: "用法：`/act 我凑近壁炉，试探着向老酒保打听那张悬赏令`（或简写 `> 行动内容`）"
                .to_string(),
            theme: CardTheme::Info,
            show_actions: true,
        });
    }

    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先发 `/start` 开一局群酒馆冒险吧！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    if !room.active {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间已归档".to_string(),
            body: "请发送 `/start` 开启新团。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }

    let Some(scenario) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 剧本配置缺失".to_string(),
            body: format!("找不到剧本 `{}`。", room.scenario_id),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };

    let player = ensure_player(engine, msg, &scenario)?;

    // Single atomic DB transaction eliminates TOCTOU race with `try_begin_resolving`!
    let Some((target_turn, is_resolving)) = engine.store.submit_action_atomic(
        &msg.channel_id,
        &msg.user_id,
        &player.character_name,
        action_text,
    )?
    else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先发 `/start` 开一局群酒馆冒险吧！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    if is_resolving {
        return Ok(EngineResponse::Notice {
            title: format!("⏳ 第 {} 轮正在结算中 · 已暂存至下一轮", target_turn - 1),
            body: format!(
                "【{}】的行动已安全放入**第 {} 轮缓冲区**，等本轮剧情输出后会自动生效，绝不会被吞掉喵！\n> {}",
                player.character_name, target_turn, action_text
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }

    let all_players = engine.store.list_players(&msg.channel_id)?;
    let active_players: Vec<&Player> = all_players.iter().filter(|p| !p.is_afk).collect();
    let all_pending = engine
        .store
        .list_pending_actions(&msg.channel_id, target_turn)?;
    let ready_active = active_players
        .iter()
        .filter(|p| {
            all_pending
                .iter()
                .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty())
        })
        .count();

    if engine.cfg.auto_resolve_all_ready
        && !active_players.is_empty()
        && ready_active >= active_players.len()
    {
        return turn::resolve_turn(engine, &msg.channel_id).await;
    }

    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: room.turn_number,
    });

    Ok(EngineResponse::Notice {
        title: format!(
            "📝 已收录【{}】的第 {} 轮行动 ({}/{})",
            player.character_name,
            target_turn,
            ready_active,
            active_players.len().max(1)
        ),
        body: format!(
            "> {}\n\n**全队进度**：{}\n人齐将自动结算；如有队友摸鱼，可随时发 `/go` 或点下方按钮立刻结算。",
            action_text,
            format_party_progress(&all_players, &all_pending)
        ),
        theme: CardTheme::Secondary,
        show_actions: true,
    })
}

pub fn cmd_roll(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let mut parts = arg.splitn(2, char::is_whitespace);
    let expr = parts.next().unwrap_or("d100").trim();
    let expr = if expr.is_empty() { "d100" } else { expr };
    let reason = parts
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let result = match roll_dice(expr, reason) {
        Ok(r) => r,
        Err(e) => {
            return Ok(EngineResponse::Notice {
                title: "❌ 掷骰格式有误".to_string(),
                body: e.to_string(),
                theme: CardTheme::Danger,
                show_actions: false,
            });
        }
    };

    let formatted = result.format_short();
    let mut attached_note = String::new();
    let existing_player = engine.store.get_player(&msg.channel_id, &msg.user_id)?;

    if let Some(ref player) = existing_player
        && let Some(target_turn) = engine.store.append_roll_atomic(
            &msg.channel_id,
            &msg.user_id,
            &player.character_name,
            &formatted,
        )?
    {
        attached_note = format!(
            "\n\n（已将该确定性掷骰结果自动绑定到第 {target_turn} 轮行动中，结算时 DM 会严格按此点数裁决）"
        );
        engine.emit_event(RoomEvent::RoomUpdated {
            channel_id: msg.channel_id.clone(),
            turn_number: target_turn.saturating_sub(1),
        });
    }

    let display_name = existing_player
        .map(|p| p.character_name)
        .unwrap_or_else(|| msg.user_name.clone());

    Ok(EngineResponse::Notice {
        title: format!("🎲 {} 的掷骰结果", display_name),
        body: format!("{formatted}{attached_note}"),
        theme: CardTheme::Info,
        show_actions: false,
    })
}
