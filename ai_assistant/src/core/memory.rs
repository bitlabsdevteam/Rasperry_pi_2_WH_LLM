use std::path::Path;

use crate::{
    adapters::storage::SqliteStore,
    config::AssistantPaths,
    util::{append_text, now_epoch, sql_escape, token_estimate, truncate},
};

#[derive(Clone, Debug)]
pub struct Message {
    pub role: String,
    pub content: String,
}

impl Message {
    pub fn new(role: &str, content: &str) -> Self {
        Self {
            role: role.to_string(),
            content: content.to_string(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct MemoryRecord {
    pub title: String,
    pub body: String,
    pub tags: String,
}

pub fn record_turn(
    paths: &AssistantPaths,
    store: &SqliteStore,
    session_id: &str,
    role: &str,
    content: &str,
) -> Result<(), String> {
    let now = now_epoch();
    store.exec(&format!(
        "INSERT INTO conversation_turns (session_id, role, content, created_at, token_estimate) VALUES ('{}', '{}', '{}', {}, {});",
        sql_escape(session_id),
        sql_escape(role),
        sql_escape(content),
        now,
        token_estimate(content)
    ))?;
    append_turn_markdown(
        &paths.conversations_dir.join(format!("{session_id}.md")),
        role,
        content,
        now,
    )?;
    Ok(())
}

pub fn recent_turns(
    store: &SqliteStore,
    session_id: &str,
    limit: usize,
) -> Result<Vec<Message>, String> {
    let rows = store.query(&format!(
        "SELECT role, replace(replace(content, char(10), ' '), char(13), ' ') FROM conversation_turns WHERE session_id = '{}' ORDER BY id DESC LIMIT {};",
        sql_escape(session_id),
        limit
    ))?;
    let mut messages = rows
        .into_iter()
        .filter_map(|row| {
            if row.len() < 2 {
                None
            } else {
                Some(Message::new(&row[0], &row[1]))
            }
        })
        .collect::<Vec<_>>();
    messages.reverse();
    Ok(messages)
}

pub fn turn_count(store: &SqliteStore, session_id: &str) -> Result<usize, String> {
    Ok(store
        .scalar(&format!(
            "SELECT COUNT(*) FROM conversation_turns WHERE session_id = '{}';",
            sql_escape(session_id)
        ))?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0))
}

pub fn session_token_estimate(store: &SqliteStore, session_id: &str) -> Result<usize, String> {
    Ok(store
        .scalar(&format!(
            "SELECT COALESCE(SUM(token_estimate), 0) FROM conversation_turns WHERE session_id = '{}';",
            sql_escape(session_id)
        ))?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0))
}

pub fn search_memories(
    store: &SqliteStore,
    query: &str,
    limit: usize,
) -> Result<Vec<MemoryRecord>, String> {
    let escaped = sql_escape(query);
    let rows = store.query(&format!(
        "SELECT
            replace(replace(title, char(10), ' '), char(13), ' '),
            replace(replace(body, char(10), ' '), char(13), ' '),
            replace(replace(tags, char(10), ' '), char(13), ' '),
            score
         FROM memories
         WHERE (expires_at IS NULL OR expires_at > {})
           AND (title LIKE '%{escaped}%' OR body LIKE '%{escaped}%' OR tags LIKE '%{escaped}%')
         ORDER BY created_at DESC
         LIMIT {};",
        now_epoch(),
        limit * 4
    ))?;
    let mut memories = rows
        .into_iter()
        .filter_map(|row| {
            if row.len() < 4 {
                None
            } else {
                Some(MemoryRecord {
                    title: row[0].clone(),
                    body: row[1].clone(),
                    tags: row[2].clone(),
                })
            }
        })
        .collect::<Vec<_>>();
    memories.sort_by(|left, right| {
        let left_score = score_memory(query, &left.title, &left.body, &left.tags);
        let right_score = score_memory(query, &right.title, &right.body, &right.tags);
        right_score.cmp(&left_score)
    });
    memories.truncate(limit);
    Ok(memories)
}

pub fn add_memory(
    store: &SqliteStore,
    kind: &str,
    source: &str,
    title: &str,
    body: &str,
    tags: &str,
    score: f64,
) -> Result<(), String> {
    add_memory_with_expiry(store, kind, source, title, body, tags, score, None)
}

pub fn add_memory_with_expiry(
    store: &SqliteStore,
    kind: &str,
    source: &str,
    title: &str,
    body: &str,
    tags: &str,
    score: f64,
    ttl_days: Option<i64>,
) -> Result<(), String> {
    let expires_at = ttl_days.map(|days| now_epoch() + (days * 24 * 60 * 60));
    store.exec(&format!(
        "INSERT INTO memories (kind, source, title, body, tags, score, created_at, expires_at) VALUES ('{}', '{}', '{}', '{}', '{}', {}, {}, {});",
        sql_escape(kind),
        sql_escape(source),
        sql_escape(title),
        sql_escape(body),
        sql_escape(tags),
        score,
        now_epoch(),
        expires_at
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string())
    ))
}

pub fn cleanup_expired_memories(store: &SqliteStore) -> Result<usize, String> {
    let count = store
        .scalar(&format!(
            "SELECT COUNT(*) FROM memories WHERE expires_at IS NOT NULL AND expires_at <= {};",
            now_epoch()
        ))?
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    store.exec(&format!(
        "DELETE FROM memories WHERE expires_at IS NOT NULL AND expires_at <= {};",
        now_epoch()
    ))?;
    Ok(count)
}

pub fn summarize_session(
    paths: &AssistantPaths,
    store: &SqliteStore,
    session_id: &str,
) -> Result<String, String> {
    let rows = store.query(&format!(
        "SELECT role, replace(replace(content, char(10), ' '), char(13), ' ') FROM conversation_turns WHERE session_id = '{}' ORDER BY id ASC;",
        sql_escape(session_id)
    ))?;
    if rows.is_empty() {
        return Ok(format!(
            "no conversation history for session `{session_id}`"
        ));
    }

    let user_lines = rows
        .iter()
        .filter(|row| row.first().map(|role| role == "user").unwrap_or(false))
        .map(|row| truncate(&row[1], 120))
        .take(3)
        .collect::<Vec<_>>();
    let assistant_lines = rows
        .iter()
        .filter(|row| row.first().map(|role| role == "assistant").unwrap_or(false))
        .map(|row| truncate(&row[1], 120))
        .take(3)
        .collect::<Vec<_>>();

    let summary = format!(
        "# Session Summary\n\nSession: {session_id}\nTurn count: {}\n\n## User Highlights\n{}\n\n## Assistant Highlights\n{}\n",
        rows.len(),
        user_lines
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        assistant_lines
            .iter()
            .map(|line| format!("- {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    add_memory_with_expiry(
        store,
        "summary",
        session_id,
        &format!("Summary for {session_id}"),
        &summary,
        "summary,conversation",
        1.0,
        Some(30),
    )?;
    append_text(
        &paths
            .summaries_dir
            .join(format!("{}-{}.md", session_id, now_epoch())),
        &summary,
    )?;

    Ok(summary)
}

pub fn compact_session(
    paths: &AssistantPaths,
    store: &SqliteStore,
    session_id: &str,
    retain_recent_turns: usize,
) -> Result<String, String> {
    let total_turns = turn_count(store, session_id)?;
    if total_turns <= retain_recent_turns {
        return Ok(format!("no compaction needed for `{session_id}`"));
    }

    let summary = summarize_session(paths, store, session_id)?;
    store.exec(&format!(
        "DELETE FROM conversation_turns WHERE session_id = '{}' AND id NOT IN (
            SELECT id FROM conversation_turns WHERE session_id = '{}' ORDER BY id DESC LIMIT {}
        );",
        sql_escape(session_id),
        sql_escape(session_id),
        retain_recent_turns
    ))?;

    Ok(format!(
        "compacted session `{session_id}` from {total_turns} turns to {retain_recent_turns}. summary length={} chars",
        summary.len()
    ))
}

fn append_turn_markdown(
    path: &Path,
    role: &str,
    content: &str,
    created_at: i64,
) -> Result<(), String> {
    append_text(path, &format!("## {role} @ {created_at}\n\n{content}\n\n"))
}

fn score_memory(query: &str, title: &str, body: &str, tags: &str) -> i64 {
    let haystack = format!(
        "{} {} {}",
        title.to_lowercase(),
        body.to_lowercase(),
        tags.to_lowercase()
    );
    query
        .split_whitespace()
        .map(|term| haystack.matches(&term.to_lowercase()).count() as i64)
        .sum()
}

#[cfg(test)]
mod tests {
    use crate::{adapters::storage::SqliteStore, config::AssistantPaths, util::unique_temp_dir};

    use super::{
        add_memory_with_expiry, cleanup_expired_memories, compact_session, record_turn,
        search_memories, session_token_estimate, summarize_session, turn_count,
    };

    #[test]
    fn memory_engine_summarizes_and_compacts_conversation() {
        let root = unique_temp_dir("assistant-memory-test");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();

        for turn in 0..8 {
            record_turn(
                &paths,
                &store,
                "default",
                "user",
                &format!("user turn {turn}"),
            )
            .unwrap();
            record_turn(
                &paths,
                &store,
                "default",
                "assistant",
                &format!("assistant turn {turn}"),
            )
            .unwrap();
        }

        let summary = summarize_session(&paths, &store, "default").unwrap();
        assert!(summary.contains("Session Summary"));

        let compacted = compact_session(&paths, &store, "default", 4).unwrap();
        assert!(compacted.contains("compacted session"));
        assert_eq!(turn_count(&store, "default").unwrap(), 4);
        assert!(session_token_estimate(&store, "default").unwrap() > 0);

        let memories = search_memories(&store, "Summary", 5).unwrap();
        assert!(!memories.is_empty());
    }

    #[test]
    fn memory_engine_cleans_up_expired_records() {
        let root = unique_temp_dir("assistant-memory-expiry");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();

        add_memory_with_expiry(
            &store,
            "note",
            "test",
            "expired",
            "stale",
            "expired",
            0.5,
            Some(-1),
        )
        .unwrap();

        assert_eq!(cleanup_expired_memories(&store).unwrap(), 1);
    }
}
