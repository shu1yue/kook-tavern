use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};
use tavern_core::{ForgeSlotOutcome, WorkshopStore};
use tavern_domain::WorkshopDraft;

use crate::repo::SqliteRoomStore;

fn cooldown_remaining(last_forge_at: Option<&str>, cooldown_secs: u64, now: DateTime<Utc>) -> u64 {
    let Some(ts) = last_forge_at.map(str::trim).filter(|s| !s.is_empty()) else {
        return 0;
    };
    let Ok(prev) = DateTime::parse_from_rfc3339(ts) else {
        return 0;
    };
    let elapsed = now
        .signed_duration_since(prev.with_timezone(&Utc))
        .num_seconds()
        .max(0) as u64;
    cooldown_secs.saturating_sub(elapsed)
}

impl WorkshopStore for SqliteRoomStore {
    fn get_workshop_draft(&self, user_id: &str) -> Result<Option<WorkshopDraft>> {
        self.with_conn(|conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT draft_json FROM workshop_drafts WHERE user_id = ?1",
                    params![user_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(json) = raw else {
                return Ok(None);
            };
            Ok(serde_json::from_str(&json).ok())
        })
    }

    fn upsert_workshop_draft(&self, draft: &WorkshopDraft) -> Result<()> {
        let json = serde_json::to_string(draft)?;
        let now = Utc::now().to_rfc3339();
        self.with_conn(|conn| {
            conn.execute(
                r#"
                INSERT INTO workshop_drafts (user_id, draft_json, last_forge_at, updated_at)
                VALUES (?1, ?2, '', ?3)
                ON CONFLICT(user_id) DO UPDATE SET
                    draft_json = excluded.draft_json,
                    updated_at = excluded.updated_at
                "#,
                params![draft.user_id, json, now],
            )?;
            Ok(())
        })
    }

    fn delete_workshop_draft(&self, user_id: &str) -> Result<bool> {
        self.with_conn(|conn| {
            let existing: Option<String> = conn
                .query_row(
                    "SELECT last_forge_at FROM workshop_drafts WHERE user_id = ?1",
                    params![user_id],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(last_forge) = existing else {
                return Ok(false);
            };
            // Preserve `last_forge_at` so resetting a draft cannot bypass AI Forge cooldown!
            let empty = WorkshopDraft::new(user_id);
            let json = serde_json::to_string(&empty)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE workshop_drafts SET draft_json = ?2, last_forge_at = ?3, updated_at = ?4 WHERE user_id = ?1",
                params![user_id, json, last_forge, now],
            )?;
            Ok(true)
        })
    }

    fn can_acquire_forge_slot(&self, user_id: &str, cooldown_secs: u64) -> Result<bool> {
        self.with_conn(|conn| {
            let last: Option<String> = conn
                .query_row(
                    "SELECT last_forge_at FROM workshop_drafts WHERE user_id = ?1",
                    params![user_id],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(cooldown_remaining(last.as_deref(), cooldown_secs, Utc::now()) == 0)
        })
    }

    fn try_acquire_forge_slot(&self, user_id: &str, cooldown_secs: u64) -> Result<ForgeSlotOutcome> {
        self.with_conn(|conn| {
            let tx = conn.transaction()?;
            let now = Utc::now();
            let last: Option<String> = tx
                .query_row(
                    "SELECT last_forge_at FROM workshop_drafts WHERE user_id = ?1",
                    params![user_id],
                    |r| r.get(0),
                )
                .optional()?;
            let wait_secs = cooldown_remaining(last.as_deref(), cooldown_secs, now);
            if wait_secs > 0 {
                return Ok(ForgeSlotOutcome::Cooldown { wait_secs });
            }
            let now_s = now.to_rfc3339();
            let fallback_draft = serde_json::to_string(&WorkshopDraft::new(user_id))?;
            tx.execute(
                r#"
                INSERT INTO workshop_drafts (user_id, draft_json, last_forge_at, updated_at)
                VALUES (?1, ?2, ?3, ?3)
                ON CONFLICT(user_id) DO UPDATE SET
                    last_forge_at = excluded.last_forge_at,
                    updated_at = excluded.updated_at
                "#,
                params![user_id, fallback_draft, now_s],
            )?;
            tx.commit()?;
            Ok(ForgeSlotOutcome::Acquired { token: now_s })
        })
    }

    fn release_forge_slot(&self, user_id: &str, token: &str) -> Result<()> {
        self.with_conn(|conn| {
            // CAS release: only clears cooldown if no newer forge overwrote `last_forge_at`.
            conn.execute(
                "UPDATE workshop_drafts SET last_forge_at = '' WHERE user_id = ?1 AND last_forge_at = ?2",
                params![user_id, token],
            )?;
            Ok(())
        })
    }
}
