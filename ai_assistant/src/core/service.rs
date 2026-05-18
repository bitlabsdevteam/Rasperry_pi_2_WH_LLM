use crate::{
    adapters::{llama_cpp::LlamaCppAdapter, storage::SqliteStore},
    config::{AppConfig, AssistantPaths},
    core::{
        context::maybe_compact,
        harness::{HarnessInput, build_prompt},
        identity::IdentityProfile,
        memory::{recent_turns, record_turn, search_memories},
        skills::{list_skills, select_skills, skill_prompt_context},
        tasks::list_tasks,
    },
    util::truncate,
};

#[derive(Clone, Debug)]
pub struct ChatOutput {
    pub response: String,
    pub compaction: Option<String>,
}

pub fn run_chat_session(
    paths: &AssistantPaths,
    config: &AppConfig,
    store: &SqliteStore,
    session: &str,
    message: &str,
    stream: bool,
) -> Result<ChatOutput, String> {
    let identity = IdentityProfile::load(paths, &config.identity)?;
    let recent = recent_turns(store, session, config.memory.recent_turn_limit)?;
    let prompt_budget = prompt_budget(config);
    record_turn(paths, store, session, "user", message)?;
    let selected_skills = select_skills(paths, message, 3)?;

    if let Some(response) = capability_response(paths, config, &identity, message)? {
        record_turn(paths, store, session, "assistant", &response)?;
        let compaction = maybe_compact(paths, store, session, &config.memory)?;
        return Ok(ChatOutput {
            response,
            compaction,
        });
    }

    let prompt = build_prompt(&HarnessInput {
        identity_name: identity.name.clone(),
        identity_style: identity.style.clone(),
        identity_profile: identity.markdown_profile.clone(),
        system_instruction: identity.system_instruction.clone(),
        prefer_code_output: asks_for_code_request(message),
        user_intent: message.to_string(),
        context_snippets: vec![
            format!("session={session}"),
            "runtime=offline-first".to_string(),
            "execution=deterministic".to_string(),
        ],
        memories: search_memories(store, message, config.memory.memory_search_limit)?,
        tool_context: config
            .tools
            .allowlist
            .iter()
            .map(|tool| format!("allowlisted command: {tool}"))
            .collect(),
        skill_context: skill_prompt_context(&selected_skills),
        tasks: list_tasks(store)?
            .into_iter()
            .filter(|task| task.status != "done")
            .take(5)
            .collect(),
        safety_rules: vec![
            "Do not rely on cloud services.".into(),
            "Prefer minimal output on constrained hardware.".into(),
            "If llama.cpp is unreachable, return a local degraded response.".into(),
        ],
        recent_messages: recent,
        token_budget: prompt_budget,
    });

    let adapter = LlamaCppAdapter::new(config.llm.clone());
    let response = match adapter.infer_chat(&prompt, message, stream) {
        Ok(value) => {
            let primary = sanitize_response(&value);
            if is_low_quality_response(message, &primary) {
                recover_response(&adapter, &identity, message)
            } else {
                primary
            }
        }
        Err(error) => degraded_response(&error, message),
    };

    record_turn(paths, store, session, "assistant", &response)?;
    let compaction = maybe_compact(paths, store, session, &config.memory)?;

    Ok(ChatOutput {
        response,
        compaction,
    })
}

fn capability_response(
    paths: &AssistantPaths,
    config: &AppConfig,
    identity: &IdentityProfile,
    message: &str,
) -> Result<Option<String>, String> {
    let normalized = message.to_ascii_lowercase();
    if let Some(response) = deterministic_code_response(&normalized) {
        return Ok(Some(response));
    }
    if asks_about_known_user(&normalized) {
        return Ok(Some(render_known_user_response(
            &identity.known_user_facts(),
        )));
    }
    if is_greeting_message(&normalized) {
        return Ok(Some(
            "Hello. I’m online locally. Ask one question and I’ll keep the reply short."
                .to_string(),
        ));
    }
    if asks_about_ml_ai(&normalized) {
        return Ok(Some("AI is the broad field of building systems that perform tasks requiring human-like intelligence. ML is a subset of AI where models learn patterns from data to make predictions or decisions.".to_string()));
    }
    if asks_about_internet(&normalized) {
        return Ok(Some(format!(
            "No internet search tool is configured right now. I can use local allowlisted commands only: {}. Add a dedicated search tool before asking me to browse the web.",
            render_tool_list(&config.tools.allowlist)
        )));
    }
    if asks_about_tools(&normalized) {
        let skills = list_skills(paths)?;
        let skill_summary = if skills.is_empty() {
            "no installed skills".to_string()
        } else {
            format!(
                "{} installed skill(s): {}",
                skills.len(),
                skills
                    .iter()
                    .take(5)
                    .map(|skill| skill.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        return Ok(Some(format!(
            "I can use these local allowlisted commands: {}. I also have {}.",
            render_tool_list(&config.tools.allowlist),
            skill_summary
        )));
    }
    Ok(None)
}

fn prompt_budget(config: &AppConfig) -> usize {
    let available = config
        .llm
        .context_size
        .saturating_sub(config.llm.predict_tokens)
        .saturating_sub(96);
    config.memory.token_budget.min(available.max(128))
}

fn degraded_response(error: &str, message: &str) -> String {
    if error.contains("exceeds the available context size") {
        return "I hit a local context limit. Please resend a shorter message while I keep the chat responsive.".to_string();
    }

    format!(
        "Local LLM unavailable; stored the turn and kept the assistant responsive.\n\nReason: {}\n\nPrompt digest: {}",
        truncate(error, 160),
        truncate(message, 160)
    )
}

fn sanitize_response(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "I don't have a useful local answer yet.".to_string();
    }
    if trimmed.contains("exceeds the available context size") {
        return "I hit a local context limit. Please resend a shorter message while I keep the chat responsive.".to_string();
    }

    let cleaned = trimmed
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.is_empty()
                && !line.starts_with("You are ")
                && !line.starts_with("Assistant name:")
                && !line.starts_with("Reply to the user directly.")
                && !line.starts_with("Style:")
                && !line.starts_with("Operate fully offline after deployment")
                && !line.starts_with("Answer the user's latest message directly.")
                && !line.starts_with("Do not repeat the prompt")
                && !line.starts_with("Keep the reply short and useful for Telegram.")
                && !line.starts_with("Identity notes:")
                && !line.starts_with("Recent context:")
                && !line.starts_with("Relevant memory:")
                && !line.starts_with("Pending tasks:")
                && !line.starts_with("Available tools:")
                && !line.starts_with("Available skills:")
                && !line.starts_with("Safety rules:")
                && !line.starts_with("User:")
                && !line.starts_with("Assistant:")
                && !line.starts_with("Name: ")
                && !line.starts_with("Question:")
                && !line.starts_with("Answer:")
                && line != "Purpose:"
                && line != "Communication:"
                && !line.starts_with("## ")
                && !line.starts_with("# ")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    let normalized = normalize_response(&strip_wrapping_quotes(&cleaned));
    let collapsed = collapse_repetition(&normalized);

    if collapsed.is_empty() {
        "I don't have a useful local answer yet.".to_string()
    } else {
        collapsed
    }
}

fn collapse_repetition(text: &str) -> String {
    if looks_like_code_response(text) {
        return text.trim().to_string();
    }
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return normalized;
    }

    let sentences = split_sentences(&normalized);
    if sentences.len() >= 3
        && sentences
            .iter()
            .all(|sentence| sentence.eq_ignore_ascii_case(&sentences[0]))
    {
        return sentences[0].clone();
    }

    normalized
}

fn recover_response(
    adapter: &LlamaCppAdapter,
    identity: &IdentityProfile,
    message: &str,
) -> String {
    if let Some(response) = deterministic_response(message) {
        return response;
    }

    let retry_prompt = if asks_for_code_request(message) {
        format!(
            "You are {}.\nStyle: {}.\nThe user asked for code.\nReturn runnable code with correct indentation.\nUse a fenced code block.\nKeep any explanation to one short line after the code.\nQuestion: {}\nAnswer:",
            identity.name.trim(),
            identity.style.trim(),
            message.trim()
        )
    } else {
        format!(
            "You are {}.\nStyle: {}.\nReply in plain text only.\nUse 1-3 short sentences.\nDo not use markdown, code fences, logs, or emojis unless the user asked for them.\nQuestion: {}\nAnswer:",
            identity.name.trim(),
            identity.style.trim(),
            message.trim()
        )
    };
    if let Ok(value) = adapter.infer(&retry_prompt, false) {
        let cleaned = sanitize_response(&value);
        if !is_low_quality_response(message, &cleaned) {
            return cleaned;
        }
    }

    fallback_response(message)
}

fn deterministic_response(message: &str) -> Option<String> {
    let normalized = message.to_ascii_lowercase();
    if let Some(response) = deterministic_code_response(&normalized) {
        return Some(response);
    }
    if is_greeting_message(&normalized) {
        return Some(
            "Hello. I’m online locally. Ask one question and I’ll keep the reply short."
                .to_string(),
        );
    }
    if asks_about_ml_ai(&normalized) {
        return Some("AI is the broad field of building systems that perform tasks requiring human-like intelligence. ML is a subset of AI where models learn patterns from data to make predictions or decisions.".to_string());
    }
    None
}

fn fallback_response(message: &str) -> String {
    if let Some(response) = deterministic_response(message) {
        return response;
    }
    if message.trim_end().ends_with('?') {
        "The last local reply came out malformed. Ask again in one short sentence and I’ll answer more directly.".to_string()
    } else {
        "The last local reply came out malformed. Please resend the request in one short sentence."
            .to_string()
    }
}

fn is_low_quality_response(message: &str, response: &str) -> bool {
    let trimmed = response.trim();
    if trimmed.is_empty() || trimmed == "I don't have a useful local answer yet." {
        return true;
    }

    let lower = trimmed.to_ascii_lowercase();
    if asks_for_code_request(message) && !looks_like_valid_code_response(trimmed) {
        return true;
    }
    if lower.contains("compacted session `")
        || lower.contains("summary length=")
        || lower.contains("prompt digest:")
    {
        return true;
    }
    if looks_like_unsolicited_code(message, trimmed) {
        return true;
    }
    if has_symbol_spam(trimmed) {
        return true;
    }
    if message.trim_end().ends_with('?')
        && starts_with_greeting(&lower)
        && keyword_overlap(message, trimmed) == 0
    {
        return true;
    }

    false
}

fn looks_like_unsolicited_code(message: &str, response: &str) -> bool {
    let lower_message = message.to_ascii_lowercase();
    let user_asked_for_code = asks_for_code_request(&lower_message);
    if user_asked_for_code {
        return false;
    }

    response.contains("```")
        || [
            "import ", "def ", "fn ", "class ", "const ", "let ", "logging.", "#include",
        ]
        .iter()
        .filter(|needle| response.contains(**needle))
        .count()
            >= 2
}

fn normalize_response(text: &str) -> String {
    if looks_like_code_response(text) {
        return text
            .lines()
            .map(|line| line.trim_end())
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();
    }
    text.trim().to_string()
}

fn looks_like_code_response(text: &str) -> bool {
    text.contains("```")
        || text
            .lines()
            .filter(|line| is_code_line(line.trim()))
            .count()
            >= 2
        || text.lines().any(|line| line.starts_with("    "))
}

fn looks_like_valid_code_response(text: &str) -> bool {
    if text.contains("```") {
        return true;
    }
    let lines = text.lines().collect::<Vec<_>>();
    let code_lines = lines
        .iter()
        .filter(|line| is_code_line(line.trim()) || line.starts_with("    "))
        .count();
    code_lines >= 2 && lines.len() >= 2
}

fn is_code_line(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }
    [
        "def ", "class ", "for ", "while ", "if ", "elif ", "else:", "print(", "return ",
        "import ", "from ", "let ", "const ", "fn ", "public ", "private ",
    ]
    .iter()
    .any(|prefix| line.starts_with(prefix))
}

fn has_symbol_spam(text: &str) -> bool {
    let mut last = '\0';
    let mut run = 0usize;
    for ch in text.chars() {
        if !ch.is_alphanumeric() && !ch.is_whitespace() {
            if ch == last {
                run += 1;
            } else {
                last = ch;
                run = 1;
            }
            if run >= 4 {
                return true;
            }
        } else {
            last = '\0';
            run = 0;
        }
    }
    false
}

fn keyword_overlap(message: &str, response: &str) -> usize {
    important_terms(message)
        .into_iter()
        .filter(|term| response.to_ascii_lowercase().contains(term))
        .count()
}

fn important_terms(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .map(|term| term.trim().to_ascii_lowercase())
        .filter(|term| term.len() >= 2)
        .filter(|term| {
            !matches!(
                term.as_str(),
                "a" | "an"
                    | "and"
                    | "are"
                    | "can"
                    | "do"
                    | "for"
                    | "hello"
                    | "hey"
                    | "hi"
                    | "how"
                    | "i"
                    | "is"
                    | "it"
                    | "me"
                    | "of"
                    | "on"
                    | "or"
                    | "please"
                    | "tell"
                    | "the"
                    | "to"
                    | "what"
                    | "when"
                    | "where"
                    | "who"
                    | "why"
                    | "you"
            )
        })
        .collect()
}

fn strip_wrapping_quotes(value: &str) -> String {
    let mut trimmed = value.trim().to_string();
    if !(trimmed.starts_with("```") && trimmed.ends_with("```")) {
        trimmed = trimmed.trim_matches('`').trim().to_string();
    }
    if trimmed.starts_with('"') && !trimmed.ends_with('"') {
        trimmed.remove(0);
    }
    if trimmed.ends_with('"') && !trimmed.starts_with('"') {
        trimmed.pop();
    }
    trimmed.trim().to_string()
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            let sentence = current.trim();
            if !sentence.is_empty() {
                sentences.push(sentence.to_string());
            }
            current.clear();
        }
    }
    let tail = current.trim();
    if !tail.is_empty() {
        sentences.push(tail.to_string());
    }
    sentences
}

fn is_greeting_message(message: &str) -> bool {
    matches!(
        message.trim(),
        "hi" | "hello" | "hey" | "yo" | "hiya" | "good morning" | "good afternoon" | "good evening"
    )
}

fn asks_about_ml_ai(message: &str) -> bool {
    (message.contains("what is ml")
        || message.contains("what's ml")
        || message.contains("what is ai")
        || message.contains("what's ai")
        || message.contains("machine learning")
        || message.contains("artificial intelligence"))
        && (message.contains(" ai") || message.contains("ml") || message.contains("machine"))
}

fn asks_for_code_request(message: &str) -> bool {
    [
        "code",
        "python",
        "javascript",
        "typescript",
        "rust",
        "java",
        "c++",
        "c#",
        "script",
        "function",
        "class",
        "program",
        "snippet",
        "for loop",
        "while loop",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn deterministic_code_response(message: &str) -> Option<String> {
    if (message.contains("python") || message.contains("py")) && message.contains("for loop") {
        return Some("```python\nfor i in range(5):\n    print(i)\n```".to_string());
    }
    None
}

fn asks_about_known_user(message: &str) -> bool {
    [
        "who am i",
        "do you know me",
        "what do you know about me",
        "tell me about me",
        "what is my name",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn render_known_user_response(facts: &[String]) -> String {
    if facts.is_empty() {
        return "Not yet. I do not have a saved user profile yet, so rerun onboarding and fill in your details.".to_string();
    }
    format!("I know this about you: {}.", facts.join("; "))
}

fn starts_with_greeting(message: &str) -> bool {
    ["hello", "hey", "hi", "greetings"]
        .iter()
        .any(|prefix| message.starts_with(prefix))
}

fn asks_about_tools(message: &str) -> bool {
    let asks = [
        "do you have",
        "what tools",
        "which tools",
        "available tools",
    ];
    asks.iter().any(|needle| message.contains(needle))
        && (message.contains("tool") || message.contains("skills"))
}

fn asks_about_internet(message: &str) -> bool {
    (message.contains("search") || message.contains("browse"))
        && (message.contains("internet") || message.contains("web") || message.contains("online"))
}

fn render_tool_list(allowlist: &[String]) -> String {
    if allowlist.is_empty() {
        "none".to_string()
    } else {
        allowlist.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::config::{
        AppConfig, AssistantPaths, IdentityConfig, LlmConfig, MemoryConfig, SchedulerConfig,
        TelegramConfig, ToolConfig,
    };
    use crate::core::identity::IdentityProfile;
    use crate::util::unique_temp_dir;

    use super::{
        asks_about_ml_ai, capability_response, collapse_repetition, degraded_response,
        deterministic_response, is_low_quality_response, prompt_budget, sanitize_response,
    };

    #[test]
    fn sanitize_response_strips_prompt_scaffold() {
        let cleaned = sanitize_response(
            "Name: Kumo\nPurpose:\n- Serve locally\nUser: hello\nAssistant:\nHello there.",
        );
        assert_eq!(cleaned, "- Serve locally Hello there.");
    }

    #[test]
    fn sanitize_response_maps_context_errors_to_user_friendly_text() {
        let cleaned = sanitize_response(
            "Error: request (320 tokens) exceeds the available context size (256 tokens)",
        );
        assert!(cleaned.contains("local context limit"));
    }

    #[test]
    fn prompt_budget_is_capped_by_available_context() {
        let config = AppConfig {
            llm: LlmConfig {
                prefer_http: false,
                endpoint: String::new(),
                health_endpoint: String::new(),
                model: "mock".into(),
                binary_path: PathBuf::from("/tmp/llama-cli").display().to_string(),
                model_path: PathBuf::from("/tmp/model.gguf").display().to_string(),
                threads: 1,
                context_size: 256,
                predict_tokens: 64,
                timeout_secs: 1,
                retries: 0,
                stream: false,
            },
            memory: MemoryConfig {
                recent_turn_limit: 4,
                compact_after_turns: 12,
                retain_recent_turns: 6,
                token_budget: 512,
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
                poll_timeout_secs: 30,
                owner_user_id: None,
                allowed_user_ids: vec![],
                pairing_enabled: true,
                pairing_code_ttl_minutes: 15,
                api_base_url: "https://api.telegram.org".into(),
            },
        };
        assert_eq!(prompt_budget(&config), 128);
    }

    #[test]
    fn degraded_response_maps_context_errors() {
        let response =
            degraded_response("request exceeds the available context size", "hello there");
        assert!(response.contains("local context limit"));
    }

    #[test]
    fn sanitize_response_strips_unmatched_wrapping_quote() {
        let cleaned = sanitize_response("\"Hey there!");
        assert_eq!(cleaned, "Hey there!");
    }

    #[test]
    fn sanitize_response_preserves_multiline_code_blocks() {
        let cleaned = sanitize_response("```python\nfor i in range(3):\n    print(i)\n```");
        assert_eq!(cleaned, "```python\nfor i in range(3):\n    print(i)\n```");
    }

    #[test]
    fn low_quality_response_detects_unsolicited_code_and_logs() {
        assert!(is_low_quality_response(
            "what is ml + ai?",
            "```python\nimport logging\nlogging.basicConfig(level=logging.DEBUG)\n```"
        ));
        assert!(is_low_quality_response(
            "hello",
            "compacted session `telegram:dm:42` from 12 turns to 6. summary length=525 chars"
        ));
        assert!(is_low_quality_response(
            "create me a python code with a for loop",
            "python def greet_user(): Converts the user's name to a string and returns it as"
        ));
    }

    #[test]
    fn collapse_repetition_reduces_identical_sentences() {
        assert_eq!(collapse_repetition("Hello! Hello! Hello! Hello!"), "Hello!");
    }

    #[test]
    fn capability_response_reports_local_tools_for_tool_questions() {
        let root = unique_temp_dir("assistant-capability-tools");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        crate::core::skills::create_skill(
            &paths,
            "daily-note",
            "Capture a note.",
            &["note".into()],
            &["echo".into()],
            "Append a note.",
            &[],
        )
        .unwrap();

        let config = base_config_with_tools(vec!["date".into(), "echo".into()]);
        let identity = IdentityProfile::load(&paths, &config.identity).unwrap();
        let response = capability_response(
            &paths,
            &config,
            &identity,
            "Do you have any tools to use now?",
        )
        .unwrap()
        .unwrap();

        assert!(response.contains("date, echo"));
        assert!(response.contains("installed skill"));
    }

    #[test]
    fn capability_response_reports_no_internet_search_tool() {
        let root = unique_temp_dir("assistant-capability-internet");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        let config = base_config_with_tools(vec!["date".into()]);
        let identity = IdentityProfile::load(&paths, &config.identity).unwrap();

        let response = capability_response(&paths, &config, &identity, "Can you search internet?")
            .unwrap()
            .unwrap();

        assert!(response.contains("No internet search tool is configured"));
        assert!(response.contains("date"));
    }

    #[test]
    fn capability_response_short_circuits_greetings() {
        let response = capability_response(
            &AssistantPaths::new(PathBuf::from("/tmp")),
            &AppConfig {
                llm: LlmConfig {
                    prefer_http: false,
                    endpoint: String::new(),
                    health_endpoint: String::new(),
                    model: "mock".into(),
                    binary_path: String::new(),
                    model_path: String::new(),
                    threads: 1,
                    context_size: 256,
                    predict_tokens: 32,
                    timeout_secs: 1,
                    retries: 0,
                    stream: false,
                },
                memory: MemoryConfig {
                    recent_turn_limit: 4,
                    compact_after_turns: 12,
                    retain_recent_turns: 6,
                    token_budget: 128,
                    memory_search_limit: 4,
                    memory_ttl_days: 30,
                },
                scheduler: SchedulerConfig {
                    poll_seconds: 30,
                    max_jobs_per_tick: 2,
                    allow_shell_jobs: false,
                },
                identity: IdentityConfig {
                    name: "Kumo".into(),
                    style: "direct".into(),
                    system_instruction: "Stay local.".into(),
                },
                tools: ToolConfig {
                    allowlist: vec!["date".into()],
                },
                telegram: TelegramConfig {
                    enabled: false,
                    bot_token: String::new(),
                    bot_token_file: String::new(),
                    poll_timeout_secs: 1,
                    owner_user_id: None,
                    allowed_user_ids: Vec::new(),
                    pairing_enabled: true,
                    pairing_code_ttl_minutes: 15,
                    api_base_url: "https://api.telegram.org".into(),
                },
            },
            &IdentityProfile {
                name: "Kumo".into(),
                style: "direct".into(),
                system_instruction: "Stay local.".into(),
                markdown_profile: "# Assistant Profile".into(),
            },
            "hello",
        )
        .unwrap()
        .unwrap();

        assert!(response.contains("Ask one question"));
    }

    #[test]
    fn ml_ai_detection_catches_common_phrasing() {
        assert!(asks_about_ml_ai("what is ml + ai?"));
        assert!(asks_about_ml_ai(
            "Explain machine learning and artificial intelligence"
        ));
    }

    #[test]
    fn deterministic_response_handles_python_for_loop_request() {
        assert_eq!(
            deterministic_response("create me a python code with a for loop").unwrap(),
            "```python\nfor i in range(5):\n    print(i)\n```"
        );
    }

    #[test]
    fn capability_response_reports_known_user_from_profile() {
        let root = unique_temp_dir("assistant-capability-user-profile");
        let paths = AssistantPaths::new(root);
        paths.ensure_defaults().unwrap();
        fs::write(
            paths.profiles_dir.join("assistant.md"),
            "# Assistant Profile

Name: Kumo

Purpose:
- Stay local

## User Profile
Name: David Bong
Telegram: @davidb2021
Role: HardCoder
Preferences:
- Prefers direct, concise replies
",
        )
        .unwrap();
        let config = base_config_with_tools(vec!["date".into()]);
        let identity = IdentityProfile::load(&paths, &config.identity).unwrap();

        let response = capability_response(&paths, &config, &identity, "Do you know me?")
            .unwrap()
            .unwrap();

        assert!(response.contains("David Bong"));
        assert!(response.contains("@davidb2021"));
        assert!(response.contains("HardCoder"));
    }

    fn base_config_with_tools(allowlist: Vec<String>) -> AppConfig {
        AppConfig {
            llm: LlmConfig {
                prefer_http: false,
                endpoint: String::new(),
                health_endpoint: String::new(),
                model: "mock".into(),
                binary_path: PathBuf::from("/tmp/llama-cli").display().to_string(),
                model_path: PathBuf::from("/tmp/model.gguf").display().to_string(),
                threads: 1,
                context_size: 256,
                predict_tokens: 64,
                timeout_secs: 1,
                retries: 0,
                stream: false,
            },
            memory: MemoryConfig {
                recent_turn_limit: 4,
                compact_after_turns: 12,
                retain_recent_turns: 6,
                token_budget: 512,
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
            tools: ToolConfig { allowlist },
            telegram: TelegramConfig {
                enabled: false,
                bot_token: String::new(),
                bot_token_file: String::new(),
                poll_timeout_secs: 30,
                owner_user_id: None,
                allowed_user_ids: vec![],
                pairing_enabled: true,
                pairing_code_ttl_minutes: 15,
                api_base_url: "https://api.telegram.org".into(),
            },
        }
    }
}
