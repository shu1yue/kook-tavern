use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tavern_domain::{
    NarrativeStyle, PendingAction, Player, RoomState, Scenario, TurnRecord,
    always_active_worldbook, format_batched_actions, match_dynamic_worldbook,
};
use tracing::info;

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

    let system_prompt = sys_template
        .replace("{{scenario_name}}", &scenario.name)
        .replace("{{genre}}", &scenario.genre)
        .replace("{{dm_style}}", scenario.dm_style.trim())
        .replace("{{narrative_style_guide}}", style_guide.trim())
        .replace("{{always_lore}}", &always_lore);

    let batched_input = format_batched_actions(actions);
    let mut lore_corpus = format!("{}\n{}", room.location, batched_input);
    for t in recent_turns.iter().rev().take(2) {
        lore_corpus.push('\n');
        lore_corpus.push_str(&t.batched_input);
        lore_corpus.push('\n');
        lore_corpus.push_str(&t.narrative);
    }
    let dynamic_lore = match_dynamic_worldbook(scenario, &lore_corpus, 5);
    info!(
        channel_id = %room.channel_id,
        matched_dynamic_lore = dynamic_lore.len(),
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

    let dynamic_block = format_dynamic_turn_prompt(
        scenario,
        room,
        players,
        &dynamic_lore,
        &batched_input,
        opening_recap,
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

    vec![
        ChatMessage {
            role: "system".to_string(),
            content: system_prompt,
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!(
                "【既有前情提要】\n{}\n\n【需要并入的新增回合】\n{}",
                if existing_summary.trim().is_empty() {
                    "（无）"
                } else {
                    existing_summary
                },
                history_blob
            ),
        },
    ]
}

fn format_dynamic_turn_prompt(
    scenario: &Scenario,
    room: &RoomState,
    players: &[Player],
    dynamic_lore: &[&tavern_domain::WorldbookEntry],
    batched_input: &str,
    opening_recap: Option<&str>,
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
                format!(
                    "- {}（{}）{} | HP {}/{} | SAN {}/{} | 五维: {} | 特质: {} | 背包: [{}] | 状态: {}",
                    p.character_name,
                    if p.role_desc.is_empty() { "冒险者" } else { &p.role_desc },
                    afk_tag,
                    p.hp,
                    p.max_hp,
                    p.san,
                    p.max_san,
                    p.stats.format_prompt(),
                    traits,
                    inv,
                    if p.status_note.trim().is_empty() {
                        "正常"
                    } else {
                        p.status_note.trim()
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    let act_block = format_act_progress(scenario, room);
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

    let clues_block = if room.clues.is_empty() {
        "（尚未发现明确线索）".to_string()
    } else {
        room.clues
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let npc_block = if room.npc_states.is_empty() {
        "（暂无已记录态度的关键 NPC）".to_string()
    } else {
        room.npc_states
            .iter()
            .map(|n| format!("- {}（{}）：{}", n.name, n.attitude, n.status))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let lore_block = if dynamic_lore.is_empty() {
        "（无新触发的隐藏世界书）".to_string()
    } else {
        dynamic_lore
            .iter()
            .map(|e| format!("### {}\n{}", e.title, e.content.trim()))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    let summary_block = if room.summary.trim().is_empty() {
        "（开局阶段，尚无历史摘要）".to_string()
    } else {
        room.summary.clone()
    };

    let opening_section = opening_recap
        .map(|o| format!("【刚刚发生的开场场景】\n{o}\n\n"))
        .unwrap_or_default();

    format!(
        r#"{opening_section}【当前权威世界状态（只读快照）】
- 即将结算：第 {next_turn} 回合
- 当前地点：{location}
- 当前时间：{world_time}
- 当前幕次进度：{act_block}
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
{lore_block}

【本轮全体玩家行动（请统一裁决并仅输出合法 JSON）】
{batched_input}"#,
        next_turn = room.turn_number + 1,
        location = room.location,
        world_time = room.world_time,
        party_count = players.len(),
    )
}

fn format_act_progress(scenario: &Scenario, room: &RoomState) -> String {
    let flags = if room.world_flags.is_empty() {
        "无".to_string()
    } else {
        room.world_flags.join("、")
    };
    if scenario.acts.is_empty() {
        return format!("自由推进 ｜ 已触发节点：[{flags}]");
    }
    let total = scenario.acts.len();
    let idx = (room.current_act.max(1) as usize)
        .saturating_sub(1)
        .min(total - 1);
    let act = &scenario.acts[idx];
    format!(
        "第 {}/{} 幕【{}】｜ 本幕目标：{} ｜ 已触发节点：[{}]",
        idx + 1,
        total,
        act.title,
        act.goal,
        flags
    )
}
