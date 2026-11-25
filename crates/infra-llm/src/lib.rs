pub mod generator;
pub mod mock;
pub mod parser;
pub mod prompts;
pub mod transport;

use std::sync::Arc;

use anyhow::Result;
use tavern_core::{AppConfig, LlmProvider};

pub use generator::generate_scenario_with_llm;
pub use mock::MockLlmProvider;
pub use transport::OpenAiLlmProvider;

pub fn build_llm_provider(cfg: &AppConfig, use_mock: bool) -> Result<Arc<dyn LlmProvider>> {
    if use_mock {
        return Ok(Arc::new(MockLlmProvider));
    }
    Ok(Arc::new(OpenAiLlmProvider::from_config(cfg)?))
}
