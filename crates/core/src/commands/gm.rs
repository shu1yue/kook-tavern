use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, RoomEvent};

use crate::engine::GameEngine;
use crate::ports::GmUpdateOutcome;

pub fn cmd_gm(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "请先 `/start` 开团后再使用房主控场指令。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    if !engine.cfg.is_gm_or_admin(&msg.user_id, &room.gm_user_id) {
        return Ok(EngineResponse::Notice {
            title: "🔒 权限不足".to_string(),
            body: "只有本局房主（开团人）或管理员（`ADMIN_USER_IDS`）才能执行 `/gm` 手动修正指令喵。"
                .to_string(),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    }

    let mut parts = arg.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let rest = parts.next().unwrap_or("").trim();

    match sub.as_str() {
        "" | "help" | "帮助" => Ok(gm_help()),
        "hp" => mutate_numeric(engine, &msg.channel_id, room.turn_number, rest, true),
        "san" => mutate_numeric(engine, &msg.channel_id, room.turn_number, rest, false),
        "add" | "加道具" => mutate_item(engine, &msg.channel_id, room.turn_number, rest, true),
        "rm" | "del" | "删道具" => {
            mutate_item(engine, &msg.channel_id, room.turn_number, rest, false)
        }
        "clue" | "加线索" => {
            if rest.is_empty() {
                return Ok(usage_notice("用法：`/gm clue 线索内容`"));
            }
            match engine.store.gm_add_clue(&msg.channel_id, rest)? {
                GmUpdateOutcome::Updated(()) => {
                    engine.emit_event(RoomEvent::RoomUpdated {
                        channel_id: msg.channel_id.clone(),
                        turn_number: room.turn_number,
                    });
                    Ok(EngineResponse::Notice {
                        title: "🛠️ GM 已手动登记线索".to_string(),
                        body: format!("🔎 已添加线索：{rest}"),
                        theme: CardTheme::Success,
                        show_actions: true,
                    })
                }
                GmUpdateOutcome::RoomResolving => Ok(resolving_notice()),
                GmUpdateOutcome::NotFound => Ok(usage_notice("未找到当前房间或线索为空。")),
            }
        }
        "delclue" | "删线索" => {
            let Ok(idx) = rest.parse::<usize>() else {
                return Ok(usage_notice("用法：`/gm delclue 序号`（如 `/gm delclue 1`）"));
            };
            match engine.store.gm_remove_clue(&msg.channel_id, idx)? {
                GmUpdateOutcome::Updated(removed) => {
                    engine.emit_event(RoomEvent::RoomUpdated {
                        channel_id: msg.channel_id.clone(),
                        turn_number: room.turn_number,
                    });
                    Ok(EngineResponse::Notice {
                        title: "🛠️ GM 已移除线索".to_string(),
                        body: format!("已移除第 {idx} 条线索：{removed}"),
                        theme: CardTheme::Success,
                        show_actions: true,
                    })
                }
                GmUpdateOutcome::RoomResolving => Ok(resolving_notice()),
                GmUpdateOutcome::NotFound => {
                    Ok(usage_notice("未找到对应序号的线索，请先 `/status` 核对编号。"))
                }
            }
        }
        _ => Ok(gm_help()),
    }
}

fn mutate_numeric(
    engine: &GameEngine,
    channel_id: &str,
    turn_number: u32,
    rest: &str,
    is_hp: bool,
) -> Result<EngineResponse> {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    let delta_str = parts.next().unwrap_or("").trim();
    let Ok(delta) = delta_str.parse::<i32>() else {
        let field = if is_hp { "hp" } else { "san" };
        return Ok(usage_notice(&format!(
            "用法：`/gm {field} 角色名 ±数值`（例如 `/gm {field} 陆沉 +2` 或 `-3`）"
        )));
    };
    let clamped = delta.clamp(-100, 100);
    let (hp_d, san_d) = if is_hp { (clamped, 0) } else { (0, clamped) };

    match engine
        .store
        .gm_mutate_player(channel_id, name, hp_d, san_d, None, None)?
    {
        GmUpdateOutcome::Updated(p) => {
            engine.emit_event(RoomEvent::RoomUpdated {
                channel_id: channel_id.to_string(),
                turn_number,
            });
            Ok(EngineResponse::Notice {
                title: format!("🛠️ GM 已修正【{}】属性", p.character_name),
                body: format!(
                    "当前状态：❤️ HP {}/{} ｜ 🧠 SAN {}/{}",
                    p.hp, p.max_hp, p.san, p.max_san
                ),
                theme: CardTheme::Success,
                show_actions: true,
            })
        }
        GmUpdateOutcome::RoomResolving => Ok(resolving_notice()),
        GmUpdateOutcome::NotFound => Ok(usage_notice(&format!(
            "找不到名为 `{name}` 的参团角色，请先 `/status` 核对角色名。"
        ))),
    }
}

fn mutate_item(
    engine: &GameEngine,
    channel_id: &str,
    turn_number: u32,
    rest: &str,
    is_add: bool,
) -> Result<EngineResponse> {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    let item = parts.next().unwrap_or("").trim();
    if name.is_empty() || item.is_empty() {
        let op = if is_add { "add" } else { "rm" };
        return Ok(usage_notice(&format!(
            "用法：`/gm {op} 角色名 物品名`（例如 `/gm {op} 陆沉 银钥匙`）"
        )));
    }
    let (add_i, rm_i) = if is_add {
        (Some(item), None)
    } else {
        (None, Some(item))
    };

    match engine
        .store
        .gm_mutate_player(channel_id, name, 0, 0, add_i, rm_i)?
    {
        GmUpdateOutcome::Updated(p) => {
            engine.emit_event(RoomEvent::RoomUpdated {
                channel_id: channel_id.to_string(),
                turn_number,
            });
            let inv = if p.inventory.is_empty() {
                "空".to_string()
            } else {
                p.inventory.join("、")
            };
            Ok(EngineResponse::Notice {
                title: format!("🛠️ GM 已更新【{}】背包", p.character_name),
                body: format!("当前背包：{inv}"),
                theme: CardTheme::Success,
                show_actions: true,
            })
        }
        GmUpdateOutcome::RoomResolving => Ok(resolving_notice()),
        GmUpdateOutcome::NotFound => Ok(usage_notice(&format!("找不到名为 `{name}` 的参团角色。"))),
    }
}

fn resolving_notice() -> EngineResponse {
    EngineResponse::Notice {
        title: "⏳ 本轮正在结算中".to_string(),
        body: "DM 正在推演本轮剧情，请等剧情出卡后再执行 `/gm` 修正，避免被本轮结算覆盖喵！"
            .to_string(),
        theme: CardTheme::Warning,
        show_actions: false,
    }
}

fn gm_help() -> EngineResponse {
    let body = [
        "房主手动兜底修正指令（防 AI 漏加道具或算错数）：",
        "- `/gm hp 角色名 +2` 或 `-3` — 调整血量",
        "- `/gm san 角色名 -1` — 调整理智值",
        "- `/gm add 角色名 物品名` — 发放道具",
        "- `/gm rm 角色名 物品名` — 移除道具",
        "- `/gm clue 线索文本` — 手动补充公共线索",
        "- `/gm delclue 序号` — 删除错误线索",
    ]
    .join("\n");
    EngineResponse::Notice {
        title: "🛠️ 房主 GM 救场指令说明".to_string(),
        body,
        theme: CardTheme::Info,
        show_actions: false,
    }
}

fn usage_notice(msg: &str) -> EngineResponse {
    EngineResponse::Notice {
        title: "ℹ️ 参数格式有误".to_string(),
        body: msg.to_string(),
        theme: CardTheme::Warning,
        show_actions: false,
    }
}
