use anyhow::Result;
use chrono::Utc;
use rand::Rng;
use tavern_domain::{
    CardTheme, ChatMode, EngineResponse, IncomingMessage, NarrativeStyle, RoomEvent, RoomState,
    ScenarioSummary, TurnPhase, ending_scope_key,
};

use crate::commands::check::format_act_badge;
use crate::engine::GameEngine;

pub fn cmd_help() -> EngineResponse {
    let body = [
        "**✨ 零代码游玩方式（推荐！全程点按钮 + 说人话）**",
        "1. 点卡片上的 **「▶️ 开启剧本」** 开桌，点 **「🎭 选职业入团」** 带五维属性与专属装备参战",
        "2. 行动阶段：**直接点剧情卡上的 ①②③ 行动按钮**，或**直接打人话**提交行动（场外闲聊请加括号 `（）` 或 `//`）",
        "3. 检定、掷骰、查私密纸条、看 Web 大屏、结局图鉴、暂离与回档——全在卡片底部一键点击！",
        "",
        "**⌨️ 进阶文字快捷键（想精细操作时再用）**",
        "- `/menu` / `/roles` — 剧本大厅 / 职业选卡",
        "- `/me` / `/check 侦查 [adv|dc15|inspire] [原因]` — 个人角色卡 / 多规则检定",
        "- `/skill` / `/rest short|long` / `/assist <队友>` / `/levelup` — 招牌技 / 营地休整 / 队友协助 / 过幕升级",
        "- `/char` / `/rule d20|2d6|d100` / `/inspire <角色>` — 自由车卡库 / 切换 D&D 规则 / 颁激励点",
        "- `/endings` / `/web` / `/export` / `/reset` — 结局图鉴 / 实况大屏 / 发战报墙 / 结团",
    ]
    .join("\n");

    EngineResponse::Notice {
        title: "🍺 群酒馆 AI DM 玩法指南".to_string(),
        body,
        theme: CardTheme::Primary,
        show_actions: false,
    }
}

pub fn cmd_list_scenarios(engine: &GameEngine, channel_id: &str) -> EngineResponse {
    let list = engine.scenarios.list();
    if list.is_empty() {
        return EngineResponse::Notice {
            title: "📚 暂无可用剧本".to_string(),
            body: "当前剧本目录下未找到 YAML 剧本文件。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        };
    }

    let scenarios = list
        .into_iter()
        .map(|s| ScenarioSummary {
            id: s.id,
            name: s.name,
            genre: s.genre,
            tagline: s.tagline,
            worldbook_count: s.worldbook.len(),
        })
        .collect();

    EngineResponse::ScenarioMenu {
        channel_id: channel_id.to_string(),
        scenarios,
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
        if existing.active && existing.ended_id.is_none() {
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
        if existing.ended_id.is_some()
            && !engine.cfg.is_gm_or_admin(&msg.user_id, &existing.gm_user_id)
        {
            return Ok(EngineResponse::Notice {
                title: "🔒 请由本局房主开启下一局".to_string(),
                body: format!(
                    "《{}》已达成结局，当前保留着战报与回档记录。请由本局房主或管理员点按钮开启新剧本（或 `/reset` 清空）！",
                    existing.title
                ),
                theme: CardTheme::Warning,
                show_actions: false,
            });
        }
        // Clean archived or ended room cascade and revoke any lingering SSE streams
        engine.store.delete_room(&msg.channel_id)?;
        engine.emit_event(RoomEvent::RoomReset {
            channel_id: msg.channel_id.clone(),
        });
    }

    let mut opening_actions: Vec<String> = scenario
        .opening_actions
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(3)
        .collect();
    if opening_actions.is_empty() {
        opening_actions = vec![
            "观察四周环境并寻找关键人物".to_string(),
            "向面前最可疑的人搭话打听消息".to_string(),
        ];
    }

    let now = Utc::now();
    let effective_guild = if msg.is_direct { String::new() } else { msg.guild_id.clone().unwrap_or_default() };
    let room = RoomState {
        channel_id: msg.channel_id.clone(),
        guild_id: effective_guild.clone(),
        scenario_id: scenario.id.clone(),
        title: scenario.name.clone(),
        location: scenario.initial_location.clone(),
        world_time: scenario.initial_time.clone(),
        turn_number: 0,
        state_version: 1,
        turn_phase: TurnPhase::Collecting,
        chat_mode: ChatMode::Roleplay,
        narrative_style: NarrativeStyle::Prose,
        gm_user_id: msg.user_id.clone(),
        web_token: generate_web_token(),
        clues: Vec::new(),
        npc_states: Vec::new(),
        suggested_actions: opening_actions.clone(),
        current_act: 1,
        world_flags: Vec::new(),
        ended_id: None,
        ended_title: None,
        bulletins: Vec::new(),
        check_system: scenario.rules.check_system,
        last_rest_turn: 0,
        summary: String::new(),
        summary_upto_turn: 0,
        active: true,
        created_at: now,
        updated_at: now,
    };
    engine.store.upsert_room(&room)?;
    engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: 0 });

    let mode_hint = if msg.is_direct {
        "🕯️ 当前为私聊单人冒险模式（直接发消息或点按钮即可推进）".to_string()
    } else {
        "💬 已默认开启「直接说人话」模式：直接打字或点下方 ①②③ 按钮即行动！场外闲聊请加括号（）或 //，点「🎛️ 面板/设置」可随时切换。".to_string()
    };
    let act_badge = format_act_badge(Some(&scenario), 1);
    let scope_key = ending_scope_key(
        (!effective_guild.is_empty()).then_some(effective_guild.as_str()),
        &msg.channel_id,
    );
    let before_ts = room.created_at.to_rfc3339();
    let echoes = engine
        .store
        .list_world_echoes(&scope_key, &scenario.id, Some(&before_ts), 2)
        .unwrap_or_default();
    let mut opening_changes = vec![
        mode_hint,
        "✨ 先点「🎛️ 面板/设置」→「🎭 选职业入团」拿五维属性与专属装备，或直接点下方 ①②③ 按钮开玩！".to_string(),
    ];
    if let Some(latest_echo) = echoes.first() {
        opening_changes.push(format!(
            "🌀 **世界线轮回残响**（《{}》）：{}",
            latest_echo.ending_title, latest_echo.echo_text
        ));
    }

    Ok(EngineResponse::StoryTurn {
        channel_id: msg.channel_id.clone(),
        turn_number: 0,
        scene_title: format!("{} · 序幕", scenario.name),
        location: scenario.initial_location,
        world_time: scenario.initial_time,
        act_badge,
        narrative: scenario.opening_scene.trim().to_string(),
        state_changes: opening_changes,
        whisper_hints: Vec::new(),
        suggested_actions: opening_actions,
        suggested_check: None,
        world_bulletin: None,
        ending_reached: None,
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
    let scenario = engine.scenarios.get(&room.scenario_id);
    let act_label = format_act_badge(scenario.as_ref(), room.current_act);

    Ok(EngineResponse::StatusBoard {
        channel_id: channel_id.to_string(),
        room,
        act_label,
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
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: channel_id.to_string(),
        turn_number: room.turn_number,
    });

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
