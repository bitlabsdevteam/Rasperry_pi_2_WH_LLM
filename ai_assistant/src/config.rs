use std::{
    env, fs,
    path::{Path, PathBuf},
};

use crate::util::{
    ensure_dir, parse_json_array, parse_json_bool, parse_json_string, parse_json_usize,
    read_with_fallback, write_if_missing,
};

const LLM_EXAMPLE: &str = r#"{
  "prefer_http": false,
  "endpoint": "http://127.0.0.1:8080/v1/chat/completions",
  "health_endpoint": "http://127.0.0.1:8080/health",
  "model": "smollm2-135m-instruct",
  "binary_path": "/absolute/path/to/llama-cli",
  "model_path": "/absolute/path/to/model.gguf",
  "threads": 2,
  "context_size": 4096,
  "predict_tokens": 512,
  "timeout_secs": 20,
  "retries": 2,
  "stream": true
}
"#;

const MEMORY_EXAMPLE: &str = r#"{
  "recent_turn_limit": 8,
  "compact_after_turns": 12,
  "retain_recent_turns": 6,
  "token_budget": 2048,
  "compact_context_threshold_percent": 70,
  "memory_search_limit": 6,
  "memory_ttl_days": 30
}
"#;

const SCHEDULER_EXAMPLE: &str = r#"{
  "poll_seconds": 30,
  "max_jobs_per_tick": 4,
  "allow_shell_jobs": false
}
"#;

const IDENTITY_EXAMPLE: &str = r#"{
  "name": "Kumo",
  "style": "concise, deterministic, and privacy-first",
  "system_instruction": "Operate fully offline after deployment, prefer local state, and degrade gracefully when the llama.cpp endpoint is unavailable."
}
"#;

const TOOLS_EXAMPLE: &str = r#"{
  "allowlist": ["date", "echo", "ls", "pwd", "cat"]
}
"#;

const TELEGRAM_EXAMPLE: &str = r#"{
  "enabled": false,
  "bot_token": "",
  "bot_token_file": "",
  "poll_timeout_secs": 30,
  "owner_user_id": "",
  "allowed_user_ids": [],
  "pairing_enabled": true,
  "pairing_code_ttl_minutes": 15,
  "api_base_url": "https://api.telegram.org"
}
"#;

const VOICE_EXAMPLE: &str = r#"{
  "enabled": false,
  "input_device": "",
  "output_device": "",
  "sample_rate": 16000,
  "capture_seconds_max": 8,
  "stt_binary_path": "whisper-cli",
  "stt_model_path": "data/models/whisper.bin",
  "tts_binary_path": "piper",
  "tts_model_path": "data/models/piper.onnx",
  "player_binary_path": "aplay",
  "recorder_binary_path": "arecord",
  "trigger_mode": "push_to_talk",
  "push_to_talk_command": "",
  "silence_timeout_ms": 1200,
  "temp_audio_dir": "data/voice/tmp"
}
"#;

const PROFILE_EXAMPLE: &str = r#"# Assistant Profile

Name: Kumo

Purpose:
- Serve as a local-first Raspberry Pi assistant.
- Preserve user context without cloud dependencies.
- Prefer deterministic formatting and low-resource execution.

Communication:
- Be direct and compact.
- Explain degraded states clearly.
- Keep outputs practical for terminal use.
"#;

#[derive(Clone, Debug)]
pub struct AssistantPaths {
    pub root: PathBuf,
    pub src_dir: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub skills_dir: PathBuf,
    pub notes_dir: PathBuf,
    pub conversations_dir: PathBuf,
    pub memory_dir: PathBuf,
    pub tasks_dir: PathBuf,
    pub summaries_dir: PathBuf,
    pub profiles_dir: PathBuf,
    pub tests_dir: PathBuf,
    pub benchmarks_dir: PathBuf,
    pub deploy_dir: PathBuf,
    pub db_path: PathBuf,
}

impl AssistantPaths {
    pub fn discover() -> Result<Self, String> {
        let root = env::var("AI_ASSISTANT_ROOT").map(PathBuf::from).unwrap_or(
            env::current_dir()
                .map_err(|error| format!("failed to resolve current directory: {error}"))?,
        );
        Ok(Self::new(root))
    }

    pub fn new(root: PathBuf) -> Self {
        let data_dir = root.join("data");
        Self {
            src_dir: root.join("src"),
            config_dir: root.join("config"),
            skills_dir: data_dir.join("skills"),
            notes_dir: data_dir.join("notes"),
            conversations_dir: data_dir.join("conversations"),
            memory_dir: data_dir.join("memory"),
            tasks_dir: data_dir.join("tasks"),
            summaries_dir: data_dir.join("summaries"),
            profiles_dir: data_dir.join("profiles"),
            tests_dir: root.join("tests"),
            benchmarks_dir: root.join("benchmarks"),
            deploy_dir: root.join("deploy"),
            db_path: data_dir.join("assistant.db"),
            root,
            data_dir,
        }
    }

    pub fn ensure_layout(&self) -> Result<(), String> {
        for directory in [
            &self.src_dir,
            &self.config_dir,
            &self.data_dir,
            &self.skills_dir,
            &self.notes_dir,
            &self.conversations_dir,
            &self.memory_dir,
            &self.tasks_dir,
            &self.summaries_dir,
            &self.profiles_dir,
            &self.tests_dir,
            &self.benchmarks_dir,
            &self.deploy_dir,
        ] {
            ensure_dir(directory)?;
        }
        Ok(())
    }

    pub fn ensure_defaults(&self) -> Result<(), String> {
        self.ensure_layout()?;
        write_if_missing(&self.config_dir.join("llm.example.json"), LLM_EXAMPLE)?;
        write_if_missing(&self.config_dir.join("memory.example.json"), MEMORY_EXAMPLE)?;
        write_if_missing(
            &self.config_dir.join("scheduler.example.json"),
            SCHEDULER_EXAMPLE,
        )?;
        write_if_missing(
            &self.config_dir.join("identity.example.json"),
            IDENTITY_EXAMPLE,
        )?;
        write_if_missing(&self.config_dir.join("tools.example.json"), TOOLS_EXAMPLE)?;
        write_if_missing(
            &self.config_dir.join("telegram.example.json"),
            TELEGRAM_EXAMPLE,
        )?;
        write_if_missing(&self.config_dir.join("voice.example.json"), VOICE_EXAMPLE)?;
        write_if_missing(&self.profiles_dir.join("assistant.md"), PROFILE_EXAMPLE)?;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct LlmConfig {
    pub prefer_http: bool,
    pub endpoint: String,
    pub health_endpoint: String,
    pub model: String,
    pub binary_path: String,
    pub model_path: String,
    pub threads: usize,
    pub context_size: usize,
    pub predict_tokens: usize,
    pub timeout_secs: usize,
    pub retries: usize,
    pub stream: bool,
}

#[derive(Clone, Debug)]
pub struct MemoryConfig {
    pub recent_turn_limit: usize,
    pub compact_after_turns: usize,
    pub retain_recent_turns: usize,
    pub token_budget: usize,
    pub compact_context_threshold_percent: usize,
    pub memory_search_limit: usize,
    pub memory_ttl_days: usize,
}

#[derive(Clone, Debug)]
pub struct SchedulerConfig {
    pub poll_seconds: usize,
    pub max_jobs_per_tick: usize,
    pub allow_shell_jobs: bool,
}

#[derive(Clone, Debug)]
pub struct IdentityConfig {
    pub name: String,
    pub style: String,
    pub system_instruction: String,
}

#[derive(Clone, Debug)]
pub struct ToolConfig {
    pub allowlist: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token: String,
    pub bot_token_file: String,
    pub poll_timeout_secs: usize,
    pub owner_user_id: Option<i64>,
    pub allowed_user_ids: Vec<i64>,
    pub pairing_enabled: bool,
    pub pairing_code_ttl_minutes: usize,
    pub api_base_url: String,
}

#[derive(Clone, Debug)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub input_device: String,
    pub output_device: String,
    pub sample_rate: usize,
    pub capture_seconds_max: usize,
    pub stt_binary_path: String,
    pub stt_model_path: String,
    pub tts_binary_path: String,
    pub tts_model_path: String,
    pub player_binary_path: String,
    pub recorder_binary_path: String,
    pub trigger_mode: String,
    pub push_to_talk_command: String,
    pub silence_timeout_ms: usize,
    pub temp_audio_dir: String,
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub llm: LlmConfig,
    pub memory: MemoryConfig,
    pub scheduler: SchedulerConfig,
    pub identity: IdentityConfig,
    pub tools: ToolConfig,
    pub telegram: TelegramConfig,
    pub voice: VoiceConfig,
}

impl AppConfig {
    pub fn load(paths: &AssistantPaths) -> Result<Self, String> {
        paths.ensure_defaults()?;
        let llm = read_with_fallback(
            &paths.config_dir.join("llm.json"),
            &paths.config_dir.join("llm.example.json"),
        )?;
        let memory = read_with_fallback(
            &paths.config_dir.join("memory.json"),
            &paths.config_dir.join("memory.example.json"),
        )?;
        let scheduler = read_with_fallback(
            &paths.config_dir.join("scheduler.json"),
            &paths.config_dir.join("scheduler.example.json"),
        )?;
        let identity = read_with_fallback(
            &paths.config_dir.join("identity.json"),
            &paths.config_dir.join("identity.example.json"),
        )?;
        let tools = read_with_fallback(
            &paths.config_dir.join("tools.json"),
            &paths.config_dir.join("tools.example.json"),
        )?;
        let telegram = read_with_fallback(
            &paths.config_dir.join("telegram.json"),
            &paths.config_dir.join("telegram.example.json"),
        )?;
        let voice = read_with_fallback(
            &paths.config_dir.join("voice.json"),
            &paths.config_dir.join("voice.example.json"),
        )?;

        Ok(Self {
            llm: LlmConfig {
                prefer_http: parse_json_bool(&llm, "prefer_http").unwrap_or(false),
                endpoint: parse_json_string(&llm, "endpoint")
                    .unwrap_or_else(|| "http://127.0.0.1:8080/v1/chat/completions".to_string()),
                health_endpoint: parse_json_string(&llm, "health_endpoint")
                    .unwrap_or_else(|| "http://127.0.0.1:8080/health".to_string()),
                model: parse_json_string(&llm, "model")
                    .unwrap_or_else(|| "smollm2-135m-instruct".to_string()),
                binary_path: parse_json_string(&llm, "binary_path")
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| default_llama_binary_path(paths)),
                model_path: parse_json_string(&llm, "model_path")
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| default_llama_model_path(paths)),
                threads: parse_json_usize(&llm, "threads").unwrap_or(2),
                context_size: parse_json_usize(&llm, "context_size").unwrap_or(4096),
                predict_tokens: parse_json_usize(&llm, "predict_tokens").unwrap_or(512),
                timeout_secs: parse_json_usize(&llm, "timeout_secs").unwrap_or(20),
                retries: parse_json_usize(&llm, "retries").unwrap_or(2),
                stream: parse_json_bool(&llm, "stream").unwrap_or(true),
            },
            memory: MemoryConfig {
                recent_turn_limit: parse_json_usize(&memory, "recent_turn_limit").unwrap_or(8),
                compact_after_turns: parse_json_usize(&memory, "compact_after_turns").unwrap_or(12),
                retain_recent_turns: parse_json_usize(&memory, "retain_recent_turns").unwrap_or(6),
                token_budget: parse_json_usize(&memory, "token_budget").unwrap_or(2048),
                compact_context_threshold_percent: parse_json_usize(
                    &memory,
                    "compact_context_threshold_percent",
                )
                .unwrap_or(70),
                memory_search_limit: parse_json_usize(&memory, "memory_search_limit").unwrap_or(6),
                memory_ttl_days: parse_json_usize(&memory, "memory_ttl_days").unwrap_or(30),
            },
            scheduler: SchedulerConfig {
                poll_seconds: parse_json_usize(&scheduler, "poll_seconds").unwrap_or(30),
                max_jobs_per_tick: parse_json_usize(&scheduler, "max_jobs_per_tick").unwrap_or(4),
                allow_shell_jobs: parse_json_bool(&scheduler, "allow_shell_jobs").unwrap_or(false),
            },
            identity: IdentityConfig {
                name: parse_json_string(&identity, "name").unwrap_or_else(|| "Kumo".to_string()),
                style: parse_json_string(&identity, "style")
                    .unwrap_or_else(|| "concise, deterministic, and privacy-first".to_string()),
                system_instruction: parse_json_string(&identity, "system_instruction").unwrap_or_else(|| {
                    "Operate fully offline after deployment and degrade gracefully when llama.cpp is unavailable."
                        .to_string()
                }),
            },
            tools: ToolConfig {
                allowlist: parse_json_array(&tools, "allowlist")
                    .unwrap_or_else(|| vec!["date".into(), "echo".into(), "ls".into(), "pwd".into(), "cat".into()]),
            },
            telegram: TelegramConfig {
                enabled: parse_json_bool(&telegram, "enabled").unwrap_or(false),
                bot_token: parse_json_string(&telegram, "bot_token").unwrap_or_default(),
                bot_token_file: parse_json_string(&telegram, "bot_token_file").unwrap_or_default(),
                poll_timeout_secs: parse_json_usize(&telegram, "poll_timeout_secs").unwrap_or(30),
                owner_user_id: parse_json_string(&telegram, "owner_user_id")
                    .and_then(|value| if value.trim().is_empty() { None } else { value.parse().ok() }),
                allowed_user_ids: parse_json_array(&telegram, "allowed_user_ids")
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|value| value.parse::<i64>().ok())
                    .collect(),
                pairing_enabled: parse_json_bool(&telegram, "pairing_enabled").unwrap_or(true),
                pairing_code_ttl_minutes: parse_json_usize(&telegram, "pairing_code_ttl_minutes").unwrap_or(15),
                api_base_url: parse_json_string(&telegram, "api_base_url")
                    .unwrap_or_else(|| "https://api.telegram.org".to_string()),
            },
            voice: VoiceConfig {
                enabled: parse_json_bool(&voice, "enabled").unwrap_or(false),
                input_device: parse_json_string(&voice, "input_device").unwrap_or_default(),
                output_device: parse_json_string(&voice, "output_device").unwrap_or_default(),
                sample_rate: parse_json_usize(&voice, "sample_rate").unwrap_or(16000),
                capture_seconds_max: parse_json_usize(&voice, "capture_seconds_max").unwrap_or(8),
                stt_binary_path: parse_json_string(&voice, "stt_binary_path")
                    .unwrap_or_else(|| "whisper-cli".to_string()),
                stt_model_path: parse_json_string(&voice, "stt_model_path")
                    .unwrap_or_else(|| default_voice_stt_model_path(paths)),
                tts_binary_path: parse_json_string(&voice, "tts_binary_path")
                    .unwrap_or_else(|| "piper".to_string()),
                tts_model_path: parse_json_string(&voice, "tts_model_path")
                    .unwrap_or_else(|| default_voice_tts_model_path(paths)),
                player_binary_path: parse_json_string(&voice, "player_binary_path")
                    .unwrap_or_else(|| "aplay".to_string()),
                recorder_binary_path: parse_json_string(&voice, "recorder_binary_path")
                    .unwrap_or_else(|| "arecord".to_string()),
                trigger_mode: parse_json_string(&voice, "trigger_mode")
                    .unwrap_or_else(|| "push_to_talk".to_string()),
                push_to_talk_command: parse_json_string(&voice, "push_to_talk_command")
                    .unwrap_or_default(),
                silence_timeout_ms: parse_json_usize(&voice, "silence_timeout_ms").unwrap_or(1200),
                temp_audio_dir: parse_json_string(&voice, "temp_audio_dir")
                    .unwrap_or_else(|| default_voice_temp_audio_dir(paths)),
            },
        })
    }
}

impl LlmConfig {
    pub fn local_first(paths: &AssistantPaths) -> Self {
        Self {
            prefer_http: false,
            endpoint: "http://127.0.0.1:8080/v1/chat/completions".to_string(),
            health_endpoint: "http://127.0.0.1:8080/health".to_string(),
            model: "smollm2-135m-instruct".to_string(),
            binary_path: default_llama_binary_path(paths),
            model_path: default_llama_model_path(paths),
            threads: 2,
            context_size: 4096,
            predict_tokens: 512,
            timeout_secs: 20,
            retries: 2,
            stream: true,
        }
    }
}

impl TelegramConfig {
    pub fn onboarding_complete(&self) -> bool {
        self.enabled
            && self.owner_user_id.is_some()
            && (!self.bot_token.trim().is_empty() || !self.bot_token_file.trim().is_empty())
    }

    pub fn resolve_bot_token(&self, paths: &AssistantPaths) -> Result<Option<String>, String> {
        if !self.bot_token.trim().is_empty() {
            return Ok(Some(self.bot_token.trim().to_string()));
        }
        if self.bot_token_file.trim().is_empty() {
            return Ok(None);
        }
        let token_path = resolve_config_path(paths, &self.bot_token_file);
        let token = fs::read_to_string(&token_path)
            .map_err(|error| format!("failed to read {}: {error}", token_path.display()))?;
        let trimmed = token.trim().to_string();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed))
    }
}

pub fn default_llama_binary_path(paths: &AssistantPaths) -> String {
    paths
        .root
        .parent()
        .unwrap_or(&paths.root)
        .join("ec2_bin/llama-cli")
        .to_string_lossy()
        .to_string()
}

pub fn default_llama_model_path(paths: &AssistantPaths) -> String {
    paths
        .root
        .parent()
        .unwrap_or(&paths.root)
        .join("SmolLM2-135M-Instruct.Q4_K_M.gguf")
        .to_string_lossy()
        .to_string()
}

pub fn default_voice_stt_model_path(paths: &AssistantPaths) -> String {
    paths
        .data_dir
        .join("models/whisper.bin")
        .to_string_lossy()
        .to_string()
}

pub fn default_voice_tts_model_path(paths: &AssistantPaths) -> String {
    paths
        .data_dir
        .join("models/piper.onnx")
        .to_string_lossy()
        .to_string()
}

pub fn default_voice_temp_audio_dir(paths: &AssistantPaths) -> String {
    paths
        .data_dir
        .join("voice/tmp")
        .to_string_lossy()
        .to_string()
}

pub fn resolve_config_path(paths: &AssistantPaths, value: &str) -> PathBuf {
    let candidate = PathBuf::from(value);
    if candidate.is_absolute() {
        candidate
    } else {
        paths.root.join(candidate)
    }
}

pub fn write_llm_config(paths: &AssistantPaths, config: &LlmConfig) -> Result<(), String> {
    write_config_file(
        &paths.config_dir.join("llm.json"),
        &format!(
            concat!(
                "{{\n",
                "  \"prefer_http\": {},\n",
                "  \"endpoint\": \"{}\",\n",
                "  \"health_endpoint\": \"{}\",\n",
                "  \"model\": \"{}\",\n",
                "  \"binary_path\": \"{}\",\n",
                "  \"model_path\": \"{}\",\n",
                "  \"threads\": {},\n",
                "  \"context_size\": {},\n",
                "  \"predict_tokens\": {},\n",
                "  \"timeout_secs\": {},\n",
                "  \"retries\": {},\n",
                "  \"stream\": {}\n",
                "}}\n"
            ),
            config.prefer_http,
            escape_json_string(&config.endpoint),
            escape_json_string(&config.health_endpoint),
            escape_json_string(&config.model),
            escape_json_string(&config.binary_path),
            escape_json_string(&config.model_path),
            config.threads,
            config.context_size,
            config.predict_tokens,
            config.timeout_secs,
            config.retries,
            config.stream
        ),
    )
}

pub fn write_telegram_config(
    paths: &AssistantPaths,
    config: &TelegramConfig,
) -> Result<(), String> {
    let allowed = if config.allowed_user_ids.is_empty() {
        String::new()
    } else {
        config
            .allowed_user_ids
            .iter()
            .map(|value| format!("\"{value}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    write_config_file(
        &paths.config_dir.join("telegram.json"),
        &format!(
            concat!(
                "{{\n",
                "  \"enabled\": {},\n",
                "  \"bot_token\": \"{}\",\n",
                "  \"bot_token_file\": \"{}\",\n",
                "  \"poll_timeout_secs\": {},\n",
                "  \"owner_user_id\": \"{}\",\n",
                "  \"allowed_user_ids\": [{}],\n",
                "  \"pairing_enabled\": {},\n",
                "  \"pairing_code_ttl_minutes\": {},\n",
                "  \"api_base_url\": \"{}\"\n",
                "}}\n"
            ),
            config.enabled,
            escape_json_string(&config.bot_token),
            escape_json_string(&config.bot_token_file),
            config.poll_timeout_secs,
            config
                .owner_user_id
                .map(|value| value.to_string())
                .unwrap_or_default(),
            allowed,
            config.pairing_enabled,
            config.pairing_code_ttl_minutes,
            escape_json_string(&config.api_base_url)
        ),
    )
}

pub fn write_tool_config(paths: &AssistantPaths, config: &ToolConfig) -> Result<(), String> {
    let allowlist = config
        .allowlist
        .iter()
        .map(|value| format!("\"{}\"", escape_json_string(value)))
        .collect::<Vec<_>>()
        .join(", ");
    write_config_file(
        &paths.config_dir.join("tools.json"),
        &format!("{{\n  \"allowlist\": [{}]\n}}\n", allowlist),
    )
}

pub fn write_identity_config(
    paths: &AssistantPaths,
    config: &IdentityConfig,
) -> Result<(), String> {
    write_config_file(
        &paths.config_dir.join("identity.json"),
        &format!(
            concat!(
                "{{\n",
                "  \"name\": \"{}\",\n",
                "  \"style\": \"{}\",\n",
                "  \"system_instruction\": \"{}\"\n",
                "}}\n"
            ),
            escape_json_string(&config.name),
            escape_json_string(&config.style),
            escape_json_string(&config.system_instruction),
        ),
    )
}

fn write_config_file(path: &Path, contents: &str) -> Result<(), String> {
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn escape_json_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

pub fn resolve_profile_path(paths: &AssistantPaths) -> PathBuf {
    paths.profiles_dir.join("assistant.md")
}

pub fn file_exists(path: &Path) -> bool {
    path.exists()
}
