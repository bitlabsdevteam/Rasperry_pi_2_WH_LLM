use std::{
    collections::BTreeSet,
};

use crate::{
    adapters::{
        storage::SqliteStore,
        telegram::{TelegramAdapter, TelegramUpdate},
    },
    config::{
        AppConfig, AssistantPaths, IdentityConfig, TelegramConfig, write_identity_config,
        write_telegram_config,
    },
    core::{
        identity::{UserProfile, write_assistant_profile},
        inbound_queue::{InboundRequest, enqueue_request},
        service::run_chat_session_with_session,
        session::Session,
    },
    util::{now_epoch, sql_escape},
};

#[derive(Clone, Debug)]
pub struct PendingPairing {
    pub code: String,
    pub user_id: i64,
    pub chat_id: i64,
    pub username: String,
    pub first_name: String,
    pub status: String,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Clone, Debug)]
pub struct TelegramRuntimeStatus {
    pub enabled: bool,
    pub owner_user_id: Option<i64>,
    pub allowed_user_ids: Vec<i64>,
    pub pending_count: usize,
    pub last_update_id: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TelegramOnboardingSession {
    user_id: i64,
    chat_id: i64,
    stage: String,
    assistant_name: String,
    assistant_style: String,
    user_name: String,
    user_role: String,
    about: String,
    goals: String,
    preferences: String,
}

pub fn session_key(user_id: i64) -> String {
    format!("telegram:dm:{user_id}")
}

pub fn adapter_from_config(
    paths: &AssistantPaths,
    config: &TelegramConfig,
) -> Result<Option<TelegramAdapter>, String> {
    if !config.enabled {
        return Ok(None);
    }
    let Some(token) = config.resolve_bot_token(paths)? else {
        return Ok(None);
    };
    Ok(Some(TelegramAdapter::new(
        token,
        config.api_base_url.clone(),
        config.poll_timeout_secs,
    )))
}

pub fn list_pending_pairings(store: &SqliteStore) -> Result<Vec<PendingPairing>, String> {
    let rows = store.query(&format!(
        "SELECT code, user_id, chat_id, username, first_name, status, created_at, expires_at
         FROM telegram_pairings
         WHERE status = 'pending' AND expires_at > {}
         ORDER BY created_at ASC;",
        now_epoch()
    ))?;
    Ok(rows
        .into_iter()
        .filter_map(|row| parse_pending_pairing(&row))
        .collect())
}

pub fn runtime_status(
    store: &SqliteStore,
    config: &TelegramConfig,
) -> Result<TelegramRuntimeStatus, String> {
    let mut allowed = config.allowed_user_ids.clone();
    for user_id in list_allowed_user_ids(store)? {
        if !allowed.contains(&user_id) {
            allowed.push(user_id);
        }
    }
    allowed.sort_unstable();
    allowed.dedup();
    Ok(TelegramRuntimeStatus {
        enabled: config.enabled,
        owner_user_id: config.owner_user_id,
        pending_count: list_pending_pairings(store)?.len(),
        last_update_id: last_update_id(store)?,
        allowed_user_ids: allowed,
    })
}

pub fn approve_pairing_code(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &TelegramConfig,
    code: &str,
) -> Result<PendingPairing, String> {
    let pairing = fetch_pairing(store, code)?
        .ok_or_else(|| format!("no pending pairing found for code `{code}`"))?;
    if pairing.status != "pending" || pairing.expires_at <= now_epoch() {
        return Err(format!("pairing code `{code}` is no longer pending"));
    }

    let is_owner = config.owner_user_id.is_none();
    store.exec(&format!(
        "INSERT OR REPLACE INTO telegram_allowlist (user_id, chat_id, username, first_name, approved_at, is_owner)
         VALUES ({}, {}, '{}', '{}', {}, {});",
        pairing.user_id,
        pairing.chat_id,
        sql_escape(&pairing.username),
        sql_escape(&pairing.first_name),
        now_epoch(),
        if is_owner { 1 } else { 0 }
    ))?;
    store.exec(&format!(
        "UPDATE telegram_pairings SET status = 'approved' WHERE code = '{}';",
        sql_escape(&pairing.code)
    ))?;

    let mut next_config = config.clone();
    if is_owner {
        next_config.owner_user_id = Some(pairing.user_id);
    }
    let mut allowed = BTreeSet::new();
    for user_id in config.allowed_user_ids.iter().copied() {
        allowed.insert(user_id);
    }
    for user_id in list_allowed_user_ids(store)? {
        allowed.insert(user_id);
    }
    allowed.insert(pairing.user_id);
    next_config.allowed_user_ids = allowed.into_iter().collect();
    next_config.enabled = true;
    write_telegram_config(paths, &next_config)?;

    Ok(pairing)
}

pub fn deny_pairing_code(store: &SqliteStore, code: &str) -> Result<String, String> {
    let Some(pairing) = fetch_pairing(store, code)? else {
        return Err(format!("no pending pairing found for code `{code}`"));
    };
    store.exec(&format!(
        "UPDATE telegram_pairings SET status = 'denied' WHERE code = '{}';",
        sql_escape(code)
    ))?;
    Ok(format!(
        "denied {} ({})",
        display_name(&pairing.username, &pairing.first_name, pairing.user_id),
        pairing.code
    ))
}

pub fn create_pending_pairing(
    store: &SqliteStore,
    update: &TelegramUpdate,
    ttl_minutes: usize,
) -> Result<PendingPairing, String> {
    if let Some(existing) = active_pairing_for_user(store, update.user_id)? {
        return Ok(existing);
    }

    let created_at = now_epoch();
    let expires_at = created_at + ((ttl_minutes as i64).max(1) * 60);
    let code = generate_pairing_code(update.user_id, created_at);
    store.exec(&format!(
        "INSERT OR REPLACE INTO telegram_pairings (code, user_id, chat_id, username, first_name, status, created_at, expires_at)
         VALUES ('{}', {}, {}, '{}', '{}', 'pending', {}, {});",
        sql_escape(&code),
        update.user_id,
        update.chat_id,
        sql_escape(&update.username),
        sql_escape(&update.first_name),
        created_at,
        expires_at
    ))?;
    Ok(PendingPairing {
        code,
        user_id: update.user_id,
        chat_id: update.chat_id,
        username: update.username.clone(),
        first_name: update.first_name.clone(),
        status: "pending".to_string(),
        created_at,
        expires_at,
    })
}

pub fn generate_pairing_code(user_id: i64, now: i64) -> String {
    let value = ((user_id.unsigned_abs() + now as u64) % 1_679_615) as u32;
    format!("{value:05X}")
}

pub fn pairing_expired(pairing: &PendingPairing, now: i64) -> bool {
    pairing.expires_at <= now
}

pub fn process_telegram_once(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    poll_timeout_secs: Option<usize>,
) -> Result<Vec<String>, String> {
    let Some(adapter) = adapter_from_config(paths, &config.telegram)? else {
        return Ok(Vec::new());
    };
    let offset = last_update_id(store)?;
    let updates = adapter.get_updates(
        if offset > 0 { Some(offset) } else { None },
        poll_timeout_secs.unwrap_or(config.telegram.poll_timeout_secs),
    )?;
    if updates.is_empty() {
        return Ok(Vec::new());
    }

    let mut logs = Vec::new();
    let mut next_offset = offset;
    for update in updates {
        next_offset = next_offset.max(update.update_id + 1);
        logs.extend(handle_update(paths, store, config, &adapter, &update)?);
    }
    set_last_update_id(store, next_offset)?;
    Ok(logs)
}

pub fn poll_for_first_pairing(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &TelegramConfig,
    rounds: usize,
) -> Result<Option<PendingPairing>, String> {
    let Some(adapter) = adapter_from_config(
        paths,
        &TelegramConfig {
            enabled: true,
            ..config.clone()
        },
    )?
    else {
        return Ok(None);
    };

    let mut offset = last_update_id(store)?;
    for _ in 0..rounds {
        let updates = adapter.get_updates(
            if offset > 0 { Some(offset) } else { None },
            config.poll_timeout_secs,
        )?;
        for update in updates {
            offset = offset.max(update.update_id + 1);
            if update.is_private_text() {
                set_last_update_id(store, offset)?;
                return create_pending_pairing(store, &update, config.pairing_code_ttl_minutes)
                    .map(Some);
            }
        }
        set_last_update_id(store, offset)?;
    }
    Ok(None)
}

pub fn send_message(
    paths: &AssistantPaths,
    config: &TelegramConfig,
    chat_id: i64,
    text: &str,
) -> Result<(), String> {
    let Some(adapter) = adapter_from_config(paths, config)? else {
        return Err("telegram is not configured".to_string());
    };
    adapter.send_message(chat_id, text)
}

pub fn send_reply(
    paths: &AssistantPaths,
    config: &TelegramConfig,
    chat_id: i64,
    text: &str,
    chunk_chars: usize,
) -> Result<(), String> {
    let chunks = split_reply_chunks(text, chunk_chars.max(64));
    if chunks.is_empty() {
        return send_message(paths, config, chat_id, text);
    }
    let total = chunks.len();
    for (index, chunk) in chunks.into_iter().enumerate() {
        let body = if total == 1 {
            chunk
        } else {
            format!("({}/{}) {}", index + 1, total, chunk)
        };
        send_message(paths, config, chat_id, &body)?;
    }
    Ok(())
}

fn handle_update(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    adapter: &TelegramAdapter,
    update: &TelegramUpdate,
) -> Result<Vec<String>, String> {
    if update.chat_type != "private" {
        return Ok(vec![format!("ignored non-DM update {}", update.update_id)]);
    }

    if !update.is_private_text() {
        if is_allowed_user(store, &config.telegram, update.user_id)? {
            adapter.send_message(
                update.chat_id,
                "Telegram v1 only supports private text messages.",
            )?;
        }
        return Ok(vec![format!(
            "ignored unsupported Telegram payload from {}",
            update.display_name()
        )]);
    }

    if !is_allowed_user(store, &config.telegram, update.user_id)? {
        let pairing =
            create_pending_pairing(store, update, config.telegram.pairing_code_ttl_minutes)?;
        adapter.send_message(
            update.chat_id,
            &format!(
                "Access is pending local approval.\nCode: {}\nRun `assistant telegram approve {}` on the device to allow this DM.",
                pairing.code, pairing.code
            ),
        )?;
        return Ok(vec![format!(
            "pending Telegram pairing {} for {}",
            pairing.code,
            display_name(&pairing.username, &pairing.first_name, pairing.user_id)
        )]);
    }

    let message = update.text.as_deref().unwrap_or_default();
    if let Some(reply) = handle_onboarding_command(paths, store, config, update, message)? {
        adapter.send_message(update.chat_id, &reply)?;
        return Ok(vec![format!(
            "processed onboarding command for {}",
            update.display_name()
        )]);
    }
    if let Some(reply) = handle_onboarding_reply(paths, store, config, update, message)? {
        adapter.send_message(update.chat_id, &reply)?;
        return Ok(vec![format!(
            "updated onboarding session for {}",
            update.display_name()
        )]);
    }
    let session = Session::telegram_dm(update.user_id, update.chat_id, config);
    if !config.messages.queue.enabled {
        let _ = adapter.send_chat_action(update.chat_id, "typing");
        let outcome = run_chat_session_with_session(paths, config, store, &session, message, false)?;
        send_reply(
            paths,
            &config.telegram,
            update.chat_id,
            &outcome.response,
            config.messages.reply.telegram_chunk_chars,
        )?;
        return Ok(vec![format!("replied to {}", update.display_name())]);
    }
    let _ = adapter.send_chat_action(update.chat_id, "typing");
    let queued = enqueue_request(
        store,
        &config.messages.queue,
        &InboundRequest {
            session,
            message_text: message.to_string(),
        },
    )?;
    Ok(vec![format!(
        "queued Telegram message for {} ({})",
        update.display_name(),
        if queued.merged { "merged" } else { "new" }
    )])
}

fn split_reply_chunks(text: &str, chunk_chars: usize) -> Vec<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return vec![String::new()];
    }
    if trimmed.chars().count() <= chunk_chars {
        return vec![trimmed.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = trimmed;
    let mut open_fence_lang = String::new();

    while !remaining.trim().is_empty() {
        let prefix = if open_fence_lang.is_empty() {
            String::new()
        } else {
            format!("```{}\n", open_fence_lang)
        };
        let mut budget = chunk_chars.saturating_sub(prefix.chars().count()).max(1);
        let mut cut = find_chunk_boundary(remaining, budget);
        let mut head = remaining[..cut].trim();
        if head.is_empty() {
            break;
        }
        let mut next_open_fence = updated_fence_language(&open_fence_lang, head);
        let suffix = if next_open_fence.is_empty() {
            String::new()
        } else {
            "\n```".to_string()
        };
        if prefix.chars().count() + head.chars().count() + suffix.chars().count() > chunk_chars
            && budget > suffix.chars().count()
        {
            budget = budget.saturating_sub(suffix.chars().count()).max(1);
            cut = find_chunk_boundary(remaining, budget);
            head = remaining[..cut].trim();
            next_open_fence = updated_fence_language(&open_fence_lang, head);
        }
        let body = format!("{prefix}{head}{suffix}");
        chunks.push(body.trim().to_string());
        open_fence_lang = next_open_fence;
        remaining = remaining[cut..].trim_start();
    }

    if chunks.is_empty() {
        vec![trimmed.to_string()]
    } else {
        chunks
    }
}

fn find_chunk_boundary(text: &str, max_chars: usize) -> usize {
    let mut count = 0usize;
    let mut last_paragraph = None;
    let mut last_line = None;
    let mut last_space = None;
    let mut last_index = 0usize;
    for (index, ch) in text.char_indices() {
        let ch_len = ch.len_utf8();
        if count + 1 > max_chars {
            break;
        }
        count += 1;
        last_index = index + ch_len;
        if text[..last_index].ends_with("\n\n") {
            last_paragraph = Some(last_index);
        }
        if ch == '\n' {
            last_line = Some(last_index);
        }
        if ch.is_whitespace() {
            last_space = Some(last_index);
        }
    }
    if last_index == text.len() {
        return text.len();
    }
    last_paragraph
        .or(last_line)
        .or(last_space)
        .unwrap_or(last_index.max(1))
}

fn updated_fence_language(current: &str, chunk: &str) -> String {
    let mut active = current.to_string();
    for line in chunk.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("```") {
            if active.is_empty() {
                active = rest.trim().to_string();
            } else {
                active.clear();
            }
        }
    }
    active
}

fn handle_onboarding_command(
    _paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    update: &TelegramUpdate,
    message: &str,
) -> Result<Option<String>, String> {
    let normalized = message.trim();
    if !normalized.starts_with('/') {
        return Ok(None);
    }
    if normalized.eq_ignore_ascii_case("/cancel") {
        if delete_onboarding_session(store, update.user_id)? {
            return Ok(Some("Onboarding canceled.".to_string()));
        }
        return Ok(Some("No onboarding session is active.".to_string()));
    }
    if !normalized.eq_ignore_ascii_case("/onboard") {
        return Ok(None);
    }

    let default_user_name = if !update.first_name.trim().is_empty() {
        update.first_name.trim().to_string()
    } else if !update.username.trim().is_empty() {
        update.username.trim().to_string()
    } else {
        "User".to_string()
    };
    let session = TelegramOnboardingSession {
        user_id: update.user_id,
        chat_id: update.chat_id,
        stage: "assistant_name".to_string(),
        assistant_name: config.identity.name.clone(),
        assistant_style: config.identity.style.clone(),
        user_name: default_user_name,
        user_role: String::new(),
        about: String::new(),
        goals: String::new(),
        preferences: "direct, concise, practical".to_string(),
    };
    save_onboarding_session(store, &session)?;
    Ok(Some(format!(
        "Identity onboarding started.\n1/6 Assistant name [{}]\nReply with a new value, /skip to keep it, or /cancel to stop.",
        session.assistant_name
    )))
}

fn handle_onboarding_reply(
    paths: &AssistantPaths,
    store: &SqliteStore,
    config: &AppConfig,
    update: &TelegramUpdate,
    message: &str,
) -> Result<Option<String>, String> {
    let Some(mut session) = fetch_onboarding_session(store, update.user_id)? else {
        return Ok(None);
    };
    let trimmed = message.trim();
    if trimmed.starts_with('/') && !trimmed.eq_ignore_ascii_case("/skip") {
        return Ok(Some(
            "Onboarding is in progress. Reply with a value, /skip, or /cancel.".to_string(),
        ));
    }

    match session.stage.as_str() {
        "assistant_name" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.assistant_name = trimmed.to_string();
            }
            session.stage = "assistant_style".to_string();
            save_onboarding_session(store, &session)?;
            Ok(Some(format!(
                "2/6 Assistant style [{}]\nDescribe how I should reply. Use /skip to keep it.",
                session.assistant_style
            )))
        }
        "assistant_style" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.assistant_style = trimmed.to_string();
            }
            session.stage = "user_name".to_string();
            save_onboarding_session(store, &session)?;
            Ok(Some(format!(
                "3/6 Your name [{}]\nUse /skip to keep it.",
                session.user_name
            )))
        }
        "user_name" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.user_name = trimmed.to_string();
            }
            session.stage = "user_role".to_string();
            save_onboarding_session(store, &session)?;
            Ok(Some(
                "4/6 Your role or what you do.\nReply with text or /skip.".to_string(),
            ))
        }
        "user_role" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.user_role = trimmed.to_string();
            }
            session.stage = "about".to_string();
            save_onboarding_session(store, &session)?;
            Ok(Some(
                "5/6 What should I know about you?\nReply with text or /skip.".to_string(),
            ))
        }
        "about" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.about = trimmed.to_string();
            }
            session.stage = "goals".to_string();
            save_onboarding_session(store, &session)?;
            Ok(Some(
                "6/6 Current goals or active projects.\nReply with text or /skip.".to_string(),
            ))
        }
        "goals" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.goals = trimmed.to_string();
            }
            session.stage = "preferences".to_string();
            save_onboarding_session(store, &session)?;
            Ok(Some(format!(
                "Final step: preferred reply style [{}]\nReply with text or /skip.",
                session.preferences
            )))
        }
        "preferences" => {
            if !trimmed.eq_ignore_ascii_case("/skip") && !trimmed.is_empty() {
                session.preferences = trimmed.to_string();
            }
            complete_onboarding(paths, config, update, &session)?;
            delete_onboarding_session(store, update.user_id)?;
            Ok(Some(format!(
                "Onboarding saved.\nAssistant name: {}\nUser name: {}\nYou can ask \"Do you know me?\" now.",
                session.assistant_name, session.user_name
            )))
        }
        _ => {
            delete_onboarding_session(store, update.user_id)?;
            Ok(Some(
                "Onboarding state was invalid and has been cleared. Send /onboard to start again."
                    .to_string(),
            ))
        }
    }
}

fn complete_onboarding(
    paths: &AssistantPaths,
    config: &AppConfig,
    update: &TelegramUpdate,
    session: &TelegramOnboardingSession,
) -> Result<(), String> {
    let identity = IdentityConfig {
        name: session.assistant_name.clone(),
        style: session.assistant_style.clone(),
        system_instruction: config.identity.system_instruction.clone(),
    };
    write_identity_config(paths, &identity)?;
    let user = UserProfile {
        name: session.user_name.clone(),
        telegram_handle: if update.username.trim().is_empty() {
            String::new()
        } else {
            format!("@{}", update.username.trim().trim_start_matches('@'))
        },
        role: session.user_role.clone(),
        about: session.about.clone(),
        goals: session.goals.clone(),
        preferences: session.preferences.clone(),
    };
    write_assistant_profile(paths, &identity, &user)
}

fn is_allowed_user(
    store: &SqliteStore,
    config: &TelegramConfig,
    user_id: i64,
) -> Result<bool, String> {
    if config.owner_user_id == Some(user_id) || config.allowed_user_ids.contains(&user_id) {
        return Ok(true);
    }
    Ok(store
        .scalar(&format!(
            "SELECT user_id FROM telegram_allowlist WHERE user_id = {};",
            user_id
        ))?
        .is_some())
}

fn fetch_onboarding_session(
    store: &SqliteStore,
    user_id: i64,
) -> Result<Option<TelegramOnboardingSession>, String> {
    Ok(store
        .query(&format!(
            "SELECT user_id, chat_id, stage, assistant_name, assistant_style, user_name, user_role, about, goals, preferences
             FROM telegram_onboarding_sessions
             WHERE user_id = {}
             LIMIT 1;",
            user_id
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_onboarding_session(&row)))
}

fn save_onboarding_session(
    store: &SqliteStore,
    session: &TelegramOnboardingSession,
) -> Result<(), String> {
    store.exec(&format!(
        "INSERT OR REPLACE INTO telegram_onboarding_sessions
         (user_id, chat_id, stage, assistant_name, assistant_style, user_name, user_role, about, goals, preferences, started_at, updated_at)
         VALUES ({}, {}, '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}',
                 COALESCE((SELECT started_at FROM telegram_onboarding_sessions WHERE user_id = {}), {}),
                 {});",
        session.user_id,
        session.chat_id,
        sql_escape(&session.stage),
        sql_escape(&session.assistant_name),
        sql_escape(&session.assistant_style),
        sql_escape(&session.user_name),
        sql_escape(&session.user_role),
        sql_escape(&session.about),
        sql_escape(&session.goals),
        sql_escape(&session.preferences),
        session.user_id,
        now_epoch(),
        now_epoch(),
    ))
}

fn delete_onboarding_session(store: &SqliteStore, user_id: i64) -> Result<bool, String> {
    let existed = fetch_onboarding_session(store, user_id)?.is_some();
    store.exec(&format!(
        "DELETE FROM telegram_onboarding_sessions WHERE user_id = {};",
        user_id
    ))?;
    Ok(existed)
}

fn parse_onboarding_session(row: &[String]) -> Option<TelegramOnboardingSession> {
    if row.len() < 10 {
        return None;
    }
    Some(TelegramOnboardingSession {
        user_id: row[0].parse().ok()?,
        chat_id: row[1].parse().ok()?,
        stage: row[2].clone(),
        assistant_name: row[3].clone(),
        assistant_style: row[4].clone(),
        user_name: row[5].clone(),
        user_role: row[6].clone(),
        about: row[7].clone(),
        goals: row[8].clone(),
        preferences: row[9].clone(),
    })
}

fn active_pairing_for_user(
    store: &SqliteStore,
    user_id: i64,
) -> Result<Option<PendingPairing>, String> {
    Ok(store
        .query(&format!(
            "SELECT code, user_id, chat_id, username, first_name, status, created_at, expires_at
             FROM telegram_pairings
             WHERE user_id = {} AND status = 'pending' AND expires_at > {}
             ORDER BY created_at DESC
             LIMIT 1;",
            user_id,
            now_epoch()
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_pending_pairing(&row)))
}

fn fetch_pairing(store: &SqliteStore, code: &str) -> Result<Option<PendingPairing>, String> {
    Ok(store
        .query(&format!(
            "SELECT code, user_id, chat_id, username, first_name, status, created_at, expires_at
             FROM telegram_pairings
             WHERE code = '{}'
             LIMIT 1;",
            sql_escape(code)
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_pending_pairing(&row)))
}

fn parse_pending_pairing(row: &[String]) -> Option<PendingPairing> {
    if row.len() < 8 {
        return None;
    }
    Some(PendingPairing {
        code: row[0].clone(),
        user_id: row[1].parse().ok()?,
        chat_id: row[2].parse().ok()?,
        username: row[3].clone(),
        first_name: row[4].clone(),
        status: row[5].clone(),
        created_at: row[6].parse().ok()?,
        expires_at: row[7].parse().ok()?,
    })
}

fn display_name(username: &str, first_name: &str, user_id: i64) -> String {
    if !username.is_empty() {
        format!("@{username}")
    } else if !first_name.is_empty() {
        first_name.to_string()
    } else {
        user_id.to_string()
    }
}

fn list_allowed_user_ids(store: &SqliteStore) -> Result<Vec<i64>, String> {
    Ok(store
        .query("SELECT user_id FROM telegram_allowlist ORDER BY user_id ASC;")?
        .into_iter()
        .filter_map(|row| row.first().and_then(|value| value.parse::<i64>().ok()))
        .collect())
}

fn last_update_id(store: &SqliteStore) -> Result<i64, String> {
    Ok(store
        .scalar("SELECT value FROM telegram_state WHERE key = 'last_update_id';")?
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(0))
}

fn set_last_update_id(store: &SqliteStore, value: i64) -> Result<(), String> {
    store.exec(&format!(
        "INSERT OR REPLACE INTO telegram_state (key, value, updated_at)
         VALUES ('last_update_id', '{}', {});",
        value,
        now_epoch()
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::{
        adapters::storage::SqliteStore,
        config::{AppConfig, AssistantPaths, TelegramConfig},
        util::unique_temp_dir,
    };

    use super::{
        TelegramOnboardingSession, TelegramUpdate, approve_pairing_code, complete_onboarding,
        create_pending_pairing, delete_onboarding_session, fetch_onboarding_session,
        generate_pairing_code, handle_onboarding_command, handle_onboarding_reply, pairing_expired,
        runtime_status, save_onboarding_session, split_reply_chunks,
    };

    #[test]
    fn pairing_codes_are_stable_and_expire_on_ttl() {
        let code = generate_pairing_code(42, 1_700_000_000);
        assert_eq!(code.len(), 5);

        let pairing = super::PendingPairing {
            code,
            user_id: 42,
            chat_id: 42,
            username: "dbong".into(),
            first_name: "David".into(),
            status: "pending".into(),
            created_at: 100,
            expires_at: 160,
        };
        assert!(!pairing_expired(&pairing, 159));
        assert!(pairing_expired(&pairing, 160));
    }

    #[test]
    fn first_approval_becomes_owner_and_persists_allowlist() {
        let root = unique_temp_dir("assistant-telegram-owner");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = TelegramConfig {
            enabled: true,
            bot_token: "123:token".into(),
            bot_token_file: String::new(),
            poll_timeout_secs: 1,
            owner_user_id: None,
            allowed_user_ids: Vec::new(),
            pairing_enabled: true,
            pairing_code_ttl_minutes: 10,
            api_base_url: "https://api.telegram.org".into(),
        };

        let pending = create_pending_pairing(
            &store,
            &TelegramUpdate {
                update_id: 1,
                user_id: 42,
                chat_id: 42,
                chat_type: "private".into(),
                username: "dbong".into(),
                first_name: "David".into(),
                text: Some("hello".into()),
            },
            10,
        )
        .unwrap();

        let approved = approve_pairing_code(&paths, &store, &config, &pending.code).unwrap();
        assert_eq!(approved.user_id, 42);

        let status = runtime_status(&store, &config).unwrap();
        assert_eq!(status.allowed_user_ids, vec![42]);
    }

    #[test]
    fn onboarding_session_round_trip_persists_and_deletes() {
        let root = unique_temp_dir("assistant-telegram-onboard-session");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let session = TelegramOnboardingSession {
            user_id: 42,
            chat_id: 99,
            stage: "user_name".into(),
            assistant_name: "Ayaka".into(),
            assistant_style: "direct".into(),
            user_name: "HardCoder".into(),
            user_role: "Builder".into(),
            about: "Builds local AI systems".into(),
            goals: "Fix the assistant".into(),
            preferences: "concise".into(),
        };

        save_onboarding_session(&store, &session).unwrap();
        let loaded = fetch_onboarding_session(&store, 42).unwrap().unwrap();
        assert_eq!(loaded, session);

        assert!(delete_onboarding_session(&store, 42).unwrap());
        assert!(fetch_onboarding_session(&store, 42).unwrap().is_none());
    }

    #[test]
    fn onboarding_flow_updates_identity_and_profile_files() {
        let root = unique_temp_dir("assistant-telegram-onboard-flow");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = AppConfig::load(&paths).unwrap();
        let update = TelegramUpdate {
            update_id: 7,
            user_id: 42,
            chat_id: 42,
            chat_type: "private".into(),
            username: "davidb2021".into(),
            first_name: "David".into(),
            text: Some("/onboard".into()),
        };

        let started = handle_onboarding_command(&paths, &store, &config, &update, "/onboard")
            .unwrap()
            .unwrap();
        assert!(started.contains("Identity onboarding started."));

        let prompts = [
            ("Ayaka", "2/6 Assistant style"),
            ("direct, practical, concise", "3/6 Your name"),
            ("HardCoder", "4/6 Your role"),
            ("Builder", "5/6 What should I know about you?"),
            ("Builds local AI systems", "6/6 Current goals"),
            (
                "Fix the Telegram assistant",
                "Final step: preferred reply style",
            ),
            ("direct, concise, practical", "Onboarding saved."),
        ];

        for (message, expected) in prompts {
            let reply = handle_onboarding_reply(&paths, &store, &config, &update, message)
                .unwrap()
                .unwrap();
            assert!(
                reply.contains(expected),
                "missing `{expected}` in `{reply}`"
            );
        }

        let identity_json = fs::read_to_string(paths.config_dir.join("identity.json")).unwrap();
        assert!(identity_json.contains("\"name\": \"Ayaka\""));
        assert!(identity_json.contains("direct, practical, concise"));

        let profile = fs::read_to_string(paths.profiles_dir.join("assistant.md")).unwrap();
        assert!(profile.contains("Name: Ayaka"));
        assert!(profile.contains("## User Profile"));
        assert!(profile.contains("Name: HardCoder"));
        assert!(profile.contains("Telegram: @davidb2021"));
        assert!(profile.contains("Role: Builder"));
        assert!(profile.contains("Fix the Telegram assistant"));
        assert!(fetch_onboarding_session(&store, 42).unwrap().is_none());
    }

    #[test]
    fn complete_onboarding_writes_identity_and_profile() {
        let root = unique_temp_dir("assistant-telegram-complete");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let config = AppConfig::load(&paths).unwrap();
        let update = TelegramUpdate {
            update_id: 1,
            user_id: 42,
            chat_id: 42,
            chat_type: "private".into(),
            username: "davidb2021".into(),
            first_name: "David".into(),
            text: Some("hello".into()),
        };
        let session = TelegramOnboardingSession {
            user_id: 42,
            chat_id: 42,
            stage: "preferences".into(),
            assistant_name: "Ayaka".into(),
            assistant_style: "direct, practical, concise".into(),
            user_name: "HardCoder".into(),
            user_role: "Builder".into(),
            about: "Builds local AI systems".into(),
            goals: "Fix the Telegram assistant".into(),
            preferences: "direct, concise, practical".into(),
        };

        complete_onboarding(&paths, &config, &update, &session).unwrap();

        let identity_json = fs::read_to_string(paths.config_dir.join("identity.json")).unwrap();
        assert!(identity_json.contains("\"name\": \"Ayaka\""));
        assert!(identity_json.contains("direct, practical, concise"));

        let profile = fs::read_to_string(paths.profiles_dir.join("assistant.md")).unwrap();
        assert!(profile.contains("Name: Ayaka"));
        assert!(profile.contains("Name: HardCoder"));
        assert!(profile.contains("Telegram: @davidb2021"));
    }

    #[test]
    fn long_plain_text_reply_splits_into_ordered_chunks() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";
        let chunks = split_reply_chunks(text, 20);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 20));
        assert!(chunks[0].contains("alpha"));
        assert!(chunks.last().unwrap().contains("mu"));
    }

    #[test]
    fn long_fenced_code_reply_reopens_fences_per_chunk() {
        let text = "```python\nfor i in range(5):\n    print(i)\n    print(i + 1)\n    print(i + 2)\n```";
        let chunks = split_reply_chunks(text, 35);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|chunk| chunk.starts_with("```")));
        assert!(chunks.iter().all(|chunk| chunk.ends_with("```")));
    }
}
