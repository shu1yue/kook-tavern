use anyhow::Result;
use chrono::Utc;
use rand::Rng;
use tavern_domain::{
    CardTheme, ChatMode, EngineResponse, IncomingMessage, NarrativeStyle, RoomEvent, RoomState,
    TurnPhase,
};

use crate::engine::GameEngine;

pub fn cmd_help() -> EngineResponse {
    let body = [
        "**开团与准备**",
        "- `/scenarios` — 查看所有可选剧本",
        "- `/start [剧本ID]` — 在当前频道开新团（默认《余烬酒馆》）",
        "- `/join 角色名 身份 | 特质1,特质2` — 建卡入团（如 `/join 陆沉 流浪机械师 | 眼尖,手巧`）",
        "",
        "**游玩与回合推进**",
        "- `/act 你的行动` 或 `> 你的行动` — 提交本轮行动（结算中提交会自动暂存至下一轮）",
        "- `/roll 2d6+1 [原因]` 或 `.r d100 侦查` — 真随机掷骰，自动附带到本轮行动交给 DM 裁决",
        "- `/go`（或点卡片按钮 `⚡ 立刻结算本轮`）— 立刻打包全员行动交由 AI DM 结算",
        "- `/status` — 查看当前场景、全员 HP/SAN/背包、线索与本轮提交进度",
        "",
        "**秩序与工具**",
        "- `/style voice` / `/style prose` — 切换语音开黑极简档（100~180字）或纯文字沉浸档",
        "- `/mode strict` / `/mode rp` — 切换严格指令模式或沉浸 RP 模式（`(`/`（`/`//` 视为 OOC）",
        "- `/afk` / `/leave` — 暂离不卡全队自动结算 / 退出本局",
        "- `/whisper` — 仅本人可见地补领查看所有 DM 秘密纸条",
        "- `/cost` — 查看本房 DeepSeek 前缀缓存命中率、估算费用与今日限额",
        "- `/gm help` — 房主手动兜底修正血量/理智/道具/线索",
        "- `/undo` / `/summary` / `/export` / `/reset` — 回档 / 前情 / 导出战报 / 结团",
    ]
    .join("\n");

    EngineResponse::Notice {
        title: "🍺 群酒馆 AI DM 指令手册".to_string(),
        body,
        theme: CardTheme::Primary,
        show_actions: false,
    }
}

pub fn cmd_list_scenarios(engine: &GameEngine) -> EngineResponse {
    let list = engine.scenarios.list();
    if list.is_empty() {
        return EngineResponse::Notice {
            title: "📚 暂无可用剧本".to_string(),
            body: "当前剧本目录下未找到 YAML 剧本文件。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        };
    }

    let body = list
        .into_iter()
        .map(|s| {
            format!(
                "**{}** (`{}`)\n> {}｜{}\n> 世界书词条：{} 条｜开团指令：`/start {}`",
                s.name,
                s.id,
                s.genre,
                s.tagline,
                s.worldbook.len(),
                s.id
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    EngineResponse::Notice {
        title: "📚 可选冒险剧本列表".to_string(),
        body,
        theme: CardTheme::Info,
        show_actions: false,
    }
}

pub fn cmd_start(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let scenario = if arg.is_empty() {
        engine.scenarios.default_scenario()
    } else {
        engine.scenarios.get(arg)
    };

    let Some(scenario) = scenario else {
        return Ok(EngineResponse::Notice {
            title: "❌ 未找到该剧本".to_string(),
            body: format!("找不到剧本 ID `{arg}`，请先发送 `/scenarios` 查看可用剧本列表。"),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };

    if let Some(existing) = engine.store.get_room(&msg.channel_id)? {
        if existing.active {
            return Ok(EngineResponse::Notice {
                title: "⚠️ 当前频道已有进行中的房间".to_string(),
                body: format!(
                    "当前正在游玩《{}》（第 {} 回合）。\n如果想重开，请先发送 `/reset` 清空当前房间（建议先 `/export` 留档）。",
                    existing.title, existing.turn_number
                ),
                theme: CardTheme::Warning,
                show_actions: true,
            });
        }
        // Clean archived inactive room cascade before creating a fresh session
        engine.store.delete_room(&msg.channel_id)?;
    }

    let now = Utc::now();
    let room = RoomState {
        channel_id: msg.channel_id.clone(),
        guild_id: msg.guild_id.clone().unwrap_or_default(),
        scenario_id: scenario.id.clone(),
        title: scenario.name.clone(),
        location: scenario.initial_location.clone(),
        world_time: scenario.initial_time.clone(),
        turn_number: 0,
        state_version: 1,
        turn_phase: TurnPhase::Collecting,
        chat_mode: ChatMode::Strict,
        narrative_style: NarrativeStyle::Prose,
        gm_user_id: msg.user_id.clone(),
        web_token: generate_web_token(),
        clues: Vec::new(),
        summary: String::new(),
        summary_upto_turn: 0,
        active: true,
        created_at: now,
        updated_at: now,
    };
    engine.store.upsert_room(&room)?;
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: 0,
    });

    let mode_hint = if msg.is_direct {
        "🕯️ 当前为私聊单人冒险模式（支持随时 `/act` 单人推进）".to_string()
    } else {
        format!("🎛️ 发言模式：{}", room.chat_mode.display_label())
    };

    Ok(EngineResponse::StoryTurn {
        channel_id: msg.channel_id.clone(),
        turn_number: 0,
        scene_title: format!("{} · 序幕", scenario.name),
        location: scenario.initial_location,
        world_time: scenario.initial_time,
        narrative: scenario.opening_scene.trim().to_string(),
        state_changes: vec![
            mode_hint,
            "✨ 请各位玩家发送 `/join 角色名 身份 | 特质1,特质2` 建卡入团，再用 `/act 行动内容` 提交第 1 轮行动！".to_string(),
        ],
        whisper_hints: Vec::new(),
        suggested_actions: vec![
            "/join 你的角色名 职业身份 | 特质1,特质2".to_string(),
            "/act 观察四周并向附近的人打听情况".to_string(),
        ],
        direct_messages: Vec::new(),
    })
}

pub fn cmd_status(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "发送 `/start` 开启新冒险，或 `/scenarios` 查看剧本。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    let players = engine.store.list_players(channel_id)?;
    let pending_actions = engine
        .store
        .list_pending_actions(channel_id, room.turn_number + 1)?;

    Ok(EngineResponse::StatusBoard {
        channel_id: channel_id.to_string(),
        room,
        players,
        pending_actions,
    })
}

pub fn cmd_mode(engine: &GameEngine, channel_id: &str, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "请先 `/start` 开团后再切换发言模式。".to_string(),
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

    let Some(mode) = ChatMode::parse(arg) else {
        return Ok(EngineResponse::Notice {
            title: "🎛️ 当前发言模式".to_string(),
            body: format!(
                "当前：**{}**\n\n可选切换：\n- `/mode strict` — 仅 `/act` 或 `>` 进入行动，日常吹水全屏蔽\n- `/mode rp` — 直接打字即行动，带 `(` / `（` / `//` 视为场外 OOC",
                room.chat_mode.display_label()
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    // Targeted column update ONLY — never touches `turn_phase` or `state_version`!
    engine.store.update_chat_mode(channel_id, mode)?;

    Ok(EngineResponse::Notice {
        title: "✅ 发言模式已切换".to_string(),
        body: format!("已切换为：**{}**", mode.display_label()),
        theme: CardTheme::Success,
        show_actions: true,
    })
}

pub fn cmd_summary(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先 `/start` 开团吧！".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    let summary_text = if room.summary.trim().is_empty() {
        "目前仍在开篇短期记忆窗口内，尚未触发章节摘要压缩。".to_string()
    } else {
        format!(
            "（已压缩至第 {} 回合）\n\n{}",
            room.summary_upto_turn, room.summary
        )
    };

    let clues_text = if room.clues.is_empty() {
        "暂无".to_string()
    } else {
        room.clues
            .iter()
            .map(|c| format!("- {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(EngineResponse::Notice {
        title: format!("📖 《{}》前情提要", room.title),
        body: format!("{summary_text}\n\n**已掌握线索**：\n{clues_text}"),
        theme: CardTheme::Info,
        show_actions: true,
    })
}

pub fn cmd_reset(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道没有进行中的房间".to_string(),
            body: "直接发送 `/start` 即可开团。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    if !engine.cfg.is_gm_or_admin(&msg.user_id, &room.gm_user_id) {
        return Ok(EngineResponse::Notice {
            title: "🔒 权限不足".to_string(),
            body: "只有本局房主（开团人）或管理员才能执行 `/reset` 结团清空房间喵。".to_string(),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    }
    engine.store.delete_room(&msg.channel_id)?;
    engine.emit_event(RoomEvent::RoomReset {
        channel_id: msg.channel_id.clone(),
    });
    Ok(EngineResponse::Notice {
        title: "🧹 房间已结团清空".to_string(),
        body: "当前频道的冒险档案已归档清空，随时可发 `/start [剧本ID]` 开启下一场故事！".to_string(),
        theme: CardTheme::Success,
        show_actions: false,
    })
}

fn generate_web_token() -> String {
    const CHARSET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";
    let mut rng = rand::thread_rng();
    (0..16)
        .map(|_| CHARSET[rng.gen_range(0..CHARSET.len())] as char)
        .collect()
}
