use anyhow::Result;
use tavern_domain::{
    CardTheme, EngineResponse, IncomingMessage, PendingAction, Player, PrivateButton, RoomState,
    Scenario,
};

use crate::engine::GameEngine;
use crate::hud_token::build_player_hud_url;

pub fn ensure_player(
    engine: &GameEngine,
    msg: &IncomingMessage,
    scenario: &Scenario,
) -> Result<Option<Player>> {
    engine.store.ensure_player_atomic(
        &msg.channel_id,
        &msg.user_id,
        &msg.user_name,
        &scenario.player_defaults,
    )
}

pub fn unique_character_name(existing_players: &[Player], raw_name: &str) -> String {
    let base_name: String = raw_name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|' && *c != '｜')
        .take(16)
        .collect();
    let base = if base_name.is_empty() {
        "冒险者".to_string()
    } else {
        base_name
    };
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while existing_players
        .iter()
        .any(|p| p.character_name.eq_ignore_ascii_case(&candidate))
    {
        candidate = format!("{base}#{suffix}");
        suffix += 1;
    }
    candidate
}

pub fn parse_join_args(arg: &str, fallback_name: &str) -> (String, String, Vec<String>) {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return (fallback_name.to_string(), String::new(), Vec::new());
    }

    let (left, traits_part) = match trimmed.split_once('|').or_else(|| trimmed.split_once('｜')) {
        Some((l, r)) => (l.trim(), Some(r.trim())),
        None => (trimmed, None),
    };

    let mut left_parts = left.splitn(2, char::is_whitespace);
    let name = left_parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let role_desc = left_parts.next().map(str::trim).unwrap_or("").to_string();

    let traits = traits_part
        .map(|s| {
            s.split([',', '，', '、', '/'])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    (name, role_desc, traits)
}

pub fn format_party_progress(all_players: &[Player], all_pending: &[PendingAction]) -> String {
    all_players
        .iter()
        .map(|p| {
            if p.is_afk {
                return format!("💤 {} (暂离)", p.character_name);
            }
            let done = all_pending
                .iter()
                .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty());
            if done {
                format!("✅ {}", p.character_name)
            } else {
                format!("⏳ {}", p.character_name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ｜ ")
}

fn clip_chars(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_chars {
        trimmed.to_string()
    } else {
        let head: String = trimmed.chars().take(max_chars).collect();
        format!("{head}…")
    }
}

fn format_drop_in_summary(summary: &str, max_chars: usize) -> String {
    let trimmed = summary.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    const NB_HEADER: &str = "【契约与未解伏笔便签】";
    const CH_HEADER: &str = "【编年史封卷（永不覆写）】";
    let (chapters_raw, notebook) = match trimmed.rsplit_once(NB_HEADER) {
        Some((l, r)) => (l.trim().strip_prefix(CH_HEADER).unwrap_or(l.trim()).trim(), r.trim()),
        None => (trimmed.strip_prefix(CH_HEADER).unwrap_or(trimmed).trim(), ""),
    };
    let chapters: Vec<&str> = chapters_raw
        .split("\n### ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut sections = Vec::new();
    if !notebook.is_empty() {
        sections.push(format!("📌 **当前未解伏笔**\n{}", clip_chars(notebook, 320)));
    }
    let recent_budget = max_chars.saturating_sub(sections.iter().map(|s| s.chars().count()).sum::<usize>());
    if let Some(latest) = chapters.last() {
        let mut recent = String::new();
        if chapters.len() >= 2 {
            let prev = chapters[chapters.len() - 2];
            if !prev.starts_with("### ") { recent.push_str("### "); }
            recent.push_str(prev);
            recent.push_str("\n\n");
        }
        if !latest.starts_with("### ") { recent.push_str("### "); }
        recent.push_str(latest);
        sections.push(clip_chars(&recent, recent_budget.max(400)));
    }
    if sections.is_empty() {
        clip_chars(trimmed, max_chars)
    } else {
        sections.join("\n\n")
    }
}

/// Tailored spoiler-safe welcome briefing delivered privately (`PrivateNotice`) when a brand-new
/// adventurer drops into an ongoing adventure (`room.turn_number >= 1`), so they immediately grasp
/// the story so far without flooding the public story stream or spoiling teammates' private whispers.
pub fn build_drop_in_briefing(
    engine: &GameEngine,
    room: &RoomState,
    scenario: &Scenario,
    player: &Player,
    user_id: &str,
) -> EngineResponse {
    let mut blocks = Vec::new();
    let inv = if player.inventory.is_empty() {
        "空".to_string()
    } else {
        player.inventory.join(" · ")
    };
    blocks.push(format!(
        "⚔️ **已入团角色**：**{}**（{}）｜ ❤️ {}/{} ｜ 🧠 {}/{}\n📊 **五维**：{}\n🎒 **行囊**：{}",
        player.character_name,
        player.role_desc,
        player.hp,
        player.max_hp,
        player.san,
        player.max_san,
        player.stats.format_inline(),
        inv
    ));

    let act_line = scenario
        .acts
        .get(room.current_act.saturating_sub(1) as usize)
        .map(|a| format!("\n🎬 **当前第 {} 幕 · {}**：{}", room.current_act, a.title, a.goal))
        .unwrap_or_default();
    blocks.push(format!(
        "📍 **当前位置**：{} ｜ 🕒 **时间**：{} ｜ 🎲 **进度**：第 **{}** 回合{act_line}",
        room.location, room.world_time, room.turn_number
    ));

    let mut recap_parts = Vec::new();
    if !room.summary.trim().is_empty() {
        let label = if room.summary_upto_turn > 0 {
            format!("📜 **前情提要（截至第 {} 轮）**", room.summary_upto_turn)
        } else {
            "📜 **前情提要**".to_string()
        };
        recap_parts.push(format!("{label}\n{}", format_drop_in_summary(&room.summary, 900)));
    }
    if let Ok(turns) = engine.store.list_recent_turns(&room.channel_id, 1)
        && let Some(latest) = turns.last()
        && (latest.turn_number > room.summary_upto_turn || room.summary.trim().is_empty())
    {
        recap_parts.push(format!(
            "🔖 **刚刚发生（第 {} 轮 · {}）**：{}",
            latest.turn_number,
            latest.scene_title,
            clip_chars(&latest.narrative, 260)
        ));
    }
    if recap_parts.is_empty() && !scenario.opening_scene.trim().is_empty() {
        recap_parts.push(format!(
            "📜 **开局背景**：{}",
            clip_chars(&scenario.opening_scene, 260)
        ));
    }
    if !recap_parts.is_empty() {
        blocks.push(recap_parts.join("\n\n"));
    }

    let mut social = Vec::new();
    if let Ok(players) = engine.store.list_players(&room.channel_id) {
        let mates: Vec<String> = players
            .iter()
            .filter(|p| p.user_id != user_id)
            .map(|p| format!("**{}**（{} · ❤️{}/{}）", p.character_name, p.role_desc, p.hp, p.max_hp))
            .collect();
        if !mates.is_empty() {
            social.push(format!("👥 **同行队友**：{}", mates.join("、")));
        }
    }
    if !room.npc_states.is_empty() {
        let npcs: Vec<String> = room
            .npc_states
            .iter()
            .take(5)
            .map(|n| format!("{}（{}）", n.name, n.attitude))
            .collect();
        social.push(format!("🤝 **关键人物**：{}", npcs.join("、")));
    }
    if !room.clues.is_empty() {
        let latest_clues: Vec<String> = room
            .clues
            .iter()
            .rev()
            .take(3)
            .rev()
            .map(|c| format!("• {}", clip_chars(c, 80)))
            .collect();
        social.push(format!("🔎 **近期线索**：\n{}", latest_clues.join("\n")));
    }
    if !social.is_empty() {
        blocks.push(social.join("\n"));
    }

    blocks.push(
        "💡 **如何自然切入**：参考最新剧情卡里的切入焦点，直接在频道打人话描述你的登场或行动（任何脑洞/掀桌都行），下一轮结算时 DM 会自然把你写进镜头里喵～"
            .to_string(),
    );

    let mut actions = Vec::new();
    if let Some(url) = build_player_hud_url(&engine.cfg, room, user_id) {
        actions.push(PrivateButton {
            label: "🪟 打开悬浮 HUD".to_string(),
            value: url,
            theme: CardTheme::Success,
        });
    }
    if !scenario.preset_roles.is_empty() && player.role_desc == "酒馆旅客" {
        actions.push(PrivateButton {
            label: "🎭 选预设职业".to_string(),
            value: format!("tavern:rolemenu:{}", room.channel_id),
            theme: CardTheme::Primary,
        });
    }

    EngineResponse::PrivateNotice {
        user_id: user_id.to_string(),
        title: format!("🎬 欢迎半路空降 · 《{}》（第 {} 回合）", room.title, room.turn_number),
        body: blocks.join("\n\n"),
        actions,
    }
}

/// Renders an interactive `/levelup` menu (5 stats + HP/SAN + skill charge) as clickable buttons.
pub fn build_levelup_menu(
    channel_id: &str,
    user_id: &str,
    player: &Player,
    is_direct: bool,
) -> EngineResponse {
    let title = format!(
        "⬆️ {} · Lv.{}（剩余升级点：{}）",
        player.character_name, player.dnd.level, player.dnd.pending_levelups
    );
    let body = format!(
        "当前五维：{}\n\n每次剧情推进到新一幕，全队自动获得 **+1 成长点**！直接点下方按钮（或发命令）分配：\n- `/levelup hp` — HP 与 SAN 上限各 +2 并回复 2 点\n- `/levelup 体魄|敏捷|侦查|交涉|灵感` — 对应属性 +1（上限 +4）\n- `/levelup skill <技能名>` — 习得新熟练技能（最多 4 项）\n- `/levelup charge` — 首个招牌技充能上限 +1 并回满",
        player.stats.format_inline()
    );
    if is_direct || player.dnd.pending_levelups == 0 {
        return EngineResponse::Notice {
            title,
            body,
            theme: CardTheme::Info,
            show_actions: false,
        };
    }
    let mut actions = vec![
        PrivateButton { label: "❤️ +2血/理智".into(), value: format!("tavern:lv:{channel_id}:hp"), theme: CardTheme::Success },
        PrivateButton { label: "💪 体魄+1".into(), value: format!("tavern:lv:{channel_id}:body"), theme: CardTheme::Primary },
        PrivateButton { label: "⚡ 敏捷+1".into(), value: format!("tavern:lv:{channel_id}:agility"), theme: CardTheme::Primary },
        PrivateButton { label: "🔍 侦查+1".into(), value: format!("tavern:lv:{channel_id}:investigate"), theme: CardTheme::Primary },
        PrivateButton { label: "🗣️ 交涉+1".into(), value: format!("tavern:lv:{channel_id}:social"), theme: CardTheme::Primary },
        PrivateButton { label: "👁️ 灵感+1".into(), value: format!("tavern:lv:{channel_id}:insight"), theme: CardTheme::Primary },
    ];
    if !player.dnd.abilities.is_empty() {
        actions.push(PrivateButton {
            label: "🔋 招牌技+1充能".into(),
            value: format!("tavern:lv:{channel_id}:charge"),
            theme: CardTheme::Warning,
        });
    }
    EngineResponse::PrivateNotice {
        user_id: user_id.to_string(),
        title,
        body,
        actions,
    }
}
