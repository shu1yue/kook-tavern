use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::json;
use tracing::warn;

use crate::api::{KOOK_API_BASE, KookClient};

#[derive(Debug, Clone)]
pub(crate) struct ChannelMeta {
    pub guild_id: String,
    pub parent_id: String,
}

#[derive(Debug, Deserialize)]
struct KookResp<T> {
    code: i64,
    #[serde(default)]
    data: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
struct ChannelViewMeta {
    #[serde(default)]
    guild_id: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChannelCreateData {
    #[serde(default)]
    id: String,
}

impl KookClient {
    /// Resolves both `guild_id` and `parent_id` (category) of a KOOK channel via `channel/view`
    /// so dynamically spawned table channels land neatly under the same category as the lobby.
    pub(crate) async fn resolve_channel_meta(&self, channel_id: &str) -> Option<ChannelMeta> {
        if channel_id.trim().is_empty() {
            return None;
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
                warn!(channel_id, status = %r.status(), "查询 KOOK 频道分组信息状态异常");
                return None;
            }
            Err(err) => {
                warn!(channel_id, error = %err, "查询 KOOK 频道分组信息失败");
                return None;
            }
        };
        let parsed: KookResp<ChannelViewMeta> = resp.json().await.ok()?;
        if parsed.code != 0 {
            return None;
        }
        let data = parsed.data?;
        let guild_id = data.guild_id.unwrap_or_default();
        let parent_id = data.parent_id.unwrap_or_default();
        if !guild_id.trim().is_empty() {
            self.cache_channel_guild(channel_id, &guild_id);
        }
        Some(ChannelMeta {
            guild_id,
            parent_id,
        })
    }

    /// Creates a brand-new KOOK text channel (`type: 1`) under the given guild & optional category.
    pub(crate) async fn create_text_channel(
        &self,
        guild_id: &str,
        parent_id: Option<&str>,
        name: &str,
    ) -> Result<String> {
        let mut payload = json!({
            "guild_id": guild_id,
            "name": name,
            "type": 1,
        });
        if let Some(pid) = parent_id.map(str::trim).filter(|s| !s.is_empty() && *s != "0") {
            payload["parent_id"] = json!(pid);
        }
        let data = self
            .post_checked(
                &format!("{KOOK_API_BASE}/channel/create"),
                &payload,
                "创建 KOOK 独立包间频道",
            )
            .await?;
        let parsed: ChannelCreateData = serde_json::from_value(data).unwrap_or(ChannelCreateData {
            id: String::new(),
        });
        if parsed.id.trim().is_empty() {
            bail!("KOOK channel/create 返回空频道 ID");
        }
        Ok(parsed.id)
    }

    /// Permanently deletes a bot-managed KOOK table channel.
    /// Callers MUST verify the channel exists in `managed_tables` BEFORE invoking this!
    pub(crate) async fn delete_channel(&self, channel_id: &str) -> Result<()> {
        if channel_id.trim().is_empty() {
            bail!("频道 ID 为空，拒绝删除");
        }
        let payload = json!({ "channel_id": channel_id });
        let _ = self
            .post_checked(
                &format!("{KOOK_API_BASE}/channel/delete"),
                &payload,
                "删除 KOOK 独立包间频道",
            )
            .await?;
        Ok(())
    }
}
