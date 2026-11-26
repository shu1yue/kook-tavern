use serde::{Deserialize, Serialize};

/// Result of a fast System One OOC (out-of-character) chat gate evaluation.
#[derive(Debug, Clone, PartialEq)]
pub struct OocVerdict {
    pub prob_ooc: f64,
    pub provider: &'static str,
    pub latency_ms: f64,
}

/// Result of a fast System One semantic audit over a generated DM reply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReplyAuditVerdict {
    pub prob_missed_vitals: f64,
    pub prob_missed_item: f64,
    pub prob_whisper_leak: f64,
    pub prob_cryptic_riddle: f64,
    pub prob_plot_stall: f64,
    pub provider: &'static str,
    pub latency_ms: f64,
}

/// Compact candidate summary passed to System One semantic lore recall.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LoreCandidate {
    pub id: String,
    pub title: String,
    pub snippet: String,
}
