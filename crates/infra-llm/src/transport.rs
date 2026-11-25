use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tavern_core::{AppConfig, BoxFuture, LlmProvider};
use tavern_domain::{
    DmTurnOutput, LlmUsage, PendingAction, Player, RoomState, Scenario, SummaryOutput, TurnRecord,
    WorldEcho, detect_consistency_nudge, merge_patch_delta, should_trigger_bulletin,
};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::parser::{parse_dm_reply_checked, parse_patch_delta};
use crate::prompts::{ChatMessage, build_dm_messages, build_summary_messages};

const MAX_TRANSIENT_RETRIES: usize = 2;

#[derive(Debug, Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Debug, Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
struct ChoiceMessage {
    content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawPromptDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
struct RawUsage {
    #[serde(default)]
    prompt_tokens: Option<u64>,
    #[serde(default)]
    completion_tokens: Option<u64>,
    #[serde(default)]
    prompt_cache_hit_tokens: Option<u64>,
    #[serde(default)]
    prompt_cache_miss_tokens: Option<u64>,
    #[serde(default)]
    prompt_tokens_details: Option<RawPromptDetails>,
}

#[derive(Clone)]
pub struct OpenAiLlmProvider {
    http: Client,
    base_url: String,
    api_key: String,
    model: String,
    summary_model: String,
    temperature: f32,
    max_tokens: u32,
    prompts_dir: PathBuf,
}

impl fmt::Debug for OpenAiLlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiLlmProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"***REDACTED***")
            .field("model", &self.model)
            .field("summary_model", &self.summary_model)
            .finish()
    }
}

impl OpenAiLlmProvider {
    pub fn from_config(cfg: &AppConfig) -> Result<Self> {
        let api_key = cfg.require_llm_key()?.to_string();
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(60))
            .build()
            .context("初始化 HTTP 客户端失败")?;
        Ok(Self {
            http,
            base_url: cfg.llm_base_url.clone(),
            api_key,
            model: cfg.llm_model.clone(),
            summary_model: cfg.llm_summary_model.clone(),
            temperature: cfg.llm_temperature,
            max_tokens: cfg.llm_max_tokens,
            prompts_dir: cfg.prompts_dir.clone(),
        })
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub async fn complete_json(&self, messages: Vec<ChatMessage>) -> Result<String> {
        let (text, _) = self.chat_completion(&self.model, messages, true, None).await?;
        Ok(text)
    }

    async fn chat_completion(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        prefer_json: bool,
        max_tokens_override: Option<u32>,
    ) -> Result<(String, LlmUsage)> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req_body = ChatCompletionRequest {
            model: model.to_string(),
            messages,
            temperature: self.temperature,
            max_tokens: max_tokens_override.unwrap_or(self.max_tokens),
            response_format: if prefer_json {
                Some(ResponseFormat { kind: "json_object".to_string() })
            } else {
                None
            },
        };

        let mut downgraded_json = false;
        let mut attempt = 0usize;

        loop {
            let send_res = self
                .http
                .post(&url)
                .bearer_auth(&self.api_key)
                .json(&req_body)
                .send()
                .await;

            let resp = match send_res {
                Ok(r) => r,
                Err(err) => {
                    // Only retry connection establishment errors; do NOT retry 60s request timeouts
                    // to avoid locking the room in `Resolving` for multi-minute retry cascades.
                    if attempt < MAX_TRANSIENT_RETRIES && err.is_connect() {
                        attempt += 1;
                        let delay_ms = 1500 * attempt as u64;
                        warn!(attempt, delay_ms, error = %err, "LLM 建连抖动，正在退避重试");
                        sleep(Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    return Err(err).context("请求 LLM 接口失败");
                }
            };

            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(|s| s.clamp(1, 10));

            if status.is_success() {
                let parsed: ChatCompletionResponse =
                    resp.json().await.context("解析 LLM 响应 JSON 失败")?;
                return extract_choice(parsed);
            }

            let err_text = resp.text().await.unwrap_or_default();

            if prefer_json
                && !downgraded_json
                && status.as_u16() == 400
                && err_text.contains("response_format")
            {
                warn!("上游模型不支持 response_format=json_object，自动降级重试");
                downgraded_json = true;
                req_body.response_format = None;
                continue;
            }

            let is_transient = matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504);
            if is_transient && attempt < MAX_TRANSIENT_RETRIES {
                attempt += 1;
                let wait = retry_after
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| Duration::from_millis(1500 * (attempt as u64) + 500));
                warn!(
                    status = status.as_u16(),
                    attempt,
                    wait_ms = wait.as_millis() as u64,
                    "LLM 接口返回瞬态错误，退避重试中"
                );
                sleep(wait).await;
                continue;
            }

            if matches!(status.as_u16(), 401 | 403) {
                warn!(status = status.as_u16(), "LLM 接口鉴权失败（已省略响应体以防敏感信息回显）");
            } else {
                warn!(
                    status = status.as_u16(),
                    body_preview = %err_text.chars().take(240).collect::<String>(),
                    "LLM 接口调用失败"
                );
            }
            bail!("LLM 接口返回 HTTP 错误 ({status})");
        }
    }
}

fn extract_choice(resp: ChatCompletionResponse) -> Result<(String, LlmUsage)> {
    let usage = resp
        .usage
        .map(|u| {
            let prompt_tokens = u.prompt_tokens.unwrap_or(0);
            let completion_tokens = u.completion_tokens.unwrap_or(0);
            let hit = u.prompt_cache_hit_tokens.unwrap_or(0).max(
                u.prompt_tokens_details.as_ref().and_then(|d| d.cached_tokens).unwrap_or(0),
            );
            let miss = u.prompt_cache_miss_tokens.unwrap_or_else(|| prompt_tokens.saturating_sub(hit));
            LlmUsage { prompt_tokens, completion_tokens, cache_hit_tokens: hit, cache_miss_tokens: miss }
        })
        .unwrap_or_default();

    let text = resp
        .choices
        .into_iter()
        .next()
        .and_then(|c| c.message.content)
        .unwrap_or_default();
    if text.trim().is_empty() {
        bail!("LLM 返回了空内容");
    }
    Ok((text, usage))
}

impl LlmProvider for OpenAiLlmProvider {
    fn generate_turn<'a>(
        &'a self, scenario: &'a Scenario, room: &'a RoomState, players: &'a [Player],
        recent_turns: &'a [TurnRecord], actions: &'a [PendingAction],
        world_echoes: &'a [WorldEcho], allow_nudge: bool,
    ) -> BoxFuture<'a, Result<DmTurnOutput>> {
        Box::pin(async move {
            let mut messages = build_dm_messages(
                &self.prompts_dir, scenario, room, players, recent_turns, actions, world_echoes,
            );
            let (raw, usage) = self.chat_completion(&self.model, messages.clone(), true, None).await?;
            let (mut reply, was_json) = parse_dm_reply_checked(&raw, room);
            let mut extra_usages = Vec::new();

            // One-shot `civ-atlas`-style consistency nudge: strictly budgeted (`allow_nudge`),
            // scoped to omitted fields only, and recorded as its own HTTP call for daily limits.
            let bulletin_due = should_trigger_bulletin(scenario, room, room.turn_number + 1);
            if allow_nudge
                && was_json
                && let Some(plan) = detect_consistency_nudge(scenario, room, players, &reply, bulletin_due)
            {
                messages.push(ChatMessage { role: "assistant".into(), content: raw });
                messages.push(ChatMessage { role: "user".into(), content: plan.prompt.clone() });
                match self.chat_completion(&self.model, messages, true, Some(400)).await {
                    Ok((fix_raw, fix_usage)) => {
                        extra_usages.push(fix_usage);
                        if let Some(delta) = parse_patch_delta(&fix_raw) {
                            info!(channel_id = %room.channel_id, "一次性状态一致性补齐生效");
                            merge_patch_delta(&mut reply, &plan, delta);
                        }
                    }
                    Err(err) => warn!(channel_id = %room.channel_id, error = %err, "状态补齐校验失败，沿用原裁决"),
                }
            }
            Ok(DmTurnOutput { reply, usage, extra_usages })
        })
    }

    fn summarize_turns<'a>(
        &'a self, scenario: &'a Scenario, existing_summary: &'a str, turns: &'a [TurnRecord],
    ) -> BoxFuture<'a, Result<SummaryOutput>> {
        Box::pin(async move {
            if turns.is_empty() {
                return Ok(SummaryOutput { summary: existing_summary.to_string(), usage: LlmUsage::default() });
            }
            let messages = build_summary_messages(&self.prompts_dir, scenario, existing_summary, turns);
            let (summary, usage) = self.chat_completion(&self.summary_model, messages, false, None).await?;
            Ok(SummaryOutput { summary: summary.trim().to_string(), usage })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_explicit_null_usage_fields() {
        let json = r#"{
            "choices": [{"message": {"content": "hello"}}],
            "usage": {
                "prompt_tokens": 120,
                "completion_tokens": null,
                "prompt_cache_hit_tokens": null,
                "prompt_cache_miss_tokens": null,
                "prompt_tokens_details": {"cached_tokens": 80}
            }
        }"#;
        let parsed: ChatCompletionResponse = serde_json::from_str(json).unwrap();
        let (text, usage) = extract_choice(parsed).unwrap();
        assert_eq!(text, "hello");
        assert_eq!(usage.prompt_tokens, 120);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.cache_hit_tokens, 80);
        assert_eq!(usage.cache_miss_tokens, 40);

        let all_null = r#"{
            "choices": [{"message": {"content": "ok"}}],
            "usage": {"prompt_tokens": null, "prompt_tokens_details": {"cached_tokens": null}}
        }"#;
        let parsed2: ChatCompletionResponse = serde_json::from_str(all_null).unwrap();
        let (_, usage2) = extract_choice(parsed2).unwrap();
        assert_eq!(usage2.prompt_tokens, 0);
        assert_eq!(usage2.cache_hit_tokens, 0);
    }
}
