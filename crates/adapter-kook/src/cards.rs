use serde_json::{Value, json};
use tavern_domain::{ChatMode, CheckSystem, EngineResponse, NarrativeStyle, format_clock_bar};

use crate::card_menus::{render_role_picker_card, render_scenario_menu_card};
use crate::card_widgets::{
    notice_action_bar, push_npc_section, push_party_cards, status_console_rows, truncate_chars,
};
use crate::card_workshop::{
    render_worldbook_detail_card, render_worldbook_store_card, render_workshop_desk_card,
};
use crate::live_cards::render_story_turn_card;

pub use crate::card_menus::render_private_card_json;

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
    render_kook_card_json_with_system(channel_id, resp, CheckSystem::default())
}

pub fn render_kook_card_json_with_system(
    channel_id: &str,
    resp: &EngineResponse,
    check_system: CheckSystem,
) -> String {
    match resp {
        EngineResponse::Ignore
        | EngineResponse::PrivateNotice { .. }
        | EngineResponse::CreateTableRequest { .. }
        | EngineResponse::CloseTableRequest { .. } => "[]".to_string(),
        EngineResponse::WorldbookStoreMenu {
            channel_id: store_chan,
            packs,
        } => render_worldbook_store_card(store_chan, packs),
        EngineResponse::WorldbookDetail {
            channel_id: det_chan,
            pack,
        } => render_worldbook_detail_card(det_chan, pack),
        EngineResponse::WorkshopDesk {
            channel_id: ws_chan,
            draft,
            base_pack,
            ..
        } => render_workshop_desk_card(ws_chan, draft, base_pack.as_deref()),
        EngineResponse::ScenarioMenu {
            channel_id: menu_chan,
            scenarios,
        } => render_scenario_menu_card(menu_chan, scenarios),
        EngineResponse::RolePicker {
            channel_id: role_chan,
            scenario_name,
            roles,
        } => render_role_picker_card(role_chan, scenario_name, roles),
        EngineResponse::ExportReport {
            saved_file,
            turn_count,
            wall_report,
        } => {
            let wall_hint = if wall_report.is_some() {
                "\n🏆 精美高光战报卡已同步张贴到 **📜冒险战报墙** 留念喵！"
            } else {
                ""
            };
            render_notice_card(
                channel_id,
                "📜 跑团战报已归档导出",
                &format!("已完整导出共 **{turn_count} 个回合** 的 Markdown 战报：`{saved_file}`{wall_hint}"),
                "success",
                false,
                check_system,
            )
        }
        EngineResponse::Notice {
            title,
            body,
            theme,
            show_actions,
        } => render_notice_card(
            channel_id,
            title,
            body,
            theme.kook_str(),
            *show_actions,
            check_system,
        ),
        EngineResponse::StoryTurn { .. } => render_story_turn_card(resp, None, false),
        EngineResponse::StatusBoard {
            channel_id: board_chan,
            room,
            act_label,
            players,
            pending_actions,
            is_managed_table,
        } => {
            let mode_badge = match room.chat_mode {
                ChatMode::Roleplay => "💬 直接说话",
                ChatMode::Strict => "⌨️ 指令前缀",
            };
            let style_badge = match room.narrative_style {
                NarrativeStyle::Voice => "🎙️ 语音极简",
                NarrativeStyle::Prose => "📖 文字沉浸",
            };
            let pause_badge = if room.manual_resolve {
                " ｜ ⏸️ 手动结算"
            } else {
                ""
            };
            let table_badge = if *is_managed_table {
                " ｜ 🚪 独立包间"
            } else {
                ""
            };
            let act_line = act_label
                .as_deref()
                .map(|a| format!("\n🎬 **当前幕次**：{a}"))
                .unwrap_or_default();
            let end_line = room
                .ended_title
                .as_deref()
                .map(|e| format!("　｜　🏆 **已达结局**：{e}"))
                .unwrap_or_default();
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
                                "📍 {} ｜ 🕒 {} ｜ 🎲 第 {} 回合 ｜ {} ｜ {}{pause_badge}{table_badge}{act_line}{end_line}",
                                room.location, room.world_time, room.turn_number, mode_badge, style_badge
                            ),
                            500
                        )
                    }]
                }),
                json!({ "type": "divider" }),
            ];

            push_party_cards(&mut modules, players, pending_actions, room.check_system);
            push_npc_section(&mut modules, &room.npc_states);

            if !room.clocks.is_empty() && modules.len() < 35 {
                let clock_txt = room
                    .clocks
                    .iter()
                    .take(4)
                    .map(format_clock_bar)
                    .collect::<Vec<_>>()
                    .join("\n");
                modules.push(json!({ "type": "divider" }));
                modules.push(json!({
                    "type": "section",
                    "text": {
                        "type": "kmarkdown",
                        "content": truncate_chars(&format!("⏳ **局势进度钟**\n{clock_txt}"), 1000)
                    }
                }));
            }

            if !room.world_flags.is_empty() && modules.len() < 36 {
                modules.push(json!({ "type": "divider" }));
                modules.push(json!({
                    "type": "section",
                    "text": {
                        "type": "kmarkdown",
                        "content": truncate_chars(&format!("🚩 **关键进展节点**：{}", room.world_flags.join(" · ")), 1000)
                    }
                }));
            }

            if !room.clues.is_empty() {
                modules.push(json!({ "type": "divider" }));
                let start = room.clues.len().saturating_sub(12);
                let clues = room.clues[start..]
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}. {}", start + i + 1, c))
                    .collect::<Vec<_>>()
                    .join("\n");
                let hdr = if start > 0 {
                    format!("🔎 **最新线索**（展示最新 {} 条 / 共 {} 条，发 `/story` 私密查任务书）", room.clues.len() - start, room.clues.len())
                } else {
                    "🔎 **已掌握线索**".to_string()
                };
                push_chunked_sections(&mut modules, &format!("{hdr}\n{clues}"), 3200);
            }

            modules.push(json!({ "type": "divider" }));
            modules.push(crate::card_widgets::action_group(vec![
                crate::card_widgets::btn(
                    "primary",
                    format!("tavern:story:{board_chan}"),
                    "📖 案情简报（私密）",
                ),
                crate::card_widgets::btn(
                    "secondary",
                    format!("tavern:tables:{board_chan}"),
                    "🚪 包间列表",
                ),
                crate::card_widgets::btn(
                    "warning",
                    format!("tavern:leave:{board_chan}"),
                    "🚪 退团离桌",
                ),
            ]));
            let [row1, row2, row3] = status_console_rows(board_chan, room, *is_managed_table);
            modules.extend([row1, row2, row3]);
            json!([{ "type": "card", "theme": "info", "size": "lg", "modules": modules }]).to_string()
        }
    }
}

fn render_notice_card(
    channel_id: &str,
    title: &str,
    body: &str,
    theme: &str,
    show_actions: bool,
    check_system: CheckSystem,
) -> String {
    let mut modules = vec![json!({
        "type": "header",
        "text": { "type": "plain-text", "content": truncate_chars(title, 100) }
    })];
    push_chunked_sections(&mut modules, body, 3200);
    if show_actions {
        modules.push(json!({ "type": "divider" }));
        modules.push(notice_action_bar(channel_id, check_system));
    }
    json!([{ "type": "card", "theme": theme, "size": "lg", "modules": modules }]).to_string()
}

pub use crate::card_widgets::chunk_kmarkdown;

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
            channel_id: "ch_99".into(), turn_number: 2, scene_title: "酒馆暗流".into(),
            location: "余烬酒馆".into(), world_time: "21:20".into(),
            act_badge: Some("第 1/3 幕 · 暴雪夜".into()),
            narrative: "老雷恩推过来一杯麦酒。".into(),
            state_changes: vec!["**陆沉**：🎒 获得「钥匙」".into()],
            whisper_hints: vec!["🤫 DM 悄悄给 **陆沉** 递了一张秘密纸条".into()],
            suggested_actions: vec!["追问白鸦下落".into(), "潜入地下酒窖".into()],
            suggested_check: Some(tavern_domain::SuggestedCheck {
                stat: "侦查".into(), reason: "细查柜台暗格".into(), dc: None,
            }),
            world_bulletin: Some(tavern_domain::WorldBulletin {
                turn_number: 2, masthead: "《余烬号外》".into(), headline: "修女封锁东栈桥".into(),
                body: "圣光教会今夜加派了灯塔巡哨。".into(), faction: "圣光教会".into(),
            }),
            ending_reached: None,
            direct_messages: vec![tavern_domain::DirectMessageOut {
                user_id: "u1".into(), player_name: "陆沉".into(),
                content: "绝密暗格密码0317".into(),
            }],
        };
        let raw = render_kook_card_json("ch_99", &resp);
        assert_valid_kook_card(&raw);
        assert!(!raw.contains("tavern:pick:"), "v2.1 drops clickable choice buttons");
        assert!(raw.contains("眼下可切入焦点"));
        assert!(raw.contains("追问白鸦下落"));
        assert!(raw.contains("tavern:check:ch_99:investigate"));
        assert!(raw.contains("tavern:me:ch_99"));
        assert!(!raw.contains("绝密暗格密码0317"));

        let menu = EngineResponse::ScenarioMenu {
            channel_id: "ch_99".into(),
            scenarios: vec![ScenarioSummary {
                id: "ember_tavern".into(), name: "余烬酒馆".into(),
                genre: "奇幻".into(), tagline: "暴雪夜悬赏".into(), worldbook_count: 5,
            }],
        };
        let menu_raw = render_kook_card_json("ch_99", &menu);
        assert_valid_kook_card(&menu_raw);
        assert!(menu_raw.contains("tavern:table:ch_99:ember_tavern"));
        assert!(menu_raw.contains("tavern:start:ch_99:ember_tavern"));

        let public_fallback = render_plain_fallback(&resp, false);
        assert!(public_fallback.contains("获得「钥匙」"));
        assert!(!public_fallback.contains("绝密暗格密码0317"));
        assert!(render_plain_fallback(&resp, true).contains("绝密暗格密码0317"));
    }
}
