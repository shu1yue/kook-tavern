use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use chrono::Utc;
use tavern_domain::{
    CardTheme, EndingBanner, EndingUnlock, EngineResponse, Player, RoomEvent, RoomState, Scenario,
    ScenarioEnding, WorldEcho, distill_echo_text, ending_scope_key,
};
use tracing::{info, warn};

use crate::engine::GameEngine;

pub struct InFlightLlmGuard {
    counter: Arc<AtomicU64>,
    prior_in_flight: u64,
}

impl InFlightLlmGuard {
    pub fn acquire(counter: Arc<AtomicU64>) -> Self {
        let prior_in_flight = counter.fetch_add(1, Ordering::AcqRel);
        Self {
            counter,
            prior_in_flight,
        }
    }

    pub fn prior_in_flight(&self) -> u64 {
        self.prior_in_flight
    }
}

impl Drop for InFlightLlmGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn is_daily_budget_exhausted(engine: &GameEngine) -> bool {
    check_daily_budget_for_slot(engine, engine.in_flight_llm.load(Ordering::Acquire))
        .ok()
        .flatten()
        .is_some()
}

pub fn check_daily_budget(engine: &GameEngine) -> Result<Option<EngineResponse>> {
    check_daily_budget_for_slot(engine, engine.in_flight_llm.load(Ordering::Acquire))
}

pub fn check_daily_budget_for_slot(
    engine: &GameEngine,
    prior_in_flight: u64,
) -> Result<Option<EngineResponse>> {
    if engine.cfg.daily_max_llm_calls == 0 && engine.cfg.daily_max_cny <= 0.0 {
        return Ok(None);
    }
    let today = engine.store.get_today_usage()?;
    let projected_calls = today.calls.saturating_add(prior_in_flight);
    let calls_hit =
        engine.cfg.daily_max_llm_calls > 0 && projected_calls >= engine.cfg.daily_max_llm_calls;
    let cny_hit =
        engine.cfg.daily_max_cny > 0.0 && today.estimated_cny >= engine.cfg.daily_max_cny;

    if calls_hit || cny_hit {
        warn!(
            day = %today.day_utc,
            calls = today.calls,
            prior_in_flight,
            cny = today.estimated_cny,
            "已触发每日预算安全熔断"
        );
        return Ok(Some(EngineResponse::Notice {
            title: "🛑 已触发今日 API 安全熔断上限".to_string(),
            body: format!(
                "今日（{} UTC）已调用 **{} 次**，预估花费 **{:.4} 元**，达到房主设定的安全上限。\n请发送 `/cost` 查看详情，或由房主调高 `.env` 限额后重启。",
                today.day_utc, today.calls, today.estimated_cny
            ),
            theme: CardTheme::Danger,
            show_actions: false,
        }));
    }
    Ok(None)
}

pub fn schedule_background_summary(engine: &GameEngine, scenario: Scenario, room: RoomState) {
    let keep_recent = (engine.cfg.recent_turns_window / 2).max(2) as u32;
    if room.turn_number < room.summary_upto_turn + engine.cfg.summary_interval_turns
        || room.turn_number <= keep_recent
    {
        return;
    }

    let target_upto = room.turn_number - keep_recent;
    if target_upto <= room.summary_upto_turn {
        return;
    }

    let engine_bg = engine.clone();
    engine.bg_runner.spawn(Box::pin(async move {
        let slot = InFlightLlmGuard::acquire(engine_bg.in_flight_llm.clone());
        if check_daily_budget_for_slot(&engine_bg, slot.prior_in_flight())
            .ok()
            .flatten()
            .is_some()
        {
            warn!(channel_id = %room.channel_id, "已达每日预算熔断，跳过后台摘要压缩");
            return;
        }
        let Ok(to_summarize) = engine_bg.store.list_turns_range(
            &room.channel_id,
            room.summary_upto_turn,
            target_upto,
        ) else {
            return;
        };

        match engine_bg
            .llm
            .summarize_turns(&scenario, &room.summary, &to_summarize)
            .await
        {
            Ok(out) => {
                let est = engine_bg.cfg.estimate_usage_cny(&out.usage);
                let _ = engine_bg
                    .store
                    .record_usage(&room.channel_id, &out.usage, est);
                if !out.summary.trim().is_empty()
                    && let Ok(true) = engine_bg.store.update_summary_cas(
                        &room.channel_id,
                        room.state_version,
                        &out.summary,
                        target_upto,
                    )
                {
                    info!(
                        channel_id = %room.channel_id,
                        upto = target_upto,
                        "后台已自动压缩历史回合至前情提要"
                    );
                    engine_bg.emit_event(RoomEvent::RoomUpdated {
                        channel_id: room.channel_id.clone(),
                        turn_number: room.turn_number,
                    });
                }
            }
            Err(err) => warn!(error = %err, "后台自动摘要压缩失败，不影响主线游玩"),
        }
    }));
}

/// Persists both the server-scoped ending achievement unlock and a fresh reincarnation
/// worldline echo for future runs of the same scenario in this guild.
pub fn record_ending_and_echo(
    engine: &GameEngine,
    room: &RoomState,
    players: &[Player],
    end: &ScenarioEnding,
    dm_echo: Option<&str>,
) -> EndingBanner {
    let party_names = players
        .iter()
        .map(|p| p.character_name.as_str())
        .collect::<Vec<_>>()
        .join("、");
    let scope_key = ending_scope_key(Some(&room.guild_id), &room.channel_id);
    let now = Utc::now();
    let unlock = EndingUnlock {
        guild_id: scope_key.clone(),
        scenario_id: room.scenario_id.clone(),
        ending_id: end.id.clone(),
        ending_title: end.title.clone(),
        ending_kind: end.kind.clone(),
        turn_count: room.turn_number,
        party_names: party_names.clone(),
        unlocked_at: now,
    };
    let newly_unlocked = engine.store.record_ending_unlock(&unlock).unwrap_or(false);
    let echo = WorldEcho {
        id: 0,
        guild_id: scope_key,
        scenario_id: room.scenario_id.clone(),
        source_channel_id: room.channel_id.clone(),
        source_turn: room.turn_number,
        ending_id: end.id.clone(),
        ending_title: end.title.clone(),
        ending_kind: end.kind.clone(),
        party_names: party_names.clone(),
        echo_text: distill_echo_text(end, dm_echo, &party_names, &room.world_flags),
        turn_count: room.turn_number,
        created_at: now,
    };
    if let Err(err) = engine.store.record_world_echo(&echo) {
        warn!(channel_id = %room.channel_id, error = %err, "写入轮回残响失败");
    }
    EndingBanner {
        id: end.id.clone(),
        title: end.title.clone(),
        kind: end.kind.clone(),
        newly_unlocked,
    }
}

/// Re-renders the room's latest saved StoryTurn card cleanly from SQLite (useful if a card was
/// scrolled far away or blocked by transient platform hiccups).
pub fn cmd_resend(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".into(),
            body: "先发 `/start` 或点「📚 换剧本」开启冒险吧喵！".into(),
            theme: CardTheme::Info,
            show_actions: false,
        });
    };
    let scenario = engine.scenarios.get(&room.scenario_id);
    let act_badge = crate::commands::check::format_act_badge(scenario.as_ref(), room.current_act);
    let latest = engine.store.list_recent_turns(channel_id, 1)?.into_iter().next();
    let (scene_title, narrative, state_changes, turn_number, location, world_time) = match latest {
        Some(t) => {
            let changes = if t.state_summary.trim().is_empty() {
                Vec::new()
            } else {
                t.state_summary.split('；').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            };
            (t.scene_title, t.narrative, changes, t.turn_number, t.location, t.world_time)
        }
        None => (
            format!("{} · 序幕", room.title),
            scenario.as_ref().map(|s| s.opening_scene.trim().to_string()).unwrap_or_default(),
            Vec::new(),
            room.turn_number,
            room.location.clone(),
            room.world_time.clone(),
        ),
    };
    let ending_reached = room.ended_id.as_ref().map(|eid| EndingBanner {
        id: eid.clone(),
        title: room.ended_title.clone().unwrap_or_else(|| eid.clone()),
        kind: "normal".into(),
        newly_unlocked: false,
    });
    Ok(EngineResponse::StoryTurn {
        channel_id: room.channel_id,
        turn_number,
        scene_title,
        location,
        world_time,
        act_badge,
        narrative,
        state_changes,
        whisper_hints: Vec::new(),
        suggested_actions: room.suggested_actions,
        suggested_check: None,
        world_bulletin: room.bulletins.last().filter(|b| b.turn_number == turn_number).cloned(),
        ending_reached,
        direct_messages: Vec::new(),
    })
}
