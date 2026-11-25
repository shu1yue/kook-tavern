use anyhow::Result;
use chrono::Utc;
use rusqlite::{Connection, params};
use tracing::info;

const MIGRATION_0001_INIT: &str = include_str!("../migrations/0001_init.sql");

pub fn run_migrations(conn: &mut Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA busy_timeout = 5000;
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
        add_column_if_missing(tx, "rooms", "state_version", "INTEGER NOT NULL DEFAULT 1")?;
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

    apply_if_needed(conn, 3, "0003_m1_usage_whispers", |tx| {
        add_column_if_missing(tx, "rooms", "gm_user_id", "TEXT NOT NULL DEFAULT ''")?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS usage_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id TEXT NOT NULL,
                day_utc TEXT NOT NULL,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
                cache_miss_tokens INTEGER NOT NULL DEFAULT 0,
                estimated_cny REAL NOT NULL DEFAULT 0.0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_usage_logs_day
                ON usage_logs (day_utc);
            CREATE INDEX IF NOT EXISTS idx_usage_logs_channel
                ON usage_logs (channel_id, id DESC);

            CREATE TABLE IF NOT EXISTS whispers (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                channel_id TEXT NOT NULL,
                turn_number INTEGER NOT NULL,
                user_id TEXT NOT NULL,
                player_name TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY (channel_id) REFERENCES rooms(channel_id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_whispers_channel_user
                ON whispers (channel_id, user_id, id DESC);
            "#,
        )?;
        Ok(())
    })?;

    apply_if_needed(conn, 4, "0004_m1_room_guild_id", |tx| {
        add_column_if_missing(tx, "rooms", "guild_id", "TEXT NOT NULL DEFAULT ''")?;
        Ok(())
    })?;

    apply_if_needed(conn, 5, "0005_m1_ux_suggested_actions", |tx| {
        add_column_if_missing(
            tx,
            "rooms",
            "suggested_actions_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        Ok(())
    })?;

    apply_if_needed(conn, 6, "0006_m1_npc_states", |tx| {
        add_column_if_missing(tx, "rooms", "npc_states_json", "TEXT NOT NULL DEFAULT '[]'")?;
        Ok(())
    })?;

    apply_if_needed(conn, 7, "0007_m3_stats_acts_endings", |tx| {
        add_column_if_missing(tx, "players", "stats_json", "TEXT NOT NULL DEFAULT '{}'")?;
        add_column_if_missing(tx, "rooms", "current_act", "INTEGER NOT NULL DEFAULT 1")?;
        add_column_if_missing(
            tx,
            "rooms",
            "world_flags_json",
            "TEXT NOT NULL DEFAULT '[]'",
        )?;
        add_column_if_missing(tx, "rooms", "ended_id", "TEXT NOT NULL DEFAULT ''")?;
        add_column_if_missing(tx, "rooms", "ended_title", "TEXT NOT NULL DEFAULT ''")?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS ending_unlocks (
                guild_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                ending_id TEXT NOT NULL,
                ending_title TEXT NOT NULL,
                ending_kind TEXT NOT NULL DEFAULT 'normal',
                turn_count INTEGER NOT NULL DEFAULT 0,
                party_names TEXT NOT NULL DEFAULT '',
                unlocked_at TEXT NOT NULL,
                PRIMARY KEY (guild_id, scenario_id, ending_id)
            );
            CREATE INDEX IF NOT EXISTS idx_ending_unlocks_guild
                ON ending_unlocks (guild_id, unlocked_at DESC);
            "#,
        )?;
        Ok(())
    })?;

    apply_if_needed(conn, 8, "0008_v12_world_map_echoes", |tx| {
        add_column_if_missing(tx, "rooms", "bulletins_json", "TEXT NOT NULL DEFAULT '[]'")?;
        add_column_if_missing(tx, "players", "location", "TEXT NOT NULL DEFAULT ''")?;
        tx.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS world_echoes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                guild_id TEXT NOT NULL,
                scenario_id TEXT NOT NULL,
                ending_id TEXT NOT NULL,
                ending_title TEXT NOT NULL,
                ending_kind TEXT NOT NULL DEFAULT 'normal',
                party_names TEXT NOT NULL DEFAULT '',
                echo_text TEXT NOT NULL,
                turn_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_world_echoes_scope
                ON world_echoes (guild_id, scenario_id, id DESC);
            "#,
        )?;
        Ok(())
    })?;

    apply_if_needed(conn, 9, "0009_v12_echo_source_channel", |tx| {
        add_column_if_missing(tx, "world_echoes", "source_channel_id", "TEXT NOT NULL DEFAULT ''")?;
        add_column_if_missing(tx, "world_echoes", "source_turn", "INTEGER NOT NULL DEFAULT 0")?;
        tx.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_world_echoes_source ON world_echoes (source_channel_id, source_turn);",
        )?;
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
