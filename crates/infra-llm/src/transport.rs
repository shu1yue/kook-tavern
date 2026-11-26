use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use tavern_core::{AppConfig, BoxFuture, DecisionGate, LlmProvider, NoopDecisionGate};
use tavern_domain::{
    DmTurnOutput, ForgeScenarioOutput, ForgeScenarioRequest, LlmUsage, PendingAction, Player,
    RoomState, Scenario, SummaryOutput, TurnRecord, WorldEcho, detect_consistency_nudge_with_audit,
    merge_patch_delta, should_trigger_bulletin,
};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::memory::merge_chapter_memory;
use crate::parser::{parse_dm_reply_checked, parse_patch_delta};
use crate::prompt_ext::{
    build_lore_corpus, collect_unmatched_lore_candidates, summarize_whispers_for_audit,
};
use crate::prompts::{ChatMessage, build_dm_messages_full, build_summary_messages};
use crate::wire::{ChatCompletionRequest, ChatCompletionResponse, ResponseFormat, extract_choice};

const MAX_TRANSIENT_RETRIES: usize = 2;

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
    decision_gate: Arc<dyn DecisionGate>,
    audit_threshold: f64,
}

impl fmt::Debug for OpenAiLlmProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenAiLlmProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"***REDACTED***")
            .field("model", &self.model)
            .field("summary_model", &self.summary_model)
            .field("jev_enabled", &self.decision_gate.is_enabled())
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
            decision_gate: Arc::new(NoopDecisionGate),
            audit_threshold: cfg.jev.audit_threshold,
        })
    }

    pub fn with_decision_gate(
        mut self,
        gate: Arc<dyn DecisionGate>,
        audit_threshold: f64,
    ) -> Self {
        self.decision_gate = gate;
        self.audit_threshold = audit_threshold.clamp(0.50, 0.98);
        self
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
                match extract_choice(parsed) {
                    Ok(ok) => return Ok(ok),
                    Err(err) if attempt < MAX_TRANSIENT_RETRIES => {
                        attempt += 1;
                        if prefer_json && !downgraded_json && attempt == MAX_TRANSIENT_RETRIES {
                            downgraded_json = true;
                            req_body.response_format = None;
                        }
                        let delay_ms = 1200 * attempt as u64;
                        warn!(
                            attempt,
                            delay_ms,
                            downgraded_json,
                            error = %err,
                            "LLM 返回空内容，正在自动退避重试"
                        );
                        sleep(Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    Err(err) => return Err(err),
                }
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

impl LlmProvider for OpenAiLlmProvider {
    fn generate_turn<'a>(
        &'a self, scenario: &'a Scenario, room: &'a RoomState, players: &'a [Player],
        recent_turns: &'a [TurnRecord], actions: &'a [PendingAction],
        world_echoes: &'a [WorldEcho], room_whispers: &'a [tavern_domain::WhisperRecord],
        allow_nudge: bool,
    ) -> BoxFuture<'a, Result<DmTurnOutput>> {
        Box::pin(async move {
            let extra_lore_ids = if self.decision_gate.is_enabled() {
                let corpus = build_lore_corpus(room, recent_turns, actions);
                let candidates = collect_unmatched_lore_candidates(scenario, &corpus);
                if candidates.is_empty() {
                    Vec::new()
                } else {
                    self.decision_gate
                        .rank_hidden_lore(&room.location, &corpus, &candidates)
                        .await
                }
            } else {
                Vec::new()
            };
            let mut messages = build_dm_messages_full(
                &self.prompts_dir, scenario, room, players, recent_turns, actions, world_echoes,
                room_whispers, &extra_lore_ids,
            );
            let (raw, usage) = self.chat_completion(&self.model, messages.clone(), true, None).await?;
            let (mut reply, was_json) = parse_dm_reply_checked(&raw, room);
            let mut extra_usages = Vec::new();

            // One-shot `civ-atlas`-style consistency nudge: strictly budgeted (`allow_nudge`),
            // enhanced with optional System One semantic audit, scoped to omitted fields only.
            let mut did_whisper_redact = false;
            let bulletin_due = should_trigger_bulletin(scenario, room, room.turn_number + 1);
            if allow_nudge && was_json {
                let audit = if self.decision_gate.is_enabled()
                    && !players.is_empty()
                    && !reply.narrative.trim().is_empty()
                {
                    let has_vitals = reply.state_patches.iter().any(|p| p.hp_delta != 0 || p.san_delta != 0);
                    let has_item = reply.state_patches.iter().any(|p| !p.add_items.is_empty());
                    let whispers = summarize_whispers_for_audit(&reply);
                    self.decision_gate
                        .audit_dm_reply(&reply.narrative, &whispers, has_vitals, has_item)
                        .await
                } else {
                    None
                };
                if let Some(plan) = detect_consistency_nudge_with_audit(
                    scenario, room, players, &reply, bulletin_due, audit.as_ref(), self.audit_threshold,
                ) {
                    let nudge_tokens = if plan.want_narrative_redact || plan.want_narrative_rewrite {
                        self.max_tokens
                    } else {
                        self.max_tokens.min(400)
                    };
                    messages.push(ChatMessage { role: "assistant".into(), content: raw });
                    messages.push(ChatMessage { role: "user".into(), content: plan.prompt.clone() });
                    match self.chat_completion(&self.model, messages, true, Some(nudge_tokens)).await {
                        Ok((fix_raw, fix_usage)) => {
                            extra_usages.push(fix_usage);
                            if let Some(delta) = parse_patch_delta(&fix_raw) {
                                info!(
                                    channel_id = %room.channel_id,
                                    redact = plan.want_narrative_redact,
                                    rewrite = plan.want_narrative_rewrite,
                                    clocks = plan.want_clocks,
                                    "一次性状态/叙事质量拦截补齐生效"
                                );
                                if plan.want_narrative_redact {
                                    did_whisper_redact = true;
                                }
                                merge_patch_delta(&mut reply, &plan, delta);
                            }
                        }
                        Err(err) => warn!(channel_id = %room.channel_id, error = %err, "状态补齐校验失败，沿用原裁决"),
                    }
                }
            }

            // Solo-player clarity guard: strictly when total enrolled players `<= 1` (never when an
            // AFK teammate exists!) and no whisper redaction was explicitly triggered, fold any stray
            // LLM `private_whispers` cleanly into `narrative` so solo play stays in ONE cohesive card.
            if players.len() <= 1 && !did_whisper_redact && !reply.private_whispers.is_empty() {
                let mut extras = Vec::new();
                for w in reply.private_whispers.drain(..) {
                    let s = w.secret.trim();
                    if !s.is_empty() && !reply.narrative.contains(s) {
                        extras.push(s.to_string());
                    }
                }
                if !extras.is_empty() {
                    if !reply.narrative.trim().is_empty() {
                        reply.narrative.push_str("\n\n");
                    }
                    reply.narrative.push_str(&extras.join("\n"));
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
            let from_turn = turns.first().map(|t| t.turn_number).unwrap_or(1);
            let to_turn = turns.last().map(|t| t.turn_number).unwrap_or(from_turn);
            let messages = build_summary_messages(&self.prompts_dir, scenario, existing_summary, turns);
            let (raw, usage) = self.chat_completion(&self.summary_model, messages, true, Some(800)).await?;
            let merged = merge_chapter_memory(existing_summary, &raw, from_turn, to_turn);
            Ok(SummaryOutput { summary: merged, usage })
        })
    }

    fn forge_scenario<'a>(
        &'a self,
        req: &'a ForgeScenarioRequest,
    ) -> BoxFuture<'a, Result<ForgeScenarioOutput>> {
        Box::pin(async move {
            let messages = crate::generator::build_forge_messages(req);
            let (raw, usage) = self.chat_completion(&self.model, messages, true, Some(4096)).await?;
            match crate::generator::parse_scenario_json(&raw)
                .and_then(|parsed| crate::generator::finalize_forged_scenario(parsed, req, None))
            {
                Ok(scenario) => Ok(ForgeScenarioOutput::Forged {
                    scenario: Box::new(scenario),
                    usage,
                }),
                Err(err) => Ok(ForgeScenarioOutput::ValidationFailed {
                    error: err.to_string(),
                    usage,
                }),
            }
        })
    }
}
