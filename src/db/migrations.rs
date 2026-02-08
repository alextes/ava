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
