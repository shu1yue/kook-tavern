use serde_json::{Value, json};
use tavern_domain::EngineResponse;

use crate::plain::format_party_block;

pub fn render_kook_card_json(channel_id: &str, resp: &EngineResponse) -> String {
    match resp {
        EngineResponse::Ignore => "[]".to_string(),
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
                modules.push(action_buttons_module(channel_id));
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
                            &format!("📍 **{}** ｜ 🕒 {} ｜ 🎲 第 **{}** 回合", location, world_time, turn_number),
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
                lines.extend(state_changes.iter().cloned());
                lines.extend(whisper_hints.iter().cloned());
                push_chunked_sections(&mut modules, &lines.join("\n"), 3200);
            }

            if !suggested_actions.is_empty() {
                let hints = suggested_actions
                    .iter()
                    .take(4)
                    .enumerate()
                    .map(|(i, s)| format!("{}. {}", i + 1, truncate_chars(s, 60)))
                    .collect::<Vec<_>>()
                    .join("　");
                modules.push(json!({
                    "type": "context",
                    "elements": [{
                        "type": "kmarkdown",
                        "content": truncate_chars(&format!("💡 **行动灵感**：{hints}"), 500)
                    }]
                }));
            }

            modules.push(json!({ "type": "divider" }));
            modules.push(action_buttons_module(turn_chan));

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
            let mut modules = vec![
                json!({
                    "type": "header",
                    "text": { "type": "plain-text", "content": truncate_chars(&format!("📊 《{}》当前战局面板", room.title), 100) }
                }),
                json!({
                    "type": "context",
                    "elements": [{
                        "type": "kmarkdown",
                        "content": truncate_chars(
                            &format!(
                                "📍 {} ｜ 🕒 {} ｜ 🎲 第 {} 回合 ｜ 模式：`{}` ｜ 档位：`{}`",
                                room.location,
                                room.world_time,
                                room.turn_number,
                                room.chat_mode.as_str(),
                                room.narrative_style.as_str()
                            ),
                            500
                        )
                    }]
                }),
                json!({ "type": "divider" }),
            ];

            let party_text = format_party_block(players, pending_actions);
            push_chunked_sections(&mut modules, &party_text, 3200);

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
            modules.push(action_buttons_module(board_chan));

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
        if modules.len() >= 42 {
            break;
        }
        modules.push(json!({
            "type": "section",
            "text": { "type": "kmarkdown", "content": chunk }
        }));
    }
}

fn action_buttons_module(channel_id: &str) -> Value {
    json!({
        "type": "action-group",
        "elements": [
            {
                "type": "button",
                "theme": "primary",
                "value": format!("tavern:resolve:{channel_id}"),
                "click": "return-val",
                "text": { "type": "plain-text", "content": "⚡ 立刻结算本轮" }
            },
            {
                "type": "button",
                "theme": "info",
                "value": format!("tavern:status:{channel_id}"),
                "click": "return-val",
                "text": { "type": "plain-text", "content": "📊 查看状态" }
            },
            {
                "type": "button",
                "theme": "warning",
                "value": format!("tavern:undo:{channel_id}"),
                "click": "return-val",
                "text": { "type": "plain-text", "content": "↩️ 回档上轮" }
            }
        ]
    })
}

pub fn chunk_kmarkdown(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for paragraph in text.split('\n') {
        if current.chars().count() + paragraph.chars().count() + 1 > max_chars && !current.is_empty()
        {
            chunks.push(current);
            current = String::new();
        }
        if paragraph.chars().count() > max_chars {
            let mut slice = String::new();
            for ch in paragraph.chars() {
                slice.push(ch);
                if slice.chars().count() >= max_chars {
                    chunks.push(slice);
                    slice = String::new();
                }
            }
            if !slice.is_empty() {
                current = slice;
            }
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(paragraph);
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

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plain::render_plain_fallback;

    #[test]
    fn builds_valid_kook_card_json_with_buttons() {
        let resp = EngineResponse::StoryTurn {
            channel_id: "ch_99".into(),
            turn_number: 2,
            scene_title: "酒馆暗流".into(),
            location: "余烬酒馆".into(),
            world_time: "21:20".into(),
            narrative: "老雷恩推过来一杯麦酒。".into(),
            state_changes: vec!["**陆沉**：🎒 获得「钥匙」".into()],
            whisper_hints: vec!["🤫 DM 悄悄给 **陆沉** 递了一张秘密纸条".into()],
            suggested_actions: vec!["追问白鸦".into()],
            direct_messages: vec![tavern_domain::DirectMessageOut {
                user_id: "u1".into(),
                player_name: "陆沉".into(),
                content: "绝密暗格密码0317".into(),
            }],
        };
        let raw = render_kook_card_json("ch_99", &resp);
        let v: Value = serde_json::from_str(&raw).expect("valid json array");
        assert!(v.is_array());
        assert_eq!(v[0]["type"], "card");
        assert!(raw.contains("tavern:resolve:ch_99"));
        assert!(!raw.contains("绝密暗格密码0317"));

        let public_fallback = render_plain_fallback(&resp, false);
        assert!(public_fallback.contains("获得「钥匙」"));
        assert!(!public_fallback.contains("绝密暗格密码0317"));

        let dm_fallback = render_plain_fallback(&resp, true);
        assert!(dm_fallback.contains("绝密暗格密码0317"));
    }
}
