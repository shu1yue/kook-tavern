use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use tavern_domain::LlmUsage;

use crate::prompts::ChatMessage;

#[derive(Debug, Serialize)]
pub(crate) struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: f32,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ResponseFormat {
    #[serde(rename = "type")]
    pub kind: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatCompletionResponse {
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub usage: Option<RawUsage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Choice {
    pub message: ChoiceMessage,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChoiceMessage {
    pub content: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RawPromptDetails {
    #[serde(default)]
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RawUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub prompt_cache_hit_tokens: Option<u64>,
    #[serde(default)]
    pub prompt_cache_miss_tokens: Option<u64>,
    #[serde(default)]
    pub prompt_tokens_details: Option<RawPromptDetails>,
}

pub(crate) fn extract_choice(resp: ChatCompletionResponse) -> Result<(String, LlmUsage)> {
    let usage = resp
        .usage
        .map(|u| {
            let prompt_tokens = u.prompt_tokens.unwrap_or(0);
            let completion_tokens = u.completion_tokens.unwrap_or(0);
            let hit = u.prompt_cache_hit_tokens.unwrap_or(0).max(
                u.prompt_tokens_details
                    .as_ref()
                    .and_then(|d| d.cached_tokens)
                    .unwrap_or(0),
            );
            let miss = u
                .prompt_cache_miss_tokens
                .unwrap_or_else(|| prompt_tokens.saturating_sub(hit));
            LlmUsage {
                prompt_tokens,
                completion_tokens,
                cache_hit_tokens: hit,
                cache_miss_tokens: miss,
            }
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
