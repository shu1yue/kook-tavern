use anyhow::Result;
use tavern_domain::{Scenario, WorldbookPack, WorkshopDraft};

use crate::ports::BoxFuture;

pub trait StRemoteImporter: Send + Sync {
    fn import_from_url<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<Scenario>>;
}

#[derive(Debug, Default)]
pub struct NoopStImporter;

impl StRemoteImporter for NoopStImporter {
    fn import_from_url<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<Scenario>> {
        Box::pin(async { anyhow::bail!("当前运行模式未启用远程 SillyTavern 导入器") })
    }
}

pub trait WorldbookCatalog: Send + Sync {
    fn list(&self) -> Vec<WorldbookPack>;
    fn get(&self, id: &str) -> Option<WorldbookPack>;
    fn reload(&self) -> Result<usize> {
        Ok(self.list().len())
    }
}

#[derive(Debug, Default)]
pub struct EmptyWorldbookCatalog;

impl WorldbookCatalog for EmptyWorldbookCatalog {
    fn list(&self) -> Vec<WorldbookPack> {
        Vec::new()
    }

    fn get(&self, _id: &str) -> Option<WorldbookPack> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeSlotOutcome {
    Acquired { token: String },
    Cooldown { wait_secs: u64 },
}

pub trait WorkshopStore: Send + Sync {
    fn get_workshop_draft(&self, user_id: &str) -> Result<Option<WorkshopDraft>>;
    fn upsert_workshop_draft(&self, draft: &WorkshopDraft) -> Result<()>;
    fn delete_workshop_draft(&self, user_id: &str) -> Result<bool>;
    fn can_acquire_forge_slot(&self, user_id: &str, cooldown_secs: u64) -> Result<bool>;
    fn try_acquire_forge_slot(&self, user_id: &str, cooldown_secs: u64) -> Result<ForgeSlotOutcome>;
    fn release_forge_slot(&self, user_id: &str, token: &str) -> Result<()>;
}
