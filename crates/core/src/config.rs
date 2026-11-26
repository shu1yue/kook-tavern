use std::env;
use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use tavern_domain::LlmUsage;

use crate::jev_config::JevConfig;

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
    pub allowed_guild_ids: Vec<String>,
    pub allowed_channel_ids: Vec<String>,
    pub admin_user_ids: Vec<String>,
    pub export_channel_id: Option<String>,
    pub web_bind_addr: Option<String>,
    pub web_public_base_url: Option<String>,
    pub daily_max_llm_calls: u64,
    pub daily_max_cny: f64,
    pub price_cache_hit_cny_per_m: f64,
    pub price_cache_miss_cny_per_m: f64,
    pub price_output_cny_per_m: f64,
    pub jev: JevConfig,
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
            .field("allowed_guild_ids", &self.allowed_guild_ids.len())
            .field("allowed_channel_ids", &self.allowed_channel_ids.len())
            .field("admin_user_ids", &self.admin_user_ids.len())
            .field("daily_max_llm_calls", &self.daily_max_llm_calls)
            .field("daily_max_cny", &self.daily_max_cny)
            .field("jev", &self.jev)
            .finish()
    }
}

impl AppConfig {
    /// Reads configuration strictly from process environment variables.
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

        let llm_temperature = env_f32("LLM_TEMPERATURE", 0.75).clamp(0.0, 2.0);
        let llm_max_tokens = env_u32("LLM_MAX_TOKENS", 2048).clamp(256, 8192);

        let db_path =
            PathBuf::from(env_non_empty("DB_PATH").unwrap_or_else(|| "data/tavern.db".to_string()));
        let scenarios_dir = PathBuf::from(
            env_non_empty("SCENARIOS_DIR").unwrap_or_else(|| "scenarios".to_string()),
        );
        let prompts_dir =
            PathBuf::from(env_non_empty("PROMPTS_DIR").unwrap_or_else(|| "prompts".to_string()));
        let exports_dir =
            PathBuf::from(env_non_empty("EXPORTS_DIR").unwrap_or_else(|| "exports".to_string()));

        let recent_turns_window = env_usize("RECENT_TURNS_WINDOW", 8).clamp(2, 30);
        let summary_interval_turns = env_u32("SUMMARY_INTERVAL_TURNS", 6).clamp(2, 50);
        let auto_resolve_all_ready = env::var("AUTO_RESOLVE_ALL_READY")
            .ok()
            .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
            .unwrap_or(true);

        let allowed_guild_ids = env_csv("ALLOWED_GUILD_IDS");
        let allowed_channel_ids = env_csv("ALLOWED_CHANNEL_IDS");
        let admin_user_ids = env_csv("ADMIN_USER_IDS");
        let export_channel_id = env_non_empty("EXPORT_CHANNEL_ID");
        let web_bind_addr = env_non_empty("WEB_BIND_ADDR");
        let web_public_base_url = env_non_empty("WEB_PUBLIC_BASE_URL")
            .map(|u| u.trim_end_matches('/').to_string());
        let daily_max_llm_calls = env_u64("DAILY_MAX_LLM_CALLS", 200);
        let daily_max_cny = env_f64("DAILY_MAX_CNY", 20.0).max(0.0);

        // DeepSeek V3.2 / deepseek-chat default RMB pricing per 1M tokens
        let price_cache_hit_cny_per_m = env_f64("PRICE_CACHE_HIT_CNY_PER_M", 0.2).max(0.0);
        let price_cache_miss_cny_per_m = env_f64("PRICE_CACHE_MISS_CNY_PER_M", 2.0).max(0.0);
        let price_output_cny_per_m = env_f64("PRICE_OUTPUT_CNY_PER_M", 3.0).max(0.0);

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
            allowed_guild_ids,
            allowed_channel_ids,
            admin_user_ids,
            export_channel_id,
            web_bind_addr,
            web_public_base_url,
            daily_max_llm_calls,
            daily_max_cny,
            price_cache_hit_cny_per_m,
            price_cache_miss_cny_per_m,
            price_output_cny_per_m,
            jev: JevConfig::from_env(),
        })
    }

    pub fn hud_secret(&self) -> [u8; 32] {
        let explicit = env_non_empty("HUD_SIGNING_SECRET");
        crate::hud_token::derive_hud_secret(
            explicit.as_deref(),
            self.kook_bot_token.as_deref(),
            self.llm_api_key.as_deref(),
        )
    }

    pub fn require_kook_token(&self) -> Result<&str> {
        self.kook_bot_token
            .as_deref()
            .context("缺少环境变量 KOOK_BOT_TOKEN，请先在 .env 中填入 KOOK 机器人 Token")
    }

    pub fn resolve_worldbooks_dir(&self) -> PathBuf {
        if let Some(raw) = env_non_empty("WORLDBOOKS_DIR") {
            return PathBuf::from(raw);
        }
        self.scenarios_dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.join("worldbooks"))
            .unwrap_or_else(|| PathBuf::from("worldbooks"))
    }

    pub fn require_llm_key(&self) -> Result<&str> {
        self.llm_api_key.as_deref().context(
            "缺少环境变量 LLM_API_KEY，请先在 .env 中填入大模型 API Key（或使用 --mock-llm 进行本地离线试玩）",
        )
    }

    pub fn is_scope_allowed(&self, guild_id: Option<&str>, channel_id: &str) -> bool {
        if !self.allowed_guild_ids.is_empty() {
            match guild_id {
                Some(g) if self.allowed_guild_ids.iter().any(|a| a == g) => {}
                _ => return false,
            }
        }
        self.allowed_channel_ids.is_empty()
            || self.allowed_channel_ids.iter().any(|c| c == channel_id)
    }

    /// When a guild/channel whitelist is configured, Direct Messages are restricted to `ADMIN_USER_IDS`
    /// so random KOOK strangers cannot bypass the whitelist to drain DeepSeek quota.
    pub fn is_dm_allowed(&self, user_id: &str) -> bool {
        if self.allowed_guild_ids.is_empty() && self.allowed_channel_ids.is_empty() {
            return true;
        }
        self.admin_user_ids.iter().any(|a| a == user_id)
    }

    /// Strict fail-closed admin check: only explicitly listed `ADMIN_USER_IDS` are admins.
    pub fn is_admin(&self, user_id: &str) -> bool {
        !user_id.is_empty() && self.admin_user_ids.iter().any(|a| a == user_id)
    }

    pub fn is_gm_or_admin(&self, user_id: &str, room_gm_id: &str) -> bool {
        (!room_gm_id.is_empty() && room_gm_id == user_id)
            || self.admin_user_ids.iter().any(|a| a == user_id)
            || (room_gm_id.is_empty() && self.admin_user_ids.is_empty())
    }

    pub fn estimate_usage_cny(&self, usage: &LlmUsage) -> f64 {
        let hit = usage.cache_hit_tokens;
        let miss = if usage.cache_miss_tokens > 0 {
            usage.cache_miss_tokens
        } else {
            usage.prompt_tokens.saturating_sub(hit)
        };
        let out = usage.completion_tokens;
        (hit as f64 / 1_000_000.0) * self.price_cache_hit_cny_per_m
            + (miss as f64 / 1_000_000.0) * self.price_cache_miss_cny_per_m
            + (out as f64 / 1_000_000.0) * self.price_output_cny_per_m
    }
}

fn env_non_empty(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn env_csv(key: &str) -> Vec<String> {
    env::var(key)
        .ok()
        .map(|s| {
            s.split([',', '，', ';', ' '])
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

fn env_f32(key: &str, default: f32) -> f32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn env_f64(key: &str, default: f64) -> f64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}

fn env_u32(key: &str, default: u32) -> u32 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cfg() -> AppConfig {
        AppConfig {
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
            allowed_guild_ids: vec!["guild_ok".into()],
            allowed_channel_ids: vec![],
            admin_user_ids: vec!["admin_1".into()],
            export_channel_id: None,
            web_bind_addr: None,
            web_public_base_url: None,
            daily_max_llm_calls: 200,
            daily_max_cny: 20.0,
            price_cache_hit_cny_per_m: 0.2,
            price_cache_miss_cny_per_m: 2.0,
            price_output_cny_per_m: 3.0,
            jev: JevConfig {
                api_key: Some("sk-secret-bailian-jev".into()),
                google_endpoint: Some("https://proxy.example/v1/decisions?key=AIzaSecretQuery".into()),
                google_api_key: Some("goog-secret-jev-key".into()),
                google_proxy_secret: Some("proxy-secret-token".into()),
                ..JevConfig::default()
            },
        }
    }

    #[test]
    fn debug_redacts_secrets_and_scope_checks_work() {
        let cfg = sample_cfg();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("super-secret-kook-token"));
        assert!(!rendered.contains("sk-secret-deepseek-key"));
        assert!(!rendered.contains("sk-secret-bailian-jev"));
        assert!(!rendered.contains("goog-secret-jev-key"));
        assert!(!rendered.contains("proxy-secret-token"));
        assert!(!rendered.contains("AIzaSecretQuery"));
        assert!(rendered.contains("***REDACTED***"));

        assert!(cfg.is_scope_allowed(Some("guild_ok"), "ch1"));
        assert!(!cfg.is_scope_allowed(Some("guild_evil"), "ch1"));
        assert!(!cfg.is_scope_allowed(None, "ch1"));
        assert!(cfg.is_gm_or_admin("admin_1", "gm_other"));
        assert!(cfg.is_gm_or_admin("gm_other", "gm_other"));
        assert!(!cfg.is_gm_or_admin("random_player", "gm_other"));
    }
}
