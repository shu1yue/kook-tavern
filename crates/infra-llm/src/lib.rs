pub mod generator;
mod generator_helpers;
pub mod jev;
pub mod jev_questions;
pub mod memory;
pub mod mock;
pub mod parser;
pub mod prompt_ext;
pub mod prompts;
pub mod remote_fetch;
pub mod transport;
mod wire;

use std::sync::Arc;

use anyhow::Result;
use tavern_core::{AppConfig, DecisionGate, LlmProvider, NoopDecisionGate};

pub use generator::generate_scenario_with_llm;
pub use jev::SystemOneJevGate;
pub use mock::MockLlmProvider;
pub use remote_fetch::{fetch_allowlisted_st_bytes, validate_allowlisted_import_url};
pub use transport::OpenAiLlmProvider;

pub fn build_decision_gate(cfg: &AppConfig, use_mock: bool) -> Arc<dyn DecisionGate> {
    if use_mock {
        return Arc::new(NoopDecisionGate);
    }
    match SystemOneJevGate::from_config(&cfg.jev) {
        Some(gate) => {
            tracing::info!(?gate, "Jev System One 潜意识反射层已启用");
            Arc::new(gate)
        }
        None => Arc::new(NoopDecisionGate),
    }
}

pub fn build_llm_provider(
    cfg: &AppConfig,
    use_mock: bool,
    decision_gate: Arc<dyn DecisionGate>,
) -> Result<Arc<dyn LlmProvider>> {
    if use_mock {
        return Ok(Arc::new(MockLlmProvider));
    }
    Ok(Arc::new(
        OpenAiLlmProvider::from_config(cfg)?
            .with_decision_gate(decision_gate, cfg.jev.audit_threshold),
    ))
}
