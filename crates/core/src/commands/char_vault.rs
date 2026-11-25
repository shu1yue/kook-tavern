use anyhow::Result;
use chrono::Utc;
use tavern_domain::{
    AbilityRecharge, CardTheme, CharacterAbility, EngineResponse, IncomingMessage,
    PlayerDndProfile, RoomEvent, SavedCharacter, parse_point_buy_stats, validate_point_buy,
};

use crate::commands::check::ended_room_notice;
use crate::engine::GameEngine;
use crate::ports::JoinPlayerOutcome;

/// Cross-campaign personal character vault & fair point-buy custom character creator (`/char`).
pub fn cmd_char(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let trimmed = arg.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("list") || trimmed == "列表" {
        return list_vault(engine, msg);
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let rest = parts.next().map(str::trim).unwrap_or("");
    match sub.as_str() {
        "create" | "new" | "新建" | "车卡" => create_char(engine, msg, rest),
        "save" | "保存" | "存卡" => save_current(engine, msg),
        "load" | "use" | "载入" | "上桌" => load_into_room(engine, msg, rest),
        "del" | "rm" | "delete" | "删除" => delete_char(engine, msg, rest),
        _ => Ok(EngineResponse::Notice {
            title: "🪪 自由车卡与个人角色库指南（`/char`）".to_string(),
            body: [
                "- `/char` — 查看我保存的跨团角色库（每人最多 8 张）",
                "- `/char create 名字 身份 | 体,敏,侦,交,灵 | 熟练技能 | 阵营 | 背景 | 个人秘密`",
                "  例：`/char create 艾琳 晨光圣武士 | 2,0,0,1,0 | 格斗,游说 | 守序善良 | 曾发誓寻回圣盾 | 秘密偷藏了教会密令`",
                "  （公平购点：单项 `-1..=+3`，总和 `≤ +3`；如需填写「个人秘密」，建议私聊机器人发送以免被队友看到！）",
                "- `/char save` — 把当前房间里的角色存进个人角色库",
                "- `/char load <名字>` — 一键带老角色进入当前房间（自动按本本初值初始化防数值膨胀）",
                "- `/char del <名字>` — 从个人角色库删除指定角色",
            ]
            .join("\n"),
            theme: CardTheme::Info,
            show_actions: false,
        }),
    }
}

fn list_vault(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let list = engine.store.list_saved_characters(&msg.user_id)?;
    if list.is_empty() {
        return Ok(EngineResponse::PrivateNotice {
            user_id: msg.user_id.clone(),
            title: "🪪 我的跨团角色库（空）".to_string(),
            body: "你还没有保存过个人角色！\n\n**两种快速入库方式**：\n1. 自由捏卡：`/char create 艾琳 游荡者 | 0,2,2,0,-1 | 潜行,开锁 | 混乱善良 | 街头长大的巧手`\n2. 先在房间里点职业入团，再发 `/char save` 存下来，下次换新剧本就能 `/char load 名字` 直接带走！".to_string(),
            actions: Vec::new(),
        });
    }
    let lines = list
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let align = if c.dnd.alignment.is_empty() { String::new() } else { format!(" · {}", c.dnd.alignment) };
            let skills = if c.dnd.proficient_skills.is_empty() { "无".into() } else { c.dnd.proficient_skills.join("、") };
            let sec = if c.dnd.personal_secret.is_empty() { String::new() } else { format!("\n  ↳ 🤫 秘密：{}", c.dnd.personal_secret) };
            format!(
                "{}. **{}**（{}{align}）\n  ↳ {} ｜ 熟练：[{skills}] ｜ 招牌技：{} 项{sec}",
                i + 1, c.character_name, c.role_desc, c.stats.format_inline(), c.dnd.abilities.len()
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("🪪 我的跨团角色库（{}/8，仅你可见）", list.len()),
        body: format!("{lines}\n\n💡 在任意开团包厢发 `/char load <名字>` 即可带 TA 上桌！"),
        actions: Vec::new(),
    })
}

fn create_char(engine: &GameEngine, msg: &IncomingMessage, rest: &str) -> Result<EngineResponse> {
    let segs: Vec<&str> = rest.split(['|', '｜']).map(str::trim).collect();
    if segs.len() < 2 {
        return Ok(warn_box(
            "🪪 自由车卡格式提示",
            "至少需要「名字 身份 | 五维」，例：\n`/char create 艾琳 晨光圣武士 | 2,0,0,1,0 | 格斗,游说 | 守序善良 | 寻找失落铸炉 | 暗中受大主教密托`",
        ));
    }
    let mut head = segs[0].splitn(2, char::is_whitespace);
    let name: String = head.next().unwrap_or("").trim().chars().take(16).collect();
    let role: String = head.next().unwrap_or("自由冒险者").trim().chars().take(24).collect();
    if name.is_empty() {
        return Ok(warn_box("❌ 角色名不能为空", "请填写角色名，例如 `/char create 艾琳 游侠 | 1,2,1,0,-1`。"));
    }
    let stats = match parse_point_buy_stats(segs[1]) {
        Ok(s) => s,
        Err(e) => return Ok(warn_box("❌ 属性购点未通过校验", &e)),
    };
    let raw_skills: Vec<String> = segs
        .get(2)
        .map(|s| {
            s.split([',', '，', '、', '/'])
                .map(|x| x.trim().chars().take(12).collect::<String>())
                .filter(|x| !x.is_empty())
                .take(3)
                .collect()
        })
        .unwrap_or_default();
    let mut skills = Vec::new();
    for sk in raw_skills {
        if sk.chars().count() < 2 {
            return Ok(warn_box("❌ 熟练技能名过短", "每项熟练技能至少需要 2 个字（例如：格斗、潜行、游说）。"));
        }
        let stat_hits = ["体魄", "敏捷", "侦查", "交涉", "灵感"]
            .iter()
            .filter(|k| sk.contains(**k))
            .count();
        if stat_hits >= 2 {
            return Ok(warn_box("❌ 熟练技能名不合法", "不能把多个属性名拼成一个熟练技能喵，请填写具体技能名。"));
        }
        if !skills.iter().any(|s| s == &sk) {
            skills.push(sk);
        }
    }
    let alignment: String = segs.get(3).map(|s| s.chars().take(12).collect()).unwrap_or_default();
    let backstory: String = segs.get(4).map(|s| s.chars().take(160).collect()).unwrap_or_default();
    let secret: String = segs.get(5).map(|s| s.chars().take(160).collect()).unwrap_or_default();

    // Grant 1 balanced signature ability automatically if none provided so custom chars shine in D&D mode!
    let default_ab = CharacterAbility {
        name: "绝境爆发".into(),
        max_uses: 1,
        uses_left: 1,
        recharge: AbilityRecharge::ShortRest,
        desc: "在危急时刻集中意志，强行扭转一次不利局势或打出高光一击".into(),
    };
    let ch = SavedCharacter {
        user_id: msg.user_id.clone(),
        character_name: name.clone(),
        role_desc: if role.is_empty() { "自由冒险者".into() } else { role },
        traits: skills.iter().take(2).cloned().collect(),
        stats,
        dnd: PlayerDndProfile {
            alignment,
            proficient_skills: skills,
            abilities: vec![default_ab],
            backstory,
            personal_secret: secret,
            ..PlayerDndProfile::default()
        },
        updated_at: Utc::now(),
    };
    if let Err(err) = engine.store.save_character(&ch) {
        return Ok(warn_box("❌ 保存角色失败", &err.to_string()));
    }
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("✅ 已创建并保存个人角色【{name}】"),
        body: format!(
            "- **身份**：{} {}\n- **公平购点五维**：{}\n- **熟练技能**：{}\n- **默认招牌技**：绝境爆发 (1/1 · 短休恢复)\n\n👉 现在去任意开团包厢发 `/char load {name}` 即可带 TA 上桌！（你的个人秘密仅你与 DM 可见）",
            ch.role_desc,
            ch.dnd.alignment,
            ch.stats.format_inline(),
            if ch.dnd.proficient_skills.is_empty() { "无".into() } else { ch.dnd.proficient_skills.join("、") }
        ),
        actions: Vec::new(),
    })
}

fn save_current(engine: &GameEngine, msg: &IncomingMessage) -> Result<EngineResponse> {
    let Some(p) = engine.store.get_player(&msg.channel_id, &msg.user_id)? else {
        return Ok(warn_box("ℹ️ 当前频道没有你的角色", "请先在当前房间入团后再 `/char save`。"));
    };
    let mut base_stats = p.stats;
    if validate_point_buy(&base_stats).is_err() {
        // Clamp raw stat fields directly into fair starter budget (-1..=3, sum <= 3) in bounded O(1) steps
        base_stats.body = base_stats.body.clamp(-1, 3);
        base_stats.agility = base_stats.agility.clamp(-1, 3);
        base_stats.investigate = base_stats.investigate.clamp(-1, 3);
        base_stats.social = base_stats.social.clamp(-1, 3);
        base_stats.insight = base_stats.insight.clamp(-1, 3);
        for _ in 0..20 {
            if base_stats.body + base_stats.agility + base_stats.investigate + base_stats.social + base_stats.insight <= 3 {
                break;
            }
            let slots = [
                &mut base_stats.body, &mut base_stats.agility, &mut base_stats.investigate,
                &mut base_stats.social, &mut base_stats.insight,
            ];
            if let Some(top) = slots.into_iter().max_by_key(|v| **v) {
                *top = (*top - 1).max(-1);
            }
        }
    }
    let ch = SavedCharacter {
        user_id: msg.user_id.clone(),
        character_name: p.character_name.clone(),
        role_desc: p.role_desc.clone(),
        traits: p.traits.clone(),
        stats: base_stats,
        dnd: tavern_domain::reset_dnd_profile_for_new_room(&p.dnd),
        updated_at: Utc::now(),
    };
    if let Err(err) = engine.store.save_character(&ch) {
        return Ok(warn_box("❌ 保存失败", &err.to_string()));
    }
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: format!("💾 已将【{}】存入跨团角色库", ch.character_name),
        body: format!("以后换新剧本开桌时，直接发 `/char load {}` 就能一键带 TA 跨团冒险！（已自动规整为 Lv.1 公平起始模板）", ch.character_name),
        actions: Vec::new(),
    })
}

fn load_into_room(engine: &GameEngine, msg: &IncomingMessage, name: &str) -> Result<EngineResponse> {
    let target = name.trim();
    if target.is_empty() {
        return Ok(warn_box("❓ 请指定角色名", "先发 `/char` 看你的角色库，再用 `/char load <角色名>` 载入。"));
    }
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(warn_box("ℹ️ 当前频道尚未开团", "请先 `/start` 开一局再载入角色。"));
    };
    if let Some(n) = ended_room_notice(&room) { return Ok(n); }
    let Some(sc) = engine.scenarios.get(&room.scenario_id) else {
        return Ok(warn_box("❌ 剧本配置缺失", "无法加载角色。"));
    };
    let list = engine.store.list_saved_characters(&msg.user_id)?;
    let Some(saved) = list.into_iter().find(|c| c.character_name.eq_ignore_ascii_case(target)) else {
        return Ok(warn_box("❓ 角色库里找不到该角色", &format!("找不到 `{target}`，发 `/char` 查看已有角色列表。")));
    };
    let existed = engine.store.get_player(&msg.channel_id, &msg.user_id)?.is_some();
    match engine.store.load_saved_character_into_room(&msg.channel_id, &msg.user_id, &saved, &sc.player_defaults)? {
        JoinPlayerOutcome::RoomUnavailable => Ok(warn_box("⏳ 当前房间不可载入", "房间已结束或正在结算中，请稍后再试。")),
        JoinPlayerOutcome::DuplicateName => Ok(warn_box("⚠️ 角色名已被队友占用", &format!("本房间已有队友叫 `{}`，请先换名或选用其他角色。", saved.character_name))),
        JoinPlayerOutcome::Joined(p) => {
            engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
            if !existed && room.turn_number >= 1 {
                return Ok(crate::commands::party_helpers::build_drop_in_briefing(
                    engine, &room, &sc, &p, &msg.user_id,
                ));
            }
            Ok(EngineResponse::Notice {
                title: format!("🪪 欢迎回归！【{}】已跨团上桌", p.character_name),
                body: format!(
                    "**{}**（{}）已载入《{}》！\n- 五维：{}\n- 熟练技能：{}\n- 起始 HP `{}/{}` ｜ SAN `{}/{}`",
                    p.character_name, p.role_desc, room.title, p.stats.format_inline(),
                    if p.dnd.proficient_skills.is_empty() { "无".into() } else { p.dnd.proficient_skills.join("、") },
                    p.hp, p.max_hp, p.san, p.max_san
                ),
                theme: CardTheme::Success, show_actions: false,
            })
        }
    }
}

fn delete_char(engine: &GameEngine, msg: &IncomingMessage, name: &str) -> Result<EngineResponse> {
    if name.trim().is_empty() {
        return Ok(warn_box("❓ 请指定要删除的角色名", "用法：`/char del <角色名>`"));
    }
    let ok = engine.store.delete_saved_character(&msg.user_id, name)?;
    Ok(EngineResponse::PrivateNotice {
        user_id: msg.user_id.clone(),
        title: if ok { "🗑️ 已从个人角色库删除" } else { "❓ 未找到该存档角色" }.into(),
        body: if ok { format!("已移除角色 `{}`。", name.trim()) } else { format!("你的角色库里没有名为 `{}` 的角色。", name.trim()) },
        actions: Vec::new(),
    })
}

fn warn_box(title: &str, body: &str) -> EngineResponse {
    EngineResponse::Notice {
        title: title.to_string(),
        body: body.to_string(),
        theme: CardTheme::Warning,
        show_actions: false,
    }
}
