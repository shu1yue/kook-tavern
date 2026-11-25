use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::{Context, Result, anyhow, bail};
use tavern_core::WorldbookCatalog;
use tavern_domain::{WorldbookPack, validate_worldbook_pack};
use tracing::warn;

const MAX_WB_YAML_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct FsWorldbookCatalog {
    dir: PathBuf,
    packs: Arc<RwLock<BTreeMap<String, WorldbookPack>>>,
}

impl FsWorldbookCatalog {
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let loaded = scan_worldbooks_dir(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            packs: Arc::new(RwLock::new(loaded)),
        })
    }
}

fn scan_worldbooks_dir(dir: &Path) -> Result<BTreeMap<String, WorldbookPack>> {
    let mut packs = BTreeMap::new();
    if !dir.exists() {
        return Ok(packs);
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).context("读取世界书商店目录失败")? {
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
        let fname = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("pack.yaml");
        let mut raw_bytes = Vec::new();
        File::open(&path)
            .with_context(|| format!("读取世界书包失败（{fname}）"))?
            .take(MAX_WB_YAML_BYTES + 1)
            .read_to_end(&mut raw_bytes)
            .with_context(|| format!("读取世界书包失败（{fname}）"))?;
        if raw_bytes.len() as u64 > MAX_WB_YAML_BYTES {
            bail!("世界书包体积超过 2 MiB 上限（{fname}）");
        }
        let raw = String::from_utf8(raw_bytes)
            .with_context(|| format!("世界书包非合法 UTF-8（{fname}）"))?;
        let pack: WorldbookPack = serde_yaml::from_str(&raw)
            .map_err(|err| anyhow!("解析世界书包 YAML 失败（{fname}）：{err}"))?;
        if let Err(errs) = validate_worldbook_pack(&pack) {
            bail!("世界书包校验失败（{fname}）：{}", errs.join("；"));
        }
        if packs.contains_key(&pack.id) {
            warn!(file = %path.display(), id = %pack.id, "发现重复的世界书包 ID");
            bail!("发现重复的世界书包 id `{}`（文件：{fname}）", pack.id);
        }
        packs.insert(pack.id.clone(), pack);
    }
    Ok(packs)
}

impl WorldbookCatalog for FsWorldbookCatalog {
    fn list(&self) -> Vec<WorldbookPack> {
        self.packs
            .read()
            .map(|g| g.values().cloned().collect())
            .unwrap_or_default()
    }

    fn get(&self, id: &str) -> Option<WorldbookPack> {
        self.packs.read().ok().and_then(|g| g.get(id).cloned())
    }

    fn reload(&self) -> Result<usize> {
        if !self.dir.exists() {
            return Ok(self.list().len());
        }
        let fresh = scan_worldbooks_dir(&self.dir)?;
        let count = fresh.len();
        let mut guard = self
            .packs
            .write()
            .map_err(|_| anyhow!("世界书商店读写锁异常"))?;
        *guard = fresh;
        Ok(count)
    }
}
