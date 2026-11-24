use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, params};
use tracing::info;

const MIGRATION_0001_INIT: &str = include_str!("../migrations/0001_init.sql");

pub fn run_migrations(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at TEXT NOT NULL
        );
        "#,
    )?;

    apply_if_needed(conn, 1, "0001_init", |tx| {
        tx.execute_batch(MIGRATION_0001_INIT)?;
        Ok(())
    })?;

    apply_if_needed(conn, 2, "0002_m05_state_machine", |tx| {
        add_column_if_missing(
            tx,
            "rooms",
            "state_version",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        add_column_if_missing(
            tx,
            "rooms",
            "turn_phase",
            "TEXT NOT NULL DEFAULT 'collecting'",
        )?;
        add_column_if_missing(
            tx,
            "rooms",
            "narrative_style",
            "TEXT NOT NULL DEFAULT 'prose'",
        )?;
        add_column_if_missing(tx, "rooms", "web_token", "TEXT NOT NULL DEFAULT ''")?;
        add_column_if_missing(tx, "players", "is_afk", "INTEGER NOT NULL DEFAULT 0")?;

        if !has_column(tx, "pending_actions", "target_turn")? {
            tx.execute_batch(
                r#"
                CREATE TABLE pending_actions_v2 (
                    channel_id TEXT NOT NULL,
                    user_id TEXT NOT NULL,
                    target_turn INTEGER NOT NULL DEFAULT 1,
                    character_name TEXT NOT NULL,
                    action_text TEXT NOT NULL DEFAULT '',
                    roll_notes_json TEXT NOT NULL DEFAULT '[]',
                    updated_at TEXT NOT NULL,
                    PRIMARY KEY (channel_id, user_id, target_turn),
                    FOREIGN KEY (channel_id) REFERENCES rooms(channel_id) ON DELETE CASCADE
                );

                INSERT INTO pending_actions_v2 (
                    channel_id, user_id, target_turn, character_name,
                    action_text, roll_notes_json, updated_at
                )
                SELECT
                    p.channel_id,
                    p.user_id,
                    COALESCE(r.turn_number, 0) + 1,
                    p.character_name,
                    p.action_text,
                    p.roll_notes_json,
                    p.updated_at
                FROM pending_actions p
                LEFT JOIN rooms r ON r.channel_id = p.channel_id;

                DROP TABLE pending_actions;
                ALTER TABLE pending_actions_v2 RENAME TO pending_actions;
                CREATE INDEX IF NOT EXISTS idx_pending_channel_turn
                    ON pending_actions (channel_id, target_turn);
                "#,
            )?;
        }
        Ok(())
    })?;

    Ok(())
}

fn apply_if_needed<F>(conn: &mut Connection, version: i64, name: &str, f: F) -> Result<()>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<()>,
{
    let already_applied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        params![version],
        |r| r.get::<_, i64>(0),
    )? != 0;

    if already_applied {
        return Ok(());
    }

    let tx = conn.transaction()?;
    f(&tx)?;
    tx.execute(
        "INSERT INTO schema_migrations (version, name, applied_at) VALUES (?1, ?2, ?3)",
        params![version, name, Utc::now().to_rfc3339()],
    )?;
    tx.commit()?;
    info!(version, name, "已执行 SQLite 数据库迁移");
    Ok(())
}

fn has_column(tx: &rusqlite::Transaction<'_>, table: &str, column: &str) -> Result<bool> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = tx.prepare(&pragma)?;
    let cols = stmt.query_map([], |r| r.get::<_, String>(1))?;
    for c in cols {
        if c? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_column_if_missing(
    tx: &rusqlite::Transaction<'_>,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    if !has_column(tx, table, column)? {
        let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
        tx.execute(&sql, [])?;
    }
    Ok(())
}
