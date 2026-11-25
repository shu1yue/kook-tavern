use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde_json::{Value, json};
use tavern_domain::{EngineResponse, LiveStoryHud};

use crate::card_widgets::{
    action_group, btn, chunk_kmarkdown, ending_toolbar, push_bulletin_section,
    push_story_action_footer, truncate_chars,
};

const MAX_TRACKED_STORY_CARDS: usize = 512;

#[derive(Debug, Clone)]
pub struct ActiveStoryCard {
    pub msg_id: String,
    pub turn_number: u32,
    pub story: Box<EngineResponse>,
}

#[derive(Debug, Clone, Default)]
pub struct LiveCardRegistry {
    cards: Arc<RwLock<HashMap<String, ActiveStoryCard>>>,
}

impl LiveCardRegistry {
    pub fn get(&self, channel_id: &str) -> Option<ActiveStoryCard> {
        self.cards
            .read()
            .ok()
            .and_then(|m| m.get(channel_id).cloned())
    }

    pub fn replace(&self, channel_id: &str, card: ActiveStoryCard) -> Option<ActiveStoryCard> {
        let mut map = self.cards.write().ok()?;
        if map.len() >= MAX_TRACKED_STORY_CARDS && !map.contains_key(channel_id) {
            map.clear();
        }
        map.insert(channel_id.to_string(), card)
    }

    pub fn take(&self, channel_id: &str) -> Option<ActiveStoryCard> {
        self.cards.write().ok()?.remove(channel_id)
    }
}

/// Renders a `StoryTurn` card in either:
/// - **Live mode (`archived = false`)**: embeds live party/turn HUD + Turn 0 inline role buttons +
///   action buttons so the player has ONE unified tabletop card;
/// - **Archived mode (`archived = true`)**: strips all buttons when advancing to the next turn,
///   leaving a clean, uninterrupted storybook feed.
pub fn render_story_turn_card(
    resp: &EngineResponse,
    hud: Option<&LiveStoryHud>,
    archived: bool,
) -> String {
    let EngineResponse::StoryTurn {
        channel_id: turn_chan,
        turn_number,
        scene_title,
        location,
        world_time,
        act_badge,
        narrative,
        state_changes,
        whisper_hints,
        suggested_actions,
        suggested_check,
        world_bulletin,
        ending_reached,
        ..
    } = resp
    else {
        return "[]".to_string();
    };

    let act_part = act_badge
        .as_deref()
        .map(|a| format!("　｜　🎬 {a}"))
        .unwrap_or_default();
    let rule_part = hud
        .filter(|h| !h.rule_badge.is_empty())
        .map(|h| format!("　｜　📐 {}", h.rule_badge))
        .unwrap_or_default();
    let header_title = if let Some(end) = ending_reached {
        format!("🏆 终局 · {}（{}）", end.title, scene_title)
    } else {
        format!("🎭 {scene_title}")
    };

    let mut modules = vec![
        json!({
            "type": "header",
            "text": { "type": "plain-text", "content": truncate_chars(&header_title, 100) }
        }),
        json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": truncate_chars(
                    &format!("📍 **{}**　｜　🕒 {}　｜　🎲 第 **{}** 回合{act_part}{rule_part}", location, world_time, turn_number),
                    500
                )
            }]
        }),
        json!({ "type": "divider" }),
    ];

    push_story_chunks(&mut modules, narrative, 3200);

    if let Some(b) = world_bulletin {
        push_bulletin_section(&mut modules, b);
    }

    if (!state_changes.is_empty() || !whisper_hints.is_empty()) && modules.len() < 36 {
        modules.push(json!({ "type": "divider" }));
        let mut lines = Vec::new();
        lines.extend(state_changes.iter().map(|s| format!("> {s}")));
        lines.extend(whisper_hints.iter().map(|s| format!("> {s}")));
        push_story_chunks(&mut modules, &lines.join("\n"), 3200);
    }

    if archived {
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": format!("📖 第 **{turn_number}** 回合已推进 · 请看下方最新剧情卡")
            }]
        }));
        return json!([{ "type": "card", "theme": "secondary", "size": "lg", "modules": modules }])
            .to_string();
    }

    if let Some(end) = ending_reached {
        let unlock_tag = if end.newly_unlocked {
            " 🆕 **本服首次解锁成就！**"
        } else {
            ""
        };
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": format!(
                    "🏆 **达成结局：《{}》**{unlock_tag}\n> 冒险已圆满收束！点下方按钮发战报墙留念、查看结局图鉴或回档重选命运。",
                    end.title
                )
            }
        }));
        modules.push(ending_toolbar(turn_chan));
    } else {
        if let Some(h) = hud
            && modules.len() < 36
        {
            push_live_hud_modules(&mut modules, turn_chan, h);
        }
        push_story_action_footer(
            &mut modules,
            turn_chan,
            *turn_number,
            suggested_actions,
            suggested_check.as_ref(),
        );
    }

    let card_theme = if ending_reached.is_some() {
        "warning"
    } else {
        "primary"
    };
    json!([{ "type": "card", "theme": card_theme, "size": "lg", "modules": modules }]).to_string()
}

fn push_live_hud_modules(modules: &mut Vec<Value>, channel_id: &str, hud: &LiveStoryHud) {
    let mut hud_lines = Vec::new();
    if !hud.party_lines.is_empty() {
        hud_lines.push(format!("👥 **当前小队**：{}", hud.party_lines.join(" ｜ ")));
    } else if !hud.opening_roles.is_empty() {
        hud_lines.push("👥 **当前小队**：暂无入团角色 — 点下方绿色职业按钮直接上桌（原地刷新不刷屏）！".into());
    }
    if !hud.turn_progress.is_empty() {
        hud_lines.push(format!("📌 **本轮动态**：{}", hud.turn_progress.join(" ｜ ")));
    }
    if !hud_lines.is_empty() {
        modules.push(json!({ "type": "divider" }));
        modules.push(json!({
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": truncate_chars(&hud_lines.join("\n"), 1800)
            }
        }));
    }

    // On Turn 0, embed the 4 preset roles directly inside the Opening Card so players NEVER
    // have to open a separate RolePicker card that interrupts the opening story!
    if !hud.opening_roles.is_empty() && modules.len() < 38 {
        let role_brief = hud
            .opening_roles
            .iter()
            .take(4)
            .map(|r| format!("**{}**（{}）", r.name, r.traits.join("/")))
            .collect::<Vec<_>>()
            .join(" · ");
        let role_btns: Vec<Value> = hud
            .opening_roles
            .iter()
            .take(4)
            .enumerate()
            .map(|(i, r)| {
                btn(
                    "success",
                    format!("tavern:pickrole:{channel_id}:{i}"),
                    &format!("🎭 {}", truncate_chars(&r.name, 10)),
                )
            })
            .collect();
        modules.push(json!({
            "type": "context",
            "elements": [{
                "type": "kmarkdown",
                "content": truncate_chars(&format!("🎭 **快捷选职业入团**：{role_brief}"), 500)
            }]
        }));
        modules.push(action_group(role_btns));
    }
}

fn push_story_chunks(modules: &mut Vec<Value>, text: &str, max_chars: usize) {
    for chunk in chunk_kmarkdown(text, max_chars) {
        if modules.len() >= 34 {
            break;
        }
        modules.push(json!({
            "type": "section",
            "text": { "type": "kmarkdown", "content": chunk }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tavern_domain::{CharacterStats, PresetRole};

    #[test]
    fn live_story_card_embeds_opening_roles_hud_and_archives_cleanly() {
        let story = EngineResponse::StoryTurn {
            channel_id: "ch_live".into(),
            turn_number: 0,
            scene_title: "余烬酒馆 · 序幕".into(),
            location: "酒馆大厅".into(),
            world_time: "21:00".into(),
            act_badge: Some("第 1/3 幕 · 暴雪夜".into()),
            narrative: "暴雪呼啸，木门被撞开。".into(),
            state_changes: vec![],
            whisper_hints: vec![],
            suggested_actions: vec!["盘问老酒保".into(), "检查断钥匙".into()],
            suggested_check: None,
            world_bulletin: None,
            ending_reached: None,
            direct_messages: vec![],
        };
        let hud = LiveStoryHud {
            turn_number: 0,
            rule_badge: "经典 D&D (d20)".into(),
            party_lines: vec!["**沐涵**（齿轮机械师 · ❤️12/12 🧠10/10）".into()],
            turn_progress: vec!["🎲 **沐涵**：🔍 侦查 16（✨成功）（待行动）".into()],
            opening_roles: vec![PresetRole {
                name: "齿轮机械师".into(),
                traits: vec!["手巧".into()],
                bonus_items: vec![],
                stats: CharacterStats::default(),
                alignment: String::new(),
                proficient_skills: vec![],
                abilities: vec![],
            }],
        };

        let live_json = render_story_turn_card(&story, Some(&hud), false);
        let v: Value = serde_json::from_str(&live_json).unwrap();
        let mods = v[0]["modules"].as_array().unwrap();
        assert!(mods.len() <= 50);
        assert!(live_json.contains("tavern:pickrole:ch_live:0"));
        assert!(live_json.contains("沐涵"));
        assert!(live_json.contains("侦查 16"));

        let archived_json = render_story_turn_card(&story, None, true);
        let av: Value = serde_json::from_str(&archived_json).unwrap();
        let amods = av[0]["modules"].as_array().unwrap();
        assert!(amods.iter().all(|m| m["type"] != "action-group"));
        assert!(archived_json.contains("已推进"));
    }
}
