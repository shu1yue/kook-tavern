use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, PrivateButton, RoomEvent};

use crate::commands::{resolve_authoritative_guild, room};
use crate::engine::GameEngine;

pub const MAX_MANAGED_TABLES_PER_GUILD: usize = 6;
pub const TABLE_CREATE_COOLDOWN_SECS: u64 = 30;

/// Authorizes either statically configured channels OR bot-managed table channels
/// belonging to an authorized guild (preventing `ALLOWED_CHANNEL_IDS` from blocking new tables).
pub(crate) fn is_channel_allowed_or_managed(
    engine: &GameEngine,
    eff_guild: Option<&str>,
    channel_id: &str,
) -> bool {
    if engine.cfg.is_scope_allowed(eff_guild, channel_id) {
        return true;
    }
    if let Ok(Some(tbl)) = engine.store.get_managed_table(channel_id) {
        return engine.cfg.allowed_guild_ids.is_empty()
            || engine.cfg.allowed_guild_ids.contains(&tbl.guild_id);
    }
    false
}

pub fn cmd_table(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    if msg.is_direct {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 私聊无需开独立包间".into(),
            body: "私聊窗口本来就是你的专属单人桌，直接发 `/start [剧本ID]` 开玩即可喵！".into(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }

    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Ok(room::cmd_list_scenarios(engine, &msg.channel_id));
    }

    let Some(scenario) = engine.scenarios.get(trimmed) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 未找到该剧本".into(),
            body: format!("找不到剧本 ID `{trimmed}`，先发 `/menu` 挑选剧本喵！"),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };

    let Some(guild_id) =
        resolve_authoritative_guild(engine, msg.guild_id.as_deref(), &msg.channel_id)?
    else {
        return Ok(EngineResponse::Notice {
            title: "⚠️ 无法识别当前服务器".into(),
            body: "请在服务器文字频道内使用 `/table <剧本ID>` 或点击菜单上的「🚪 开独立包间」按钮喵！"
                .into(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    let cooldown = engine
        .store
        .user_table_cooldown_remaining(&msg.user_id, TABLE_CREATE_COOLDOWN_SECS)?;
    if cooldown > 0 {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "⏳ 开包间冷却中".into(),
            body: format!("为了防止刷屏建频，每人每 {TABLE_CREATE_COOLDOWN_SECS} 秒最多新开 1 间包间，请 **{cooldown} 秒**后再试喵！"),
            actions: Vec::new(),
        });
    }

    let active_tables = engine.store.list_guild_managed_tables(&guild_id)?;
    if active_tables.len() >= MAX_MANAGED_TABLES_PER_GUILD {
        let links = active_tables
            .iter()
            .map(|t| format!("• (chn){}(chn) — 《{}》", t.channel_id, t.scenario_name))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(EngineResponse::Notice {
            title: format!("🚪 本服包间已达上限（{}/{MAX_MANAGED_TABLES_PER_GUILD}）", active_tables.len()),
            body: format!(
                "当前服务器已开启的独立包间：\n{links}\n\n请先进入已跑完的包间点「🗑️ 拆掉包间」收桌，再开新包间喵！"
            ),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }

    Ok(EngineResponse::CreateTableRequest {
        lobby_channel_id: msg.channel_id.clone(),
        guild_id,
        scenario_id: scenario.id,
        scenario_name: scenario.name,
        creator_user_id: msg.user_id.clone(),
        creator_user_name: msg.user_name.clone(),
    })
}

pub fn cmd_tables(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    if msg.is_direct {
        return Ok(EngineResponse::Notice {
            title: "🚪 独立包间列表".into(),
            body: "请在服务器文字频道内使用 `/tables` 查看本服进行中的跑团包间喵！".into(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }
    let Some(guild_id) =
        resolve_authoritative_guild(engine, msg.guild_id.as_deref(), &msg.channel_id)?
    else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 无法识别当前服务器".into(),
            body: "请在服务器文字频道内发送 `/tables` 查看包间列表喵。".into(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    let tables = engine.store.list_guild_managed_tables(&guild_id)?;
    if tables.is_empty() {
        return Ok(EngineResponse::Notice {
            title: format!("🚪 本服包间列表（0/{MAX_MANAGED_TABLES_PER_GUILD}）"),
            body: "当前暂无机器人创建的独立跑团包间。发 `/menu` 后点任意剧本的 **「🚪 开独立包间」** 即可秒开全新干净子频道喵！".into(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }
    let lines = tables
        .iter()
        .map(|t| {
            let status = match engine.store.get_room(&t.channel_id).ok().flatten() {
                Some(r) if r.ended_id.is_some() => format!(
                    "已结局：{}",
                    r.ended_title.as_deref().unwrap_or("已完结")
                ),
                Some(r) => format!("第 {} 回合 · {}", r.turn_number, r.location),
                None => "空桌待拆".to_string(),
            };
            format!("• (chn){}(chn) — **《{}》**（{status}）", t.channel_id, t.scenario_name)
        })
        .collect::<Vec<_>>()
        .join("\n");
    Ok(EngineResponse::Notice {
        title: format!("🚪 本服包间列表（{}/{MAX_MANAGED_TABLES_PER_GUILD}）", tables.len()),
        body: format!("{lines}\n\n点上方频道蓝字即可直接传送进桌；跑完后在包间面板点「🗑️ 拆掉包间」即可一键收桌喵！"),
        theme: CardTheme::Primary,
        show_actions: false,
    })
}

pub fn handle_close_table_ask(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(tbl) = engine.store.get_managed_table(&msg.channel_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🛡️ 安全护栏：不可删除非机器人包间".into(),
            body: "当前频道不是机器人创建的独立跑团包间，系统已硬锁拒绝删除人类频道！\n如需重开，请使用「🛑 结束本局」清空进度即可喵。".into(),
            actions: Vec::new(),
        });
    };
    let room = engine.store.get_room(&msg.channel_id)?;
    let room_gm = room.as_ref().map(|r| r.gm_user_id.as_str()).unwrap_or("");
    let allowed = engine.cfg.is_gm_or_admin(&msg.user_id, room_gm)
        || msg.user_id == tbl.creator_user_id;
    if !allowed {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🔒 仅房主、包间创建人或管理员可拆掉包间".into(),
            body: "拆掉包间会彻底删除本子频道与本局进度，请由本局房主或管理员确认操作喵！".into(),
            actions: Vec::new(),
        });
    }
    let title = room
        .as_ref()
        .map(|r| r.title.as_str())
        .unwrap_or(&tbl.scenario_name);
    let turn_hint = room
        .as_ref()
        .map(|r| format!("当前进度：第 **{}** 回合。", r.turn_number))
        .unwrap_or_else(|| "当前包间内暂无进行中存档。".to_string());
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("🗑️ 确认拆掉《{title}》独立包间？"),
        body: format!(
            "{turn_hint}\n确认后将**永久删除本 KOOK 子频道**并清空本局存档（绝不影响大厅和其他包间）。如果想留纪念，建议先点「📜 先发战报墙」喵！"
        ),
        actions: vec![
            PrivateButton {
                label: "🗑️ 确认拆掉并删频道".into(),
                value: format!("tavern:close_table_confirm:{}", msg.channel_id),
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

pub fn handle_close_table_confirm(
    engine: &GameEngine,
    msg: &IncomingMessage,
) -> Result<EngineResponse> {
    let Some(tbl) = engine.store.get_managed_table(&msg.channel_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🛡️ 安全护栏：拒绝删除非托管频道".into(),
            body: "该频道不在机器人包间台账中（或已被拆除），已安全中止删除喵。".into(),
            actions: Vec::new(),
        });
    };
    let room = engine.store.get_room(&msg.channel_id)?;
    let room_gm = room.as_ref().map(|r| r.gm_user_id.as_str()).unwrap_or("");
    let allowed = engine.cfg.is_gm_or_admin(&msg.user_id, room_gm)
        || msg.user_id == tbl.creator_user_id;
    if !allowed {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🔒 权限不足".into(),
            body: "仅本局房主、包间创建人或管理员可拆掉该包间喵。".into(),
            actions: Vec::new(),
        });
    }
    let title = room
        .as_ref()
        .map(|r| r.title.clone())
        .unwrap_or_else(|| tbl.scenario_name.clone());
    engine.store.delete_room(&msg.channel_id)?;
    engine.store.delete_managed_table(&msg.channel_id)?;
    engine.emit_event(RoomEvent::RoomReset {
        channel_id: msg.channel_id.clone(),
    });
    Ok(EngineResponse::CloseTableRequest {
        channel_id: msg.channel_id.clone(),
        lobby_channel_id: tbl.lobby_channel_id,
        title,
        closed_by_name: msg.user_name.clone(),
    })
}

pub fn cmd_pause(engine: &GameEngine, channel_id: &str, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".into(),
            body: "请先开团后再切换手动结算 / 等人飙戏模式喵！".into(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };
    if !room.active || room.ended_id.is_some() {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前房间不可切换".into(),
            body: "房间已结束或已归档，请开启新局喵。".into(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }
    let next = match arg.trim().to_ascii_lowercase().as_str() {
        "" => !room.manual_resolve,
        "on" | "1" | "true" | "pause" | "manual" | "开" | "手动" | "暂停" | "等人" | "飙戏" => true,
        "off" | "0" | "false" | "auto" | "resume" | "关" | "自动" | "恢复" => false,
        _ => {
            return Ok(EngineResponse::Notice {
                title: "⏸️ 手动结算 / 自由飙戏模式用法".into(),
                body: format!(
                    "当前状态：**{}**\n- `/pause` 或 `/等人` 或 `/飙戏` — 一键切换开关\n- `/pause on` — 暂停自动结算，人齐也不会打断聊天，点 **⚡ 立刻结算** 或 `/go` 才由 DM 出卡\n- `/pause off` — 恢复全员就绪自动结算",
                    if room.manual_resolve { "⏸️ 手动结算（自由飙戏中）" } else { "▶️ 自动结算（人齐即出卡）" }
                ),
                theme: CardTheme::Info,
                show_actions: false,
            });
        }
    };
    engine.store.update_manual_resolve(channel_id, next)?;
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: channel_id.to_string(),
        turn_number: room.turn_number,
    });
    if next {
        Ok(EngineResponse::Notice {
            title: "⏸️ 已开启手动结算（等人 / 自由飙戏模式）".into(),
            body: "现在就算全员都发了言，DM 也不会中途抢话！大家可以在本频道自由等队友进桌或角色内飙戏，聊完随时点 **「⚡ 立刻结算」** 或发 `/go` 一起结算喵！".into(),
            theme: CardTheme::Info,
            show_actions: true,
        })
    } else {
        Ok(EngineResponse::Notice {
            title: "▶️ 已恢复自动结算模式".into(),
            body: "全员提交本轮行动后，DM 将自动汇总并推进下一轮剧情喵！".into(),
            theme: CardTheme::Success,
            show_actions: true,
        })
    }
}
