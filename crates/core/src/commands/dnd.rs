use anyhow::Result;
use tavern_domain::{
    CardTheme, CheckSystem, EngineResponse, IncomingMessage, LevelUpChoice, Player, RestKind,
    RollMode, RoomEvent, StatKind,
};

use crate::commands::check::ended_room_notice;
use crate::engine::GameEngine;
use crate::{AbilityUseOutcome, AssistOutcome, GmUpdateOutcome, LevelUpOutcome, RestOutcome};

pub(crate) struct ParsedCheckArgs {
    pub stat: StatKind,
    pub raw_stat_token: String,
    pub mode: RollMode,
    pub dc: Option<i32>,
    pub use_inspiration: bool,
    pub reason: Option<String>,
}

pub(crate) fn parse_check_args(arg: &str) -> Option<ParsedCheckArgs> {
    let tokens: Vec<&str> = arg.split_whitespace().collect();
    let first = tokens.first().copied()?.trim();
    let stat = StatKind::parse(first)?;
    let mut seen_adv = false;
    let mut seen_dis = false;
    let mut dc: Option<i32> = None;
    let mut use_inspiration = false;
    let mut reason_parts = Vec::new();
    let mut i = 1usize;
    while i < tokens.len() {
        let low = tokens[i].to_ascii_lowercase();
        if matches!(low.as_str(), "adv" | "advantage" | "优势") {
            seen_adv = true;
        } else if matches!(low.as_str(), "dis" | "disadvantage" | "劣势") {
            seen_dis = true;
        } else if matches!(low.as_str(), "inspire" | "inspiration" | "激励") {
            use_inspiration = true;
        } else if let Some(rest) = low.strip_prefix("dc") {
            if let Ok(n) = rest.trim_start_matches(['=', ':']).parse::<i32>() {
                dc = Some(n.clamp(5, 25));
            } else if rest.is_empty()
                && let Some(next_tok) = tokens.get(i + 1)
                && let Ok(n) = next_tok.parse::<i32>()
            {
                dc = Some(n.clamp(5, 25));
                i += 1;
            } else {
                reason_parts.push(tokens[i]);
            }
        } else {
            reason_parts.push(tokens[i]);
        }
        i += 1;
    }
    let mode = match (seen_adv, seen_dis) {
        (true, false) => RollMode::Advantage,
        (false, true) => RollMode::Disadvantage,
        _ => RollMode::Normal,
    };
    let reason = (!reason_parts.is_empty()).then(|| reason_parts.join(" "));
    Some(ParsedCheckArgs {
        stat,
        raw_stat_token: first.to_string(),
        mode,
        dc,
        use_inspiration,
        reason,
    })
}

pub(crate) fn format_dnd_me_sections(p: &Player, sys: CheckSystem) -> Vec<String> {
    let mut out = Vec::new();
    let mut badges = Vec::new();
    if sys != CheckSystem::D100 {
        badges.push(format!("Lv.**{}**", p.dnd.level));
    }
    badges.push(format!("规则：`{}`", sys.short_badge()));
    if !p.dnd.alignment.trim().is_empty() {
        badges.push(format!("阵营：{}", p.dnd.alignment.trim()));
    }
    if p.dnd.inspiration {
        badges.push("🌟 **持有激励点**（`/check <属性> inspire` 享优势）".into());
    }
    if !p.dnd.assisted_by.trim().is_empty() {
        badges.push(format!("🤝 **已获 {} 协助**（下次检定自动优势）", p.dnd.assisted_by.trim()));
    }
    if p.dnd.pending_levelups > 0 {
        badges.push(format!("⬆️ **待分配升级点 ×{}**（发 `/levelup`）", p.dnd.pending_levelups));
    }
    out.push(format!("🛡️ **冒险者档案**：{}", badges.join(" ｜ ")));

    if !p.dnd.proficient_skills.is_empty() {
        out.push(format!("🎓 **熟练技能**：{}", p.dnd.proficient_skills.join(" · ")));
    }
    if !p.dnd.abilities.is_empty() {
        let lines = p.dnd.abilities.iter().enumerate().map(|(i, a)| {
            let d = if a.desc.trim().is_empty() { String::new() } else { format!(" — {}", a.desc.trim()) };
            format!("{}. **{}** `{}/{}`（{}）{d}", i + 1, a.name, a.uses_left, a.max_uses, a.recharge.short_label())
        }).collect::<Vec<_>>().join("\n");
        out.push(format!("✨ **招牌技能（`/skill <编号或名> [目标]`）**\n{lines}"));
    }
    if !p.dnd.backstory.trim().is_empty() || !p.dnd.personal_secret.trim().is_empty() {
        let mut lines = Vec::new();
        if !p.dnd.backstory.trim().is_empty() {
            lines.push(format!("- 背景：{}", p.dnd.backstory.trim()));
        }
        if !p.dnd.personal_secret.trim().is_empty() {
            lines.push(format!("- 🤫 个人秘密（仅你与 DM 可见）：{}", p.dnd.personal_secret.trim()));
        }
        out.push(format!("📖 **角色背景与秘密**\n{}", lines.join("\n")));
    }
    out
}

pub fn cmd_rule(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(notice_warn("ℹ️ 当前频道尚未开团", "请先 `/start` 开一局再切换规则体系。"));
    };
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Ok(EngineResponse::Notice {
            title: format!("📐 当前检定规则：{}", room.check_system.short_badge()),
            body: format!(
                "{}\n\n房主或管理员可发 `/rule d20`（经典 D&D）、`/rule 2d6`（轻量三档）或 `/rule d100`（克苏鲁风）随时切换！",
                room.check_system.prompt_guide()
            ),
            theme: CardTheme::Info,
            show_actions: false,
        });
    }
    if !room.active {
        return Ok(notice_warn("ℹ️ 当前房间已归档", "请开启新剧本后再切换规则体系。"));
    }
    if let Some(n) = ended_room_notice(&room) {
        return Ok(n);
    }
    if !engine.cfg.is_gm_or_admin(&msg.user_id, &room.gm_user_id) {
        return Ok(notice_warn("🔒 仅房主或管理员可切换规则", "如需更换本桌骰子体系，请由开团房主或管理员执行 `/rule d20|2d6|d100`。"));
    }
    let Some(next) = CheckSystem::parse(trimmed) else {
        return Ok(notice_warn("❓ 未知规则体系", "支持：`/rule d20`（D&D）、`/rule 2d6`（三档）、`/rule d100`（CoC）。"));
    };
    if !engine.store.update_check_system(&msg.channel_id, next)? {
        return Ok(notice_warn("⏳ 暂不可切换规则", "房间正在结算中或已结束，请等本轮出卡后再试。"));
    }
    engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
    Ok(EngineResponse::Notice {
        title: format!("✅ 已切换检定规则 → {}", next.short_badge()),
        body: format!("{}\n\n本桌接下来的 `/check` 与 DM 裁决将自动采用该规则体系！", next.prompt_guide()),
        theme: CardTheme::Success,
        show_actions: false,
    })
}

pub fn cmd_skill(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(notice_warn("ℹ️ 当前频道尚未开团", "先开团入桌后再施放招牌技能喵！"));
    };
    if !room.active {
        return Ok(notice_warn("ℹ️ 当前房间已归档", "请开启新剧本后再施放技能。"));
    }
    if let Some(n) = ended_room_notice(&room) { return Ok(n); }
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        let Some(player) = engine.store.get_player(&msg.channel_id, &msg.user_id)? else {
            return Ok(notice_warn("ℹ️ 你尚未入团", "请先选职业入团或 `/char load` 载入角色！"));
        };
        if player.dnd.abilities.is_empty() {
            return Ok(notice_warn("✨ 暂无招牌技能", "当前角色暂无充能型招牌技能（选带技能的职业或用 `/char` 自定义角色即可拥有）。"));
        }
        let lines = player.dnd.abilities.iter().enumerate()
            .map(|(i, a)| format!("{}. **{}** `{}/{}`（{}）— {}", i + 1, a.name, a.uses_left, a.max_uses, a.recharge.short_label(), a.desc))
            .collect::<Vec<_>>().join("\n");
        return Ok(EngineResponse::Notice {
            title: format!("✨ {} 的招牌技能列表", player.character_name),
            body: format!("{lines}\n\n用法：`/skill 1 [目标或描述]` 或 `/skill 技能名 [目标]`"),
            theme: CardTheme::Info, show_actions: false,
        });
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let query = parts.next().unwrap_or("");
    let target = parts.next().map(str::trim).unwrap_or("");
    match engine.store.use_player_ability(&msg.channel_id, &msg.user_id, query, target)? {
        AbilityUseOutcome::Used { player, ability_name, turn_no, roll_note, .. } => {
            engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
            Ok(EngineResponse::Notice {
                title: format!("✨ {} 施放了【{ability_name}】！", player.character_name),
                body: format!("{roll_note}\n\n（已绑定至第 **{turn_no}** 轮行动，接下来打字或点行动按钮，DM 会为你演出招牌技的高光效果！）"),
                theme: CardTheme::Success, show_actions: false,
            })
        }
        AbilityUseOutcome::Exhausted { ability_name, max_uses, recharge } => Ok(notice_warn(
            &format!("🔋 【{ability_name}】充能已耗尽 (0/{max_uses})"),
            &format!("该技能需要 **{recharge}** 恢复（使用 `/rest short` 或由房主发 `/rest long` 营地休整）。"),
        )),
        AbilityUseOutcome::NotAllowed(reason) => Ok(notice_warn("⚠️ 暂不可施放技能", &reason)),
        AbilityUseOutcome::RoomResolving => Ok(notice_warn("⏳ DM 正在结算中", "请等待本轮出卡后再施放技能。")),
        AbilityUseOutcome::NotFound => Ok(notice_warn("❓ 未找到该招牌技", "发 `/skill` 查看你拥有的技能编号与名称。")),
        AbilityUseOutcome::RoomUnavailable => Ok(notice_warn("ℹ️ 当前房间不可操作", "房间已结束或未开启。")),
    }
}

pub fn cmd_rest(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(notice_warn("ℹ️ 当前频道尚未开团", "先开团再营地休整喵！"));
    };
    if !room.active {
        return Ok(notice_warn("ℹ️ 当前房间已归档", "请开启新剧本后再营地休整。"));
    }
    if let Some(n) = ended_room_notice(&room) { return Ok(n); }
    let kind = match arg.trim().to_ascii_lowercase().as_str() {
        "" | "short" | "短" | "短休" => RestKind::Short,
        "long" | "长" | "长休" | "营地" => RestKind::Long,
        _ => return Ok(notice_warn("🏕️ 营地休整用法", "- `/rest short`（`/短休`）：恢复部分 HP、少量 SAN 与短休招牌技\n- `/rest long`（`/长休`，房主/管理员）：满血满 SAN 并恢复全部招牌技")),
    };
    let is_gm = engine.cfg.is_gm_or_admin(&msg.user_id, &room.gm_user_id);
    let gap = if kind == RestKind::Long { 2 } else { 1 };
    match engine.store.apply_party_rest(&msg.channel_id, &msg.user_id, is_gm, kind, gap)? {
        RestOutcome::Rested { notes, new_world_time } => {
            engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
            let label = if kind == RestKind::Long { "🏕️ 全队完成长休（状态全满）" } else { "🔥 小队完成短暂休整" };
            let detail = if notes.is_empty() { "全员目前状态完好，士气高涨！".to_string() } else { notes.join("\n") };
            Ok(EngineResponse::Notice {
                title: label.to_string(),
                body: format!("{detail}\n\n🕰️ 当前时间：{new_world_time}"),
                theme: CardTheme::Success, show_actions: false,
            })
        }
        RestOutcome::Cooldown { wait_turns } => Ok(notice_warn("⏳ 刚刚才休整过", &format!("请至少推进 **{wait_turns}** 回合剧情后再进行下一次休整，防止原地刷满状态喵！"))),
        RestOutcome::NotAllowed(reason) => Ok(notice_warn("🔒 暂不可休整", &reason)),
        RestOutcome::RoomResolving => Ok(notice_warn("⏳ DM 正在结算中", "请等本轮结算完毕后再休整。")),
        RestOutcome::RoomUnavailable => Ok(notice_warn("ℹ️ 房间不可休整", "房间已结束或归档。")),
    }
}

pub fn cmd_inspire(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(notice_warn("ℹ️ 当前频道尚未开团", "先开团再颁发激励点。"));
    };
    if !room.active {
        return Ok(notice_warn("ℹ️ 当前房间已归档", "请开启新剧本后再颁发激励点。"));
    }
    if let Some(n) = ended_room_notice(&room) { return Ok(n); }
    if !engine.cfg.is_gm_or_admin(&msg.user_id, &room.gm_user_id) {
        return Ok(notice_warn("🔒 仅房主或管理员可颁发激励点", "精彩扮演时由房主或 DM 自动赏赐激励点（`/inspire <角色名>`）。"));
    }
    let target = arg.trim();
    if target.is_empty() {
        return Ok(notice_warn("🌟 颁发激励点用法", "输入 `/inspire <角色名>`，赏赐对方 1 颗激励点（检定时加 `inspire` 直接享优势双骰）。"));
    }
    match engine.store.set_player_inspiration(&msg.channel_id, target, true)? {
        GmUpdateOutcome::Updated(p) => {
            engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
            Ok(EngineResponse::Notice {
                title: format!("🌟 已授予 {} 激励点！", p.character_name),
                body: format!("**{}** 获得 1 颗激励点！下次关键检定输入 `/check <属性> inspire` 即可消耗它触发 **Advantage 优势双骰**！", p.character_name),
                theme: CardTheme::Success, show_actions: false,
            })
        }
        GmUpdateOutcome::RoomResolving => Ok(notice_warn("⏳ DM 正在结算中", "请等本轮出卡后再授予激励点。")),
        GmUpdateOutcome::NotFound => Ok(notice_warn("❓ 未找到该角色", &format!("当前房间找不到名为 `{target}` 的队员。"))),
    }
}

pub fn cmd_assist(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(notice_warn("ℹ️ 当前频道尚未开团", "先开团再协助队友。"));
    };
    if !room.active {
        return Ok(notice_warn("ℹ️ 当前房间已归档", "请开启新剧本后再协助队友。"));
    }
    if let Some(n) = ended_room_notice(&room) { return Ok(n); }
    let mut parts = arg.trim().splitn(2, char::is_whitespace);
    let target_name = parts.next().unwrap_or("").trim();
    let how = parts.next().map(str::trim).unwrap_or("");
    if target_name.is_empty() {
        return Ok(notice_warn("🤝 队友协助（Help）用法", "输入 `/assist <队友角色名> [怎么帮]`，立即给对方下一次 `/check` 挂上 **Advantage 优势双骰**！"));
    }
    match engine.store.set_player_assist(&msg.channel_id, &msg.user_id, target_name, how)? {
        AssistOutcome::Assisted { helper, target, turn_no } => {
            engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
            Ok(EngineResponse::Notice {
                title: format!("🤝 {} 正在协助 {}！", helper.character_name, target.character_name),
                body: format!("**{}** 的下一次 `/check` 检定将自动获得 **【优势双骰】**！（已将你的配合记录绑定至第 **{turn_no}** 轮）", target.character_name),
                theme: CardTheme::Info, show_actions: false,
            })
        }
        AssistOutcome::NotAllowed(reason) => Ok(notice_warn("⚠️ 无法协助队友", &reason)),
        AssistOutcome::RoomResolving => Ok(notice_warn("⏳ DM 正在结算中", "请等本轮结算完再协助队友。")),
        AssistOutcome::TargetNotFound => Ok(notice_warn("❓ 未找到该队友", &format!("房间内找不到名为 `{target_name}` 的队友。"))),
        AssistOutcome::RoomUnavailable => Ok(notice_warn("ℹ️ 当前房间不可操作", "房间已结束或归档。")),
    }
}

pub fn cmd_levelup(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(&msg.channel_id)? else {
        return Ok(notice_warn("ℹ️ 当前频道尚未开团", "先开团再升级喵！"));
    };
    if !room.active {
        return Ok(notice_warn("ℹ️ 当前房间已归档", "请开启新剧本后再升级。"));
    }
    if let Some(n) = ended_room_notice(&room) { return Ok(n); }
    let Some(player) = engine.store.get_player(&msg.channel_id, &msg.user_id)? else {
        return Ok(notice_warn("ℹ️ 你尚未入团", "先选职业入团后再分配升级点。"));
    };
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Ok(crate::commands::party_helpers::build_levelup_menu(
            &msg.channel_id,
            &msg.user_id,
            &player,
            msg.is_direct,
        ));
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("").to_ascii_lowercase();
    let tail = parts.next().map(str::trim).unwrap_or("");
    let choice = if matches!(head.as_str(), "hp" | "san" | "生命" | "血量") {
        LevelUpChoice::HpSan
    } else if matches!(head.as_str(), "charge" | "skill_charge" | "充能" | "绝活") {
        LevelUpChoice::AbilityCharge
    } else if matches!(head.as_str(), "skill" | "prof" | "熟练" | "技能") {
        LevelUpChoice::Skill(tail.to_string())
    } else if let Some(st) = StatKind::parse(&head) {
        LevelUpChoice::Stat(st)
    } else {
        return Ok(notice_warn("❓ 未识别的成长选项", "请用 `/levelup hp`、`/levelup <属性名>`、`/levelup skill <技能名>` 或 `/levelup charge`。"));
    };
    match engine.store.apply_player_levelup(&msg.channel_id, &msg.user_id, choice)? {
        LevelUpOutcome::Upgraded { player, summary, remaining } => {
            engine.emit_event(RoomEvent::RoomUpdated { channel_id: msg.channel_id.clone(), turn_number: room.turn_number });
            Ok(EngineResponse::Notice {
                title: format!("🎉 {} 成长强化完成（Lv.{}）", player.character_name, player.dnd.level),
                body: format!("{summary}\n\n剩余待分配升级点：**{remaining}**"),
                theme: CardTheme::Success, show_actions: false,
            })
        }
        LevelUpOutcome::Error(err) => Ok(notice_warn("⚠️ 无法分配升级点", &err)),
        LevelUpOutcome::RoomResolving => Ok(notice_warn("⏳ DM 正在结算中", "请等本轮结算完再升级。")),
        LevelUpOutcome::RoomUnavailable => Ok(notice_warn("ℹ️ 房间不可操作", "房间已结束或归档。")),
    }
}

fn notice_warn(title: &str, body: &str) -> EngineResponse {
    EngineResponse::Notice { title: title.to_string(), body: body.to_string(), theme: CardTheme::Warning, show_actions: false }
}
