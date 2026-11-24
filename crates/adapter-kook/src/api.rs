use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tavern_domain::EngineResponse;
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tracing::{error, warn};

use crate::cards::{chunk_kmarkdown, render_kook_card_json};
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

    pub async fn send_channel_card(&self, channel_id: &str, card_json: &str) -> Result<()> {
        let url = format!("{KOOK_API_BASE}/message/create");
        let payload = json!({
            "type": 10,
            "target_id": channel_id,
            "content": card_json,
        });
        self.post_checked(&url, &payload, "发送 KOOK 频道卡片").await
    }

    pub async fn send_channel_kmarkdown(&self, channel_id: &str, content: &str) -> Result<()> {
        let url = format!("{KOOK_API_BASE}/message/create");
        let payload = json!({
            "type": 9,
            "target_id": channel_id,
            "content": content,
        });
        self.post_checked(&url, &payload, "发送 KOOK 频道文本")
            .await
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
        self.post_checked(&url, &payload, "发送 KOOK 频道临时消息")
            .await
    }

    pub async fn send_direct_kmarkdown(&self, user_id: &str, content: &str) -> Result<()> {
        let url = format!("{KOOK_API_BASE}/direct-message/create");
        let payload = json!({
            "type": 9,
            "target_id": user_id,
            "content": content,
        });
        self.post_checked(&url, &payload, "发送 KOOK 私信").await
    }

    async fn post_checked(&self, url: &str, payload: &Value, op: &str) -> Result<()> {
        let http_resp = self
            .http
            .post(url)
            .header("Authorization", self.auth_header())
            .json(payload)
            .send()
            .await
            .with_context(|| format!("{op}请求失败"))?;

        let status = http_resp.status();
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
        Ok(())
    }

    /// Sends public channel card FIRST (falling back to chunked KMarkdown if card fails),
    /// then delivers private whispers asynchronously under bounded concurrency with pacing
    /// and automatic fallback to `temp_target_id` ephemeral channel messages.
    pub async fn dispatch_response(&self, channel_id: &str, resp: EngineResponse) {
        if matches!(resp, EngineResponse::Ignore) {
            return;
        }

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
                            "私信递纸条失败，自动降级为频道仅本人可见临时消息"
                        );
                        if let Err(tmp_err) = client
                            .send_channel_temp_kmarkdown(&chan, &dm.user_id, &dm.content)
                            .await
                        {
                            warn!(
                                user_id = %dm.user_id,
                                player = %dm.player_name,
                                error = %tmp_err,
                                "临时消息降级同样失败"
                            );
                        }
                    }
                }
            });
        }
    }
}
