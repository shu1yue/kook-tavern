CREATE TABLE IF NOT EXISTS rooms (
    channel_id TEXT PRIMARY KEY,
    scenario_id TEXT NOT NULL,
    title TEXT NOT NULL,
    location TEXT NOT NULL,
    world_time TEXT NOT NULL,
    turn_number INTEGER NOT NULL DEFAULT 0,
    chat_mode TEXT NOT NULL DEFAULT 'strict',
    clues_json TEXT NOT NULL DEFAULT '[]',
    summary TEXT NOT NULL DEFAULT '',
    summary_upto_turn INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS players (
    channel_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    character_name TEXT NOT NULL,
    role_desc TEXT NOT NULL DEFAULT '',
    traits_json TEXT NOT NULL DEFAULT '[]',
    hp INTEGER NOT NULL,
    max_hp INTEGER NOT NULL,
    san INTEGER NOT NULL,
    max_san INTEGER NOT NULL,
    inventory_json TEXT NOT NULL DEFAULT '[]',
    status_note TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (channel_id, user_id),
    FOREIGN KEY (channel_id) REFERENCES rooms(channel_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS pending_actions (
    channel_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    character_name TEXT NOT NULL,
    action_text TEXT NOT NULL DEFAULT '',
    roll_notes_json TEXT NOT NULL DEFAULT '[]',
    updated_at TEXT NOT NULL,
    PRIMARY KEY (channel_id, user_id),
    FOREIGN KEY (channel_id) REFERENCES rooms(channel_id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    batched_input TEXT NOT NULL,
    scene_title TEXT NOT NULL,
    narrative TEXT NOT NULL,
    state_summary TEXT NOT NULL DEFAULT '',
    location TEXT NOT NULL,
    world_time TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES rooms(channel_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_turns_channel_turn
    ON turns (channel_id, turn_number);

CREATE TABLE IF NOT EXISTS snapshots (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    channel_id TEXT NOT NULL,
    turn_number INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (channel_id) REFERENCES rooms(channel_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_snapshots_channel_id
    ON snapshots (channel_id, id DESC);
