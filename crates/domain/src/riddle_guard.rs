use crate::models::{PendingAction, Player, RoomState, TurnRecord};
use crate::protocol::DmStructuredReply;

/// Spotlight fairness analysis inspired by multi-agent narrative pacing lessons:
/// ensures quieter active players aren't starved of camera focus by chatty teammates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpotlightGuidance {
    pub sidelined_players: Vec<String>,
    pub prompt_directive: Option<String>,
}

pub fn analyze_spotlight(
    players: &[Player],
    recent_turns: &[TurnRecord],
    current_actions: &[PendingAction],
) -> SpotlightGuidance {
    let active: Vec<&Player> = players.iter().filter(|p| !p.is_afk).collect();
    if active.len() < 2 || recent_turns.len() < 2 {
        return SpotlightGuidance::default();
    }

    let window: Vec<&TurnRecord> = recent_turns.iter().rev().take(3).collect();
    let mut sidelined = Vec::new();

    for p in &active {
        let name = p.character_name.trim();
        if name.is_empty() {
            continue;
        }
        let mut neglected = 0usize;
        for turn in &window {
            if turn.narrative.contains(name) {
                break;
            }
            neglected += 1;
        }
        let acted_now = current_actions
            .iter()
            .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty());
        if neglected >= 2 || (neglected >= 1 && acted_now && window.len() >= 2) {
            sidelined.push(name.to_string());
        }
    }

    if sidelined.is_empty() {
        return SpotlightGuidance::default();
    }

    let names = sidelined.join("、");
    let prompt_directive = format!(
        "【聚光灯均衡提醒】以下活跃角色最近几轮主镜头偏少：{names}。本轮描写请主动给他们安排专属环境细节、NPC 搭话或发现线索的契机，确保镜头均衡、不冷落任何在线队友（仍严禁替其做决定）。"
    );
    SpotlightGuidance {
        sidelined_players: sidelined,
        prompt_directive: Some(prompt_directive),
    }
}

/// Local zero-dependency detector for cryptic AI-riddle / ellipsis-spam prose.
/// Precision-tuned to catch obvious evasive riddles without false-positiving on normal scenes.
pub fn detect_cryptic_riddle_slop(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.chars().count() < 40 {
        return false;
    }
    let ellipsis_count = trimmed.matches("……").count() + trimmed.matches("...").count();
    let riddle_cues = [
        "时机未到",
        "天机不可",
        "时候到了你自然会懂",
        "有些事知道太多",
        "你还没准备好知道",
        "谜底藏在",
        "仿佛在诉说着什么",
        "讳莫如深",
        "意味深长地笑",
        "欲言又止",
        "话锋一转",
        "只是淡淡一笑",
        "没有直接回答",
        "却没有说下去",
        "似乎另有所指",
        "不可言说的",
    ];
    let riddle_hits = riddle_cues.iter().filter(|k| trimmed.contains(**k)).count();
    ellipsis_count >= 5 || riddle_hits >= 2 || (ellipsis_count >= 3 && riddle_hits >= 1)
}

/// Local zero-dependency detector for bloated atmosphere-filler turns with zero plot progress.
pub fn detect_plot_stall_slop(reply: &DmStructuredReply, room: &RoomState) -> bool {
    let text = reply.narrative.trim();
    if text.chars().count() < 150 {
        return false;
    }
    let reply_loc = reply.location.as_deref().map(str::trim).unwrap_or("");
    let location_unchanged = reply_loc.is_empty() || reply_loc == room.location.trim();
    let zero_concrete_progress = reply.new_clues.is_empty()
        && reply.state_patches.is_empty()
        && reply.add_flags.is_empty()
        && reply.clock_updates.is_empty()
        && reply.npc_updates.is_empty()
        && !reply.advance_act
        && reply.trigger_ending_id.is_none()
        && reply.private_whispers.is_empty()
        && location_unchanged;
    if !zero_concrete_progress {
        return false;
    }
    let filler_cues = [
        "空气仿佛凝固",
        "沉默蔓延",
        "摇曳的烛火",
        "无人开口",
        "静得可怕",
        "压抑的寂静",
        "一切都显得扑朔迷离",
        "似乎隐藏着什么",
        "深吸一口气",
        "陷入了短暂的沉默",
    ];
    let filler_hits = filler_cues.iter().filter(|k| text.contains(**k)).count();
    reply.turn_outcome.trim().is_empty() || filler_hits >= 2
}
