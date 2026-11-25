use std::env;
use std::fmt;

#[derive(Clone)]
pub struct JevConfig {
    pub api_key: Option<String>,
    pub endpoint: String,
    pub model: String,
    pub google_endpoint: Option<String>,
    pub google_api_key: Option<String>,
    pub google_proxy_secret: Option<String>,
    pub google_model: String,
    pub timeout_ms: u64,
    pub ooc_threshold: f64,
    pub lore_threshold: f64,
    pub audit_threshold: f64,
}

impl Default for JevConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            endpoint: "https://dashscope.aliyuncs.com/compatible-mode/v1/systemone".to_string(),
            model: "decision-model-preview".to_string(),
            google_endpoint: None,
            google_api_key: None,
            google_proxy_secret: None,
            google_model: "gemini-decision-1".to_string(),
            timeout_ms: 2000,
            ooc_threshold: 0.85,
            lore_threshold: 0.72,
            audit_threshold: 0.80,
        }
    }
}

fn redact_url_query(url: &str) -> &str {
    url.split('?').next().unwrap_or(url)
}

impl fmt::Debug for JevConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JevConfig")
            .field("api_key", &self.api_key.as_ref().map(|_| "***REDACTED***"))
            .field("endpoint", &redact_url_query(&self.endpoint))
            .field("model", &self.model)
            .field(
                "google_endpoint",
                &self.google_endpoint.as_deref().map(redact_url_query),
            )
            .field(
                "google_api_key",
                &self.google_api_key.as_ref().map(|_| "***REDACTED***"),
            )
            .field(
                "google_proxy_secret",
                &self.google_proxy_secret.as_ref().map(|_| "***REDACTED***"),
            )
            .field("google_model", &self.google_model)
            .field("timeout_ms", &self.timeout_ms)
            .field("ooc_threshold", &self.ooc_threshold)
            .field("lore_threshold", &self.lore_threshold)
            .field("audit_threshold", &self.audit_threshold)
            .finish()
    }
}

impl JevConfig {
    pub fn is_configured(&self) -> bool {
        (self.api_key.as_deref().is_some_and(|s| !s.trim().is_empty())
            && !self.endpoint.trim().is_empty())
            || self
                .google_endpoint
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
    }

    pub fn from_env() -> Self {
        let def = Self::default();
        let api_key = env_non_empty("JEV_API_KEY");
        let endpoint = env_non_empty("JEV_ENDPOINT").unwrap_or(def.endpoint);
        let model = env_non_empty("JEV_MODEL").unwrap_or(def.model);
        let google_endpoint = env_non_empty("JEV_GOOGLE_ENDPOINT");
        let google_api_key = env_non_empty("JEV_GOOGLE_API_KEY");
        let google_proxy_secret = env_non_empty("JEV_GOOGLE_PROXY_SECRET");
        let google_model = env_non_empty("JEV_GOOGLE_MODEL").unwrap_or(def.google_model);
        let timeout_ms = env_u64("JEV_TIMEOUT_MS", def.timeout_ms).clamp(500, 6000);
        let ooc_threshold = env_f64("JEV_OOC_THRESHOLD", def.ooc_threshold).clamp(0.50, 0.98);
        let lore_threshold = env_f64("JEV_LORE_THRESHOLD", def.lore_threshold).clamp(0.50, 0.98);
        let audit_threshold = env_f64("JEV_AUDIT_THRESHOLD", def.audit_threshold).clamp(0.50, 0.98);

        Self {
            api_key,
            endpoint,
            model,
            google_endpoint,
            google_api_key,
            google_proxy_secret,
            google_model,
            timeout_ms,
            ooc_threshold,
            lore_threshold,
            audit_threshold,
        }
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}
