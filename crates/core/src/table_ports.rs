use anyhow::Result;
use tavern_domain::ManagedTable;

/// Persistence port for isolated bot-managed table channels and manual-resolve mode.
pub trait TableStore: Send + Sync {
    fn update_manual_resolve(&self, channel_id: &str, manual: bool) -> Result<bool>;
    fn register_managed_table(&self, table: &ManagedTable) -> Result<()>;
    fn get_managed_table(&self, channel_id: &str) -> Result<Option<ManagedTable>>;
    fn list_guild_managed_tables(&self, guild_id: &str) -> Result<Vec<ManagedTable>>;
    fn delete_managed_table(&self, channel_id: &str) -> Result<bool>;
    /// Returns remaining cooldown seconds (>0 means user must wait before creating another table).
    fn user_table_cooldown_remaining(&self, user_id: &str, cooldown_secs: u64) -> Result<u64>;
}
