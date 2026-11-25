use serde_json::{Value, json};
use tavern_domain::{WorldbookPack, WorldbookPackSummary, WorkshopDraft};

use crate::card_widgets::{action_group, btn, truncate_chars};

pub fn render_worldbook_store_card(channel_id: &str, packs: &[WorldbookPackSummary]) -> String {
    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": "📦 开源世界书商店 · 挑选你的世界观底座" }
        }),
        json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": "✨ **零门槛共创剧本**：先选一套开源世界书当底座（含丰富设定+势力+3个现成故事钩子），进工坊一键 AI 锻造就能直接上架开团喵！"
            }]
        }),
    ];

    for p in packs.iter().take(8) {
        if modules.len() >= 40 {
            break;
        }
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!(
                    "**《{}》** `{}`\n> {}\n> 📐 推荐：**{}** · **{} 幕** ｜ 🗂️ **{}** 条世界书 ｜ 📄 `{}`",
                    p.name,
                    p.genre,
                    truncate_chars(&p.summary, 110),
                    p.recommended_system.short_badge(),
                    p.recommended_acts,
                    p.entry_count,
                    p.license
                )
            }
        }));
        modules.push(action_group(vec![
            btn("info", format!("tavern:store:{channel_id}:{}", p.id), "👁️ 看设定与灵感钩子"),
            btn("success", format!("tavern:ws:{channel_id}:wb:{}", p.id), "✍️ 用此世界书创作"),
        ]));
    }

    modules.push(json!({ "type": "divider" }));
    modules.push(action_group(vec![
        btn("primary", format!("tavern:ws:{channel_id}:desk"), "🛠️ 我的创作工坊"),
        btn("secondary", format!("tavern:menu:{channel_id}"), "📚 返回已上架剧本"),
    ]));

    json!([{ "type": "card", "theme": "primary", "size": "lg", "modules": modules }]).to_string()
}

pub fn render_worldbook_detail_card(channel_id: &str, pack: &WorldbookPack) -> String {
    let facs = if pack.factions.is_empty() {
        "无固定势力".to_string()
    } else {
        pack.factions
            .iter()
            .map(|f| format!("- **{}**：{}", f.name, truncate_chars(&f.goal, 48)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let toc = pack
        .entries
        .iter()
        .take(8)
        .map(|e| {
            let tag = if e.always_active { "常驻" } else { "触发" };
            format!("- `[{}]` **{}** — {}", tag, e.title, truncate_chars(&e.content, 52))
        })
        .collect::<Vec<_>>()
        .join("\n");

    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": truncate_chars(&format!("📖 开源世界书 · 《{}》", pack.name), 100) }
        }),
        json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": format!(
                    "🏷️ {} ｜ 📐 推荐 {} · {} 幕 ｜ 📄 {} ｜ 作者：{}",
                    pack.genre,
                    pack.recommended_system.short_badge(),
                    pack.recommended_acts,
                    pack.license,
                    if pack.author.is_empty() { "开源社区" } else { &pack.author }
                )
            }]
        }),
        json!({ "type": "divider" }),
        json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": truncate_chars(
                    &format!("{}\n\n**⚔️ 核心势力**\n{facs}\n\n**🗂️ 收录设定（{} 条）**\n{toc}", pack.summary, pack.entries.len()),
                    3200
                )
            }
        }),
    ];

    if !pack.starter_hooks.is_empty() {
        let badges = ["①", "②", "③"];
        let hook_lines = pack
            .starter_hooks
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, h)| format!("**{}** {}", badges[i], h))
            .collect::<Vec<_>>()
            .join("\n");
        let hook_btns: Vec<Value> = pack
            .starter_hooks
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, _)| {
                btn(
                    "primary",
                    format!("tavern:ws:{channel_id}:hook{}:{}", i + 1, pack.id),
                    &format!("🎯 选用钩子{}", badges[i]),
                )
            })
            .collect();
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!("**🎯 防卡壳灵感种子（点按钮直接带入工坊草稿！）**\n{hook_lines}")
            }
        }));
        modules.push(action_group(hook_btns));
    }

    modules.push(json!({ "type": "divider" }));
    modules.push(action_group(vec![
        btn("success", format!("tavern:ws:{channel_id}:wb:{}", pack.id), "✍️ 选用世界书并自由写脑洞"),
        btn("secondary", format!("tavern:store:{channel_id}"), "📦 返回商店"),
    ]));

    json!([{ "type": "card", "theme": "info", "size": "lg", "modules": modules }]).to_string()
}

pub fn render_workshop_desk_card(
    channel_id: &str,
    draft: &WorkshopDraft,
    base_pack: Option<&WorldbookPack>,
) -> String {
    let wb_line = match base_pack {
        Some(p) => format!("**《{}》**（自带 {} 条世界书 + {} 个势力）", p.name, p.entries.len(), p.factions.len()),
        None => "✨ **空白原创世界观**（可点下方「📦 挑世界书」引入现成背景）".to_string(),
    };
    let title_line = if draft.title.trim().is_empty() {
        "（留空由 AI 自动起名，或发 `/workshop title <剧名>`）".to_string()
    } else {
        format!("**《{}》**", draft.title.trim())
    };
    let premise_line = if draft.premise.trim().is_empty() {
        "⏳ **尚未填写** — 点下方灵感钩子，或发 `/workshop idea <你的主线脑洞>`".to_string()
    } else {
        format!("✅ {}", truncate_chars(draft.premise.trim(), 180))
    };
    let custom_lore_line = if draft.custom_lore.is_empty() {
        "0 / 6 条（可选：发 `/workshop lore 标题 | 关键词 | 设定` 追加私房秘密）".to_string()
    } else {
        let names = draft.custom_lore.iter().map(|e| e.title.as_str()).collect::<Vec<_>>().join("、");
        format!("{} / 6 条（{}）", draft.custom_lore.len(), truncate_chars(&names, 80))
    };

    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": "✍️ 剧本引导创作工坊 · 5 步锻造你的跑团模组" }
        }),
        json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!(
                    "**① 世界书底座**：{wb_line}\n\
                     **② 篇幅与规则**：**{}** ｜ **{}** ｜ 主题 `{}`\n\
                     **③ 自定义剧名**：{title_line}\n\
                     **④ 主线脑洞**：{premise_line}\n\
                     **⑤ 私房世界书**：{custom_lore_line}",
                    draft.length_label(),
                    draft.check_system.short_badge(),
                    draft.theme
                )
            }
        }),
    ];

    if !draft.forged_summary.trim().is_empty() && draft.forged_scenario.is_some() {
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!(
                    "✨ **锻造预览已就绪（已通过 Schema 强类型校验）**\n{}",
                    truncate_chars(&draft.forged_summary, 900)
                )
            }
        }));
    }

    modules.push(json!({ "type": "divider" }));
    modules.push(action_group(vec![
        btn(if draft.act_count == 3 { "success" } else { "secondary" }, format!("tavern:ws:{channel_id}:acts:3"), "⚡ 3幕短篇"),
        btn(if draft.act_count == 5 { "success" } else { "secondary" }, format!("tavern:ws:{channel_id}:acts:5"), "⚔️ 5幕中篇"),
        btn(if draft.act_count == 6 { "success" } else { "secondary" }, format!("tavern:ws:{channel_id}:acts:6"), "👑 6幕史诗"),
        btn("info", format!("tavern:store:{channel_id}"), "📦 挑世界书"),
    ]));
    modules.push(action_group(vec![
        btn(if draft.check_system.as_str() == "d20" { "success" } else { "secondary" }, format!("tavern:ws:{channel_id}:rule:d20"), "🐉 D&D d20"),
        btn(if draft.check_system.as_str() == "2d6" { "success" } else { "secondary" }, format!("tavern:ws:{channel_id}:rule:2d6"), "🎲 2d6三档"),
        btn(if draft.check_system.as_str() == "d100" { "success" } else { "secondary" }, format!("tavern:ws:{channel_id}:rule:d100"), "🐙 CoC d100"),
    ]));

    if let Some(pack) = base_pack
        && !pack.starter_hooks.is_empty()
    {
        let badges = ["①", "②", "③"];
        let h_btns: Vec<Value> = pack
            .starter_hooks
            .iter()
            .take(3)
            .enumerate()
            .map(|(i, _)| {
                btn("info", format!("tavern:ws:{channel_id}:hook:{}", i + 1), &format!("🎯 灵感钩子{}", badges[i]))
            })
            .collect();
        modules.push(action_group(h_btns));
    }

    let mut final_row = vec![btn("primary", format!("tavern:ws:{channel_id}:forge"), "🔥 AI 锻造剧本")];
    if draft.forged_scenario.is_some() {
        final_row.push(btn("success", format!("tavern:ws:{channel_id}:publish"), "🚀 一键上架酒馆"));
    }
    final_row.push(btn("danger", format!("tavern:ws:{channel_id}:clear"), "🗑️ 清空草稿"));
    modules.push(action_group(final_row));

    let theme = if draft.forged_scenario.is_some() { "success" } else { "info" };
    json!([{ "type": "card", "theme": theme, "size": "lg", "modules": modules }]).to_string()
}
