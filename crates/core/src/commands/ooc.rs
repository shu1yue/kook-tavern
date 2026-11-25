use tavern_domain::{ChatMode, RoomState};
use tracing::info;

use crate::commands::{
    parse_known_or_strict_command, resolve_authoritative_guild, turn_support,
};
use crate::engine::GameEngine;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OocPrecheck {
    /// Message did not qualify for OOC evaluation (command, DM, non-RP, out-of-scope, or budget hit).
    NotApplicable,
    /// Jev evaluated the message and allowed it (IC action or fail-open timeout/error).
    Allow,
    /// Jev confirmed pure OOC group banter (`prob_ooc >= threshold`); drop silently.
    IgnoreOoc,
}

/// Evaluates whether a plain-text group message in `Roleplay` mode is pure OOC banter via Jev.
/// Strictly conservative: fails open (treats as IC action) on any timeout, error, budget limit, or low score.
pub(crate) async fn evaluate_roleplay_ooc(
    engine: &GameEngine,
    room: &RoomState,
    sender_name: &str,
    text: &str,
) -> bool {
    if !engine.decision.is_enabled() || turn_support::is_daily_budget_exhausted(engine) {
        return false;
    }
    let mut recent_scene = engine
        .store
        .list_recent_turns(&room.channel_id, 1)
        .ok()
        .and_then(|mut v| v.pop())
        .map(|t| format!("{}：{}", t.scene_title, t.narrative))
        .unwrap_or_else(|| room.summary.clone());
    if recent_scene.trim().is_empty()
        && let Some(sc) = engine.scenarios.get(&room.scenario_id)
    {
        recent_scene = sc.opening_scene;
    }
    let Some(verdict) = engine
        .decision
        .evaluate_ooc(&room.title, &room.location, &recent_scene, sender_name, text)
        .await
    else {
        return false;
    };
    let is_ooc = verdict.prob_ooc >= engine.cfg.jev.ooc_threshold;
    if is_ooc {
        info!(
            channel_id = %room.channel_id,
            sender = sender_name,
            provider = verdict.provider,
            prob_ooc = verdict.prob_ooc,
            latency_ms = verdict.latency_ms,
            "Jev OOC 吹水门卫已静默放行场外闲聊"
        );
    }
    is_ooc
}

/// Prechecks whether a plain-text group `Roleplay` message should be treated as OOC BEFORE
/// KOOK sends a thinking placeholder card (eliminating placeholder flashes & duplicate Jev calls).
pub async fn precheck_roleplay_ooc(
    engine: &GameEngine,
    guild_id: Option<&str>,
    channel_id: &str,
    _user_id: &str,
    user_name: &str,
    content: &str,
    is_direct: bool,
) -> OocPrecheck {
    if is_direct || !engine.decision.is_enabled() || turn_support::is_daily_budget_exhausted(engine)
    {
        return OocPrecheck::NotApplicable;
    }
    let eff_guild = resolve_authoritative_guild(engine, guild_id, channel_id)
        .ok()
        .flatten();
    if !engine.cfg.is_scope_allowed(eff_guild.as_deref(), channel_id) {
        return OocPrecheck::NotApplicable;
    }
    let trimmed = content.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('>')
        || trimmed.starts_with('》')
        || trimmed.starts_with('(')
        || trimmed.starts_with('（')
        || trimmed.starts_with("//")
        || parse_known_or_strict_command(trimmed).is_some()
    {
        return OocPrecheck::NotApplicable;
    }
    let Ok(Some(room)) = engine.store.get_room(channel_id) else {
        return OocPrecheck::NotApplicable;
    };
    if !room.active || room.ended_id.is_some() || room.chat_mode != ChatMode::Roleplay {
        return OocPrecheck::NotApplicable;
    }
    if evaluate_roleplay_ooc(engine, &room, user_name, trimmed).await {
        OocPrecheck::IgnoreOoc
    } else {
        OocPrecheck::Allow
    }
}

pub async fn is_plain_roleplay_ooc(
    engine: &GameEngine,
    guild_id: Option<&str>,
    channel_id: &str,
    user_id: &str,
    user_name: &str,
    content: &str,
    is_direct: bool,
) -> bool {
    matches!(
        precheck_roleplay_ooc(engine, guild_id, channel_id, user_id, user_name, content, is_direct)
            .await,
        OocPrecheck::IgnoreOoc
    )
}
