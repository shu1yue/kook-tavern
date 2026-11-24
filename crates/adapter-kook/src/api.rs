use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tavern_domain::{CardTheme, EngineResponse};
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tracing::{error, warn};

use crate::cards::{chunk_kmarkdown, render_kook_card_json, thinking_placeholder_card_json};
use crate::plain::render_plain_fallback;

const KOOK_API_BASE: &str = "https://www.kookapp.cn/api/v3";

#[derive(Clone)]
pub struct KookClient {
    http: Client,
    token: String,
    whisper_permits: Arc<Semaphore>,
}

impl fmt::Debug for KookClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KookClient")
            .field("token", &"***REDACTED***")
            .finish()
    }
}

#[derive(Debug, Deserialize)]
struct KookApiResp<T> {
    code: i64,
    message: String,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct GatewayData {
    url: String,
}

#[derive(Debug, Deserialize)]
struct MsgCreateData {
    msg_id: String,
}

impl KookClient {
    pub fn new(token: String) -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()
            .context("初始化 KOOK HTTP 客户端失败")?;
        Ok(Self {
            http,
            token,
            whisper_permits: Arc::new(Semaphore::new(16)),
        })
    }

    fn auth_header(&self) -> String {
        format!("Bot {}", self.token)
    }

    pub async fn get_gateway_url(&self) -> Result<String> {
        let url = format!("{KOOK_API_BASE}/gateway/index?compress=1");
        let http_resp = self
            .http
            .get(&url)
            .header("Authorization", self.auth_header())
            .send()
            .await
            .context("请求 KOOK Gateway 失败")?;
        let status = http_resp.status();
        if !status.is_success() {
            bail!("KOOK Gateway HTTP 状态异常: {status}");
        }
        let resp: KookApiResp<GatewayData> = http_resp
            .json()
            .await
            .context("解析 KOOK Gateway JSON 失败")?;
        if resp.code != 0 {
            bail!("KOOK Gateway 返回错误 (code={}): {}", resp.code, resp.message);
        }
        resp.data
            .map(|d| d.url)
            .context("KOOK Gateway 响应缺少 url 字段")
    }

    pub async fn send_thinking_placeholder(&self, channel_id: &str) -> Option<String> {
        let card = thinking_placeholder_card_json();
        match self.send_channel_card_returning_id(channel_id, &card).await {
            Ok(id) if !id.trim().is_empty() => Some(id),
            Ok(_) => {
                warn!(channel_id, "推演占位卡片返回空 msg_id，跳过后续删卡步骤");
                None
            }
            Err(err) => {
                warn!(channel_id, error = %err, "发送推演占位卡片失败（不影响主流程）");
                None
            }
        }
    }

    pub async fn delete_channel_message(&self, msg_id: &str) {
        if msg_id.trim().is_empty() {
            return;
        }
        let url = format!("{KOOK_API_BASE}/message/delete");
        let payload = json!({ "msg_id": msg_id });
        if let Err(err) = self.post_checked(&url, &payload, "删除 KOOK 占位消息").await {
            warn!(msg_id, error = %err, "删除占位消息失败");
        }
    }

    pub async fn send_channel_card(&self, channel_id: &str, card_json: &str) -> Result<()> {
        let _ = self
            .send_channel_card_returning_id(channel_id, card_json)
            .await?;
        Ok(())
    }

    pub async fn send_channel_card_returning_id(
        &self,
        channel_id: &str,
        card_json: &str,
    ) -> Result<String> {
        let url = format!("{KOOK_API_BASE}/message/create");
        let payload = json!({
            "type": 10,
            "target_id": channel_id,
            "content": card_json,
        });
        let data = self
            .post_checked(&url, &payload, "发送 KOOK 频道卡片")
            .await?;
        let parsed: MsgCreateData = serde_json::from_value(data).unwrap_or(MsgCreateData {
            msg_id: String::new(),
        });
        Ok(parsed.msg_id)
    }

    pub async fn send_channel_kmarkdown(&self, channel_id: &str, content: &str) -> Result<()> {
        let url = format!("{KOOK_API_BASE}/message/create");
        let payload = json!({
            "type": 9,
            "target_id": channel_id,
            "content": content,
        });
        let _ = self
            .post_checked(&url, &payload, "发送 KOOK 频道文本")
            .await?;
        Ok(())
    }

    pub async fn send_channel_temp_kmarkdown(
        &self,
        channel_id: &str,
        temp_target_id: &str,
        content: &str,
    ) -> Result<()> {
        let url = format!("{KOOK_API_BASE}/message/create");
        let payload = json!({
            "type": 9,
            "target_id": channel_id,
            "temp_target_id": temp_target_id,
            "content": content,
        });
        let _ = self
            .post_checked(&url, &payload, "发送 KOOK 频道临时消息")
            .await?;
        Ok(())
    }

    pub async fn send_direct_kmarkdown(&self, user_id: &str, content: &str) -> Result<()> {
        let url = format!("{KOOK_API_BASE}/direct-message/create");
        let payload = json!({
            "type": 9,
            "target_id": user_id,
            "content": content,
        });
        let _ = self.post_checked(&url, &payload, "发送 KOOK 私信").await?;
        Ok(())
    }

    async fn post_checked(&self, url: &str, payload: &Value, op: &str) -> Result<Value> {
        for attempt in 0..=1 {
            let http_resp = self
                .http
                .post(url)
                .header("Authorization", self.auth_header())
                .json(payload)
                .send()
                .await
                .with_context(|| format!("{op}请求失败"))?;

            let status = http_resp.status();
            if status.as_u16() == 429 && attempt == 0 {
                let wait_secs = http_resp
                    .headers()
                    .get("x-rate-limit-reset")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(2)
                    .clamp(1, 5);
                warn!(op, wait_secs, "触发 KOOK API 429 频控，错峰重试中");
                sleep(Duration::from_secs(wait_secs)).await;
                continue;
            }
            if !status.is_success() {
                bail!("{op} HTTP 状态异常: {status}");
            }
            let resp: KookApiResp<Value> = http_resp
                .json()
                .await
                .with_context(|| format!("解析{op}响应失败"))?;
            if resp.code != 0 {
                bail!("{op}失败 (code={}): {}", resp.code, resp.message);
            }
            return Ok(resp.data.unwrap_or(Value::Null));
        }
        bail!("{op}在 429 重试后仍然失败")
    }

    pub async fn dispatch_response(&self, channel_id: &str, resp: EngineResponse) {
        if matches!(resp, EngineResponse::Ignore) {
            return;
        }

        // PrivateNotice MUST NEVER be sent to the public channel!
        if let EngineResponse::PrivateNotice {
            user_id,
            title,
            body,
        } = resp
        {
            let content = format!("**{title}**\n{body}");
            let mut dm_failed = false;
            for (idx, chunk) in chunk_kmarkdown(&content, 3500).into_iter().enumerate() {
                if idx > 0 {
                    sleep(Duration::from_millis(250)).await;
                }
                if !dm_failed {
                    if let Err(dm_err) = self.send_direct_kmarkdown(&user_id, &chunk).await {
                        warn!(user_id, error = %dm_err, "私信发送专属纸条失败，自动降级为频道仅本人可见临时消息");
                        dm_failed = true;
                    } else {
                        continue;
                    }
                }
                if let Err(tmp_err) = self
                    .send_channel_temp_kmarkdown(channel_id, &user_id, &chunk)
                    .await
                {
                    warn!(user_id, error = %tmp_err, "专属纸条临时消息降级同样失败");
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

        // 2. Deliver private whispers asynchronously with pacing + 3-tier fallback callback
        if let EngineResponse::StoryTurn {
            direct_messages, ..
        } = resp
            && !direct_messages.is_empty()
        {
            let client = self.clone();
            let chan = channel_id.to_string();
            let permits = self.whisper_permits.clone();
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
                        if let Err(tmp_err) = client
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
                if !undelivered_names.is_empty() {
                    let notice = EngineResponse::Notice {
                        title: "⚠️ 秘密纸条送达提醒".to_string(),
                        body: format!(
                            "给 **{}** 的秘密纸条未能直接送达（可能关闭了私信或遇临时限流）。\n请随时在频道发送 `/whisper`（仅本人可见）补领查看喵！",
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
