use serde_json::{Value, json};
use tavern_domain::{ChatMode, EngineResponse, NarrativeStyle};

use crate::card_widgets::{
    notice_action_bar, push_party_cards, render_scenario_menu_card, status_console_rows,
    story_suggestion_buttons, story_toolbar, truncate_chars,
};

pub fn thinking_placeholder_card_json() -> String {
    json!([{
        "type": "card",
        "theme": "secondary",
        "size": "lg",
        "modules": [{
            "type": "section",
            "text": {
                "type": "kmarkdown",
                "content": "⏳ **DM 正在翻阅模组、推演本轮剧情，请稍候几秒喵…**"
            }
        }]
    }])
    .to_string()
}

pub fn render_kook_card_json(channel_id: &str, resp: &EngineResponse) -> String {
    match resp {
        EngineResponse::Ignore | EngineResponse::PrivateNotice { .. } => "[]".to_string(),
        EngineResponse::ScenarioMenu {
            channel_id: menu_chan,
            scenarios,
        } => render_scenario_menu_card(menu_chan, scenarios),
        EngineResponse::Notice {
            title,
            body,
            theme,
            show_actions,
        } => {
            let mut modules = vec![json!({
                "type": "header",
                "text": { "type": "plain-text", "content": truncate_chars(title, 100) }
            })];
            push_chunked_sections(&mut modules, body, 3200);
            if *show_actions {
                modules.push(json!({ "type": "divider" }));
                modules.push(notice_action_bar(channel_id));
            }
            json!([{
                "type": "card",
                "theme": theme.kook_str(),
                "size": "lg",
                "modules": modules
            }])
            .to_string()
        }
        EngineResponse::StoryTurn {
            channel_id: turn_chan,
            turn_number,
            scene_title,
            location,
            world_time,
            narrative,
            state_changes,
            whisper_hints,
            suggested_actions,
            ..
        } => {
            let mut modules = vec![
                json!({
                    "type": "header",
                    "text": { "type": "plain-text", "content": truncate_chars(&format!("🎭 {scene_title}"), 100) }
                }),
                json!({
                    "type": "context",
                    "elements": [{
                        "type": "kmarkdown",
                        "content": truncate_chars(
                            &format!("📍 **{}**　｜　🕒 {}　｜　🎲 第 **{}** 回合", location, world_time, turn_number),
                            500
                        )
                    }]
                }),
                json!({ "type": "divider" }),
            ];

            push_chunked_sections(&mut modules, narrative, 3200);

            if !state_changes.is_empty() || !whisper_hints.is_empty() {
                modules.push(json!({ "type": "divider" }));
                let mut lines = Vec::new();
                lines.extend(state_changes.iter().map(|s| format!("> {s}")));
                lines.extend(whisper_hints.iter().map(|s| format!("> {s}")));
                push_chunked_sections(&mut modules, &lines.join("\n"), 3200);
            }

            if let Some(pick_row) =
                story_suggestion_buttons(turn_chan, *turn_number, suggested_actions)
            {
                let badges = ["①", "②", "③"];
                let hints = suggested_actions
                    .iter()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .take(3)
                    .enumerate()
                    .map(|(i, s)| format!("{} {}", badges[i], truncate_chars(s, 45)))
                    .collect::<Vec<_>>()
                    .join("　");
                modules.push(json!({ "type": "divider" }));
                modules.push(json!({
                    "type": "context",
                    "elements": [{
                        "type": "kmarkdown",
                        "content": truncate_chars(&format!("✨ **点按钮直接行动**（或直接打人话）：{hints}"), 500)
                    }]
                }));
                modules.push(pick_row);
            } else {
                modules.push(json!({ "type": "divider" }));
            }

            modules.push(story_toolbar(turn_chan));

            json!([{
                "type": "card",
                "theme": "primary",
                "size": "lg",
                "modules": modules
            }])
            .to_string()
        }
        EngineResponse::StatusBoard {
            channel_id: board_chan,
            room,
            players,
            pending_actions,
        } => {
            let mode_badge = match room.chat_mode {
                ChatMode::Roleplay => "💬 直接说话",
                ChatMode::Strict => "⌨️ 指令前缀",
            };
            let style_badge = match room.narrative_style {
                NarrativeStyle::Voice => "🎙️ 语音极简",
                NarrativeStyle::Prose => "📖 文字沉浸",
            };
            let mut modules = vec![
                json!({
                    "type": "header",
                    "text": { "type": "plain-text", "content": truncate_chars(&format!("📊 《{}》战局与控制台", room.title), 100) }
                }),
                json!({
                    "type": "context",
                    "elements": [{
                        "type": "kmarkdown",
                        "content": truncate_chars(
                            &format!(
                                "📍 {} ｜ 🕒 {} ｜ 🎲 第 {} 回合 ｜ {} ｜ {}",
                                room.location, room.world_time, room.turn_number, mode_badge, style_badge
                            ),
                            500
                        )
                    }]
                }),
                json!({ "type": "divider" }),
            ];

            push_party_cards(&mut modules, players, pending_actions);

            if !room.clues.is_empty() {
                modules.push(json!({ "type": "divider" }));
                let clues = room
                    .clues
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n");
                push_chunked_sections(&mut modules, &format!("🔎 **已掌握线索**\n{clues}"), 3200);
            }

            modules.push(json!({ "type": "divider" }));
            let [row1, row2] = status_console_rows(board_chan, room);
            modules.push(row1);
            modules.push(row2);

            json!([{
                "type": "card",
                "theme": "info",
                "size": "lg",
                "modules": modules
            }])
            .to_string()
        }
    }
}

fn push_chunked_sections(modules: &mut Vec<Value>, text: &str, max_chars: usize) {
    for chunk in chunk_kmarkdown(text, max_chars) {
        if modules.len() >= 40 {
            break;
        }
        modules.push(json!({
            "type": "section",
            "text": { "type": "kmarkdown", "content": chunk }
        }));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plain::render_plain_fallback;
    use tavern_domain::ScenarioSummary;

    fn assert_valid_kook_card(raw: &str) {
        let v: Value = serde_json::from_str(raw).expect("valid json array");
        assert!(v.is_array());
        let modules = v[0]["modules"].as_array().expect("modules array");
        assert!(modules.len() <= 50, "KOOK max 50 modules per card");
        for m in modules {
            if m["type"] == "action-group" {
                let elems = m["elements"].as_array().expect("elements array");
                assert!(
                    elems.len() <= 4,
                    "KOOK max 4 buttons per action-group, got {}",
                    elems.len()
                );
            }
        }
    }

    #[test]
    fn builds_valid_kook_cards_with_interactive_buttons_and_zero_whisper_leak() {
        let resp = EngineResponse::StoryTurn {
            channel_id: "ch_99".into(),
            turn_number: 2,
            scene_title: "酒馆暗流".into(),
            location: "余烬酒馆".into(),
            world_time: "21:20".into(),
            narrative: "老雷恩推过来一杯麦酒。".into(),
            state_changes: vec!["**陆沉**：🎒 获得「钥匙」".into()],
            whisper_hints: vec!["🤫 DM 悄悄给 **陆沉** 递了一张秘密纸条".into()],
            suggested_actions: vec!["追问白鸦下落".into(), "潜入地下酒窖".into()],
            direct_messages: vec![tavern_domain::DirectMessageOut {
                user_id: "u1".into(),
                player_name: "陆沉".into(),
                content: "绝密暗格密码0317".into(),
            }],
        };
        let raw = render_kook_card_json("ch_99", &resp);
        assert_valid_kook_card(&raw);
        assert!(raw.contains("tavern:pick:ch_99:2:0"));
        assert!(raw.contains("tavern:pick:ch_99:2:1"));
        assert!(raw.contains("tavern:roll:ch_99:2d6"));
        assert!(raw.contains("tavern:whisper:ch_99"));
        assert!(!raw.contains("绝密暗格密码0317"));

        let menu = EngineResponse::ScenarioMenu {
            channel_id: "ch_99".into(),
            scenarios: vec![ScenarioSummary {
                id: "ember_tavern".into(),
                name: "余烬酒馆".into(),
                genre: "奇幻".into(),
                tagline: "暴雪夜悬赏".into(),
                worldbook_count: 5,
            }],
        };
        let menu_raw = render_kook_card_json("ch_99", &menu);
        assert_valid_kook_card(&menu_raw);
        assert!(menu_raw.contains("tavern:start:ch_99:ember_tavern"));

        let public_fallback = render_plain_fallback(&resp, false);
        assert!(public_fallback.contains("获得「钥匙」"));
        assert!(!public_fallback.contains("绝密暗格密码0317"));

        let dm_fallback = render_plain_fallback(&resp, true);
        assert!(dm_fallback.contains("绝密暗格密码0317"));
    }
}
