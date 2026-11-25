use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tracing::warn;

use crate::cards::thinking_placeholder_card_json;
use crate::live_cards::LiveCardRegistry;

const KOOK_API_BASE: &str = "https://www.kookapp.cn/api/v3";
const MAX_CHANNEL_GUILD_CACHE: usize = 1024;

#[derive(Clone)]
pub struct KookClient {
    http: Client,
    token: String,
    whisper_permits: Arc<Semaphore>,
    channel_guild_cache: Arc<RwLock<HashMap<String, String>>>,
    live_cards: LiveCardRegistry,
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

#[derive(Debug, Deserialize)]
struct ChannelViewData {
    #[serde(default)]
    guild_id: Option<String>,
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
            channel_guild_cache: Arc::new(RwLock::new(HashMap::new())),
            live_cards: LiveCardRegistry::default(),
        })
    }

    pub(crate) fn live_cards(&self) -> &LiveCardRegistry {
        &self.live_cards
    }

    pub(crate) fn whisper_permits_handle(&self) -> Arc<Semaphore> {
        self.whisper_permits.clone()
    }

    fn auth_header(&self) -> String {
        format!("Bot {}", self.token)
    }

    pub fn cache_channel_guild(&self, channel_id: &str, guild_id: &str) {
        if channel_id.is_empty() || guild_id.is_empty() {
            return;
        }
        if let Ok(mut map) = self.channel_guild_cache.write() {
            if map.len() >= MAX_CHANNEL_GUILD_CACHE && !map.contains_key(channel_id) {
                map.clear();
            }
            map.insert(channel_id.to_string(), guild_id.to_string());
        }
    }

    /// Authoritatively resolves a KOOK channel's owning `guild_id` (cached, falling back to
    /// `GET /api/v3/channel/view`), so button clicks in brand-new rooms still pass whitelist checks.
    pub async fn resolve_channel_guild(&self, channel_id: &str) -> Option<String> {
        if channel_id.is_empty() {
            return None;
        }
        if let Ok(map) = self.channel_guild_cache.read()
            && let Some(g) = map.get(channel_id)
        {
            return Some(g.clone());
        }
        let url = format!("{KOOK_API_BASE}/channel/view");
        let resp = match self
            .http
            .get(&url)
            .query(&[("target_id", channel_id)])
            .header("Authorization", self.auth_header())
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => r,
            Ok(r) => {
                warn!(channel_id, status = %r.status(), "查询 KOOK 频道所属服务器状态异常");
                return None;
            }
            Err(err) => {
                warn!(channel_id, error = %err, "查询 KOOK 频道所属服务器失败");
                return None;
            }
        };
        let parsed: KookApiResp<ChannelViewData> = match resp.json().await {
            Ok(v) => v,
            Err(err) => {
                warn!(channel_id, error = %err, "解析 KOOK channel/view 响应失败");
                return None;
            }
        };
        if parsed.code != 0 {
            warn!(channel_id, code = parsed.code, "KOOK channel/view 返回非零错误码");
            return None;
        }
        let guild_id = parsed.data?.guild_id.unwrap_or_default();
        if guild_id.trim().is_empty() {
            return None;
        }
        self.cache_channel_guild(channel_id, &guild_id);
        Some(guild_id)
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
        let payload = json!({ "type": 10, "target_id": channel_id, "content": card_json });
        let data = self
            .post_checked(&format!("{KOOK_API_BASE}/message/create"), &payload, "发送 KOOK 频道卡片")
            .await?;
        let parsed: MsgCreateData = serde_json::from_value(data).unwrap_or(MsgCreateData { msg_id: String::new() });
        Ok(parsed.msg_id)
    }

    /// Updates an existing public KOOK CardMessage in-place via `POST /api/v3/message/update`.
    pub async fn update_channel_card(&self, msg_id: &str, card_json: &str) -> Result<()> {
        if msg_id.trim().is_empty() {
            bail!("msg_id 为空，无法原地刷新卡片");
        }
        let payload = json!({ "msg_id": msg_id, "content": card_json });
        let _ = self
            .post_checked(&format!("{KOOK_API_BASE}/message/update"), &payload, "原地更新 KOOK 频道卡片")
            .await?;
        Ok(())
    }

    pub async fn send_channel_kmarkdown(&self, channel_id: &str, content: &str) -> Result<()> {
        let payload = json!({ "type": 9, "target_id": channel_id, "content": content });
        let _ = self
            .post_checked(&format!("{KOOK_API_BASE}/message/create"), &payload, "发送 KOOK 频道文本")
            .await?;
        Ok(())
    }

    pub async fn send_channel_temp_card(
        &self,
        channel_id: &str,
        temp_target_id: &str,
        card_json: &str,
    ) -> Result<()> {
        if channel_id.trim().is_empty()
            || channel_id.starts_with("dm:")
            || temp_target_id.trim().is_empty()
        {
            bail!("拒绝发送频道仅本人可见卡片：channel_id 或 temp_target_id 非法");
        }
        let url = format!("{KOOK_API_BASE}/message/create");
        let payload = json!({
            "type": 10,
            "target_id": channel_id,
            "temp_target_id": temp_target_id,
            "content": card_json,
        });
        let _ = self
            .post_checked(&url, &payload, "发送 KOOK 频道仅本人可见卡片")
            .await?;
        Ok(())
    }

    pub async fn send_channel_temp_kmarkdown(
        &self,
        channel_id: &str,
        temp_target_id: &str,
        content: &str,
    ) -> Result<()> {
        if channel_id.trim().is_empty()
            || channel_id.starts_with("dm:")
            || temp_target_id.trim().is_empty()
        {
            bail!("拒绝发送频道临时消息：channel_id 或 temp_target_id 非法");
        }
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
}
