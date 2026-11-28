use serde_json::{Value, json};
use tavern_domain::{
    ChatMode, CheckSystem, NarrativeStyle, NpcState, PendingAction, Player, RoomState,
    SuggestedCheck, WorldBulletin,
};

pub fn btn(theme: &str, value: String, label: &str) -> Value {
    json!({
        "type": "button", "theme": theme, "value": value, "click": "return-val",
        "text": { "type": "plain-text", "content": truncate_chars(label, 24) }
    })
}

pub fn action_group(buttons: Vec<Value>) -> Value {
    let bounded: Vec<Value> = buttons.into_iter().take(4).collect();
    json!({ "type": "action-group", "elements": bounded })
}

pub fn render_meter(cur: i32, max: i32, filled: &str, empty: &str) -> String {
    let max = max.max(1);
    let clamped = cur.clamp(0, max);
    let slots = 5usize;
    let lit = (((clamped as f32 / max as f32) * slots as f32).round() as usize).min(slots);
    format!(
        "{}{} `{cur}/{max}`",
        filled.repeat(lit),
        empty.repeat(slots - lit)
    )
}

pub fn quick_roll_button(channel_id: &str, system: CheckSystem) -> Value {
    let (expr, label) = match system {
        CheckSystem::TwoD6 => ("2d6", "🎲 掷 2d6"),
        CheckSystem::D20 => ("1d20", "🎲 掷 d20"),
        CheckSystem::D100 => ("1d100", "🎲 掷 d100"),
    };
    btn("secondary", format!("tavern:roll:{channel_id}:{expr}"), label)
}

pub fn notice_action_bar(channel_id: &str, system: CheckSystem) -> Value {
    action_group(vec![
        btn("primary", format!("tavern:resolve:{channel_id}"), "⚡ 立刻结算"),
        quick_roll_button(channel_id, system),
        btn("info", format!("tavern:status:{channel_id}"), "🎛️ 面板/设置"),
        btn("warning", format!("tavern:afk:{channel_id}"), "💤 暂离/归队"),
    ])
}

pub fn story_toolbar(
    channel_id: &str,
    system: CheckSystem,
    suggested_check: Option<&SuggestedCheck>,
) -> Value {
    let dice_btn = if let Some(ck) = suggested_check
        && let Some(stat) = ck.parsed_stat()
    {
        let (val, dc_tag) = match ck.dc {
            Some(d) => (
                format!("tavern:check:{channel_id}:{}:{}", stat.key(), d.clamp(5, 25)),
                format!(" DC{}", d.clamp(5, 25)),
            ),
            None => (format!("tavern:check:{channel_id}:{}", stat.key()), String::new()),
        };
        btn("warning", val, &format!("{} {}检定{dc_tag}", stat.icon(), stat.short_name()))
    } else {
        quick_roll_button(channel_id, system)
    };
    action_group(vec![
        dice_btn,
        btn("primary", format!("tavern:resolve:{channel_id}"), "⚡ 立刻结算"),
        btn("info", format!("tavern:me:{channel_id}"), "🪪 我的角色卡"),
        btn("secondary", format!("tavern:status:{channel_id}"), "🎛️ 面板/设置"),
    ])
}

/// Renders open-ended directional focus hints (plain text, NO multiple-choice buttons!)
/// followed by the 4-button utility toolbar.
pub fn push_story_action_footer(
    modules: &mut Vec<Value>,
    channel_id: &str,
    system: CheckSystem,
    suggested_actions: &[String],
    suggested_check: Option<&SuggestedCheck>,
) {
    let hints: Vec<String> = suggested_actions
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .take(3)
        .map(|s| format!("• {}", truncate_chars(s, 60)))
        .collect();
    let ck_hint = suggested_check
        .and_then(|c| c.parsed_stat().map(|st| (st, c.reason.trim())))
        .map(|(st, r)| {
            if r.is_empty() {
                format!("🎲 **参考检定**：{}（点下方检定按钮自动带五维加值）", st.display_label())
            } else {
                format!(
                    "🎲 **参考检定**：{} — {}（点下方检定按钮自动带五维加值）",
                    st.display_label(),
                    truncate_chars(r, 28)
                )
            }
        });

    modules.push(json!({ "type": "divider" }));
    if !hints.is_empty() || ck_hint.is_some() {
        let mut lines = Vec::new();
        if !hints.is_empty() {
            lines.push("💡 **眼下可切入焦点**（直接打人话自由行动，任何脑洞/掀桌都行）：".to_string());
            lines.extend(hints);
        }
        if let Some(ck) = ck_hint {
            lines.push(ck);
        }
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": truncate_chars(&lines.join("\n"), 1200)
            }
        }));
    }
    modules.push(story_toolbar(channel_id, system, suggested_check));
}

pub fn ending_toolbar(channel_id: &str, is_managed_table: bool) -> Value {
    let fourth = if is_managed_table {
        btn("danger", format!("tavern:close_table_ask:{channel_id}"), "🗑️ 拆掉包间")
    } else {
        btn("secondary", format!("tavern:menu:{channel_id}"), "📚 换剧本")
    };
    action_group(vec![
        btn("success", format!("tavern:export:{channel_id}"), "📜 发战报墙"),
        btn("primary", format!("tavern:endings:{channel_id}"), "🏆 结局图鉴"),
        btn("warning", format!("tavern:undo:{channel_id}"), "↩️ 回档改命"),
        fourth,
    ])
}

pub fn status_console_rows(
    channel_id: &str,
    room: &RoomState,
    is_managed_table: bool,
) -> [Value; 3] {
    let style_label = match room.narrative_style {
        NarrativeStyle::Voice => "📖 切文字沉浸",
        NarrativeStyle::Prose => "🎙️ 切语音极简",
    };
    let mode_label = match room.chat_mode {
        ChatMode::Roleplay => "⌨️ 切指令前缀",
        ChatMode::Strict => "💬 切直接说话",
    };
    let pause_label = if room.manual_resolve {
        "▶️ 切自动结算"
    } else {
        "⏸️ 切手动结算"
    };
    let row1 = action_group(vec![
        btn("success", format!("tavern:rolemenu:{channel_id}"), "🎭 选职业入团"),
        btn("secondary", format!("tavern:rest:{channel_id}:short"), "🔥 营地短休"),
        btn("warning", format!("tavern:afk:{channel_id}"), "💤 暂离/归队"),
        btn("danger", format!("tavern:undo:{channel_id}"), "↩️ 回档上轮"),
    ]);
    let row2 = action_group(vec![
        btn("info", format!("tavern:toggle_style:{channel_id}"), style_label),
        btn("info", format!("tavern:toggle_mode:{channel_id}"), mode_label),
        btn("info", format!("tavern:toggle_pause:{channel_id}"), pause_label),
        btn("success", format!("tavern:export:{channel_id}"), "📜 发战报墙"),
    ]);
    let table_or_menu = if is_managed_table {
        btn("danger", format!("tavern:close_table_ask:{channel_id}"), "🗑️ 拆掉包间")
    } else {
        btn("secondary", format!("tavern:menu:{channel_id}"), "📚 换剧本")
    };
    let row3 = action_group(vec![
        btn("primary", format!("tavern:web:{channel_id}"), "🌐 Web 看板"),
        btn("warning", format!("tavern:endings:{channel_id}"), "🏆 结局图鉴"),
        table_or_menu,
        btn("danger", format!("tavern:reset_ask:{channel_id}"), "🛑 结束本局"),
    ]);
    [row1, row2, row3]
}

pub fn push_party_cards(
    modules: &mut Vec<Value>,
    players: &[Player],
    pending_actions: &[PendingAction],
    check_system: CheckSystem,
) {
    if players.is_empty() {
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": "🪑 **酒馆长桌空着呢** — 点下方 **「🎭 选职业入团」** 一键挑角色拿专属装备，或直接打人话上桌喵！"
            }
        }));
        return;
    }

    for p in players {
        if modules.len() >= 30 {
            break;
        }
        let badge = if p.is_afk {
            "💤 暂离"
        } else if pending_actions
            .iter()
            .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty())
        {
            "✅ 已就绪"
        } else {
            "⏳ 思考中"
        };
        let hp_bar = render_meter(p.hp, p.max_hp, "🟥", "⬜");
        let san_bar = render_meter(p.san, p.max_san, "🟦", "⬜");
        let status_txt = if p.status_note.is_empty() { "状态正常" } else { &p.status_note };
        let inv = if p.inventory.is_empty() { "空".to_string() } else { p.inventory.join(" · ") };
        let role_tag = if check_system == CheckSystem::D100 {
            if p.dnd.inspiration {
                format!("{} 🌟", truncate_chars(&p.role_desc, 20))
            } else {
                truncate_chars(&p.role_desc, 20)
            }
        } else {
            format!(
                "{} · Lv.{}{}",
                truncate_chars(&p.role_desc, 20),
                p.dnd.level,
                if p.dnd.inspiration { " 🌟" } else { "" }
            )
        };
        let content = format!(
            "**{}**（{role_tag}） `{}`\n❤️ {}　｜　🧠 {}\n📊 {}\n🏷️ {}　｜　🎒 {}",
            truncate_chars(&p.character_name, 24),
            badge, hp_bar, san_bar, p.stats.format_inline(),
            truncate_chars(status_txt, 40), truncate_chars(&inv, 140)
        );
        modules.push(json!({
            "type": "section",
            "text": { "type": "kmarkdown", "content": content }
        }));
    }
}

pub fn push_npc_section(modules: &mut Vec<Value>, npcs: &[NpcState]) {
    if npcs.is_empty() || modules.len() >= 36 {
        return;
    }
    let lines = npcs
        .iter()
        .take(8)
        .map(|n| {
            let icon = if n.attitude.contains("信任")
                || n.attitude.contains("友好")
                || n.attitude.contains("信赖")
            {
                "🤝"
            } else if n.attitude.contains("敌对") || n.attitude.contains("愤怒") {
                "⚔️"
            } else {
                "👀"
            };
            format!(
                "- {} **{}** `{}` — {}",
                icon,
                truncate_chars(&n.name, 20),
                truncate_chars(&n.attitude, 12),
                truncate_chars(&n.status, 60)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    modules.push(json!({ "type": "divider" }));
    modules.push(json!({
        "type": "section",
        "text": { "type": "kmarkdown", "content": format!("🗣️ **关键 NPC 动向与态度**\n{lines}") }
    }));
}

pub fn push_bulletin_section(modules: &mut Vec<Value>, b: &WorldBulletin) {
    if modules.len() >= 38 { return; }
    let fac = if b.faction.trim().is_empty() { String::new() } else { format!(" `{}`", b.faction.trim()) };
    let body = b.body.trim().replace('\n', "\n> ");
    let content = format!("📰 **{}**{fac} — **【{}】**\n> {body}", b.masthead, b.headline);
    modules.push(json!({ "type": "divider" }));
    modules.push(json!({ "type": "section", "text": { "type": "kmarkdown", "content": truncate_chars(&content, 1200) } }));
}

pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
    }
}

pub fn chunk_kmarkdown(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for paragraph in text.split('\n') {
        let p_len = paragraph.chars().count();
        if current_len + p_len + 1 > max_chars && !current.is_empty() {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
        if p_len > max_chars {
            let mut slice = String::new();
            let mut slice_len = 0usize;
            for ch in paragraph.chars() {
                slice.push(ch);
                slice_len += 1;
                if slice_len >= max_chars {
                    chunks.push(slice);
                    slice = String::new();
                    slice_len = 0;
                }
            }
            if !slice.is_empty() {
                current = slice;
                current_len = slice_len;
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
                current_len += 1;
            }
            current.push_str(paragraph);
            current_len += p_len;
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(" ".to_string());
    }
    chunks
}
