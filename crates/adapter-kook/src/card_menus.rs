use serde_json::{Value, json};
use tavern_domain::{PresetRole, PrivateButton, ScenarioSummary, WallReport};

use crate::card_widgets::{action_group, btn, chunk_kmarkdown, truncate_chars};

pub fn render_private_card_json(title: &str, body: &str, actions: &[PrivateButton]) -> String {
    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": truncate_chars(title, 100) }
        }),
        json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": "🔒 **仅你可见** · 队友完全看不到这张私密卡片（包含你的个人底牌与秘密纸条）"
            }]
        }),
        json!({ "type": "divider" }),
    ];
    for chunk in chunk_kmarkdown(body, 3200) {
        if modules.len() >= 40 {
            break;
        }
        modules.push(json!({
            "type": "section",
            "text": { "type": "kmarkdown", "content": chunk }
        }));
    }
    if !actions.is_empty() {
        modules.push(json!({ "type": "divider" }));
        for chunk in actions.chunks(4).take(2) {
            let btns: Vec<Value> = chunk
                .iter()
                .map(|a| {
                    if a.value.starts_with("https://") || a.value.starts_with("http://") {
                        json!({
                            "type": "button",
                            "theme": a.theme.kook_str(),
                            "value": a.value,
                            "click": "link",
                            "text": { "type": "plain-text", "content": truncate_chars(&a.label, 32) }
                        })
                    } else {
                        btn(a.theme.kook_str(), a.value.clone(), &a.label)
                    }
                })
                .collect();
            modules.push(action_group(btns));
        }
    }
    json!([{ "type": "card", "theme": "info", "size": "lg", "modules": modules }]).to_string()
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
                "content": "✨ **推荐点「🚪 开独立包间」**：秒建全新干净子频道自动开桌，绝不串旧记录、跑完一键拆桌！也可直接点绿色按钮在本频道开桌喵！"
            }]
        }),
    ];

    let mut shown = 0usize;
    for s in scenarios {
        if modules.len() >= 40 {
            break;
        }
        shown += 1;
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!(
                    "**《{}》**  `{}`\n> {}\n> 🗂️ 深度世界书 **{}** 条",
                    s.name, s.genre, s.tagline, s.worldbook_count
                )
            }
        }));
        modules.push(action_group(vec![
            btn(
                "primary",
                format!("tavern:table:{channel_id}:{}", s.id),
                "🚪 开独立包间",
            ),
            btn(
                "success",
                format!("tavern:start:{channel_id}:{}", s.id),
                &format!("▶️ 本频道开《{}》", truncate_chars(&s.name, 8)),
            ),
        ]));
    }

    if scenarios.len() > shown {
        modules.push(json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": format!("ℹ️ 当前展示前 {shown} / {} 个剧本，其余社区剧本可直接发 `/table <剧本ID>` 或 `/start <剧本ID>` 开启。", scenarios.len())
            }]
        }));
    }

    modules.push(json!({ "type": "divider" }));
    modules.push(action_group(vec![
        btn("info", format!("tavern:store:{channel_id}"), "📦 世界书商店"),
        btn("primary", format!("tavern:ws:{channel_id}:desk"), "✍️ 创作新剧本"),
        btn("secondary", format!("tavern:tables:{channel_id}"), "🚪 包间列表"),
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

pub fn render_role_picker_card(
    channel_id: &str,
    scenario_name: &str,
    roles: &[PresetRole],
) -> String {
    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": truncate_chars(&format!("🎭 《{scenario_name}》一键选职业"), 100) }
        }),
        json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": "✨ 点下方对应按钮直接携带专属装备入团（已入团玩家点击会平滑切换职业，不丢失血量与背包喵）！"
            }]
        }),
        json!({ "type": "divider" }),
    ];

    let mut role_buttons = Vec::new();
    let desc = roles
        .iter()
        .take(4)
        .enumerate()
        .map(|(i, r)| {
            role_buttons.push(btn(
                "success",
                format!("tavern:pickrole:{channel_id}:{i}"),
                &format!("🎭 {}", truncate_chars(&r.name, 10)),
            ));
            format!(
                "**{}. {}**\n> 📊 {}\n> 🏷️ 特质：{} ｜ 🎒 专属装备：{}",
                i + 1,
                r.name,
                r.stats.format_inline(),
                r.traits.join(" / "),
                if r.bonus_items.is_empty() {
                    "标准装备".to_string()
                } else {
                    r.bonus_items.join("、")
                }
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    modules.push(json!({
        "type": "section",
        "text": { "type": "kmarkdown", "content": desc }
    }));
    modules.push(json!({ "type": "divider" }));
    if !role_buttons.is_empty() {
        modules.push(action_group(role_buttons));
    }

    json!([{
        "type": "card",
        "theme": "success",
        "size": "lg",
        "modules": modules
    }])
    .to_string()
}

pub fn render_wall_report_card(report: &WallReport) -> String {
    json!([{
        "type": "card",
        "theme": "warning",
        "size": "lg",
        "modules": [
            {
                "type": "header",
                "text": { "type": "plain-text", "content": truncate_chars(&report.title, 100) }
            },
            { "type": "divider" },
            {
                "type": "section",
                "text": { "type": "kmarkdown", "content": truncate_chars(&report.body, 3500) }
            },
            {
                "type": "context",
                "elements": [{
                    "type": "kmarkdown",
                    "content": "🍺 由 AI 群酒馆引擎自动归档留念 · 完整文字实录已同步备份至服务器 exports/"
                }]
            }
        ]
    }])
    .to_string()
}
