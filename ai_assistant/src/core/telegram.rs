use std::{
    collections::BTreeSet,
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::{
    adapters::{
        storage::SqliteStore,
        telegram::{TelegramAdapter, TelegramUpdate},
    },
    config::{AppConfig, AssistantPaths, TelegramConfig, write_telegram_config},
    core::service::run_chat_session,
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

struct TypingIndicator {
    stop_tx: mpsc::Sender<()>,
    handle: JoinHandle<()>,
}

impl TypingIndicator {
    fn stop(self) {
        let _ = self.stop_tx.send(());
        let _ = self.handle.join();
    }
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

fn start_typing_indicator(adapter: TelegramAdapter, chat_id: i64) -> TypingIndicator {
    let _ = adapter.send_chat_action(chat_id, "typing");

    let (stop_tx, stop_rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        loop {
            match stop_rx.recv_timeout(Duration::from_secs(4)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = adapter.send_chat_action(chat_id, "typing");
                }
            }
        }
    });

    TypingIndicator { stop_tx, handle }
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
    let typing = start_typing_indicator(adapter.clone(), update.chat_id);
    let outcome = run_chat_session(
        paths,
        config,
        store,
        &session_key(update.user_id),
        message,
        config.llm.stream,
    )?;
    typing.stop();
    let _compaction = outcome.compaction;
    adapter.send_message(update.chat_id, &outcome.response)?;
    Ok(vec![format!("replied to {}", update.display_name())])
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
    use crate::{
        adapters::storage::SqliteStore,
        config::{AssistantPaths, TelegramConfig},
        util::unique_temp_dir,
    };

    use super::{
        TelegramUpdate, approve_pairing_code, create_pending_pairing, generate_pairing_code,
        pairing_expired, runtime_status,
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
}
