use std::{
    path::{Path, PathBuf},
    process::Command,
};

use crate::{config::AssistantPaths, util::ensure_dir};

#[derive(Clone, Debug)]
pub struct SqliteStore {
    pub db_path: PathBuf,
}

impl SqliteStore {
    pub fn new(paths: &AssistantPaths) -> Result<Self, String> {
        ensure_dir(&paths.data_dir)?;
        let store = Self {
            db_path: paths.db_path.clone(),
        };
        store.init()?;
        Ok(store)
    }

    pub fn from_path(db_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            ensure_dir(parent)?;
        }
        let store = Self { db_path };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> Result<(), String> {
        let schema = r#"
CREATE TABLE IF NOT EXISTS conversation_turns (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    token_estimate INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    source TEXT NOT NULL,
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    tags TEXT NOT NULL,
    score REAL NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    expires_at INTEGER
);
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    priority INTEGER NOT NULL,
    notes TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    every_minutes INTEGER NOT NULL,
    action TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    last_run INTEGER,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS rag_documents (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    indexed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS telegram_state (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS telegram_pairings (
    code TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    chat_id INTEGER NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    first_name TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS telegram_allowlist (
    user_id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL,
    username TEXT NOT NULL DEFAULT '',
    first_name TEXT NOT NULL DEFAULT '',
    approved_at INTEGER NOT NULL,
    is_owner INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS telegram_onboarding_sessions (
    user_id INTEGER PRIMARY KEY,
    chat_id INTEGER NOT NULL,
    stage TEXT NOT NULL,
    assistant_name TEXT NOT NULL DEFAULT '',
    assistant_style TEXT NOT NULL DEFAULT '',
    user_name TEXT NOT NULL DEFAULT '',
    user_role TEXT NOT NULL DEFAULT '',
    about TEXT NOT NULL DEFAULT '',
    goals TEXT NOT NULL DEFAULT '',
    preferences TEXT NOT NULL DEFAULT '',
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS rag_fts USING fts5(path, title, content);
"#;
        self.exec(schema)
    }

    pub fn exec(&self, sql: &str) -> Result<(), String> {
        let output = Command::new("sqlite3")
            .arg("-cmd")
            .arg(".timeout 5000")
            .arg(&self.db_path)
            .arg(sql)
            .output()
            .map_err(|error| format!("failed to invoke sqlite3: {error}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "sqlite3 exec failed for {}: {}",
                self.db_path.display(),
                String::from_utf8_lossy(&output.stderr)
            ))
        }
    }

    pub fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, String> {
        let output = Command::new("sqlite3")
            .arg("-cmd")
            .arg(".timeout 5000")
            .arg("-tabs")
            .arg("-noheader")
            .arg(&self.db_path)
            .arg(sql)
            .output()
            .map_err(|error| format!("failed to invoke sqlite3: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "sqlite3 query failed for {}: {}",
                self.db_path.display(),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let rows = stdout
            .lines()
            .map(|line| {
                line.split('\t')
                    .map(|item| item.to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        Ok(rows)
    }

    pub fn scalar(&self, sql: &str) -> Result<Option<String>, String> {
        Ok(self
            .query(sql)?
            .into_iter()
            .next()
            .and_then(|row| row.into_iter().next()))
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}
