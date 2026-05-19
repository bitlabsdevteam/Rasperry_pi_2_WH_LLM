use crate::{
    adapters::storage::SqliteStore,
    config::AppConfig,
    util::{now_epoch, sql_escape},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Direct,
    Group,
    Device,
    Background,
}

impl SessionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Group => "group",
            Self::Device => "device",
            Self::Background => "background",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "group" => Self::Group,
            "device" => Self::Device,
            "background" => Self::Background,
            _ => Self::Direct,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActivationMode {
    Direct,
    MentionOnly,
    PushToTalk,
    Scheduled,
}

impl ActivationMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::MentionOnly => "mention_only",
            Self::PushToTalk => "push_to_talk",
            Self::Scheduled => "scheduled",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "mention_only" => Self::MentionOnly,
            "push_to_talk" => Self::PushToTalk,
            "scheduled" => Self::Scheduled,
            _ => Self::Direct,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplyPolicy {
    Immediate,
    Debounced,
    FinalOnly,
    Silent,
}

impl ReplyPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Immediate => "immediate",
            Self::Debounced => "debounced",
            Self::FinalOnly => "final_only",
            Self::Silent => "silent",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "debounced" => Self::Debounced,
            "final_only" => Self::FinalOnly,
            "silent" => Self::Silent,
            _ => Self::Immediate,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantState {
    Idle,
    Queued,
    Thinking,
    Acting,
    Waiting,
    Responding,
    Failed,
}

impl AssistantState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::Thinking => "thinking",
            Self::Acting => "acting",
            Self::Waiting => "waiting",
            Self::Responding => "responding",
            Self::Failed => "failed",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "queued" => Self::Queued,
            "thinking" => Self::Thinking,
            "acting" => Self::Acting,
            "waiting" => Self::Waiting,
            "responding" => Self::Responding,
            "failed" => Self::Failed,
            _ => Self::Idle,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolPolicy {
    pub allowlisted_commands: Vec<String>,
    pub allow_memory_write: bool,
    pub trusted: bool,
}

impl ToolPolicy {
    pub fn allows_command(&self, command: &str) -> bool {
        self.allowlisted_commands.iter().any(|item| item == command)
    }

    pub fn encode(&self) -> String {
        format!(
            "trusted={};memory_write={};commands={}",
            self.trusted,
            self.allow_memory_write,
            self.allowlisted_commands.join(",")
        )
    }

    fn decode(value: &str) -> Self {
        let mut trusted = false;
        let mut allow_memory_write = false;
        let mut allowlisted_commands = Vec::new();
        for part in value.split(';') {
            let mut pair = part.splitn(2, '=');
            let key = pair.next().unwrap_or_default().trim();
            let raw = pair.next().unwrap_or_default().trim();
            match key {
                "trusted" => trusted = raw == "true",
                "memory_write" => allow_memory_write = raw == "true",
                "commands" => {
                    allowlisted_commands = raw
                        .split(',')
                        .filter(|item| !item.trim().is_empty())
                        .map(|item| item.trim().to_string())
                        .collect();
                }
                _ => {}
            }
        }
        Self {
            allowlisted_commands,
            allow_memory_write,
            trusted,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelPolicy {
    pub task: String,
    pub fallback_order: Vec<String>,
    pub allow_degraded_fallback: bool,
}

impl ModelPolicy {
    pub fn encode(&self) -> String {
        format!(
            "task={};fallback={};degraded={}",
            self.task,
            self.fallback_order.join(","),
            self.allow_degraded_fallback
        )
    }

    fn decode(value: &str) -> Self {
        let mut task = "general".to_string();
        let mut fallback_order = Vec::new();
        let mut allow_degraded_fallback = true;
        for part in value.split(';') {
            let mut pair = part.splitn(2, '=');
            let key = pair.next().unwrap_or_default().trim();
            let raw = pair.next().unwrap_or_default().trim();
            match key {
                "task" => task = raw.to_string(),
                "fallback" => {
                    fallback_order = raw
                        .split(',')
                        .filter(|item| !item.trim().is_empty())
                        .map(|item| item.trim().to_string())
                        .collect();
                }
                "degraded" => allow_degraded_fallback = raw != "false",
                _ => {}
            }
        }
        Self {
            task,
            fallback_order,
            allow_degraded_fallback,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    pub session_id: String,
    pub surface: String,
    pub peer_id: Option<String>,
    pub chat_id: Option<String>,
    pub session_kind: SessionKind,
    pub activation_mode: ActivationMode,
    pub reply_policy: ReplyPolicy,
    pub tool_policy: ToolPolicy,
    pub model_policy: ModelPolicy,
    pub state: AssistantState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionSummary {
    pub session: Session,
    pub last_message_at: Option<i64>,
    pub updated_at: i64,
}

impl Session {
    pub fn cli(session_id: &str, config: &AppConfig) -> Self {
        Self {
            session_id: session_id.to_string(),
            surface: "cli".to_string(),
            peer_id: Some("local-user".to_string()),
            chat_id: None,
            session_kind: SessionKind::Direct,
            activation_mode: ActivationMode::Direct,
            reply_policy: ReplyPolicy::Immediate,
            tool_policy: ToolPolicy {
                allowlisted_commands: config.tools.allowlist.clone(),
                allow_memory_write: true,
                trusted: true,
            },
            model_policy: default_model_policy("general"),
            state: AssistantState::Idle,
        }
    }

    pub fn telegram_dm(user_id: i64, chat_id: i64, config: &AppConfig) -> Self {
        let trusted = config.telegram.owner_user_id == Some(user_id)
            || config.telegram.allowed_user_ids.contains(&user_id);
        Self {
            session_id: format!("telegram:dm:{user_id}"),
            surface: "telegram".to_string(),
            peer_id: Some(user_id.to_string()),
            chat_id: Some(chat_id.to_string()),
            session_kind: SessionKind::Direct,
            activation_mode: ActivationMode::Direct,
            reply_policy: if config.messages.queue.enabled {
                ReplyPolicy::Debounced
            } else {
                ReplyPolicy::Immediate
            },
            tool_policy: ToolPolicy {
                allowlisted_commands: config.tools.allowlist.clone(),
                allow_memory_write: trusted,
                trusted,
            },
            model_policy: default_model_policy("general"),
            state: AssistantState::Idle,
        }
    }

    pub fn voice(session_id: &str, config: &AppConfig) -> Self {
        Self {
            session_id: session_id.to_string(),
            surface: "voice".to_string(),
            peer_id: None,
            chat_id: None,
            session_kind: SessionKind::Device,
            activation_mode: ActivationMode::PushToTalk,
            reply_policy: ReplyPolicy::FinalOnly,
            tool_policy: ToolPolicy {
                allowlisted_commands: config.tools.allowlist.clone(),
                allow_memory_write: true,
                trusted: true,
            },
            model_policy: default_model_policy("voice"),
            state: AssistantState::Idle,
        }
    }
}

pub fn default_model_policy(task: &str) -> ModelPolicy {
    ModelPolicy {
        task: task.to_string(),
        fallback_order: vec!["deterministic".into(), "degraded".into()],
        allow_degraded_fallback: true,
    }
}

pub fn infer_task_type(message: &str, surface: &str) -> String {
    let normalized = message.to_ascii_lowercase();
    if surface == "voice" {
        return "voice".to_string();
    }
    if normalized.contains("code")
        || normalized.contains("rust")
        || normalized.contains("python")
        || normalized.contains("typescript")
    {
        return "coding".to_string();
    }
    if normalized.contains("summarize") || normalized.contains("summary") {
        return "summarization".to_string();
    }
    if normalized.starts_with("classify ") || normalized.contains("classify") {
        return "classification".to_string();
    }
    "general".to_string()
}

pub fn upsert_session(store: &SqliteStore, session: &Session) -> Result<(), String> {
    let now = now_epoch();
    store.exec(&format!(
        "INSERT OR REPLACE INTO sessions
         (session_id, surface, peer_id, chat_id, session_kind, activation_mode, reply_policy,
          tool_policy, model_policy, state, last_message_at, updated_at)
         VALUES ('{}', '{}', {}, {}, '{}', '{}', '{}', '{}', '{}', '{}',
                 COALESCE((SELECT last_message_at FROM sessions WHERE session_id = '{}'), NULL),
                 {});
        ",
        sql_escape(&session.session_id),
        sql_escape(&session.surface),
        optional_sql_text(&session.peer_id),
        optional_sql_text(&session.chat_id),
        session.session_kind.as_str(),
        session.activation_mode.as_str(),
        session.reply_policy.as_str(),
        sql_escape(&session.tool_policy.encode()),
        sql_escape(&session.model_policy.encode()),
        session.state.as_str(),
        sql_escape(&session.session_id),
        now
    ))
}

pub fn touch_session(store: &SqliteStore, session_id: &str, state: AssistantState) -> Result<(), String> {
    let now = now_epoch();
    store.exec(&format!(
        "UPDATE sessions
         SET state = '{}',
             last_message_at = {},
             updated_at = {}
         WHERE session_id = '{}';",
        state.as_str(),
        now,
        now,
        sql_escape(session_id)
    ))
}

pub fn set_session_state(
    store: &SqliteStore,
    session_id: &str,
    state: AssistantState,
) -> Result<(), String> {
    let now = now_epoch();
    store.exec(&format!(
        "UPDATE sessions
         SET state = '{}',
             updated_at = {}
         WHERE session_id = '{}';",
        state.as_str(),
        now,
        sql_escape(session_id)
    ))
}

pub fn fetch_session(store: &SqliteStore, session_id: &str) -> Result<Option<SessionSummary>, String> {
    Ok(store
        .query(&format!(
            "SELECT session_id, surface, COALESCE(peer_id, ''), COALESCE(chat_id, ''),
                    session_kind, activation_mode, reply_policy, tool_policy, model_policy,
                    state, COALESCE(last_message_at, ''), updated_at
             FROM sessions
             WHERE session_id = '{}'
             LIMIT 1;",
            sql_escape(session_id)
        ))?
        .into_iter()
        .next()
        .and_then(|row| parse_session_row(&row)))
}

pub fn list_sessions(store: &SqliteStore) -> Result<Vec<SessionSummary>, String> {
    Ok(store
        .query(
            "SELECT session_id, surface, COALESCE(peer_id, ''), COALESCE(chat_id, ''),
                    session_kind, activation_mode, reply_policy, tool_policy, model_policy,
                    state, COALESCE(last_message_at, ''), updated_at
             FROM sessions
             ORDER BY COALESCE(last_message_at, updated_at) DESC, session_id ASC;",
        )?
        .into_iter()
        .filter_map(|row| parse_session_row(&row))
        .collect())
}

fn parse_session_row(row: &[String]) -> Option<SessionSummary> {
    if row.len() < 12 {
        return None;
    }
    Some(SessionSummary {
        session: Session {
            session_id: row[0].clone(),
            surface: row[1].clone(),
            peer_id: empty_to_none(&row[2]),
            chat_id: empty_to_none(&row[3]),
            session_kind: SessionKind::from_str(&row[4]),
            activation_mode: ActivationMode::from_str(&row[5]),
            reply_policy: ReplyPolicy::from_str(&row[6]),
            tool_policy: ToolPolicy::decode(&row[7]),
            model_policy: ModelPolicy::decode(&row[8]),
            state: AssistantState::from_str(&row[9]),
        },
        last_message_at: empty_to_none(&row[10]).and_then(|value| value.parse::<i64>().ok()),
        updated_at: row[11].parse().ok()?,
    })
}

fn optional_sql_text(value: &Option<String>) -> String {
    value
        .as_ref()
        .filter(|item| !item.trim().is_empty())
        .map(|item| format!("'{}'", sql_escape(item)))
        .unwrap_or_else(|| "NULL".to_string())
}

fn empty_to_none(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        adapters::storage::SqliteStore,
        config::AssistantPaths,
        util::unique_temp_dir,
    };

    use super::{AssistantState, Session, fetch_session, set_session_state, upsert_session};

    #[test]
    fn session_registry_persists_stateful_sessions() {
        let root = unique_temp_dir("assistant-session-store");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let store = SqliteStore::new(&paths).unwrap();
        let config = crate::config::AppConfig::load(&paths).unwrap();
        let session = Session::cli("default", &config);

        upsert_session(&store, &session).unwrap();
        set_session_state(&store, "default", AssistantState::Queued).unwrap();

        let stored = fetch_session(&store, "default").unwrap().unwrap();
        assert_eq!(stored.session.surface, "cli");
        assert_eq!(stored.session.state, AssistantState::Queued);
    }
}
