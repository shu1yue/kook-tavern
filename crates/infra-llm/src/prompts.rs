use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tavern_domain::{
    NarrativeStyle, PendingAction, Player, RoomState, Scenario, TurnRecord, WhisperRecord,
    WorldEcho, always_active_worldbook, format_batched_actions,
};
use tracing::info;

use crate::memory::{
    detect_drop_in_players, format_dm_whisper_ledger, format_focused_clues,
    render_memory_book_for_prompt, split_memory_book,
};
use crate::prompt_ext::{
    build_lore_corpus, format_act_progress, format_dynamic_director_sections,
    format_player_dnd_tail, format_static_rules_and_map, inject_climax_worldbook,
    merge_dynamic_worldbook,
};

const DEFAULT_DM_SYSTEM: &str = include_str!("../../../prompts/dm_system.md");
const DEFAULT_STYLE_PROSE: &str = include_str!("../../../prompts/style_prose.md");
const DEFAULT_STYLE_VOICE: &str = include_str!("../../../prompts/style_voice.md");
const DEFAULT_SUMMARIZER: &str = include_str!("../../../prompts/summarizer.md");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

fn load_prompt_file(prompts_dir: &Path, filename: &str, fallback: &str) -> String {
    let path = prompts_dir.join(filename);
    match fs::read_to_string(&path) {
        Ok(content) if !content.trim().is_empty() => content,
        _ => fallback.to_string(),
    }
}

/// Builds a DeepSeek-cache-friendly message sequence:
/// - Message 0 (`system`): 100% static per scenario + narrative style (maximizes Prefix Caching hits!)
/// - Middle messages: recent turns (assistant/user history)
/// - Final `user` message: authoritative live state snapshot + dynamically triggered Lorebook + current actions
pub fn build_dm_messages(
    prompts_dir: &Path,
    scenario: &Scenario,
    room: &RoomState,
    players: &[Player],
    recent_turns: &[TurnRecord],
    actions: &[PendingAction],
    world_echoes: &[WorldEcho],
) -> Vec<ChatMessage> {
    build_dm_messages_with_extra_lore(
        prompts_dir,
        scenario,
        room,
        players,
        recent_turns,
        actions,
        world_echoes,
        &[],
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_dm_messages_with_extra_lore(
    prompts_dir: &Path, scenario: &Scenario, room: &RoomState, players: &[Player],
    recent_turns: &[TurnRecord], actions: &[PendingAction], world_echoes: &[WorldEcho],
    extra_lore_ids: &[String],
) -> Vec<ChatMessage> {
    build_dm_messages_full(
        prompts_dir, scenario, room, players, recent_turns, actions, world_echoes, &[], extra_lore_ids,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn build_dm_messages_full(
    prompts_dir: &Path, scenario: &Scenario, room: &RoomState, players: &[Player],
    recent_turns: &[TurnRecord], actions: &[PendingAction], world_echoes: &[WorldEcho],
    room_whispers: &[WhisperRecord], extra_lore_ids: &[String],
) -> Vec<ChatMessage> {
    let sys_template = load_prompt_file(prompts_dir, "dm_system.md", DEFAULT_DM_SYSTEM);
    let style_guide = match room.narrative_style {
        NarrativeStyle::Prose => {
            load_prompt_file(prompts_dir, "style_prose.md", DEFAULT_STYLE_PROSE)
        }
        NarrativeStyle::Voice => {
            load_prompt_file(prompts_dir, "style_voice.md", DEFAULT_STYLE_VOICE)
        }
    };

    let always_entries = always_active_worldbook(scenario);
    let always_lore = if always_entries.is_empty() {
        "（无常驻设定）".to_string()
    } else {
        always_entries
            .iter()
            .map(|e| format!("### {}\n{}", e.title, e.content.trim()))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let mut system_prompt = sys_template
        .replace("{{scenario_name}}", &scenario.name)
        .replace("{{genre}}", &scenario.genre)
        .replace("{{dm_style}}", scenario.dm_style.trim())
        .replace("{{narrative_style_guide}}", style_guide.trim())
        .replace("{{always_lore}}", &always_lore);
    let static_ext = format_static_rules_and_map(scenario);
    if !static_ext.is_empty() {
        system_prompt.push_str("\n\n");
        system_prompt.push_str(&static_ext);
    }

    let batched_input = format_batched_actions(actions);
    let lore_corpus = build_lore_corpus(room, recent_turns, actions);
    let mut dynamic_lore = merge_dynamic_worldbook(scenario, &lore_corpus, extra_lore_ids, 7);
    inject_climax_worldbook(scenario, room, &mut dynamic_lore, 9);
    info!(
        channel_id = %room.channel_id,
        matched_dynamic_lore = dynamic_lore.len(),
        jev_extra_lore = extra_lore_ids.len(),
        style = room.narrative_style.as_str(),
        "组装 DM Prompt 完成"
    );

    let mut messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system_prompt,
    }];

    let opening_recap = if recent_turns.is_empty() {
        Some(scenario.opening_scene.trim())
    } else {
        for turn in recent_turns {
            messages.push(ChatMessage {
                role: "user".to_string(),
                content: format!("第 {} 回合玩家行动：\n{}", turn.turn_number, turn.batched_input),
            });
            messages.push(ChatMessage {
                role: "assistant".to_string(),
                content: format!("【{}】\n{}", turn.scene_title, turn.narrative),
            });
        }
        None
    };

    let mut director_ext = format_dynamic_director_sections(
        scenario, room, players, recent_turns, actions, world_echoes,
    );
    let newcomers = detect_drop_in_players(room, players, recent_turns, actions);
    if !newcomers.is_empty() {
        director_ext.push_str(&format!(
            "\n\n【半路空降新队友登场】角色「{}」本轮首次加入本局，请在剧情中自然安排一个符合当前地点的合理出场与汇合桥段！",
            newcomers.join("、")
        ));
    }
    let whisper_ledger = format_dm_whisper_ledger(room_whispers, 20);
    if !whisper_ledger.is_empty() {
        director_ext.push_str("\n\n");
        director_ext.push_str(&whisper_ledger);
    }
    let dynamic_block = format_dynamic_turn_prompt(
        scenario, room, players, &dynamic_lore, &lore_corpus, &batched_input, opening_recap, &director_ext,
    );
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: dynamic_block,
    });

    messages
}

pub fn build_summary_messages(
    prompts_dir: &Path,
    scenario: &Scenario,
    existing_summary: &str,
    turns: &[TurnRecord],
) -> Vec<ChatMessage> {
    let sys_template = load_prompt_file(prompts_dir, "summarizer.md", DEFAULT_SUMMARIZER);
    let system_prompt = sys_template.replace("{{scenario_name}}", &scenario.name);

    let mut history_blob = String::new();
    for t in turns {
        history_blob.push_str(&format!(
            "--- 第 {} 回合（{} / {}）---\n玩家行动:\n{}\nDM剧情:\n{}\n状态变动: {}\n\n",
            t.turn_number, t.location, t.world_time, t.batched_input, t.narrative, t.state_summary
        ));
    }

    let (_, active_notebook) = split_memory_book(existing_summary);
    let notebook_hint = if !active_notebook.is_empty() {
        active_notebook
    } else if !existing_summary.trim().is_empty() {
        existing_summary.chars().take(360).collect()
    } else {
        "（尚无未解伏笔便签）".to_string()
    };

    vec![
        ChatMessage { role: "system".to_string(), content: system_prompt },
        ChatMessage {
            role: "user".to_string(),
            content: format!(
                "【当前未解伏笔便签（已解决请删去，未解决请保留或更新）】\n{notebook_hint}\n\n【需要封卷的新增回合】\n{history_blob}"
            ),
        },
    ]
}

#[allow(clippy::too_many_arguments)]
fn format_dynamic_turn_prompt(
    scenario: &Scenario,
    room: &RoomState,
    players: &[Player],
    dynamic_lore: &[&tavern_domain::WorldbookEntry],
    recall_corpus: &str,
    batched_input: &str,
    opening_recap: Option<&str>,
    director_ext: &str,
) -> String {
    let party_block = if players.is_empty() {
        "（暂无正式建卡角色，按冒险者小队统一描写）".to_string()
    } else {
        players
            .iter()
            .map(|p| {
                let traits = if p.traits.is_empty() {
                    "无".to_string()
                } else {
                    p.traits.join("、")
                };
                let inv = if p.inventory.is_empty() {
                    "空".to_string()
                } else {
                    p.inventory.join("、")
                };
                let afk_tag = if p.is_afk { " [暂离]" } else { "" };
                let p_loc = if p.location.trim().is_empty() {
                    room.location.as_str()
                } else {
                    p.location.trim()
                };
                format!(
                    "- {}（{}）{} | 位置: {} | HP {}/{} | SAN {}/{} | 五维: {} | 特质: {} | 背包: [{}] | 状态: {}{}",
                    p.character_name,
                    if p.role_desc.is_empty() { "冒险者" } else { &p.role_desc },
                    afk_tag, p_loc, p.hp, p.max_hp, p.san, p.max_san,
                    p.stats.format_prompt(), traits, inv,
                    if p.status_note.trim().is_empty() { "正常" } else { p.status_note.trim() },
                    format_player_dnd_tail(p)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let act_block = format_act_progress(scenario, room);
    let clocks_block = if room.clocks.is_empty() {
        "（暂未启用进度钟）".to_string()
    } else {
        room.clocks
            .iter()
            .map(|c| {
                let auto_hint = if c.auto_tick_every > 0 {
                    format!("，每{}轮自动+1", c.auto_tick_every)
                } else {
                    String::new()
                };
                let full_tag = if c.filled >= c.segments { "【已满·必须兑现！】" } else { "" };
                format!(
                    "- `{}`（{} · {}）：{}/{}{}{} ｜ 满格后果：{}",
                    c.id,
                    c.name,
                    c.kind.label(),
                    c.filled,
                    c.segments,
                    auto_hint,
                    full_tag,
                    c.on_full
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let endings_block = if scenario.endings.is_empty() {
        "（自由探索模组，无需触发预设结局）".to_string()
    } else {
        scenario
            .endings
            .iter()
            .map(|e| format!("- `{}`（{} / {}）：{}", e.id, e.title, e.kind, e.summary))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let clues_block = format_focused_clues(&room.clues, recall_corpus, 25, 12);

    let npc_block = if room.npc_states.is_empty() {
        "（暂无已记录态度的关键 NPC）".to_string()
    } else {
        room.npc_states
            .iter()
            .map(|n| {
                let nloc = if n.location.trim().is_empty() { String::new() } else { format!(" @{}", n.location.trim()) };
                format!("- {}（{}{}）：{}", n.name, n.attitude, nloc, n.status)
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let lore_block = if dynamic_lore.is_empty() {
        "（无新触发的隐藏世界书）".to_string()
    } else {
        dynamic_lore.iter().map(|e| format!("### {}\n{}", e.title, e.content.trim())).collect::<Vec<_>>().join("\n\n")
    };

    let summary_block = render_memory_book_for_prompt(&room.summary, recall_corpus, 6);

    let opening_section = opening_recap
        .map(|o| format!("【刚刚发生的开场场景】\n{o}\n\n"))
        .unwrap_or_default();

    format!(
        r#"{opening_section}【当前权威世界状态（只读快照）】
- 即将结算：第 {next_turn} 回合 ｜ 本桌检定体系：{check_rule}
- 当前地点：{location}
- 当前时间：{world_time}
- 当前幕次进度：{act_block}
- 局势进度钟与威胁钟（用 `clock_updates` 推进，满格必须兑现后果）：
{clocks_block}
- 队员状态（当前共 {party_count} 人）：
{party_block}
- 已掌握线索：
{clues_block}
- 关键 NPC 当前态度与状态：
{npc_block}
- 前情提要：
{summary_block}

【本剧本可用结局清单（仅在真正满足条件且进入收束时填入 trigger_ending_id，平时必须填 null）】
{endings_block}

【本轮触发的隐藏世界书（仅供 DM 裁决参考，切勿直接全盘剧透）】
{lore_block}{director_ext}

【本轮全体玩家行动（请统一裁决并仅输出合法 JSON）】
{batched_input}"#,
        next_turn = room.turn_number + 1,
        check_rule = room.check_system.prompt_guide(),
        location = room.location,
        world_time = room.world_time,
        party_count = players.len(),
    )
}
