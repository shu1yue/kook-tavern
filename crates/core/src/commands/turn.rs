use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use tavern_domain::{
    CardTheme, EngineResponse, PendingAction, RoomEvent, RoomSnapshot, TurnPhase, TurnRecord,
    apply_dm_reply, format_batched_actions,
};
use tracing::{info, warn};

use crate::commands::turn_support::{
    InFlightLlmGuard, check_daily_budget, check_daily_budget_for_slot, schedule_background_summary,
};
use crate::engine::GameEngine;
use crate::ports::RoomStore;

struct ResolvingLeaseGuard {
    store: Arc<dyn RoomStore>,
    channel_id: String,
    expected_version: u64,
    armed: bool,
}

impl ResolvingLeaseGuard {
    fn new(store: Arc<dyn RoomStore>, channel_id: String, expected_version: u64) -> Self {
        Self {
            store,
            channel_id,
            expected_version,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ResolvingLeaseGuard {
    fn drop(&mut self) {
        if self.armed
            && let Err(err) = self
                .store
                .abort_resolving(&self.channel_id, self.expected_version)
        {
            warn!(
                channel_id = %self.channel_id,
                expected_version = self.expected_version,
                error = %err,
                "RAII 守卫回滚 Resolving 状态失败"
            );
        }
    }
}

pub async fn resolve_turn(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    let Some(room_check) = engine.store.get_room(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 当前频道尚未开团".to_string(),
            body: "请先发送 `/start` 开启冒险。".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    if room_check.turn_phase == TurnPhase::Resolving {
        return Ok(EngineResponse::Notice {
            title: "⏳ DM 正在结算本轮剧情".to_string(),
            body: "上一条结算请求还在生成中，稍等几秒再操作喵！新发 `/act` 会自动暂存到下一轮。"
                .to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    }

    if let Some(limit_notice) = check_daily_budget(engine)? {
        return Ok(limit_notice);
    }

    let Some(lease) = engine.store.try_begin_resolving(channel_id)? else {
        return Ok(EngineResponse::Notice {
            title: "⏳ DM 正在结算本轮剧情".to_string(),
            body: "上一条结算请求刚被触发，稍等几秒喵！".to_string(),
            theme: CardTheme::Warning,
            show_actions: false,
        });
    };

    let mut lease_guard = ResolvingLeaseGuard::new(
        engine.store.clone(),
        channel_id.to_string(),
        lease.expected_version,
    );
    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: channel_id.to_string(),
        turn_number: lease.room.turn_number,
    });

    let actions: Vec<PendingAction> = lease
        .actions
        .iter()
        .filter(|a| !a.action_text.trim().is_empty() || !a.roll_notes.is_empty())
        .cloned()
        .map(|mut a| {
            if a.action_text.trim().is_empty() {
                a.action_text = "（仅进行了掷骰检定）".to_string();
            }
            a
        })
        .collect();

    if actions.is_empty() {
        return Ok(EngineResponse::Notice {
            title: "ℹ️ 本轮还没有玩家提交行动".to_string(),
            body: "请至少有一位玩家先发送 `/act 你的行动`（或 `> 行动`）后再结算！".to_string(),
            theme: CardTheme::Info,
            show_actions: true,
        });
    }

    let Some(scenario) = engine.scenarios.get(&lease.room.scenario_id) else {
        return Ok(EngineResponse::Notice {
            title: "❌ 剧本配置缺失".to_string(),
            body: format!("找不到剧本 `{}`。", lease.room.scenario_id),
            theme: CardTheme::Danger,
            show_actions: false,
        });
    };

    let recent_turns = engine
        .store
        .list_recent_turns(channel_id, engine.cfg.recent_turns_window)?;

    let mut pre_turn_room = lease.room.clone();
    pre_turn_room.turn_phase = TurnPhase::Collecting;
    let snapshot = RoomSnapshot {
        room: pre_turn_room,
        players: lease.players.clone(),
        pending_actions: actions.clone(),
    };

    // Reserve an ordered in-flight slot before LLM call so concurrent multi-room bursts
    // respect daily limits without mutually rejecting each other.
    let slot = InFlightLlmGuard::acquire(engine.in_flight_llm.clone());
    if let Some(limit_notice) = check_daily_budget_for_slot(engine, slot.prior_in_flight())? {
        return Ok(limit_notice);
    }

    let turn_output = engine
        .llm
        .generate_turn(&scenario, &lease.room, &lease.players, &recent_turns, &actions)
        .await?;

    let est_cny = engine.cfg.estimate_usage_cny(&turn_output.usage);
    if let Err(err) = engine
        .store
        .record_usage(channel_id, &turn_output.usage, est_cny)
    {
        warn!(channel_id, error = %err, "记录 LLM 消费日志失败");
    }

    info!(
        channel_id,
        turn = lease.target_turn,
        prompt_tokens = turn_output.usage.prompt_tokens,
        completion_tokens = turn_output.usage.completion_tokens,
        cache_hit_tokens = turn_output.usage.cache_hit_tokens,
        est_cny,
        "DM 回合生成完成"
    );

    let dm_reply = turn_output.reply;
    let mut updated_room = lease.room;
    let mut updated_players = lease.players;

    let clean_suggestions: Vec<String> = dm_reply
        .suggested_actions
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .take(3)
        .collect();

    let effects = apply_dm_reply(&mut updated_room, &mut updated_players, &dm_reply);
    updated_room.turn_number = lease.target_turn;
    updated_room.turn_phase = TurnPhase::Collecting;
    updated_room.state_version = lease.expected_version + 1;
    updated_room.suggested_actions = clean_suggestions.clone();
    updated_room.updated_at = Utc::now();

    let batched_input = format_batched_actions(&actions);
    let turn_record = TurnRecord {
        id: 0,
        channel_id: channel_id.to_string(),
        turn_number: updated_room.turn_number,
        batched_input,
        scene_title: dm_reply.scene_title.clone(),
        narrative: dm_reply.narrative.clone(),
        state_summary: effects.state_changes.join("；"),
        location: updated_room.location.clone(),
        world_time: updated_room.world_time.clone(),
        created_at: Utc::now(),
    };

    let committed = engine.store.commit_turn_cas(
        lease.expected_version,
        &snapshot,
        &updated_room,
        &updated_players,
        &turn_record,
        lease.target_turn,
        &effects.direct_messages,
    )?;
    lease_guard.disarm();

    if !committed {
        warn!(
            channel_id,
            expected_version = lease.expected_version,
            "检测到房间在结算期间已被回档或重置，安全丢弃过期的 LLM 响应"
        );
        return Ok(EngineResponse::Notice {
            title: "⚠️ 本轮结算已失效（房间刚被回档或重置）".to_string(),
            body: "刚才在 AI 推演期间房间状态已变更，旧结算结果已安全丢弃，未造成任何脏数据喵！"
                .to_string(),
            theme: CardTheme::Warning,
            show_actions: true,
        });
    }

    schedule_background_summary(engine, scenario, updated_room.clone());

    engine.emit_event(RoomEvent::RoomUpdated {
        channel_id: channel_id.to_string(),
        turn_number: updated_room.turn_number,
    });

    Ok(EngineResponse::StoryTurn {
        channel_id: channel_id.to_string(),
        turn_number: updated_room.turn_number,
        scene_title: dm_reply.scene_title,
        location: updated_room.location,
        world_time: updated_room.world_time,
        narrative: dm_reply.narrative,
        state_changes: effects.state_changes,
        whisper_hints: effects.whisper_hints,
        suggested_actions: clean_suggestions,
        direct_messages: effects.direct_messages,
    })
}

pub fn cmd_undo(engine: &GameEngine, channel_id: &str) -> Result<EngineResponse> {
    match engine.store.pop_and_restore_snapshot(channel_id)? {
        Some(restored) => {
            engine.emit_event(RoomEvent::RoomUpdated {
                channel_id: channel_id.to_string(),
                turn_number: restored.room.turn_number,
            });
            Ok(EngineResponse::Notice {
                title: "↩️ 时光倒流 · 回档成功".to_string(),
                body: format!(
                    "已完整恢复到 **第 {} 回合结束时** 的状态（HP / SAN / 背包 / 线索 / 待提交行动均已还原）。\n你可以重新修改行动，或再次 `/go` 让 DM 重新演绎！",
                    restored.room.turn_number
                ),
                theme: CardTheme::Success,
                show_actions: true,
            })
        }
        None => Ok(EngineResponse::Notice {
            title: "ℹ️ 没有可回档的历史快照".to_string(),
            body: "当前还没有已结算的回合可供撤销。".to_string(),
            theme: CardTheme::Info,
            show_actions: false,
        }),
    }
}
