use serde_json::{Value, json};
use tavern_domain::{ChatMode, NarrativeStyle, PendingAction, Player, RoomState, ScenarioSummary};

pub fn btn(theme: &str, value: String, label: &str) -> Value {
    json!({
        "type": "button",
        "theme": theme,
        "value": value,
        "click": "return-val",
        "text": { "type": "plain-text", "content": truncate_chars(label, 24) }
    })
}

pub fn action_group(buttons: Vec<Value>) -> Value {
    // KOOK CardMessage hard limit: at most 4 buttons per action-group!
    let bounded: Vec<Value> = buttons.into_iter().take(4).collect();
    json!({
        "type": "action-group",
        "elements": bounded
    })
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

pub fn notice_action_bar(channel_id: &str) -> Value {
    action_group(vec![
        btn("primary", format!("tavern:resolve:{channel_id}"), "⚡ 立刻结算"),
        btn("secondary", format!("tavern:roll:{channel_id}:2d6"), "🎲 掷 2d6"),
        btn("info", format!("tavern:status:{channel_id}"), "🎛️ 面板/设置"),
        btn("warning", format!("tavern:afk:{channel_id}"), "💤 暂离/归队"),
    ])
}

pub fn story_suggestion_buttons(
    channel_id: &str,
    turn_number: u32,
    suggested_actions: &[String],
) -> Option<Value> {
    let badges = ["①", "②", "③"];
    let mut elems = Vec::new();
    for (idx, text) in suggested_actions
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .take(3)
        .enumerate()
    {
        let label = format!("{} {}", badges[idx], truncate_chars(text, 13));
        elems.push(btn(
            "info",
            format!("tavern:pick:{channel_id}:{turn_number}:{idx}"),
            &label,
        ));
    }
    if elems.is_empty() {
        None
    } else {
        Some(action_group(elems))
    }
}

pub fn story_toolbar(channel_id: &str) -> Value {
    action_group(vec![
        btn("secondary", format!("tavern:roll:{channel_id}:2d6"), "🎲 掷 2d6"),
        btn("primary", format!("tavern:resolve:{channel_id}"), "⚡ 立刻结算"),
        btn("info", format!("tavern:whisper:{channel_id}"), "🤫 查纸条"),
        btn("secondary", format!("tavern:status:{channel_id}"), "🎛️ 面板/设置"),
    ])
}

pub fn status_console_rows(channel_id: &str, room: &RoomState) -> [Value; 2] {
    let style_label = match room.narrative_style {
        NarrativeStyle::Voice => "📖 切文字沉浸",
        NarrativeStyle::Prose => "🎙️ 切语音极简",
    };
    let mode_label = match room.chat_mode {
        ChatMode::Roleplay => "⌨️ 切指令前缀",
        ChatMode::Strict => "💬 切直接说话",
    };
    let row1 = action_group(vec![
        btn("success", format!("tavern:quickjoin:{channel_id}"), "⚔️ 快速入团"),
        btn("secondary", format!("tavern:roll:{channel_id}:1d100"), "🎲 掷 d100"),
        btn("warning", format!("tavern:afk:{channel_id}"), "💤 暂离/归队"),
        btn("danger", format!("tavern:undo:{channel_id}"), "↩️ 回档上轮"),
    ]);
    let row2 = action_group(vec![
        btn("info", format!("tavern:toggle_style:{channel_id}"), style_label),
        btn("info", format!("tavern:toggle_mode:{channel_id}"), mode_label),
        btn("secondary", format!("tavern:cost:{channel_id}"), "💰 查账单"),
        btn("secondary", format!("tavern:menu:{channel_id}"), "📚 换剧本"),
    ]);
    [row1, row2]
}

pub fn push_party_cards(
    modules: &mut Vec<Value>,
    players: &[Player],
    pending_actions: &[PendingAction],
) {
    if players.is_empty() {
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": "🪑 **酒馆长桌空着呢** — 点下方 **「⚔️ 快速入团」** 一键上桌，或发 `/join 角色名 职业 | 特质` 捏人喵！"
            }
        }));
        return;
    }

    for p in players {
        if modules.len() >= 36 {
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
        let status_txt = if p.status_note.is_empty() {
            "状态正常"
        } else {
            &p.status_note
        };
        let inv = if p.inventory.is_empty() {
            "空".to_string()
        } else {
            p.inventory.join(" · ")
        };
        let content = format!(
            "**{}**（{}） `{}`\n❤️ {}　｜　🧠 {}\n🏷️ {}　｜　🎒 {}",
            p.character_name,
            p.role_desc,
            badge,
            hp_bar,
            san_bar,
            status_txt,
            truncate_chars(&inv, 140)
        );
        modules.push(json!({
            "type": "section",
            "text": { "type": "kmarkdown", "content": content }
        }));
    }
}

pub fn render_scenario_menu_card(channel_id: &str, scenarios: &[ScenarioSummary]) -> String {
    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": "🍺 群酒馆 · 选择冒险剧本" }
        }),
        json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": "✨ **零代码开玩**：直接点下方绿色按钮在本频道开桌，全员直接打人话或点选项按钮即可推进剧情喵！"
            }]
        }),
    ];

    for s in scenarios {
        if modules.len() >= 40 {
            break;
        }
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!(
                    "**《{}》**  `{}`\n> {}\n> 🗂️ 内置世界书词条 **{}** 条",
                    s.name, s.genre, s.tagline, s.worldbook_count
                )
            }
        }));
        modules.push(action_group(vec![btn(
            "success",
            format!("tavern:start:{channel_id}:{}", s.id),
            &format!("▶️ 开启《{}》", truncate_chars(&s.name, 12)),
        )]));
    }

    modules.push(json!({ "type": "divider" }));
    modules.push(action_group(vec![
        btn("info", format!("tavern:status:{channel_id}"), "📊 当前战局"),
        btn("secondary", format!("tavern:help:{channel_id}"), "📖 玩法指南"),
    ]));

    json!([{
        "type": "card",
        "theme": "primary",
        "size": "lg",
        "modules": modules
    }])
    .to_string()
}

pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
    }
}
