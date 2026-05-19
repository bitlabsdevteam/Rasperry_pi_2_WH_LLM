use std::{thread, time::Duration};

use crate::{
    adapters::storage::SqliteStore,
    config::{AppConfig, AssistantPaths, MessageQueueConfig},
    core::{
        service::run_chat_session_with_session,
        session::{
            AssistantState, Session, fetch_session, set_session_state, touch_session,
            upsert_session,
        },
    },
    util::{now_epoch_ms, sql_escape, truncate},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InboundRequest {
    pub session: Session,
    pub message_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnqueueResult {
    pub item_id: i64,
    pub merged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct QueueItem {
    id: i64,
    surface: String,
    session_id: String,
    peer_id: Option<String>,
    chat_id: Option<String>,
    message_text: String,
    status: String,
    created_at: i64,
    available_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    merged_count: usize,
    summary_text: String,
    error_text: String,
    response_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DrainBatch {
    primary_id: i64,
    surface: String,
    session_id: String,
    peer_id: Option<String>,
    chat_id: Option<String>,
    prompt: String,
    item_ids: Vec<i64>,
}

pub fn enqueue_request(
    store: &SqliteStore,
    config: &MessageQueueConfig,
    request: &InboundRequest,
) -> Result<EnqueueResult, String> {
    let now = now_epoch_ms();
    let trimmed = request.message_text.trim();
    if trimmed.is_empty() {
        return Err("cannot enqueue an empty message".to_string());
    }
    upsert_session(store, &request.session)?;
    touch_session(store, &request.session.session_id, AssistantState::Queued)?;

    if let Some(existing) = find_merge_candidate(store, request, config, now)? {
        let merged_text = format!("{}\n{}", existing.message_text.trim_end(), trimmed);
        let merged_count = existing.merged_count + 1;
        let available_at = now + debounce_ms(config, &request.session.surface) as i64;
        store.exec(&format!(
            "UPDATE inbound_queue
             SET message_text = '{}',
                 available_at = {},
                 merged_count = {},
                 peer_id = {},
                 chat_id = {},
                 created_at = {}
             WHERE id = {};",
            sql_escape(&merged_text),
            available_at,
            merged_count,
            optional_sql_text(&request.session.peer_id),
            optional_sql_text(&request.session.chat_id),
            now,
            existing.id
        ))?;
        enforce_session_cap(store, config, &request.session.session_id)?;
        return Ok(EnqueueResult {
            item_id: existing.id,
            merged: true,
        });
    }

    let has_existing_session_activity = store
        .scalar(&format!(
            "SELECT COUNT(*) FROM inbound_queue
             WHERE session_id = '{}' AND status IN ('queued', 'running');",
            sql_escape(&request.session.session_id)
        ))?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0)
        > 0;
    let available_at = if has_existing_session_activity {
        now + debounce_ms(config, &request.session.surface) as i64
    } else {
        now
    };
    store.exec(&format!(
        "INSERT INTO inbound_queue
         (surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at, merged_count, summary_text, error_text, response_text)
         VALUES ('{}', '{}', {}, {}, '{}', 'queued', {}, {}, 1, '', '', '');",
        sql_escape(&request.session.surface),
        sql_escape(&request.session.session_id),
        optional_sql_text(&request.session.peer_id),
        optional_sql_text(&request.session.chat_id),
        sql_escape(trimmed),
        now,
        available_at
    ))?;
    let item_id = store
        .scalar(&format!(
            "SELECT id FROM inbound_queue
             WHERE session_id = '{}' AND status = 'queued'
             ORDER BY created_at DESC, id DESC
             LIMIT 1;",
            sql_escape(&request.session.session_id)
        ))?
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(|| "failed to read queued item id".to_string())?;
    enforce_session_cap(store, config, &request.session.session_id)?;
    Ok(EnqueueResult {
        item_id,
        merged: false,
    })
}

pub fn recover_stale_leases(store: &SqliteStore, config: &MessageQueueConfig) -> Result<usize, String> {
    let now = now_epoch_ms();
    store.exec(&format!(
        "DELETE FROM inbound_queue_leases WHERE expires_at <= {};",
        now
    ))?;
    let stale = list_running_items(store, now - config.lease_timeout_ms as i64)?;
    if stale.is_empty() {
        return Ok(0);
    }

    for item in &stale {
        store.exec(&format!(
            "UPDATE inbound_queue
             SET status = 'queued',
                 started_at = NULL,
                 available_at = {}
             WHERE id = {};",
            now,
            item.id
        ))?;
        let _ = set_session_state(store, &item.session_id, AssistantState::Queued);
        release_leases(store, item.id, &item.session_id)?;
    }
    Ok(stale.len())
}

pub fn dispatch_due_once(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
) -> Result<Vec<String>, String> {
    if !config.messages.queue.enabled {
        return Ok(Vec::new());
    }

    let mut logs = Vec::new();
    let recovered = recover_stale_leases(store, &config.messages.queue)?;
    if recovered > 0 {
        logs.push(format!("recovered {recovered} stale inbound queue item(s)"));
    }

    let active = global_running_count(store)?;
    if active >= config.messages.queue.global_max_concurrency.max(1) {
        return Ok(logs);
    }

    let Some(batch) = lease_next_batch(store, &config.messages.queue)? else {
        return Ok(logs);
    };

    let session = fetch_session(store, &batch.session_id)?
        .map(|summary| summary.session)
        .unwrap_or_else(|| {
            if batch.surface == "voice" {
                Session::voice(&batch.session_id, config)
            } else {
                Session::cli(&batch.session_id, config)
            }
        });
    let result = run_chat_session_with_session(paths, config, store, &session, &batch.prompt, false);
    match result {
        Ok(output) => {
            complete_batch(store, &batch, "done", "", &output.response)?;
            if batch.surface == "telegram" {
                if let Some(chat_id) = batch.chat_id.as_deref().and_then(|value| value.parse::<i64>().ok()) {
                    match crate::core::telegram::send_reply(
                        paths,
                        &config.telegram,
                        chat_id,
                        &output.response,
                        config.messages.reply.telegram_chunk_chars,
                    ) {
                        Ok(()) => logs.push(format!(
                            "delivered queued telegram reply for {}",
                            batch.session_id
                        )),
                        Err(error) => logs.push(format!(
                            "queued telegram delivery failed for {}: {}",
                            batch.session_id, error
                        )),
                    }
                }
            } else {
                logs.push(format!("completed queued voice reply for {}", batch.session_id));
            }
        }
        Err(error) => {
            complete_batch(store, &batch, "failed", &error, "")?;
            if batch.surface == "telegram" {
                if let Some(chat_id) = batch.chat_id.as_deref().and_then(|value| value.parse::<i64>().ok()) {
                    let _ = crate::core::telegram::send_reply(
                        paths,
                        &config.telegram,
                        chat_id,
                        "Local assistant failed to generate a reply. Please try again.",
                        config.messages.reply.telegram_chunk_chars,
                    );
                }
            }
            logs.push(format!(
                "failed queued {} reply for {}: {}",
                batch.surface, batch.session_id, error
            ));
        }
    }

    Ok(logs)
}

pub fn wait_for_response(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    item_id: i64,
    timeout_ms: usize,
) -> Result<String, String> {
    let deadline = now_epoch_ms() + timeout_ms as i64;
    loop {
        if let Some(item) = fetch_item(store, item_id)? {
            match item.status.as_str() {
                "done" => return Ok(item.response_text),
                "failed" => {
                    return Err(if item.error_text.is_empty() {
                        "queued response failed".to_string()
                    } else {
                        item.error_text
                    })
                }
                "dropped" => {
                    return Err("queued response was dropped before execution".to_string())
                }
                _ => {}
            }
        }

        if now_epoch_ms() >= deadline {
            return Err("timed out waiting for queued response".to_string());
        }

        let _ = dispatch_due_once(paths, store, config)?;
        thread::sleep(Duration::from_millis(50));
    }
}

pub fn queue_depth(store: &SqliteStore, session_id: &str, status: &str) -> Result<usize, String> {
    Ok(store
        .scalar(&format!(
            "SELECT COUNT(*) FROM inbound_queue
             WHERE session_id = '{}' AND status = '{}';",
            sql_escape(session_id),
            sql_escape(status)
        ))?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0))
}

fn debounce_ms(config: &MessageQueueConfig, surface: &str) -> usize {
    match surface {
        "voice" => config.voice_debounce_ms.max(1),
        _ => config.telegram_debounce_ms.max(1),
    }
}

fn find_merge_candidate(
    store: &SqliteStore,
    request: &InboundRequest,
    config: &MessageQueueConfig,
    now: i64,
) -> Result<Option<QueueItem>, String> {
    let Some(item) = store
        .query(&format!(
            "SELECT id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at,
                    started_at, finished_at, merged_count, COALESCE(summary_text, ''), COALESCE(error_text, ''),
                    COALESCE(response_text, '')
             FROM inbound_queue
             WHERE session_id = '{}' AND status = 'queued'
             ORDER BY created_at DESC
             LIMIT 1;",
            sql_escape(&request.session.session_id)
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_item(&row))
    else {
        return Ok(None);
    };

    if item.surface != request.session.surface {
        return Ok(None);
    }

    let window_close = item.created_at + debounce_ms(config, &request.session.surface) as i64;
    if now <= window_close {
        Ok(Some(item))
    } else {
        Ok(None)
    }
}

fn enforce_session_cap(
    store: &SqliteStore,
    config: &MessageQueueConfig,
    session_id: &str,
) -> Result<(), String> {
    let cap = config.per_session_cap.max(1);
    let items = store
        .query(&format!(
            "SELECT id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at,
                    started_at, finished_at, merged_count, COALESCE(summary_text, ''), COALESCE(error_text, ''),
                    COALESCE(response_text, '')
             FROM inbound_queue
             WHERE session_id = '{}' AND status = 'queued'
             ORDER BY created_at ASC;",
            sql_escape(session_id)
        ))?
        .into_iter()
        .filter_map(|row| parse_item(&row))
        .collect::<Vec<_>>();
    if items.len() <= cap || config.drop_policy != "summarize" {
        return Ok(());
    }

    let drop_count = items.len() - cap;
    let dropped = &items[..drop_count];
    let kept = &items[drop_count..];
    let summary = summarize_items(dropped);
    let now = now_epoch_ms();
    let anchor = kept
        .first()
        .ok_or_else(|| "queue cap logic lost kept items".to_string())?;
    let merged_summary = join_summary(&anchor.summary_text, &summary);

    store.exec(&format!(
        "UPDATE inbound_queue
         SET summary_text = '{}'
         WHERE id = {};",
        sql_escape(&merged_summary),
        anchor.id
    ))?;
    for item in dropped {
        store.exec(&format!(
            "UPDATE inbound_queue
             SET status = 'dropped',
                 finished_at = {},
                 summary_text = '{}',
                 error_text = 'dropped after queue overflow'
             WHERE id = {};",
            now,
            sql_escape(&summary),
            item.id
        ))?;
    }
    Ok(())
}

fn summarize_items(items: &[QueueItem]) -> String {
    let mut parts = Vec::new();
    for item in items {
        if !item.summary_text.trim().is_empty() {
            parts.push(item.summary_text.trim().to_string());
        }
        parts.push(truncate(item.message_text.trim(), 120));
    }
    format!("Earlier queued messages: {}", parts.join(" | "))
}

fn join_summary(existing: &str, extra: &str) -> String {
    match (existing.trim(), extra.trim()) {
        ("", "") => String::new(),
        ("", value) => value.to_string(),
        (value, "") => value.to_string(),
        (value, extra) => format!("{value}\n{extra}"),
    }
}

fn global_running_count(store: &SqliteStore) -> Result<usize, String> {
    Ok(store
        .scalar(
            "SELECT COUNT(*) FROM inbound_queue_leases
             WHERE scope = 'global';",
        )?
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0))
}

fn lease_next_batch(
    store: &SqliteStore,
    config: &MessageQueueConfig,
) -> Result<Option<DrainBatch>, String> {
    let now = now_epoch_ms();
    let Some(primary) = store
        .query(&format!(
            "SELECT id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at,
                    started_at, finished_at, merged_count, COALESCE(summary_text, ''), COALESCE(error_text, ''),
                    COALESCE(response_text, '')
             FROM inbound_queue
             WHERE status = 'queued'
               AND available_at <= {}
               AND session_id NOT IN (
                    SELECT session_id FROM inbound_queue_leases WHERE scope = 'session'
               )
             ORDER BY created_at ASC
             LIMIT 1;",
            now
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_item(&row))
    else {
        return Ok(None);
    };

    let mut grouped = vec![primary.clone()];
    let mut window_close = primary.available_at;
    for item in queued_for_session(store, &primary.session_id)? {
        if item.id == primary.id || item.status != "queued" || item.available_at > now {
            continue;
        }
        if item.created_at > window_close {
            break;
        }
        window_close = window_close.max(item.available_at);
        grouped.push(item);
    }

    let primary_id = primary.id;
    let last = grouped.last().cloned().unwrap_or(primary.clone());
    let prompt = build_prompt_from_group(&grouped);
    acquire_leases(store, &primary.session_id, primary_id, config.lease_timeout_ms)?;
    let ids = grouped.iter().map(|item| item.id).collect::<Vec<_>>();
    let started_at = now_epoch_ms();
    store.exec(&format!(
        "UPDATE inbound_queue
         SET status = 'running',
             started_at = {}
         WHERE id IN ({});",
        started_at,
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let _ = set_session_state(store, &primary.session_id, AssistantState::Thinking);
    Ok(Some(DrainBatch {
        primary_id,
        surface: primary.surface,
        session_id: primary.session_id,
        peer_id: last.peer_id,
        chat_id: last.chat_id,
        prompt,
        item_ids: ids,
    }))
}

fn build_prompt_from_group(items: &[QueueItem]) -> String {
    let mut segments = Vec::new();
    let mut summaries = items
        .iter()
        .filter_map(|item| {
            let value = item.summary_text.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        })
        .collect::<Vec<_>>();
    summaries.dedup();
    if !summaries.is_empty() {
        segments.push(format!(
            "Queued context for this same session:\n{}",
            summaries.join("\n")
        ));
    }
    if items.len() == 1 {
        segments.push(items[0].message_text.clone());
    } else {
        let turns = items
            .iter()
            .enumerate()
            .map(|(index, item)| format!("Message {}:\n{}", index + 1, item.message_text))
            .collect::<Vec<_>>()
            .join("\n\n");
        segments.push(format!(
            "Respond to the latest burst as one reply.\n\n{}",
            turns
        ));
    }
    segments.join("\n\n")
}

fn queued_for_session(store: &SqliteStore, session_id: &str) -> Result<Vec<QueueItem>, String> {
    Ok(store
        .query(&format!(
            "SELECT id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at,
                    started_at, finished_at, merged_count, COALESCE(summary_text, ''), COALESCE(error_text, ''),
                    COALESCE(response_text, '')
             FROM inbound_queue
             WHERE session_id = '{}' AND status = 'queued'
             ORDER BY created_at ASC;",
            sql_escape(session_id)
        ))?
        .into_iter()
        .filter_map(|row| parse_item(&row))
        .collect())
}

fn acquire_leases(
    store: &SqliteStore,
    session_id: &str,
    item_id: i64,
    lease_timeout_ms: usize,
) -> Result<(), String> {
    let now = now_epoch_ms();
    let expires_at = now + lease_timeout_ms.max(1) as i64;
    store.exec(&format!(
        "INSERT OR REPLACE INTO inbound_queue_leases
         (lease_key, scope, item_id, session_id, leased_at, expires_at)
         VALUES ('global:{}', 'global', {}, '{}', {}, {}),
                ('session:{}', 'session', {}, '{}', {}, {});",
        item_id,
        item_id,
        sql_escape(session_id),
        now,
        expires_at,
        sql_escape(session_id),
        item_id,
        sql_escape(session_id),
        now,
        expires_at
    ))
}

fn release_leases(store: &SqliteStore, item_id: i64, session_id: &str) -> Result<(), String> {
    store.exec(&format!(
        "DELETE FROM inbound_queue_leases
         WHERE lease_key IN ('global:{}', 'session:{}');",
        item_id,
        sql_escape(session_id)
    ))
}

fn complete_batch(
    store: &SqliteStore,
    batch: &DrainBatch,
    status: &str,
    error_text: &str,
    response_text: &str,
) -> Result<(), String> {
    let finished_at = now_epoch_ms();
    store.exec(&format!(
        "UPDATE inbound_queue
         SET status = '{}',
             finished_at = {},
             error_text = '{}',
             response_text = '{}'
         WHERE id IN ({});",
        sql_escape(status),
        finished_at,
        sql_escape(error_text),
        sql_escape(response_text),
        batch
            .item_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    ))?;
    let next_state = if status == "failed" {
        AssistantState::Failed
    } else {
        AssistantState::Idle
    };
    let _ = set_session_state(store, &batch.session_id, next_state);
    release_leases(store, batch.primary_id, &batch.session_id)
}

fn list_running_items(store: &SqliteStore, started_before: i64) -> Result<Vec<QueueItem>, String> {
    Ok(store
        .query(&format!(
            "SELECT id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at,
                    started_at, finished_at, merged_count, COALESCE(summary_text, ''), COALESCE(error_text, ''),
                    COALESCE(response_text, '')
             FROM inbound_queue
             WHERE status = 'running'
               AND COALESCE(started_at, 0) <= {};",
            started_before
        ))?
        .into_iter()
        .filter_map(|row| parse_item(&row))
        .collect())
}

fn fetch_item(store: &SqliteStore, item_id: i64) -> Result<Option<QueueItem>, String> {
    Ok(store
        .query(&format!(
            "SELECT id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at,
                    started_at, finished_at, merged_count, COALESCE(summary_text, ''), COALESCE(error_text, ''),
                    COALESCE(response_text, '')
             FROM inbound_queue
             WHERE id = {}
             LIMIT 1;",
            item_id
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_item(&row)))
}

fn parse_item(row: &[String]) -> Option<QueueItem> {
    if row.len() < 15 {
        return None;
    }
    Some(QueueItem {
        id: row[0].parse().ok()?,
        surface: row[1].clone(),
        session_id: row[2].clone(),
        peer_id: empty_to_none(&row[3]),
        chat_id: empty_to_none(&row[4]),
        message_text: row[5].clone(),
        status: row[6].clone(),
        created_at: row[7].parse().ok()?,
        available_at: row[8].parse().ok()?,
        started_at: parse_optional_epoch(&row[9]),
        finished_at: parse_optional_epoch(&row[10]),
        merged_count: row[11].parse().ok()?,
        summary_text: row[12].clone(),
        error_text: row[13].clone(),
        response_text: row[14].clone(),
    })
}

fn empty_to_none(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_optional_epoch(value: &str) -> Option<i64> {
    if value.trim().is_empty() {
        None
    } else {
        value.parse().ok()
    }
}

fn optional_sql_text(value: &Option<String>) -> String {
    value
        .as_ref()
        .filter(|item| !item.trim().is_empty())
        .map(|item| format!("'{}'", sql_escape(item)))
        .unwrap_or_else(|| "NULL".to_string())
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use crate::{
        adapters::storage::SqliteStore,
        config::{
            AppConfig, AssistantPaths, IdentityConfig, LlmConfig, MessageQueueConfig,
            MessageReplyConfig, MessagesConfig, SchedulerConfig, TelegramConfig, ToolConfig,
            VoiceConfig, default_voice_stt_model_path, default_voice_temp_audio_dir,
            default_voice_tts_model_path,
        },
        core::session::Session,
        util::unique_temp_dir,
    };

    use super::{
        InboundRequest, dispatch_due_once, enqueue_request, queue_depth, recover_stale_leases,
    };

    #[test]
    fn telegram_enqueue_merges_same_session_inside_debounce_window() {
        let root = unique_temp_dir("assistant-queue-merge-telegram");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = queue_config();
        let app = app_config(&paths);

        let first = enqueue_request(
            &store,
            &config,
            &InboundRequest {
                session: Session::telegram_dm(42, 42, &app),
                message_text: "first".into(),
            },
        )
        .unwrap();
        let second = enqueue_request(
            &store,
            &config,
            &InboundRequest {
                session: Session::telegram_dm(42, 42, &app),
                message_text: "second".into(),
            },
        )
        .unwrap();

        assert_eq!(first.item_id, second.item_id);
        assert!(second.merged);
        assert_eq!(queue_depth(&store, "telegram:dm:42", "queued").unwrap(), 1);
    }

    #[test]
    fn voice_enqueue_merges_same_session_inside_debounce_window() {
        let root = unique_temp_dir("assistant-queue-merge-voice");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = queue_config();
        let app = app_config(&paths);

        enqueue_request(
            &store,
            &config,
            &InboundRequest {
                session: Session::voice("voice:local:default", &app),
                message_text: "hello".into(),
            },
        )
        .unwrap();
        let merged = enqueue_request(
            &store,
            &config,
            &InboundRequest {
                session: Session::voice("voice:local:default", &app),
                message_text: "again".into(),
            },
        )
        .unwrap();

        assert!(merged.merged);
        assert_eq!(queue_depth(&store, "voice:local:default", "queued").unwrap(), 1);
    }

    #[test]
    fn overflow_summarizes_oldest_queued_items() {
        let root = unique_temp_dir("assistant-queue-overflow");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let mut config = queue_config();
        config.telegram_debounce_ms = 1;
        let app = app_config(&paths);

        for index in 0..6 {
            enqueue_request(
                &store,
                &config,
                &InboundRequest {
                    session: Session::telegram_dm(7, 7, &app),
                    message_text: format!("message {index}"),
                },
            )
            .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        assert_eq!(queue_depth(&store, "telegram:dm:7", "queued").unwrap(), 5);
        assert_eq!(queue_depth(&store, "telegram:dm:7", "dropped").unwrap(), 1);
        let summary = store
            .scalar(
                "SELECT summary_text FROM inbound_queue
                 WHERE session_id = 'telegram:dm:7' AND status = 'queued'
                 ORDER BY created_at ASC
                 LIMIT 1;",
            )
            .unwrap()
            .unwrap();
        assert!(summary.contains("Earlier queued messages"));
        assert!(summary.contains("message 0"));
    }

    #[test]
    fn stale_running_items_are_returned_to_queue() {
        let root = unique_temp_dir("assistant-queue-stale");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = queue_config();
        store.exec(
            "INSERT INTO inbound_queue
             (id, surface, session_id, peer_id, chat_id, message_text, status, created_at, available_at, started_at, merged_count, summary_text, error_text, response_text)
             VALUES (1, 'telegram', 'telegram:dm:1', '1', '1', 'hello', 'running', 0, 0, 0, 1, '', '', '');",
        )
        .unwrap();
        store.exec(
            "INSERT INTO inbound_queue_leases
             (lease_key, scope, item_id, session_id, leased_at, expires_at)
             VALUES ('global:1', 'global', 1, 'telegram:dm:1', 0, 0),
                    ('session:telegram:dm:1', 'session', 1, 'telegram:dm:1', 0, 0);",
        )
        .unwrap();

        let recovered = recover_stale_leases(&store, &config).unwrap();
        assert_eq!(recovered, 1);
        assert_eq!(queue_depth(&store, "telegram:dm:1", "queued").unwrap(), 1);
    }

    #[test]
    fn queue_drain_respects_single_global_and_session_run() {
        let root = unique_temp_dir("assistant-queue-drain-limits");
        let paths = AssistantPaths::new(root.clone());
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = app_config(&paths);

        enqueue_request(
            &store,
            &config.messages.queue,
            &InboundRequest {
                session: Session::voice("voice:one", &config),
                message_text: "first".into(),
            },
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        enqueue_request(
            &store,
            &config.messages.queue,
            &InboundRequest {
                session: Session::voice("voice:one", &config),
                message_text: "second".into(),
            },
        )
        .unwrap();

        let logs = dispatch_due_once(&paths, &store, &config).unwrap();
        assert!(logs.iter().any(|line| line.contains("completed queued voice reply")));
        assert_eq!(queue_depth(&store, "voice:one", "done").unwrap(), 1);
        assert_eq!(
            store.scalar("SELECT COUNT(*) FROM inbound_queue_leases;")
                .unwrap()
                .unwrap(),
            "0"
        );
    }

    fn queue_config() -> MessageQueueConfig {
        MessageQueueConfig {
            enabled: true,
            mode: "collect".into(),
            global_max_concurrency: 1,
            per_session_cap: 5,
            telegram_debounce_ms: 1200,
            voice_debounce_ms: 300,
            drop_policy: "summarize".into(),
            lease_timeout_ms: 1_000,
        }
    }

    fn app_config(paths: &AssistantPaths) -> AppConfig {
        let root = &paths.root;
        let binary = root.join("llama-cli");
        let model = root.join("model.gguf");
        fs::write(&binary, "#!/bin/sh\nprintf 'queued reply\\n'\n").unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&model, "mock").unwrap();

        let mut queue = queue_config();
        queue.voice_debounce_ms = 1;

        AppConfig {
            llm: LlmConfig {
                prefer_http: false,
                endpoint: String::new(),
                health_endpoint: String::new(),
                model: "mock".into(),
                binary_path: binary.display().to_string(),
                model_path: model.display().to_string(),
                threads: 1,
                context_size: 256,
                predict_tokens: 64,
                timeout_secs: 1,
                retries: 0,
                stream: false,
            },
            memory: crate::config::MemoryConfig {
                recent_turn_limit: 4,
                compact_after_turns: 12,
                retain_recent_turns: 6,
                token_budget: 256,
                compact_context_threshold_percent: 70,
                memory_search_limit: 4,
                memory_ttl_days: 30,
            },
            scheduler: SchedulerConfig {
                poll_seconds: 30,
                max_jobs_per_tick: 4,
                allow_shell_jobs: false,
            },
            identity: IdentityConfig {
                name: "Kumo".into(),
                style: "direct".into(),
                system_instruction: "Stay local".into(),
            },
            tools: ToolConfig { allowlist: vec![] },
            telegram: TelegramConfig {
                enabled: false,
                bot_token: String::new(),
                bot_token_file: String::new(),
                poll_timeout_secs: 0,
                owner_user_id: None,
                allowed_user_ids: vec![],
                pairing_enabled: true,
                pairing_code_ttl_minutes: 15,
                api_base_url: "https://api.telegram.org".into(),
            },
            voice: VoiceConfig {
                enabled: false,
                input_device: String::new(),
                output_device: String::new(),
                sample_rate: 16_000,
                capture_seconds_max: 8,
                stt_binary_path: "whisper-cli".into(),
                stt_model_path: default_voice_stt_model_path(paths),
                tts_binary_path: "piper".into(),
                tts_model_path: default_voice_tts_model_path(paths),
                player_binary_path: "aplay".into(),
                recorder_binary_path: "arecord".into(),
                trigger_mode: "push_to_talk".into(),
                push_to_talk_command: String::new(),
                silence_timeout_ms: 1_200,
                temp_audio_dir: default_voice_temp_audio_dir(paths),
            },
            messages: MessagesConfig {
                queue,
                reply: MessageReplyConfig {
                    telegram_chunk_chars: 3000,
                },
            },
        }
    }
}
