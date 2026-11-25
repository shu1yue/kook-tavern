use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, anyhow, bail};
use tavern_core::ScenarioCatalog;
use tavern_domain::{Scenario, validate_scenario};
use tracing::warn;

const MAX_YAML_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FsScenarioCatalog {
    dir: PathBuf,
    scenarios: Arc<RwLock<BTreeMap<String, Scenario>>>,
}

impl FsScenarioCatalog {
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let loaded = scan_scenarios_dir(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            scenarios: Arc::new(RwLock::new(loaded)),
        })
    }
}

fn file_label(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.yaml")
}

fn scan_scenarios_dir(dir: &Path) -> Result<BTreeMap<String, Scenario>> {
    let mut scenarios = BTreeMap::new();
    if !dir.exists() {
        return Ok(scenarios);
    }

    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir).context("读取剧本目录失败")? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        if matches!(ext.as_deref(), Some("yaml" | "yml")) {
            entries.push(path);
        }
    }
    entries.sort();

    for path in entries {
        let fname = file_label(&path);
        let mut raw_bytes = Vec::new();
        File::open(&path)
            .with_context(|| format!("读取剧本文件失败（{fname}）"))?
            .take(MAX_YAML_BYTES + 1)
            .read_to_end(&mut raw_bytes)
            .with_context(|| format!("读取剧本文件失败（{fname}）"))?;
        if raw_bytes.len() as u64 > MAX_YAML_BYTES {
            warn!(file = %path.display(), "剧本 YAML 超过 2 MiB 限制");
            bail!("剧本文件体积超过 2 MiB 上限（{fname}）");
        }
        let raw = String::from_utf8(raw_bytes)
            .with_context(|| format!("剧本文件非合法 UTF-8（{fname}）"))?;
        let scenario: Scenario = serde_yaml::from_str(&raw).map_err(|err| {
            warn!(file = %path.display(), error = %err, "解析剧本 YAML 失败");
            anyhow!("解析剧本 YAML 失败（{fname}）：{err}")
        })?;
        if let Err(errs) = validate_scenario(&scenario) {
            warn!(file = %path.display(), errors = ?errs, "剧本 Schema 校验失败");
            bail!("剧本 Schema 校验失败（{fname}）：{}", errs.join("；"));
        }
        if scenarios.contains_key(&scenario.id) {
            warn!(file = %path.display(), id = %scenario.id, "重复剧本 ID");
            bail!("发现重复的剧本 id `{}`（文件：{fname}）", scenario.id);
        }
        scenarios.insert(scenario.id.clone(), scenario);
    }

    Ok(scenarios)
}

impl ScenarioCatalog for FsScenarioCatalog {
    fn list(&self) -> Vec<Scenario> {
        self.scenarios
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Option<Scenario> {
        self.scenarios.read().ok().and_then(|g| g.get(id).cloned())
    }

    fn default_scenario(&self) -> Option<Scenario> {
        self.scenarios.read().ok().and_then(|g| {
            g.get("ember_tavern")
                .or_else(|| g.values().next())
                .cloned()
        })
    }

    fn reload(&self) -> Result<usize> {
        if !self.dir.exists() {
            warn!(dir = %self.dir.display(), "热重载失败：剧本目录不存在");
            bail!("剧本目录不存在或暂时不可用，已保留原线上剧本库");
        }
        // Parse & validate ALL YAML files first; refuse empty catalog & only swap if 100% valid!
        let fresh = scan_scenarios_dir(&self.dir)?;
        if fresh.is_empty() {
            bail!("剧本目录中未找到任何合法 YAML 剧本，已保留原线上剧本库");
        }
        let count = fresh.len();
        let mut guard = self
            .scenarios
            .write()
            .map_err(|_| anyhow!("剧本库读写锁异常"))?;
        *guard = fresh;
        Ok(count)
    }
}
