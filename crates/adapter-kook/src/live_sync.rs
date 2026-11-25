use tavern_core::GameEngine;
use tavern_domain::{EngineResponse, LiveStoryHud};
use tracing::{error, warn};

use crate::api::KookClient;
use crate::dispatcher::InPlaceSyncMode;
use crate::live_cards::{ActiveStoryCard, render_orphan_archived_card, render_story_turn_card};
use crate::sanitize::{
    is_kook_moderation_error, safe_story_turn_fallback, soften_live_hud, soften_response,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncOutcome {
    /// Active StoryTurn card on KOOK actually changed and was updated in-place.
    Updated,
    /// Active StoryTurn card already matched the live HUD (no HTTP call needed).
    Unchanged,
    /// No active StoryTurn card could be synced (room reset/ended, turn mismatch, or HTTP error).
    Skipped,
}

impl KookClient {
    /// Serialized per-channel StoryTurn delivery: archives the previous live card (in-memory or
    /// SQLite-persisted across restarts), sends the new StoryTurn with KOOK 40011 self-healing,
    /// and persists the new active `msg_id` atomically under the channel mutex.
    pub(crate) async fn deliver_story_turn_serialized(
        &self,
        channel_id: &str,
        turn_number: u32,
        is_ended: bool,
        resp: EngineResponse,
        hud_opt: Option<&LiveStoryHud>,
        engine: Option<&GameEngine>,
    ) {
        let lock = self.live_cards().channel_lock(channel_id);
        let _guard = lock.lock().await;

        self.archive_previous_story_card_locked(channel_id, engine).await;

        let mut delivered = resp;
        let mut sent_json = render_story_turn_card(&delivered, hud_opt, false);
        let mut send_res = self.send_channel_card_returning_id(channel_id, &sent_json).await;
        if let Err(ref err) = send_res && is_kook_moderation_error(err) {
            warn!(channel_id, error = %err, "剧情卡触发 KOOK 40011 审核，自动柔化措辞并重发完整卡片");
            let softened = soften_response(&delivered);
            let soft_hud = hud_opt.map(soften_live_hud);
            let soft_json = render_story_turn_card(&softened, soft_hud.as_ref(), false);
            send_res = self.send_channel_card_returning_id(channel_id, &soft_json).await;
            if send_res.is_ok() {
                delivered = softened;
                sent_json = soft_json;
            }
        }
        match send_res {
            Ok(msg_id) if !msg_id.is_empty() && !is_ended => {
                self.remember_active_story_card_locked(
                    channel_id, msg_id, turn_number, delivered, sent_json, engine,
                );
            }
            Ok(_) => {}
            Err(err) if is_kook_moderation_error(&err) => {
                warn!(channel_id, error = %err, "柔化后仍触发 KOOK 40011，下发无敏感词保底交互卡");
                let safe_resp = safe_story_turn_fallback(&delivered);
                let safe_json = render_story_turn_card(&safe_resp, None, false);
                match self.send_channel_card_returning_id(channel_id, &safe_json).await {
                    Ok(msg_id) if !msg_id.is_empty() && !is_ended => {
                        self.remember_active_story_card_locked(
                            channel_id, msg_id, turn_number, safe_resp, safe_json, engine,
                        );
                    }
                    Ok(_) => {}
                    Err(fb_err) => {
                        error!(channel_id, error = %fb_err, "保底剧情卡发送仍然失败（请联系房主执行 /resend）");
                    }
                }
            }
            Err(err) => {
                warn!(channel_id, error = %err, "发送剧情卡片失败，自动降级为纯文本");
                self.send_plain_channel_fallback(channel_id, &delivered).await;
            }
        }
    }

    async fn archive_previous_story_card_locked(
        &self,
        channel_id: &str,
        engine: Option<&GameEngine>,
    ) {
        if let Some(prev) = self.live_cards().take(channel_id) {
            self.update_archived_card_resilient(channel_id, &prev.msg_id, prev.turn_number, Some(&prev.story))
                .await;
            if let Some(eng) = engine {
                let _ = eng.set_persisted_story_card(channel_id, None);
            }
            return;
        }
        let Some(eng) = engine else { return };
        let Ok(Some((old_msg_id, old_turn))) = eng.get_persisted_story_card(channel_id) else {
            return;
        };
        let story = eng.rebuild_story_turn_for_archive(channel_id, old_turn).ok().flatten();
        self.update_archived_card_resilient(channel_id, &old_msg_id, old_turn, story.as_ref())
            .await;
        let _ = eng.set_persisted_story_card(channel_id, None);
    }

    async fn update_archived_card_resilient(
        &self,
        channel_id: &str,
        msg_id: &str,
        turn_number: u32,
        story: Option<&EngineResponse>,
    ) {
        let primary = story
            .map(|s| render_story_turn_card(s, None, true))
            .unwrap_or_else(|| render_orphan_archived_card(turn_number));
        let Err(err) = self.update_channel_card(msg_id, &primary).await else {
            return;
        };
        if is_kook_moderation_error(&err) {
            if let Some(s) = story {
                let soft = render_story_turn_card(&soften_response(s), None, true);
                if self.update_channel_card(msg_id, &soft).await.is_ok() {
                    return;
                }
            }
            let orphan = render_orphan_archived_card(turn_number);
            if self.update_channel_card(msg_id, &orphan).await.is_ok() {
                return;
            }
        }
        warn!(channel_id, msg_id, error = %err, "归档上一张剧情卡按钮失败（不影响新卡发送）");
    }

    fn remember_active_story_card_locked(
        &self,
        channel_id: &str,
        msg_id: String,
        turn_number: u32,
        story: EngineResponse,
        last_card_json: String,
        engine: Option<&GameEngine>,
    ) {
        if let Some(eng) = engine
            && let Err(err) = eng.set_persisted_story_card(channel_id, Some((&msg_id, turn_number)))
        {
            warn!(channel_id, error = %err, "持久化当前剧情卡 msg_id 失败");
        }
        self.live_cards().replace(
            channel_id,
            ActiveStoryCard {
                msg_id,
                turn_number,
                story: Box::new(story),
                last_card_json,
            },
        );
    }

    /// Synchronizes the channel's active StoryTurn card in-place under the per-channel lock so
    /// concurrent events can NEVER clobber a newer turn's tracked `msg_id`.
    pub(crate) async fn sync_active_story_hud(
        &self,
        channel_id: &str,
        hud: Option<&LiveStoryHud>,
        engine: Option<&GameEngine>,
        allow_rehydrate: bool,
    ) -> SyncOutcome {
        let lock = self.live_cards().channel_lock(channel_id);
        let _guard = lock.lock().await;

        let active = match self.live_cards().get(channel_id) {
            Some(c) => c,
            None => {
                let Some(eng) = engine else { return SyncOutcome::Skipped };
                let Ok(Some((msg_id, saved_turn))) = eng.get_persisted_story_card(channel_id) else {
                    return SyncOutcome::Skipped;
                };
                match hud {
                    None => {
                        self.archive_previous_story_card_locked(channel_id, Some(eng)).await;
                        return SyncOutcome::Skipped;
                    }
                    Some(h) if h.turn_number != saved_turn => {
                        self.archive_previous_story_card_locked(channel_id, Some(eng)).await;
                        return SyncOutcome::Skipped;
                    }
                    Some(_) if !allow_rehydrate => return SyncOutcome::Skipped,
                    Some(_) => match eng.rebuild_story_turn_for_archive(channel_id, saved_turn) {
                        Ok(Some(story)) => ActiveStoryCard {
                            msg_id,
                            turn_number: saved_turn,
                            story: Box::new(story),
                            last_card_json: String::new(),
                        },
                        _ => return SyncOutcome::Skipped,
                    },
                }
            }
        };

        let Some(h) = hud else {
            self.archive_previous_story_card_locked(channel_id, engine).await;
            return SyncOutcome::Skipped;
        };
        if h.turn_number != active.turn_number {
            self.archive_previous_story_card_locked(channel_id, engine).await;
            return SyncOutcome::Skipped;
        }
        let updated = render_story_turn_card(&active.story, Some(h), false);
        if updated == active.last_card_json {
            return SyncOutcome::Unchanged;
        }
        match self.update_channel_card(&active.msg_id, &updated).await {
            Ok(()) => {
                self.live_cards().replace(
                    channel_id,
                    ActiveStoryCard { last_card_json: updated, ..active },
                );
                SyncOutcome::Updated
            }
            Err(err) if is_kook_moderation_error(&err) => {
                let soft_story = soften_response(&active.story);
                let soft_hud = soften_live_hud(h);
                let soft_json = render_story_turn_card(&soft_story, Some(&soft_hud), false);
                if self.update_channel_card(&active.msg_id, &soft_json).await.is_ok() {
                    self.live_cards().replace(
                        channel_id,
                        ActiveStoryCard {
                            story: Box::new(soft_story),
                            last_card_json: soft_json,
                            ..active
                        },
                    );
                    return SyncOutcome::Updated;
                }
                warn!(channel_id, error = %err, "柔化后原地刷新剧情卡仍触发 40011，降级为常规提示卡");
                SyncOutcome::Skipped
            }
            Err(err) => {
                warn!(channel_id, error = %err, "原地刷新剧情卡动态栏失败，降级为常规提示卡");
                SyncOutcome::Skipped
            }
        }
    }

    pub(crate) fn should_rehydrate_on_mode(sync_mode: InPlaceSyncMode) -> bool {
        matches!(sync_mode, InPlaceSyncMode::SilentOnSuccess | InPlaceSyncMode::TempToast)
    }
}
