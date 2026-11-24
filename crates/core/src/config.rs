use std::env;
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Clone)]
pub struct AppConfig {
    pub kook_bot_token: Option<String>,
    pub llm_base_url: String,
    pub llm_api_key: Option<String>,
    pub llm_model: String,
    pub llm_summary_model: String,
    pub llm_temperature: f32,
    pub llm_max_tokens: u32,
    pub db_path: PathBuf,
    pub scenarios_dir: PathBuf,
    pub prompts_dir: PathBuf,
    pub exports_dir: PathBuf,
    pub recent_turns_window: usize,
    pub summary_interval_turns: u32,
    pub auto_resolve_all_ready: bool,
}

impl fmt::Debug for AppConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppConfig")
            .field(
                "kook_bot_token",
                &self.kook_bot_token.as_ref().map(|_| "***REDACTED***"),
            )
            .field("llm_base_url", &self.llm_base_url)
            .field(
                "llm_api_key",
                &self.llm_api_key.as_ref().map(|_| "***REDACTED***"),
            )
            .field("llm_model", &self.llm_model)
            .field("llm_summary_model", &self.llm_summary_model)
            .field("llm_temperature", &self.llm_temperature)
            .field("llm_max_tokens", &self.llm_max_tokens)
            .field("db_path", &self.db_path)
            .field("scenarios_dir", &self.scenarios_dir)
            .field("prompts_dir", &self.prompts_dir)
            .field("exports_dir", &self.exports_dir)
            .field("recent_turns_window", &self.recent_turns_window)
            .field("summary_interval_turns", &self.summary_interval_turns)
            .field("auto_resolve_all_ready", &self.auto_resolve_all_ready)
            .finish()
    }
}

impl AppConfig {
    /// Reads configuration strictly from process environment variables.
    /// Loading `.env` from disk is the responsibility of the binary entrypoint (`crates/app`).
    pub fn from_env() -> Result<Self> {
        let kook_bot_token = env_non_empty("KOOK_BOT_TOKEN");
        let llm_base_url = env_non_empty("LLM_BASE_URL")
            .unwrap_or_else(|| "https://api.deepseek.com/v1".to_string())
            .trim_end_matches('/')
            .to_string();
        let llm_api_key = env_non_empty("LLM_API_KEY");
        let llm_model =
            env_non_empty("LLM_MODEL").unwrap_or_else(|| "deepseek-chat".to_string());
        let llm_summary_model =
            env_non_empty("LLM_SUMMARY_MODEL").unwrap_or_else(|| llm_model.clone());

        let llm_temperature = env::var("LLM_TEMPERATURE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(0.85)
            .clamp(0.0, 2.0);
        let llm_max_tokens = env::var("LLM_MAX_TOKENS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(2048)
            .clamp(256, 8192);

        let db_path =
            PathBuf::from(env_non_empty("DB_PATH").unwrap_or_else(|| "data/tavern.db".to_string()));
        let scenarios_dir = PathBuf::from(
            env_non_empty("SCENARIOS_DIR").unwrap_or_else(|| "scenarios".to_string()),
        );
        let prompts_dir =
            PathBuf::from(env_non_empty("PROMPTS_DIR").unwrap_or_else(|| "prompts".to_string()));
        let exports_dir =
            PathBuf::from(env_non_empty("EXPORTS_DIR").unwrap_or_else(|| "exports".to_string()));

        let recent_turns_window = env::var("RECENT_TURNS_WINDOW")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(8)
            .clamp(2, 30);
        let summary_interval_turns = env::var("SUMMARY_INTERVAL_TURNS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(6)
            .clamp(2, 50);
        let auto_resolve_all_ready = env::var("AUTO_RESOLVE_ALL_READY")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);

        Ok(Self {
            kook_bot_token,
            llm_base_url,
            llm_api_key,
            llm_model,
            llm_summary_model,
            llm_temperature,
            llm_max_tokens,
            db_path,
            scenarios_dir,
            prompts_dir,
            exports_dir,
            recent_turns_window,
            summary_interval_turns,
            auto_resolve_all_ready,
        })
    }

    pub fn require_kook_token(&self) -> Result<&str> {
        self.kook_bot_token
            .as_deref()
            .context("缺少环境变量 KOOK_BOT_TOKEN，请先在 .env 中填入 KOOK 机器人 Token")
    }

    pub fn require_llm_key(&self) -> Result<&str> {
        self.llm_api_key.as_deref().context(
            "缺少环境变量 LLM_API_KEY，请先在 .env 中填入大模型 API Key（或使用 --mock-llm 进行本地离线试玩）",
        )
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secrets() {
        let cfg = AppConfig {
            kook_bot_token: Some("super-secret-kook-token".into()),
            llm_base_url: "https://api.deepseek.com/v1".into(),
            llm_api_key: Some("sk-secret-deepseek-key".into()),
            llm_model: "deepseek-chat".into(),
            llm_summary_model: "deepseek-chat".into(),
            llm_temperature: 0.8,
            llm_max_tokens: 1024,
            db_path: PathBuf::from("data/tavern.db"),
            scenarios_dir: PathBuf::from("scenarios"),
            prompts_dir: PathBuf::from("prompts"),
            exports_dir: PathBuf::from("exports"),
            recent_turns_window: 8,
            summary_interval_turns: 6,
            auto_resolve_all_ready: true,
        };
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("super-secret-kook-token"));
        assert!(!rendered.contains("sk-secret-deepseek-key"));
        assert!(rendered.contains("***REDACTED***"));
    }
}
