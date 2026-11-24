use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tavern_core::{AppConfig, BoxFuture, LlmProvider};
use tavern_domain::{
    DmTurnOutput, LlmUsage, PendingAction, Player, RoomState, Scenario, TurnRecord,
};
use tracing::warn;

use crate::parser::parse_dm_reply;
use crate::prompts::{ChatMessage, build_dm_messages, build_summary_messages};

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
struct RawUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_cache_hit_tokens: u64,
    #[serde(default)]
    prompt_cache_miss_tokens: u64,
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
            .timeout(Duration::from_secs(90))
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

    async fn chat_completion(
        &self,
        model: &str,
        messages: Vec<ChatMessage>,
        prefer_json: bool,
    ) -> Result<(String, LlmUsage)> {
        let url = format!("{}/chat/completions", self.base_url);
        let mut req_body = ChatCompletionRequest {
            model: model.to_string(),
            messages,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            response_format: if prefer_json {
                Some(ResponseFormat {
                    kind: "json_object".to_string(),
                })
            } else {
                None
            },
        };

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&req_body)
            .send()
            .await
            .with_context(|| format!("请求 LLM 接口失败: {url}"))?;

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp.text().await.unwrap_or_default();
            if prefer_json && status.as_u16() == 400 && err_text.contains("response_format") {
                warn!("上游模型不支持 response_format=json_object，自动降级重试");
                req_body.response_format = None;
                let retry = self
                    .http
                    .post(&url)
                    .bearer_auth(&self.api_key)
                    .json(&req_body)
                    .send()
                    .await
                    .context("LLM 降级重试失败")?;
                let retry_status = retry.status();
                if !retry_status.is_success() {
                    let retry_err = retry.text().await.unwrap_or_default();
                    bail!("LLM 接口返回错误 ({retry_status}): {retry_err}");
                }
                let parsed: ChatCompletionResponse = retry.json().await?;
                return extract_choice(parsed);
            }
            bail!("LLM 接口返回错误 ({status}): {err_text}");
        }

        let parsed: ChatCompletionResponse = resp.json().await.context("解析 LLM 响应 JSON 失败")?;
        extract_choice(parsed)
    }
}

fn extract_choice(resp: ChatCompletionResponse) -> Result<(String, LlmUsage)> {
    let usage = resp
        .usage
        .map(|u| LlmUsage {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            cache_hit_tokens: u.prompt_cache_hit_tokens,
            cache_miss_tokens: u.prompt_cache_miss_tokens,
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
        &'a self,
        scenario: &'a Scenario,
        room: &'a RoomState,
        players: &'a [Player],
        recent_turns: &'a [TurnRecord],
        actions: &'a [PendingAction],
    ) -> BoxFuture<'a, Result<DmTurnOutput>> {
        Box::pin(async move {
            let messages = build_dm_messages(
                &self.prompts_dir,
                scenario,
                room,
                players,
                recent_turns,
                actions,
            );
            let (raw, usage) = self.chat_completion(&self.model, messages, true).await?;
            let reply = parse_dm_reply(&raw, room);
            Ok(DmTurnOutput { reply, usage })
        })
    }

    fn summarize_turns<'a>(
        &'a self,
        scenario: &'a Scenario,
        existing_summary: &'a str,
        turns: &'a [TurnRecord],
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            if turns.is_empty() {
                return Ok(existing_summary.to_string());
            }
            let messages =
                build_summary_messages(&self.prompts_dir, scenario, existing_summary, turns);
            let (summary, _) = self
                .chat_completion(&self.summary_model, messages, false)
                .await?;
            Ok(summary.trim().to_string())
        })
    }
}
