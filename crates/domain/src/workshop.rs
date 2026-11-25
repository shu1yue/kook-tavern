use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::dnd::CheckSystem;
use crate::models::{Scenario, WorldbookEntry};
use crate::protocol::{CardTheme, LlmUsage};
use crate::world::FactionAgenda;

/// Curated open-source Worldbook & Setting Pack (`worldbooks/*.yaml`) used as the creative foundation
/// in `/store` and `/workshop`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorldbookPack {
    pub id: String,
    pub name: String,
    pub genre: String,
    #[serde(default = "default_ember")]
    pub theme: String,
    #[serde(default)]
    pub recommended_system: CheckSystem,
    #[serde(default = "default_acts")]
    pub recommended_acts: u32,
    #[serde(default = "default_license")]
    pub license: String,
    #[serde(default)]
    pub author: String,
    pub summary: String,
    #[serde(default)]
    pub starter_hooks: Vec<String>,
    #[serde(default)]
    pub factions: Vec<FactionAgenda>,
    #[serde(default)]
    pub entries: Vec<WorldbookEntry>,
}

fn default_ember() -> String {
    "ember".to_string()
}

fn default_acts() -> u32 {
    5
}

fn default_license() -> String {
    "CC0-1.0".to_string()
}

#[derive(Debug, Clone)]
pub struct WorldbookPackSummary {
    pub id: String,
    pub name: String,
    pub genre: String,
    pub recommended_system: CheckSystem,
    pub recommended_acts: u32,
    pub entry_count: usize,
    pub summary: String,
    pub license: String,
}

impl WorldbookPack {
    pub fn summary_view(&self) -> WorldbookPackSummary {
        WorldbookPackSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            genre: self.genre.clone(),
            recommended_system: self.recommended_system,
            recommended_acts: self.recommended_acts,
            entry_count: self.entries.len(),
            summary: self.summary.clone(),
            license: self.license.clone(),
        }
    }
}

/// Per-user guided creation draft persisted in SQLite (`workshop_drafts`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkshopDraft {
    pub user_id: String,
    #[serde(default)]
    pub worldbook_id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub premise: String,
    #[serde(default = "default_acts")]
    pub act_count: u32,
    #[serde(default)]
    pub check_system: CheckSystem,
    #[serde(default = "default_ember")]
    pub theme: String,
    #[serde(default)]
    pub custom_lore: Vec<WorldbookEntry>,
    #[serde(default)]
    pub forged_scenario: Option<Box<Scenario>>,
    #[serde(default)]
    pub forged_summary: String,
    pub updated_at: DateTime<Utc>,
}

impl WorkshopDraft {
    pub fn new(user_id: impl Into<String>) -> Self {
        Self {
            user_id: user_id.into(),
            worldbook_id: None,
            title: String::new(),
            premise: String::new(),
            act_count: 5,
            check_system: CheckSystem::D20,
            theme: "ember".to_string(),
            custom_lore: Vec::new(),
            forged_scenario: None,
            forged_summary: String::new(),
            updated_at: Utc::now(),
        }
    }

    pub fn invalidate_forged(&mut self) {
        self.forged_scenario = None;
        self.forged_summary.clear();
    }

    pub fn length_label(&self) -> &'static str {
        match self.act_count {
            1..=3 => "短篇 3 幕（1晚·约1~2小时）",
            4..=5 => "中篇 5 幕（2~3晚·标准战役）",
            _ => "史诗 6 幕（多晚长团）",
        }
    }
}

/// Interactive button attached to a private user-only card (e.g., `/me` quick skill/levelup buttons).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateButton {
    pub label: String,
    pub value: String,
    pub theme: CardTheme,
}

#[derive(Debug, Clone)]
pub struct ForgeScenarioRequest {
    pub draft: WorkshopDraft,
    pub base_pack: Option<WorldbookPack>,
}

#[derive(Debug, Clone)]
pub enum ForgeScenarioOutput {
    Forged {
        scenario: Box<Scenario>,
        usage: LlmUsage,
    },
    ValidationFailed {
        error: String,
        usage: LlmUsage,
    },
}

pub fn validate_worldbook_pack(pack: &WorldbookPack) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    let id = pack.id.as_str();
    let valid_id = id == id.trim()
        && (2..=48).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
        && id
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !valid_id {
        errors.push(format!("世界书包 id `{id}` 无效"));
    }
    if pack.name.trim().is_empty() || pack.genre.trim().is_empty() || pack.summary.trim().is_empty() {
        errors.push("世界书包缺少名称、题材或简介".into());
    }
    if !matches!(pack.theme.trim().to_ascii_lowercase().as_str(), "" | "ember" | "fog" | "metro") {
        errors.push(format!("世界书包主题 `{}` 无效", pack.theme));
    }
    if !(3..=6).contains(&pack.recommended_acts) {
        errors.push("世界书包 `recommended_acts` 应在 3~6 之间".into());
    }
    for f in &pack.factions {
        if f.name.trim().is_empty() || f.goal.trim().is_empty() {
            errors.push(format!("世界书包 `{id}` 存在缺少名称或目标的势力"));
        }
    }
    if pack.entries.is_empty() {
        errors.push("世界书包至少需要 1 条世界书设定".into());
    }
    let mut ids = HashSet::new();
    for e in &pack.entries {
        if e.id.trim().is_empty() || !ids.insert(e.id.clone()) {
            errors.push(format!("世界书包 `{id}` 条目 id 为空或重复：`{}`", e.id));
        }
        if e.title.trim().is_empty() || e.content.trim().is_empty() {
            errors.push(format!("世界书包 `{id}` 条目 `{}` 缺少标题或正文", e.id));
        }
    }
    if errors.is_empty() { Ok(()) } else { Err(errors) }
}
