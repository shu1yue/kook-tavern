use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, Result, anyhow, bail};
use tavern_core::ScenarioCatalog;
use tavern_domain::{Scenario, validate_scenario};
use tracing::warn;

use crate::scenario_io::write_scenario_yaml;

const MAX_YAML_BYTES: u64 = 2 * 1024 * 1024;
const MAX_COMMUNITY_SCENARIOS: usize = 32;

struct CatalogState {
    scenarios: BTreeMap<String, Scenario>,
    builtin_ids: HashSet<String>,
}

#[derive(Debug, Clone)]
pub struct FsScenarioCatalog {
    dir: PathBuf,
    state: Arc<RwLock<CatalogState>>,
    write_mu: Arc<Mutex<()>>,
}

impl std::fmt::Debug for CatalogState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CatalogState")
            .field("total", &self.scenarios.len())
            .field("builtin", &self.builtin_ids.len())
            .finish()
    }
}

impl FsScenarioCatalog {
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let loaded = scan_all_scenarios(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            state: Arc::new(RwLock::new(loaded)),
            write_mu: Arc::new(Mutex::new(())),
        })
    }

    fn community_dir(&self) -> PathBuf {
        self.dir.join("community")
    }

    fn reload_locked(&self) -> Result<usize> {
        if !self.dir.exists() {
            warn!(dir = %self.dir.display(), "热重载失败：剧本目录不存在");
            bail!("剧本目录不存在或暂时不可用，已保留原线上剧本库");
        }
        let fresh = scan_all_scenarios(&self.dir)?;
        if fresh.scenarios.is_empty() {
            bail!("剧本目录中未找到任何合法 YAML 剧本，已保留原线上剧本库");
        }
        let count = fresh.scenarios.len();
        let mut guard = self
            .state
            .write()
            .map_err(|_| anyhow!("剧本库读写锁异常"))?;
        *guard = fresh;
        Ok(count)
    }
}

fn file_label(path: &Path) -> &str {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.yaml")
}

fn collect_yaml_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = Vec::new();
    if !dir.exists() {
        return Ok(entries);
    }
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
    Ok(entries)
}

fn load_single_yaml(path: &Path) -> Result<Scenario> {
    let fname = file_label(path);
    let mut raw_bytes = Vec::new();
    File::open(path)
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
    Ok(scenario)
}

fn scan_all_scenarios(dir: &Path) -> Result<CatalogState> {
    let mut scenarios = BTreeMap::new();
    let mut builtin_ids = HashSet::new();
    for path in collect_yaml_files(dir)? {
        let scenario = load_single_yaml(&path)?;
        if scenarios.contains_key(&scenario.id) {
            bail!(
                "发现重复的剧本 id `{}`（文件：{}）",
                scenario.id,
                file_label(&path)
            );
        }
        builtin_ids.insert(scenario.id.clone());
        scenarios.insert(scenario.id.clone(), scenario);
    }

    let community_dir = dir.join("community");
    for path in collect_yaml_files(&community_dir)? {
        match load_single_yaml(&path) {
            Ok(scenario) => {
                if builtin_ids.contains(&scenario.id) || scenarios.contains_key(&scenario.id) {
                    warn!(
                        file = %path.display(),
                        id = %scenario.id,
                        "社区剧本 ID 与现有剧本冲突，已跳过加载以保护内置剧本"
                    );
                    continue;
                }
                scenarios.insert(scenario.id.clone(), scenario);
            }
            Err(err) => {
                warn!(file = %path.display(), error = %err, "跳过损坏的社区剧本文件");
            }
        }
    }

    Ok(CatalogState { scenarios, builtin_ids })
}

impl ScenarioCatalog for FsScenarioCatalog {
    /// Returns official built-in scenarios FIRST, followed by community scenarios,
    /// so community uploads can never evict built-in scenarios from KOOK's `/menu` card.
    fn list(&self) -> Vec<Scenario> {
        let Ok(guard) = self.state.read() else {
            return Vec::new();
        };
        let mut builtins = Vec::new();
        let mut community = Vec::new();
        for (id, sc) in &guard.scenarios {
            if guard.builtin_ids.contains(id) {
                builtins.push(sc.clone());
            } else {
                community.push(sc.clone());
            }
        }
        builtins.extend(community);
        builtins
    }

    fn get(&self, id: &str) -> Option<Scenario> {
        self.state
            .read()
            .ok()
            .and_then(|g| g.scenarios.get(id).cloned())
    }

    fn default_scenario(&self) -> Option<Scenario> {
        self.state.read().ok().and_then(|g| {
            g.scenarios
                .get("ember_tavern")
                .or_else(|| g.scenarios.values().next())
                .cloned()
        })
    }

    fn is_builtin(&self, id: &str) -> bool {
        self.state
            .read()
            .map(|g| g.builtin_ids.contains(id))
            .unwrap_or(false)
    }

    fn reload(&self) -> Result<usize> {
        let _lock = self.write_mu.lock().map_err(|_| anyhow!("剧本库写锁异常"))?;
        self.reload_locked()
    }

    fn publish_community(&self, scenario: &Scenario, allow_overwrite: bool) -> Result<String> {
        let _lock = self.write_mu.lock().map_err(|_| anyhow!("剧本库写锁异常"))?;
        if self.is_builtin(&scenario.id) {
            bail!("剧本 ID `{}` 属于官方内置剧本，禁止覆盖！请改用新剧本名重新锻造", scenario.id);
        }
        let community_dir = self.community_dir();
        let existing_count = collect_yaml_files(&community_dir)?.len();
        let already_exists = self.get(&scenario.id).is_some();
        if !already_exists && existing_count >= MAX_COMMUNITY_SCENARIOS {
            bail!("社区工坊剧本库已达上限（{MAX_COMMUNITY_SCENARIOS} 个），请联系管理员清理旧剧本后再上架");
        }
        let written = write_scenario_yaml(&community_dir, scenario, allow_overwrite)?;
        if let Err(err) = self.reload_locked() {
            if !allow_overwrite {
                let _ = fs::remove_file(&written);
            }
            return Err(err);
        }
        if self.get(&scenario.id).is_none() {
            if !allow_overwrite {
                let _ = fs::remove_file(&written);
            }
            bail!("剧本落盘后未能通过热重载校验，已自动清理并取消上架");
        }
        Ok(scenario.id.clone())
    }

    fn delete_community(&self, id: &str) -> Result<bool> {
        let _lock = self.write_mu.lock().map_err(|_| anyhow!("剧本库写锁异常"))?;
        let safe = id.trim();
        if safe.is_empty()
            || safe.contains(['/', '\\', '.'])
            || !safe
                .bytes()
                .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
        {
            bail!("无效的社区剧本 ID：`{safe}`");
        }
        if self.is_builtin(safe) {
            bail!("`{safe}` 是官方内置剧本，不可通过社区指令删除");
        }
        let community_dir = self.community_dir();
        let mut removed = false;
        for ext in ["yaml", "yml"] {
            let p = community_dir.join(format!("{safe}.{ext}"));
            if p.is_file() {
                fs::remove_file(&p).context("删除社区剧本文件失败")?;
                removed = true;
            }
        }
        if removed {
            let _ = self.reload_locked()?;
        }
        Ok(removed)
    }
}
