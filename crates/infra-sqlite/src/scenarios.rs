use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use tavern_core::ScenarioCatalog;
use tavern_domain::Scenario;

#[derive(Debug, Clone)]
pub struct FsScenarioCatalog {
    scenarios: BTreeMap<String, Scenario>,
}

impl FsScenarioCatalog {
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut scenarios = BTreeMap::new();
        if !dir.exists() {
            return Ok(Self { scenarios });
        }

        for entry in
            fs::read_dir(dir).with_context(|| format!("读取剧本目录失败: {}", dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase());
            if !matches!(ext.as_deref(), Some("yaml" | "yml")) {
                continue;
            }

            let raw = fs::read_to_string(&path)
                .with_context(|| format!("读取剧本文件失败: {}", path.display()))?;
            let scenario: Scenario = serde_yaml::from_str(&raw)
                .with_context(|| format!("解析剧本 YAML 失败: {}", path.display()))?;
            scenarios.insert(scenario.id.clone(), scenario);
        }

        Ok(Self { scenarios })
    }
}

impl ScenarioCatalog for FsScenarioCatalog {
    fn list(&self) -> Vec<Scenario> {
        self.scenarios.values().cloned().collect()
    }

    fn get(&self, id: &str) -> Option<Scenario> {
        self.scenarios.get(id).cloned()
    }

    fn default_scenario(&self) -> Option<Scenario> {
        self.scenarios
            .get("ember_tavern")
            .or_else(|| self.scenarios.values().next())
            .cloned()
    }
}
