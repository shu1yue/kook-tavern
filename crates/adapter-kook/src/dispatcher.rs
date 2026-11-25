use std::time::Duration;

use tavern_core::GameEngine;
use tavern_domain::{CardTheme, DirectMessageOut, EngineResponse, LiveStoryHud};
use tokio::time::sleep;
use tracing::{error, warn};

use crate::api::KookClient;
use crate::card_menus::render_wall_report_card;
use crate::cards::{chunk_kmarkdown, render_kook_card_json, render_private_card_json};
use crate::live_sync::SyncOutcome;
use crate::plain::render_plain_fallback;
use crate::sanitize::{is_kook_moderation_error, soften_response};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InPlaceSyncMode {
    /// Refreshes active StoryTurn card in-place and emits ZERO extra card on success (true native feel).
    SilentOnSuccess,
    /// Refreshes active StoryTurn card in-place and shows a personal temp toast to the caller.
    TempToast,
    /// Refreshes active StoryTurn card (or archives on reset/undo) and posts the normal public card.
    PublicCard,
}

#[derive(Debug, Clone, Copy)]
enum HudSync<'a> {
    /// Do NOT touch or archive the active StoryTurn card (e.g. error notices or standalone cards).
    Skip,
    /// Authoritative HUD fetched from the engine; `None` means the room was genuinely reset/ended.
    Live(Option<&'a LiveStoryHud>),
}

impl KookClient {
    pub async fn dispatch_response(&self, channel_id: &str, resp: EngineResponse) {
        self.dispatch_response_inner(
            channel_id,
            None,
            InPlaceSyncMode::PublicCard,
            resp,
            HudSync::Skip,
            None,
        )
        .await;
    }

    /// Cohesive v1.6–v1.8 delivery pipeline:
    /// - Archives previous StoryTurn buttons via `message/update` (even across restarts via SQLite);
    /// - Refreshes the active StoryTurn card's live HUD in-place on mid-turn actions/checks/drop-ins;
    /// - Keeps `StatusBoard`, `RolePicker`, and in-place action receipts out of the public feed.
    pub async fn dispatch_response_live(
        &self,
        channel_id: &str,
        caller_user_id: Option<&str>,
        sync_mode: InPlaceSyncMode,
        resp: EngineResponse,
        engine: &GameEngine,
    ) {
        let hud_owned = engine.get_live_story_hud(channel_id).ok();
        let hud_sync = match hud_owned.as_ref() {
            Some(opt) => HudSync::Live(opt.as_ref()),
            None => HudSync::Skip,
        };
        self.dispatch_response_inner(
            channel_id,
            caller_user_id,
            sync_mode,
            resp,
            hud_sync,
            Some(engine),
        )
        .await;
    }

    async fn dispatch_response_inner(
        &self,
        channel_id: &str,
        caller_user_id: Option<&str>,
        sync_mode: InPlaceSyncMode,
        resp: EngineResponse,
        hud_sync: HudSync<'_>,
        engine: Option<&GameEngine>,
    ) {
        if matches!(resp, EngineResponse::Ignore) {
            return;
        }
        let is_dm_chan = channel_id.starts_with("dm:");

        // 1. PrivateNotice MUST NEVER be sent to the public channel!
        //    Also refresh teammates' live StoryTurn HUD if the private notice came from a state
        //    mutation (e.g. mid-game drop-in briefing); read-only `/me` / `/hud` skip HTTP updates.
        if let EngineResponse::PrivateNotice { user_id, title, body, actions } = resp {
            if !is_dm_chan && let HudSync::Live(hud) = hud_sync {
                let allow_rehydrate = Self::should_rehydrate_on_mode(sync_mode);
                let _ = self.sync_active_story_hud(channel_id, hud, engine, allow_rehydrate).await;
            }
            if !is_dm_chan {
                let card_json = render_private_card_json(&title, &body, &actions);
                if self.send_channel_temp_card(channel_id, &user_id, &card_json).await.is_ok() {
                    return;
                }
                warn!(user_id, "频道仅本人可见卡片发送失败，自动降级为临时文本/私信");
            }
            let mut content = format!("**{title}**\n{body}");
            for btn in &actions {
                if (btn.value.starts_with("https://") || btn.value.starts_with("http://"))
                    && !content.contains(&btn.value)
                {
                    content.push_str(&format!("\n🔗 **{}**：{}", btn.label, btn.value));
                }
            }
            self.send_private_text_fallback(channel_id, &user_id, &content, !is_dm_chan)
                .await;
            return;
        }

        // 2. WorkshopDesk is strictly per-user private (never falls back to public channel card!)
        if let EngineResponse::WorkshopDesk { ref user_id, .. } = resp {
            let uid = user_id.clone();
            if !is_dm_chan {
                let card_json = render_kook_card_json(channel_id, &resp);
                if self.send_channel_temp_card(channel_id, &uid, &card_json).await.is_ok() {
                    return;
                }
                warn!(user_id = %uid, "工坊临时卡发送失败，降级为仅本人可见临时文本/私信");
            }
            let plain = render_plain_fallback(&resp, true);
            self.send_private_text_fallback(channel_id, &uid, &plain, !is_dm_chan)
                .await;
            return;
        }

        // 3. Utility panels (`StatusBoard` & `RolePicker`) open as per-user temp overlays so they
        //    never interrupt teammates or pollute the story stream (safe fallback to public card).
        if matches!(resp, EngineResponse::StatusBoard { .. } | EngineResponse::RolePicker { .. })
            && !is_dm_chan
            && let Some(uid) = caller_user_id
        {
            let card_json = render_kook_card_json(channel_id, &resp);
            if self.send_channel_temp_card(channel_id, uid, &card_json).await.is_ok() {
                return;
            }
            warn!(user_id = uid, "面板临时浮层发送失败，降级为普通频道卡片");
        }

        // 4. Cohesive StoryTurn: serialized per-channel archive + send + persist!
        if let EngineResponse::StoryTurn { turn_number, ref ending_reached, ref direct_messages, .. } = resp {
            let dms = direct_messages.clone();
            let is_ended = ending_reached.is_some();
            if !is_dm_chan {
                let hud_opt = match hud_sync { HudSync::Live(h) => h, HudSync::Skip => None };
                self.deliver_story_turn_serialized(
                    channel_id, turn_number, is_ended, resp, hud_opt, engine,
                )
                .await;
            } else {
                self.send_public_card_with_fallback(channel_id, &resp).await;
            }
            if !dms.is_empty() { self.spawn_whisper_delivery(channel_id.to_string(), dms); }
            return;
        }

        // 5. Notice responses: sync active StoryTurn card in-place ONLY when `HudSync::Live`!
        //    Under `SilentOnSuccess`, ONLY suppress the extra notice when the live card actually
        //    changed (`Updated`) AND theme is `Success`/`Secondary` — so buffered mid-resolving
        //    actions, stale button notices, or `Info` hints ALWAYS display as personal TempToast!
        if let EngineResponse::Notice { ref title, ref body, theme, .. } = resp
            && !is_dm_chan
            && let HudSync::Live(hud) = hud_sync
        {
            let allow_rehydrate = Self::should_rehydrate_on_mode(sync_mode);
            let outcome = self.sync_active_story_hud(channel_id, hud, engine, allow_rehydrate).await;
            let suppressible = matches!(theme, CardTheme::Success | CardTheme::Secondary);
            if outcome == SyncOutcome::Updated
                && sync_mode == InPlaceSyncMode::SilentOnSuccess
                && suppressible
            {
                return;
            }
            let synced_or_info = outcome != SyncOutcome::Skipped || !suppressible;
            if sync_mode != InPlaceSyncMode::PublicCard
                && (sync_mode == InPlaceSyncMode::TempToast || synced_or_info)
                && let Some(uid) = caller_user_id
            {
                let toast_resp = EngineResponse::Notice {
                    title: title.clone(),
                    body: body.clone(),
                    theme,
                    show_actions: false,
                };
                let toast_card = render_kook_card_json(channel_id, &toast_resp);
                if self.send_channel_temp_card(channel_id, uid, &toast_card).await.is_ok() {
                    return;
                }
            }
        }

        // 6. Standard public card delivery (menu, store, export, public notices, etc.)
        self.send_public_card_with_fallback(channel_id, &resp).await;

        if let EngineResponse::ExportReport { wall_report: Some(ref wall), .. } = resp {
            let wall_card = render_wall_report_card(wall);
            if let Err(err) = self.send_channel_card(&wall.target_channel_id, &wall_card).await {
                warn!(wall_channel = %wall.target_channel_id, error = %err, "同步发送高光战报卡至战报墙失败");
            }
        }
    }

    async fn send_public_card_with_fallback(&self, channel_id: &str, resp: &EngineResponse) {
        let card_json = render_kook_card_json(channel_id, resp);
        if let Err(err) = self.send_channel_card(channel_id, &card_json).await {
            if is_kook_moderation_error(&err) {
                let soft = soften_response(resp);
                let soft_json = render_kook_card_json(channel_id, &soft);
                if self.send_channel_card(channel_id, &soft_json).await.is_ok() {
                    return;
                }
            }
            warn!(channel_id, error = %err, "发送卡片消息失败，自动降级为分段 KMarkdown 发送");
            self.send_plain_channel_fallback(channel_id, resp).await;
        }
    }

    pub(crate) async fn send_plain_channel_fallback(&self, channel_id: &str, resp: &EngineResponse) {
        let plain = render_plain_fallback(resp, false);
        for chunk in chunk_kmarkdown(&plain, 3500) {
            if let Err(fb_err) = self.send_channel_kmarkdown(channel_id, &chunk).await {
                error!(channel_id, error = %fb_err, "降级发送频道消息同样失败");
                break;
            }
        }
    }

    async fn send_private_text_fallback(
        &self,
        channel_id: &str,
        user_id: &str,
        content: &str,
        mut prefer_temp: bool,
    ) {
        for (idx, chunk) in chunk_kmarkdown(content, 3500).into_iter().enumerate() {
            if idx > 0 {
                sleep(Duration::from_millis(250)).await;
            }
            if prefer_temp {
                if self.send_channel_temp_kmarkdown(channel_id, user_id, &chunk).await.is_ok() {
                    continue;
                }
                warn!(user_id, "频道临时消息发送失败，自动降级为私信发送");
                prefer_temp = false;
            }
            if let Err(dm_err) = self.send_direct_kmarkdown(user_id, &chunk).await {
                warn!(user_id, error = %dm_err, "私密通知私信兜底同样失败");
                break;
            }
        }
    }

    fn spawn_whisper_delivery(&self, chan: String, direct_messages: Vec<DirectMessageOut>) {
        let client = self.clone();
        let permits = self.whisper_permits_handle();
        tokio::spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            let mut undelivered_names = Vec::new();
            for (idx, dm) in direct_messages.into_iter().enumerate() {
                if idx > 0 {
                    sleep(Duration::from_millis(400)).await;
                }
                if let Err(dm_err) = client.send_direct_kmarkdown(&dm.user_id, &dm.content).await {
                    warn!(user_id = %dm.user_id, player = %dm.player_name, error = %dm_err, "私信递纸条失败，降级临时消息");
                    if chan.starts_with("dm:")
                        || client.send_channel_temp_kmarkdown(&chan, &dm.user_id, &dm.content).await.is_err()
                    {
                        undelivered_names.push(dm.player_name);
                    }
                }
            }
            if !undelivered_names.is_empty() && !chan.starts_with("dm:") {
                let notice = EngineResponse::Notice {
                    title: "⚠️ 秘密纸条送达提醒".to_string(),
                    body: format!(
                        "给 **{}** 的秘密纸条未能直接送达。请随时点剧情卡上的 **「🪪 我的角色卡」**（仅本人可见）补领查看喵！",
                        undelivered_names.join("、")
                    ),
                    theme: CardTheme::Warning,
                    show_actions: false,
                };
                let card = render_kook_card_json(&chan, &notice);
                let _ = client.send_channel_card(&chan, &card).await;
            }
        });
    }
}
