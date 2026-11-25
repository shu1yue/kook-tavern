use std::time::Duration;

use tavern_domain::{CardTheme, EngineResponse};
use tokio::time::sleep;
use tracing::{error, warn};

use crate::api::KookClient;
use crate::card_menus::render_wall_report_card;
use crate::cards::{chunk_kmarkdown, render_kook_card_json, render_private_card_json};
use crate::plain::render_plain_fallback;

impl KookClient {
    pub async fn dispatch_response(&self, channel_id: &str, resp: EngineResponse) {
        if matches!(resp, EngineResponse::Ignore) {
            return;
        }

        // PrivateNotice MUST NEVER be sent to the public channel!
        if let EngineResponse::PrivateNotice {
            user_id,
            title,
            body,
            actions,
        } = resp
        {
            let content = format!("**{title}**\n{body}");
            let is_dm_chan = channel_id.starts_with("dm:");
            if !is_dm_chan && body.chars().count() <= 3800 {
                let card_json = render_private_card_json(&title, &body, &actions);
                if self
                    .send_channel_temp_card(channel_id, &user_id, &card_json)
                    .await
                    .is_ok()
                {
                    return;
                }
                warn!(
                    user_id,
                    "频道仅本人可见卡片发送失败，自动降级为临时文本/私信"
                );
            }
            let mut prefer_temp = !is_dm_chan;
            for (idx, chunk) in chunk_kmarkdown(&content, 3500).into_iter().enumerate() {
                if idx > 0 {
                    sleep(Duration::from_millis(250)).await;
                }
                if prefer_temp {
                    if self
                        .send_channel_temp_kmarkdown(channel_id, &user_id, &chunk)
                        .await
                        .is_ok()
                    {
                        continue;
                    }
                    warn!(user_id, "频道临时消息发送失败，自动降级为私信发送");
                    prefer_temp = false;
                }
                if let Err(dm_err) = self.send_direct_kmarkdown(&user_id, &chunk).await {
                    warn!(user_id, error = %dm_err, "私密通知私信兜底同样失败");
                    break;
                }
            }
            return;
        }

        // WorkshopDesk is strictly per-user private (never falls back to public channel card!)
        if let EngineResponse::WorkshopDesk { ref user_id, .. } = resp {
            let uid = user_id.clone();
            let is_dm_chan = channel_id.starts_with("dm:");
            if !is_dm_chan {
                let card_json = render_kook_card_json(channel_id, &resp);
                if self
                    .send_channel_temp_card(channel_id, &uid, &card_json)
                    .await
                    .is_ok()
                {
                    return;
                }
                warn!(user_id = %uid, "工坊临时卡发送失败，降级为仅本人可见临时文本/私信");
            }
            let plain = render_plain_fallback(&resp, true);
            let mut prefer_temp = !is_dm_chan;
            for chunk in chunk_kmarkdown(&plain, 3500) {
                if prefer_temp
                    && self
                        .send_channel_temp_kmarkdown(channel_id, &uid, &chunk)
                        .await
                        .is_ok()
                {
                    continue;
                }
                prefer_temp = false;
                if let Err(err) = self.send_direct_kmarkdown(&uid, &chunk).await {
                    warn!(user_id = %uid, error = %err, "工坊私密兜底发送失败");
                    break;
                }
            }
            return;
        }

        // 1. Send public channel card FIRST for low latency
        let card_json = render_kook_card_json(channel_id, &resp);
        if let Err(err) = self.send_channel_card(channel_id, &card_json).await {
            warn!(channel_id, error = %err, "发送卡片消息失败，自动降级为分段 KMarkdown 发送");
            let plain = render_plain_fallback(&resp, false);
            for chunk in chunk_kmarkdown(&plain, 3500) {
                if let Err(fb_err) = self.send_channel_kmarkdown(channel_id, &chunk).await {
                    error!(channel_id, error = %fb_err, "降级发送频道消息同样失败");
                    break;
                }
            }
        }

        // 2. Publish highlight card to the configured report wall channel if present
        if let EngineResponse::ExportReport {
            wall_report: Some(ref wall),
            ..
        } = resp
        {
            let wall_card = render_wall_report_card(wall);
            if let Err(err) = self
                .send_channel_card(&wall.target_channel_id, &wall_card)
                .await
            {
                warn!(
                    wall_channel = %wall.target_channel_id,
                    error = %err,
                    "同步发送高光战报卡至战报墙失败（不影响本地导出）"
                );
            }
        }

        // 3. Deliver private whispers asynchronously with pacing + 3-tier fallback callback
        if let EngineResponse::StoryTurn {
            direct_messages, ..
        } = resp
            && !direct_messages.is_empty()
        {
            let client = self.clone();
            let chan = channel_id.to_string();
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
                    if let Err(dm_err) = client.send_direct_kmarkdown(&dm.user_id, &dm.content).await
                    {
                        warn!(
                            user_id = %dm.user_id,
                            player = %dm.player_name,
                            error = %dm_err,
                            "私信递纸条失败，降级为频道仅本人可见临时消息"
                        );
                        if chan.starts_with("dm:") {
                            undelivered_names.push(dm.player_name);
                        } else if let Err(tmp_err) = client
                            .send_channel_temp_kmarkdown(&chan, &dm.user_id, &dm.content)
                            .await
                        {
                            warn!(
                                user_id = %dm.user_id,
                                player = %dm.player_name,
                                error = %tmp_err,
                                "临时消息降级同样失败，记入公屏补领提醒"
                            );
                            undelivered_names.push(dm.player_name);
                        }
                    }
                }
                if !undelivered_names.is_empty() && !chan.starts_with("dm:") {
                    let notice = EngineResponse::Notice {
                        title: "⚠️ 秘密纸条送达提醒".to_string(),
                        body: format!(
                            "给 **{}** 的秘密纸条未能直接送达（可能关闭了私信或遇临时限流）。\n请随时点击剧情卡上的 **「🪪 我的角色卡」** 按钮（或发 `/me`、`/whisper`，均仅本人可见）补领查看喵！",
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
}
