use rusqlite::Connection;

use crate::error::Error;

const MIGRATIONS: &[&str] = &[
    // v1: initial schema
    r#"
    CREATE TABLE IF NOT EXISTS sessions (
        id INTEGER PRIMARY KEY,
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now')),
        title TEXT,
        model TEXT
    );

    CREATE TABLE IF NOT EXISTS messages (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        role TEXT NOT NULL CHECK (role IN ('user', 'assistant', 'system')),
        content TEXT NOT NULL,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
    "#,
    // v2: facts table
    r#"
    CREATE TABLE IF NOT EXISTS facts (
        id INTEGER PRIMARY KEY,
        category TEXT NOT NULL,
        key TEXT NOT NULL,
        value TEXT NOT NULL,
        source TEXT NOT NULL DEFAULT 'agent',
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now')),
        UNIQUE(category, key)
    );

    CREATE INDEX IF NOT EXISTS idx_facts_category ON facts(category);
    CREATE INDEX IF NOT EXISTS idx_facts_updated ON facts(updated_at DESC);
    "#,
    // v3: approval rules table
    r#"
    CREATE TABLE IF NOT EXISTS approval_rules (
        id INTEGER PRIMARY KEY,
        pattern TEXT NOT NULL UNIQUE,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    "#,
    // v4: activate sessions + messages tables, add channel column
    r#"
    ALTER TABLE sessions ADD COLUMN active INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE messages ADD COLUMN channel TEXT;
    INSERT INTO sessions (active, title) VALUES (1, 'default');
    "#,
    // v5: summary column for context compaction
    r#"
    ALTER TABLE sessions ADD COLUMN summary TEXT;
    "#,
    // v6: unified memories table (replaces facts)
    r#"
    BEGIN;

    CREATE TABLE IF NOT EXISTS memories (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL CHECK (kind IN ('fact', 'episode', 'character')),
        content TEXT NOT NULL,
        category TEXT,
        key TEXT,
        source TEXT NOT NULL DEFAULT 'agent',
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );

    CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_fact_unique
        ON memories(category, key) WHERE kind = 'fact';
    CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_character_unique
        ON memories(key) WHERE kind = 'character';
    CREATE INDEX IF NOT EXISTS idx_memories_kind ON memories(kind);
    CREATE INDEX IF NOT EXISTS idx_memories_updated ON memories(updated_at DESC);

    CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
        content, content=memories, content_rowid=id, tokenize='porter unicode61'
    );

    CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
        INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
    END;
    CREATE TRIGGER IF NOT EXISTS memories_fts_au AFTER UPDATE ON memories BEGIN
        INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
        INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
    END;
    CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
        INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
    END;

    INSERT INTO memories (kind, content, category, key, source, created_at, updated_at)
        SELECT 'fact', value, category, key, source, created_at, updated_at FROM facts;

    INSERT INTO memories_fts(rowid, content) SELECT id, content FROM memories;

    DROP TABLE IF EXISTS facts;

    COMMIT;
    "#,
    // v7: schedules table for cron tool
    r#"
    CREATE TABLE IF NOT EXISTS schedules (
        id INTEGER PRIMARY KEY,
        description TEXT NOT NULL,
        prompt TEXT NOT NULL,
        cron_expr TEXT,
        next_run_at TEXT NOT NULL,
        last_run_at TEXT,
        active INTEGER NOT NULL DEFAULT 1,
        created_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    CREATE INDEX IF NOT EXISTS idx_schedules_active_next
        ON schedules(next_run_at) WHERE active = 1;
    "#,
    // v8: tasks table for agent scratchpad
    r#"
    CREATE TABLE IF NOT EXISTS tasks (
        id INTEGER PRIMARY KEY,
        title TEXT NOT NULL,
        detail TEXT,
        status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done')),
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        completed_at TEXT
    );
    CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
    "#,
    // v9: context usage tracking on sessions
    r#"
    ALTER TABLE sessions ADD COLUMN last_input_tokens INTEGER;
    ALTER TABLE sessions ADD COLUMN last_context_window INTEGER;
    "#,
    // v10: time-limited approval rules
    r#"
    ALTER TABLE approval_rules ADD COLUMN expires_at TEXT;
    "#,
    // v11: allowed users and chats whitelists
    r#"
    CREATE TABLE IF NOT EXISTS allowed_users (
        user_id INTEGER PRIMARY KEY,
        added_at TEXT NOT NULL DEFAULT (datetime('now')),
        added_by TEXT
    );

    CREATE TABLE IF NOT EXISTS allowed_chats (
        chat_id INTEGER PRIMARY KEY,
        added_at TEXT NOT NULL DEFAULT (datetime('now')),
        added_by TEXT
    );
    "#,
    // v12: channels registry
    r#"
    CREATE TABLE IF NOT EXISTS channels (
        chat_id INTEGER PRIMARY KEY,
        chat_type TEXT NOT NULL,
        title TEXT,
        added_at TEXT NOT NULL DEFAULT (datetime('now')),
        last_seen_at TEXT NOT NULL DEFAULT (datetime('now'))
    );
    "#,
    // v13: rename memory kind 'character' → 'identity'
    r#"
    BEGIN;

    -- drop FTS infrastructure before dropping the memories table
    DROP TRIGGER IF EXISTS memories_fts_ai;
    DROP TRIGGER IF EXISTS memories_fts_au;
    DROP TRIGGER IF EXISTS memories_fts_ad;
    DROP TABLE IF EXISTS memories_fts;

    CREATE TABLE memories_new (
        id INTEGER PRIMARY KEY,
        kind TEXT NOT NULL CHECK (kind IN ('fact', 'episode', 'identity')),
        content TEXT NOT NULL,
        category TEXT,
        key TEXT,
        source TEXT NOT NULL DEFAULT 'agent',
        created_at TEXT NOT NULL DEFAULT (datetime('now')),
        updated_at TEXT NOT NULL DEFAULT (datetime('now'))
    );

    INSERT INTO memories_new (id, kind, content, category, key, source, created_at, updated_at)
        SELECT id,
               CASE WHEN kind = 'character' THEN 'identity' ELSE kind END,
               content, category, key, source, created_at, updated_at
        FROM memories;

    DROP TABLE memories;
    ALTER TABLE memories_new RENAME TO memories;

    CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_fact_unique
        ON memories(category, key) WHERE kind = 'fact';
    CREATE UNIQUE INDEX IF NOT EXISTS idx_memories_identity_unique
        ON memories(key) WHERE kind = 'identity';
    CREATE INDEX IF NOT EXISTS idx_memories_kind ON memories(kind);
    CREATE INDEX IF NOT EXISTS idx_memories_updated ON memories(updated_at DESC);

    CREATE VIRTUAL TABLE IF NOT EXISTS memories_fts USING fts5(
        content, content=memories, content_rowid=id, tokenize='porter unicode61'
    );

    CREATE TRIGGER IF NOT EXISTS memories_fts_ai AFTER INSERT ON memories BEGIN
        INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
    END;
    CREATE TRIGGER IF NOT EXISTS memories_fts_au AFTER UPDATE ON memories BEGIN
        INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
        INSERT INTO memories_fts(rowid, content) VALUES (new.id, new.content);
    END;
    CREATE TRIGGER IF NOT EXISTS memories_fts_ad AFTER DELETE ON memories BEGIN
        INSERT INTO memories_fts(memories_fts, rowid, content) VALUES('delete', old.id, old.content);
    END;

    INSERT INTO memories_fts(rowid, content) SELECT id, content FROM memories;

    COMMIT;
    "#,
    // v14: compaction cursor — skip messages already summarized
    r#"
    ALTER TABLE sessions ADD COLUMN compacted_before_id INTEGER;
    "#,
    // v15: track when the last provider completion happened, so the cold-resume
    // prompt can detect when the prompt cache has expired between turns.
    // stored as unix epoch seconds, nullable until first completion.
    r#"
    ALTER TABLE sessions ADD COLUMN last_completion_at INTEGER;
    "#,
];

pub fn migrate(conn: &Connection) -> Result<(), Error> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (version INTEGER PRIMARY KEY)",
        [],
    )?;

    let current: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    for (i, migration) in MIGRATIONS.iter().enumerate() {
        let version = (i + 1) as i32;
        if version > current {
            conn.execute_batch(migration)?;
            conn.execute("INSERT INTO schema_version (version) VALUES (?)", [version])?;
        }
    }

    Ok(())
}

#[allow(dead_code)]
pub fn schema_version(conn: &Connection) -> Result<i32, Error> {
    let version = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    Ok(version)
}
