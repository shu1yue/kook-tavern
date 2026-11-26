use std::fmt;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::Value;
use tavern_core::{BoxFuture, DecisionGate, JevConfig};
use tavern_domain::{LoreCandidate, OocVerdict, ReplyAuditVerdict};
use tracing::{debug, warn};

use crate::jev_questions::{
    BailianSystemOneRequest, DecisionResponse, GoogleDecisionsRequest,
    build_audit_state_and_questions, build_lore_state_and_questions,
    build_ooc_state_and_questions, clip_chars, extract_yes_prob,
};

const MAX_JEV_BODY_BYTES: usize = 256 * 1024;
const LORE_KEYS: [&str; 8] = [
    "lore_0", "lore_1", "lore_2", "lore_3", "lore_4", "lore_5", "lore_6", "lore_7",
];

#[derive(Clone)]
struct BailianBackend {
    endpoint: String,
    api_key: String,
    model: String,
}

#[derive(Clone)]
struct GoogleBackend {
    endpoint: String,
    api_key: Option<String>,
    proxy_secret: Option<String>,
    model: String,
}

#[derive(Clone)]
pub struct SystemOneJevGate {
    http: Client,
    bailian: Option<BailianBackend>,
    google: Option<GoogleBackend>,
    lore_threshold: f64,
    total_timeout: Duration,
}

impl fmt::Debug for SystemOneJevGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemOneJevGate")
            .field("has_bailian", &self.bailian.is_some())
            .field("has_google", &self.google.is_some())
            .field("lore_threshold", &self.lore_threshold)
            .field("total_timeout_ms", &(self.total_timeout.as_millis() as u64))
            .finish()
    }
}

fn clean_opt(s: &Option<String>) -> Option<String> {
    s.as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(String::from)
}

impl SystemOneJevGate {
    pub fn from_config(cfg: &JevConfig) -> Option<Self> {
        let bailian = clean_opt(&cfg.api_key)
            .filter(|_| !cfg.endpoint.trim().is_empty())
            .map(|k| BailianBackend {
                endpoint: cfg.endpoint.trim().to_string(),
                api_key: k,
                model: cfg.model.trim().to_string(),
            });

        let google = clean_opt(&cfg.google_endpoint).map(|ep| GoogleBackend {
            endpoint: ep,
            api_key: clean_opt(&cfg.google_api_key),
            proxy_secret: clean_opt(&cfg.google_proxy_secret),
            model: cfg.google_model.trim().to_string(),
        });

        if bailian.is_none() && google.is_none() {
            return None;
        }

        let timeout_ms = cfg.timeout_ms.clamp(500, 6000);
        let total_timeout = Duration::from_millis(timeout_ms);
        let connect_timeout = Duration::from_millis(timeout_ms.min(1500));
        let http = Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(total_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;

        Some(Self {
            http,
            bailian,
            google,
            lore_threshold: cfg.lore_threshold.clamp(0.50, 0.98),
            total_timeout,
        })
    }

    async fn call_bailian(
        &self,
        backend: &BailianBackend,
        state: String,
        questions: Value,
    ) -> Result<(DecisionResponse, f64)> {
        let t0 = Instant::now();
        let req_body = BailianSystemOneRequest { model: &backend.model, state, questions };
        let resp = self
            .http
            .post(&backend.endpoint)
            .bearer_auth(&backend.api_key)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
        parse_bounded_decision(resp, t0, "百炼 SystemOne").await
    }

    async fn call_google(
        &self,
        backend: &GoogleBackend,
        input: String,
        questions: Value,
    ) -> Result<(DecisionResponse, f64)> {
        let t0 = Instant::now();
        let req_body = GoogleDecisionsRequest { model: &backend.model, input, questions };
        let mut req = self.http.post(&backend.endpoint).json(&req_body);
        if let Some(ref k) = backend.api_key {
            req = req.header("x-goog-api-key", k);
        }
        if let Some(ref ps) = backend.proxy_secret {
            req = req.header("x-proxy-secret", ps);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("{}", e.without_url()))?;
        parse_bounded_decision(resp, t0, "Google Decisions").await
    }

    async fn query(
        &self,
        state: String,
        questions: Value,
        required_keys: &[&str],
    ) -> Option<(DecisionResponse, f64, &'static str)> {
        let has_both = self.google.is_some() && self.bailian.is_some();
        let google_budget = if has_both {
            Duration::from_millis(((self.total_timeout.as_millis() as u64) * 6 / 10).max(400))
        } else {
            self.total_timeout
        };

        tokio::time::timeout(self.total_timeout, async move {
            if let Some(ref g) = self.google {
                match tokio::time::timeout(
                    google_budget,
                    self.call_google(g, state.clone(), questions.clone()),
                )
                .await
                {
                    Ok(Ok((parsed, ms)))
                        if required_keys.iter().all(|k| extract_yes_prob(&parsed, k).is_some()) =>
                    {
                        return Some((parsed, ms, "google"));
                    }
                    Ok(Ok(_)) => warn!("Google Decisions 响应缺少必要判定键或值越界，降级尝试百炼/本地兜底"),
                    Ok(Err(err)) => warn!(error = %err, "Google Decisions 调用失败，降级尝试百炼/本地兜底"),
                    Err(_) => warn!("Google Decisions 预算超时，降级尝试百炼/本地兜底"),
                }
            }
            if let Some(ref b) = self.bailian {
                match self.call_bailian(b, state, questions).await {
                    Ok((parsed, ms))
                        if required_keys.iter().all(|k| extract_yes_prob(&parsed, k).is_some()) =>
                    {
                        return Some((parsed, ms, "bailian"));
                    }
                    Ok(_) => warn!("百炼 SystemOne 响应缺少必要判定键或值越界，自动回退纯 Rust 规则"),
                    Err(err) => warn!(error = %err, "百炼 SystemOne 调用超时或失败，自动回退纯 Rust 规则"),
                }
            }
            None
        })
        .await
        .unwrap_or_else(|_| {
            warn!("Jev 决策门达到总超时上限，静默回退纯 Rust 规则");
            None
        })
    }
}

impl DecisionGate for SystemOneJevGate {
    fn is_enabled(&self) -> bool {
        true
    }

    fn evaluate_ooc<'a>(
        &'a self,
        scenario_name: &'a str,
        location: &'a str,
        recent_scene: &'a str,
        sender_name: &'a str,
        message: &'a str,
    ) -> BoxFuture<'a, Option<OocVerdict>> {
        Box::pin(async move {
            let (state, questions) = build_ooc_state_and_questions(
                scenario_name, location, recent_scene, sender_name, message,
            );
            let (parsed, latency_ms, provider) =
                self.query(state, questions, &["is_in_game_action"]).await?;
            let prob_ic = extract_yes_prob(&parsed, "is_in_game_action")?;
            let prob_ooc = (1.0 - prob_ic).clamp(0.0, 1.0);
            debug!(provider, latency_ms, prob_ic, prob_ooc, "Jev OOC 吹水门卫判定完成");
            Some(OocVerdict { prob_ooc, provider, latency_ms })
        })
    }

    fn rank_hidden_lore<'a>(
        &'a self,
        location: &'a str,
        action_corpus: &'a str,
        candidates: &'a [LoreCandidate],
    ) -> BoxFuture<'a, Vec<String>> {
        Box::pin(async move {
            if candidates.is_empty() {
                return Vec::new();
            }
            let limited = &candidates[..candidates.len().min(LORE_KEYS.len())];
            let (state, questions) =
                build_lore_state_and_questions(location, action_corpus, limited);
            let Some((parsed, latency_ms, provider)) =
                self.query(state, questions, &LORE_KEYS[..limited.len()]).await
            else {
                return Vec::new();
            };

            let mut scored: Vec<(f64, String)> = limited
                .iter()
                .enumerate()
                .filter_map(|(idx, c)| {
                    let p = extract_yes_prob(&parsed, LORE_KEYS[idx])?;
                    (p >= self.lore_threshold).then_some((p, c.id.clone()))
                })
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            let picked: Vec<String> = scored.into_iter().take(3).map(|(_, id)| id).collect();
            debug!(provider, latency_ms, matched = picked.len(), "Jev 隐藏世界书语义补漏完成");
            picked
        })
    }

    fn audit_dm_reply<'a>(
        &'a self,
        narrative: &'a str,
        whispers_summary: &'a str,
        has_vitals_patch: bool,
        has_item_patch: bool,
    ) -> BoxFuture<'a, Option<ReplyAuditVerdict>> {
        Box::pin(async move {
            if has_vitals_patch
                && has_item_patch
                && whispers_summary.trim().is_empty()
                && narrative.trim().chars().count() < 40
            {
                return Some(ReplyAuditVerdict::default());
            }
            let (state, questions, req_keys) = build_audit_state_and_questions(
                narrative, whispers_summary, has_vitals_patch, has_item_patch,
            );
            let (parsed, latency_ms, provider) = self.query(state, questions, &req_keys).await?;
            let verdict = ReplyAuditVerdict {
                prob_missed_vitals: extract_yes_prob(&parsed, "missed_vitals").unwrap_or(0.0),
                prob_missed_item: extract_yes_prob(&parsed, "missed_item").unwrap_or(0.0),
                prob_whisper_leak: extract_yes_prob(&parsed, "leaked_whisper").unwrap_or(0.0),
                prob_cryptic_riddle: extract_yes_prob(&parsed, "is_cryptic_riddle").unwrap_or(0.0),
                prob_plot_stall: extract_yes_prob(&parsed, "is_plot_stall").unwrap_or(0.0),
                provider,
                latency_ms,
            };
            debug!(provider, latency_ms, ?verdict, "Jev DM 回复语义质检完成");
            Some(verdict)
        })
    }
}

async fn parse_bounded_decision(
    mut resp: reqwest::Response,
    t0: Instant,
    label: &str,
) -> Result<(DecisionResponse, f64)> {
    let status = resp.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| anyhow::anyhow!("{}", e.without_url()))? {
        if bytes.len() + chunk.len() > MAX_JEV_BODY_BYTES {
            bail!("{label} 响应体超过 256KiB 上限");
        }
        bytes.extend_from_slice(&chunk);
    }
    let body = String::from_utf8_lossy(&bytes);
    if !status.is_success() {
        if matches!(status.as_u16(), 401 | 403) {
            bail!("{label} 鉴权失败 ({status})");
        }
        bail!("{label} 返回 HTTP {status}: {}", clip_chars(&body, 180));
    }
    let parsed: DecisionResponse = serde_json::from_str(&body)?;
    let ms = match parsed.latency_ms {
        Some(v) if v.is_finite() && v > 0.0 => v,
        _ => t0.elapsed().as_secs_f64() * 1000.0,
    };
    Ok((parsed, ms))
}
