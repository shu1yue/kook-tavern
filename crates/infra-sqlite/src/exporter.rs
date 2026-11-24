use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use tavern_core::ExportWriter;

#[derive(Debug, Clone)]
pub struct FsExportWriter {
    exports_dir: PathBuf,
}

impl FsExportWriter {
    pub fn new(exports_dir: PathBuf) -> Self {
        Self { exports_dir }
    }
}

impl ExportWriter for FsExportWriter {
    fn write_markdown(&self, safe_filename: &str, content: &str) -> Result<String> {
        if safe_filename.is_empty()
            || safe_filename.contains('/')
            || safe_filename.contains('\\')
            || safe_filename.contains("..")
        {
            bail!("非法导出文件名");
        }
        fs::create_dir_all(&self.exports_dir)
            .with_context(|| format!("创建导出目录失败: {}", self.exports_dir.display()))?;
        let path = self.exports_dir.join(safe_filename);
        fs::write(&path, content).with_context(|| format!("写入战报失败: {}", path.display()))?;
        Ok(safe_filename.to_string())
    }
}
