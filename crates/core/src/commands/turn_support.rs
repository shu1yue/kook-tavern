use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tavern_domain::{CardTheme, EngineResponse, RoomState, Scenario};
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
                }
            }
            Err(err) => warn!(error = %err, "后台自动摘要压缩失败，不影响主线游玩"),
        }
    }));
}
