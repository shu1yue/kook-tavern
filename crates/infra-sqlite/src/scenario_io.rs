use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use tavern_domain::{Scenario, validate_scenario};

static TMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempFileGuard {
    path: PathBuf,
    armed: bool,
}

impl TempFileGuard {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Safely writes a validated `Scenario` to `<scenarios_dir>/<safe_id>.yaml`:
/// - Re-validates schema and enforces strict ASCII `[a-z0-9_-]` filename defense-in-depth;
/// - Uses unique per-call temp file + `create_new(true)` + RAII cleanup guard;
/// - Rejects overwriting existing `.yaml`/`.yml` atomically (via `hard_link`) unless `overwrite == true`.
pub fn write_scenario_yaml(
    scenarios_dir: &Path,
    scenario: &Scenario,
    overwrite: bool,
) -> Result<PathBuf> {
    if let Err(errs) = validate_scenario(scenario) {
        bail!("剧本未通过 Schema 校验，拒绝写入磁盘：{}", errs.join("；"));
    }
    let id = scenario.id.as_str();
    if id.is_empty()
        || id.contains(['/', '\\', '.'])
        || !id
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-'))
    {
        bail!("剧本 ID `{id}` 包含非法路径字符，拒绝写入");
    }

    fs::create_dir_all(scenarios_dir)
        .with_context(|| format!("创建剧本目录失败: {}", scenarios_dir.display()))?;

    let target = scenarios_dir.join(format!("{id}.yaml"));
    let alt_yml = scenarios_dir.join(format!("{id}.yml"));
    if !overwrite && (target.exists() || alt_yml.exists()) {
        bail!(
            "目标剧本文件已存在（{id}.yaml/.yml），为防止误覆盖请改用 `--id <新ID>` 或显式加 `--overwrite`"
        );
    }

    let yaml = serde_yaml::to_string(scenario).context("序列化剧本 YAML 失败")?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp_path = scenarios_dir.join(format!(".{id}.{}.{nanos}.{seq}.tmp", std::process::id()));
    let mut guard = TempFileGuard::new(tmp_path.clone());

    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .with_context(|| format!("创建临时剧本文件失败: {}", tmp_path.display()))?;
    f.write_all(yaml.as_bytes())
        .with_context(|| format!("写入临时剧本文件失败: {}", tmp_path.display()))?;
    f.sync_all().ok();
    drop(f);

    if overwrite {
        fs::rename(&tmp_path, &target)
            .with_context(|| format!("原子覆盖剧本文件失败: {}", target.display()))?;
        guard.disarm();
    } else {
        // Atomic no-clobber link prevents TOCTOU overwrite races!
        match fs::hard_link(&tmp_path, &target) {
            Ok(()) => {
                // Keep guard armed so `tmp_path` is unlinked on drop!
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                bail!("并发冲突：目标剧本 `{id}.yaml` 已存在，已安全放弃写入");
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("原子落盘剧本文件失败: {}", target.display()));
            }
        }
    }
    Ok(target)
}
