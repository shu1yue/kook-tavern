use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, NarrativeStyle, RoomEvent};

use crate::commands::turn;
use crate::engine::GameEngine;

pub fn cmd_style(engine: &GameEngine, channel_id: &str, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "请先 `/start` 开团后再切换叙事档位。".to_string(),
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
    if let Some(notice) = crate::commands::check::ended_room_notice(&room) {
        return Ok(notice);
    }

    let Some(style) = NarrativeStyle::parse(arg) else {
        return Ok(EngineResponse::Notice {
            title: "🎙️ 当前叙事档位".to_string(),
            body: format!(
                "当前：**{}**\n\n可选切换：\n- `/style voice` — 语音开黑极简档（100~180字，边开麦边一眼扫完）\n- `/style prose` — 纯文字沉浸档（200~420字画面描写）",
                room.narrative_style.display_label()
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    engine.store.update_narrative_style(channel_id, style)?;
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: channel_id.to_string(),
        turn_number: room.turn_number,
    });

    Ok(EngineResponse::Notice {
        title: "✅ 叙事档位已切换".to_string(),
        body: format!("已切换为：**{}**（下轮结算立刻生效）", style.display_label()),
        theme: CardTheme::Success,
        show_actions: true,
    })
}

pub async fn cmd_afk(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "当前频道没有进行中的房间。".to_string(),
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
    if let Some(notice) = crate::commands::check::ended_room_notice(&room) {
        return Ok(notice);
    }
    let Some(existing) = engine.store.get_player(&msg.channel_id, &msg.user_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 你尚未入团".to_string(),
            body: "你目前不在本局参团名单中。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };

    let next_afk = !existing.is_afk;
    let Some(updated) = engine
        .store
        .set_player_afk(&msg.channel_id, &msg.user_id, next_afk)?
    else {
        return Ok(EngineResponse::Ignore);
    };

    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: msg.channel_id.clone(),
        turn_number: room.turn_number,
    });

    if next_afk {
        // ONLY auto-resolve if room is currently Collecting; during Resolving, return clean AFK confirmation!
        if room.turn_phase == tavern_domain::TurnPhase::Collecting
            && engine.cfg.auto_resolve_all_ready
        {
            let all_players = engine.store.list_players(&msg.channel_id)?;
            let active_players: Vec<_> = all_players.iter().filter(|p| !p.is_afk).collect();
            let pending = engine
                .store
                .list_pending_actions(&msg.channel_id, room.turn_number + 1)?;
            let ready = active_players
                .iter()
                .filter(|p| {
                    pending
                        .iter()
                        .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty())
                })
                .count();
            if !active_players.is_empty() && ready >= active_players.len() {
                return turn::resolve_turn(engine, &msg.channel_id).await;
            }
        }

        Ok(EngineResponse::Notice {
            title: format!("💤 【{}】已切换为暂离状态", updated.character_name),
            body: "自动结算将跳过你的行动进度，不会卡住队友；回来后直接 `/act` 或再敲 `/afk` 即可自动归队！".to_string(),
            theme: CardTheme::Info,
            show_actions: true,
        })
    } else {
        Ok(EngineResponse::Notice {
            title: format!("✨ 【{}】欢迎归队！", updated.character_name),
            body: "已恢复正常参团状态，现在可以继续提交 `/act` 行动啦。".to_string(),
            theme: CardTheme::Success,
            show_actions: true,
        })
    }
}

pub fn cmd_leave(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "当前没有可退出的房间。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    match engine.store.remove_player(&msg.channel_id, &msg.user_id)? {
        Some(removed) => {
            engine.emit_event(RoomEvent::RoomUpdated {
                channel_id: msg.channel_id.clone(),
                turn_number: room.turn_number,
            });
            Ok(EngineResponse::Notice {
                title: format!("👋 【{}】已退出本局冒险", removed.character_name),
                body: "已移除角色并清理待提交行动；随时可再发 `/join` 重新加入。".to_string(),
                theme: CardTheme::Info,
                show_actions: true,
            })
        }
        None => Ok(EngineResponse::Notice {
            title: "ℹ️ 你不在当前参团名单中".to_string(),
            body: "无需重复退出。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        }),
    }
}

pub fn cmd_whisper(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let whispers = engine
        .store
        .list_player_whispers(&msg.channel_id, &msg.user_id, 5)?;

    let body = if whispers.is_empty() {
        "你在本频道暂时还没有收到过 DM 的秘密纸条。".to_string()
    } else {
        whispers
            .into_iter()
            .map(|w| format!("**[第 {} 回合]**\n{}", w.turn_number, w.content))
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    };

    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: "🤫 你的专属秘密纸条档案（仅本人可见）".to_string(),
        body,
        actions: Vec::new(),
    })
}

pub fn cmd_cost(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let room_usage = engine.store.get_room_usage(channel_id)?;
    let today = engine.store.get_today_usage()?;

    let call_limit_str = if engine.cfg.daily_max_llm_calls == 0 { "不限".into() } else { engine.cfg.daily_max_llm_calls.to_string() };
    let cny_limit_str = if engine.cfg.daily_max_cny <= 0.0 { "不限".into() } else { format!("{:.2} 元", engine.cfg.daily_max_cny) };

    let body = format!(
        "**本频道累计统计**\n\
         - 模型调用次数：**{}** 次\n\
         - 输入 Tokens：**{}**（缓存命中 **{}** ｜ 未命中 **{}**）\n\
         - 前缀缓存命中率：**{:.1}%**\n\
         - 输出 Tokens：**{}**\n\
         - 预估花费：**{:.4} 元**\n\n\
         **今日全局安全配额（{} UTC）**\n\
         - 今日调用：**{} / {}** 次\n\
         - 今日预估花费：**{:.4} 元 / {}**",
        room_usage.total_calls,
        room_usage.prompt_tokens,
        room_usage.cache_hit_tokens,
        room_usage.cache_miss_tokens,
        room_usage.cache_hit_ratio_pct(),
        room_usage.completion_tokens,
        room_usage.estimated_cny,
        today.day_utc,
        today.calls,
        call_limit_str,
        today.estimated_cny,
        cny_limit_str
    );

    Ok(EngineResponse::Notice {
        title: "💰 DeepSeek 消费与缓存命中看板".to_string(),
        body,
        theme: CardTheme::Info,
        show_actions: false,
    })
}

pub fn cmd_reload(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    if !engine.cfg.is_admin(&msg.user_id) {
        return Ok(EngineResponse::Notice {
            title: "🔒 权限不足".to_string(),
            body: "只有配置文件 `ADMIN_USER_IDS` 中的管理员才能执行 `/reload` 热重载剧本库。".to_string(),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    }
    match engine.scenarios.reload() {
        Ok(count) => {
            let wb_note = match engine.worldbooks.reload() {
                Ok(n) => format!("{n} 套世界书"),
                Err(err) => format!("世界书重载失败：{err}"),
            };
            let names = engine
                .scenarios
                .list()
                .into_iter()
                .map(|s| format!("- **《{}》** (`{}`)", s.name, s.id))
                .collect::<Vec<_>>()
                .join("\n");
            Ok(EngineResponse::Notice {
                title: format!("🔄 剧本库热重载成功（共 {count} 个剧本 · {wb_note}）"),
                body: format!(
                    "已从磁盘原子刷新并全部通过强类型 Schema 校验，零重启即时生效！\n\n{names}\n\n现在可直接发 `/menu` 或 `/start <剧本ID>` 开新团。"
                ),
                theme: CardTheme::Success,
                show_actions: false,
            })
        }
        Err(err) => Ok(EngineResponse::Notice {
            title: "❌ 剧本热重载已安全中止（原线上剧本不受影响）".to_string(),
            body: format!(
                "检测到新剧本 YAML 格式或 Schema 校验未通过，已保留原有可用剧本库：\n> {err:#}"
            ),
            theme: CardTheme::Danger,
            show_actions: false,
        }),
    }
}

pub fn cmd_web(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "先开启一局冒险，再来点「🌐 Web 看板」获取本桌专属实况大屏链接喵！".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    if room.web_token.trim().is_empty() {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 旧版存档未生成看板凭证".to_string(),
            body: "本局为旧版本存档，重新开新团后即可使用实时 Web 看板喵！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }
    let Some(ref bind) = engine.cfg.web_bind_addr else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 伴生 Web 看板未启用".to_string(),
            body: "房主尚未在 `.env` 中配置 `WEB_BIND_ADDR`（如 `0.0.0.0:8787`）。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    let base = match engine.cfg.web_public_base_url.as_deref() {
        Some(url) => url.to_string(),
        None if bind.starts_with("0.0.0.0:") || bind.starts_with("[::]:") => {
            return Ok(EngineResponse::Notice {
                title: "⚙️ 请先配置公网访问前缀".to_string(),
                body: "看板服务已在监听，但 `.env` 尚未填写 `WEB_PUBLIC_BASE_URL`（例如 `http://你的服务器地址:8787`），配置后点此按钮即可生成可直达链接喵！".to_string(),
                theme: CardTheme::Warning,
                show_actions: false,
            });
        }
        None => format!("http://{bind}"),
    };
    let encoded_chan = room.channel_id.replace(':', "%3A");
    let url = format!("{base}/room/{encoded_chan}?token={}", room.web_token);
    Ok(EngineResponse::Notice {
        title: format!("🌐 《{}》实况只读 Web 看板", room.title),
        body: format!(
            "点击链接在手机或电脑副屏打开实况大屏（无需刷新，1 秒内同步血条/NPC/线索与剧情）：\n\n{url}\n\n🔒 **安全说明**：本链接仅支持只读观看，绝不泄露任何人的秘密纸条；本局 `/reset` 结团后旧链接自动失效喵！"
        ),
        theme: CardTheme::Info,
        show_actions: false,
    })
}

pub fn cmd_hud(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(), title: "🪟 个人悬浮 HUD".into(),
            body: "当前频道还没开团，先开一桌并选好职业再来打开专属悬浮窗喵！".into(), actions: vec![],
        });
    };
    let Some(p) = engine.store.get_player(&msg.channel_id, &msg.user_id)? else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(), title: format!("🪟 《{}》尚未入团", room.title),
            body: "请先点剧情卡上的职业按钮入团，再开启你的专属悬浮角色卡 HUD 喵！".into(), actions: vec![],
        });
    };
    let Some(url) = crate::hud_token::build_player_hud_url(&engine.cfg, &room, &msg.user_id) else {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(), title: "⚙️ 悬浮 HUD 尚未配置公网地址".into(),
            body: "服务端需配置 `WEB_BIND_ADDR` 与 `WEB_PUBLIC_BASE_URL` 后才可生成专属浮窗链接喵。".into(), actions: vec![],
        });
    };
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("🪟 {} 的个人实时悬浮 HUD", p.character_name),
        body: format!("点下方按钮打开专属悬浮窗（实时同步你的血量/五维/背包/私密纸条 + 人物关系图 + 线索百科，绝不剧透，结团自动失效）：\n\n{url}"),
        actions: vec![tavern_domain::PrivateButton { label: "🪟 打开悬浮 HUD".into(), value: url, theme: CardTheme::Success }],
    })
}
