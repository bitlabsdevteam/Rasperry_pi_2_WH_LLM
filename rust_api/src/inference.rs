use std::{
    convert::Infallible,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::response::sse::Event;
use tokio::{
    io::AsyncReadExt,
    process::Command,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc},
    task::JoinHandle,
};
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error, warn};

use crate::{
    config::Config,
    models::{
        ChatCompletionChoice, ChatCompletionChunkChoice, ChatCompletionChunkResponse,
        ChatCompletionRequest, ChatCompletionResponse, ChatMessageDelta, ChatMessageInput,
        ChatMessageOutput, ChatRole, ErrorBody, ErrorEnvelope, FinishReason, StopSequence,
    },
};

const DEFAULT_REPEAT_PENALTY: &str = "1.1";
const SSE_CHANNEL_CAPACITY: usize = 8;
const STREAM_READ_BUFFER_BYTES: usize = 128;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    semaphore: Arc<Semaphore>,
    request_counter: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrency));
        Self {
            config: Arc::new(config),
            semaphore,
            request_counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn next_request_id(&self) -> String {
        let seq = self.request_counter.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or(0);
        format!("chatcmpl-{millis}-{seq}")
    }
}

#[derive(Debug, Clone)]
pub struct ApiError {
    pub status_code: axum::http::StatusCode,
    pub message: String,
    pub error_type: String,
    pub code: String,
}

impl ApiError {
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(
            axum::http::StatusCode::BAD_REQUEST,
            message,
            "invalid_request_error",
            "invalid_request_error",
        )
    }

    pub fn rate_limit(message: impl Into<String>) -> Self {
        Self::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            message,
            "rate_limit_exceeded",
            "rate_limit_exceeded",
        )
    }

    pub fn inference(message: impl Into<String>) -> Self {
        Self::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            message,
            "inference_error",
            "inference_error",
        )
    }

    fn new(
        status_code: axum::http::StatusCode,
        message: impl Into<String>,
        error_type: impl Into<String>,
        code: impl Into<String>,
    ) -> Self {
        Self {
            status_code,
            message: message.into(),
            error_type: error_type.into(),
            code: code.into(),
        }
    }
}

impl axum::response::IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let body = axum::Json(ErrorEnvelope {
            error: ErrorBody {
                message: self.message,
                error_type: self.error_type,
                code: self.code,
            },
        });
        (self.status_code, body).into_response()
    }
}

pub async fn collect_inference(
    state: AppState,
    request: ChatCompletionRequest,
) -> Result<ChatCompletionResponse, ApiError> {
    let PreparedInference {
        request_id,
        created,
        model,
        prompt,
        permit,
    } = prepare_inference(&state, &request)?;
    let result =
        run_inference_command(&state.config, &prompt, &request_id, &request, permit).await?;
    let (content, finish_reason) =
        extract_assistant_content(&result.stdout, &prompt, request.stop.as_ref())
            .ok_or_else(|| ApiError::inference("assistant output was empty"))?;

    Ok(ChatCompletionResponse {
        id: request_id,
        object: "chat.completion",
        created,
        model,
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatMessageOutput {
                role: ChatRole::Assistant,
                content,
            },
            finish_reason,
        }],
    })
}

pub async fn start_streaming_inference(
    state: AppState,
    request: ChatCompletionRequest,
) -> Result<ReceiverStream<Result<Event, Infallible>>, ApiError> {
    let prepared = prepare_inference(&state, &request)?;
    let mut command = build_command(&state.config, &prepared.prompt, &request);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .current_dir(
            state
                .config
                .binary
                .parent()
                .unwrap_or_else(|| Path::new(".")),
        )
        .envs(build_runtime_env(&state.config.binary));

    let mut child = command.spawn().map_err(|error| {
        error!(
            request_id = prepared.request_id,
            binary = %state.config.binary.display(),
            "failed to spawn llama-cli: {error}"
        );
        ApiError::inference("failed to start inference process")
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::inference("failed to capture inference output"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::inference("failed to capture inference output"))?;

    let timeout_duration = Duration::from_secs(state.config.timeout_secs);
    let (tx, rx) = mpsc::channel(SSE_CHANNEL_CAPACITY);
    let stream_meta = StreamResponseMeta {
        request_id: prepared.request_id,
        created: prepared.created,
        model: prepared.model,
    };
    let permit = prepared.permit;

    tokio::spawn(async move {
        let _permit = permit;
        let stderr_task = spawn_collector(stderr);

        if send_chunk_event(
            &tx,
            &stream_meta,
            ChatMessageDelta {
                role: Some(ChatRole::Assistant),
                content: None,
            },
            None,
        )
        .await
        .is_err()
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = join_collector(stderr_task).await;
            return;
        }

        let result = tokio::time::timeout(timeout_duration, async {
            let sent_any = stream_stdout(stdout, &tx, &stream_meta).await?;
            let status = child.wait().await.map_err(|error| {
                ApiError::inference(format!("failed waiting on llama-cli: {error}"))
            })?;
            Ok::<_, ApiError>((sent_any, status))
        })
        .await;

        match result {
            Ok(Ok((sent_any, status))) => {
                let stderr =
                    String::from_utf8_lossy(&join_collector(stderr_task).await.unwrap_or_default())
                        .to_string();

                if !status.success() {
                    warn!(
                        request_id = stream_meta.request_id,
                        exit_code = status.code().unwrap_or(-1),
                        stderr = %stderr,
                        "llama-cli exited unsuccessfully during streaming"
                    );
                    let _ = send_error_event(&tx, ApiError::inference("inference process failed"))
                        .await;
                } else if !sent_any {
                    let _ =
                        send_error_event(&tx, ApiError::inference("assistant output was empty"))
                            .await;
                } else {
                    let _ = send_chunk_event(
                        &tx,
                        &stream_meta,
                        ChatMessageDelta {
                            role: None,
                            content: None,
                        },
                        Some(FinishReason::Stop),
                    )
                    .await;
                }
            }
            Ok(Err(error)) => {
                warn!(
                    request_id = stream_meta.request_id,
                    "streaming inference failed: {}", error.message
                );
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = join_collector(stderr_task).await;
                let _ = send_error_event(&tx, error).await;
            }
            Err(_) => {
                warn!(
                    request_id = stream_meta.request_id,
                    timeout_secs = timeout_duration.as_secs(),
                    "streaming inference timed out"
                );
                let _ = child.kill().await;
                let _ = child.wait().await;
                let _ = join_collector(stderr_task).await;
                let _ = send_error_event(&tx, ApiError::inference("inference timed out")).await;
            }
        }

        let _ = send_done_event(&tx).await;
    });

    Ok(ReceiverStream::new(rx))
}

struct PreparedInference {
    request_id: String,
    created: u64,
    model: String,
    prompt: String,
    permit: OwnedSemaphorePermit,
}

struct StreamResponseMeta {
    request_id: String,
    created: u64,
    model: String,
}

fn prepare_inference(
    state: &AppState,
    request: &ChatCompletionRequest,
) -> Result<PreparedInference, ApiError> {
    request
        .validate(&state.config)
        .map_err(ApiError::invalid_request)?;

    validate_runtime(&state.config)?;

    let permit = state
        .semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::rate_limit("inference device is busy, try again later"))?;

    Ok(PreparedInference {
        request_id: state.next_request_id(),
        created: current_unix_timestamp(),
        model: request.effective_model(&state.config).to_string(),
        prompt: render_prompt(&request.messages),
        permit,
    })
}

fn current_unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn validate_runtime(config: &Config) -> Result<(), ApiError> {
    let binary = &config.binary;
    let model = &config.model;

    if !binary.exists() {
        error!("llama binary missing at {}", binary.display());
        return Err(ApiError::inference("inference runtime is not ready"));
    }

    if !is_executable(binary) {
        error!("llama binary is not executable at {}", binary.display());
        return Err(ApiError::inference("inference runtime is not ready"));
    }

    if !model.exists() {
        error!("model missing at {}", model.display());
        return Err(ApiError::inference("inference runtime is not ready"));
    }

    Ok(())
}

pub fn runtime_ready(config: &Config) -> bool {
    config.binary.exists() && is_executable(&config.binary) && config.model.exists()
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }

    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

async fn run_inference_command(
    config: &Config,
    prompt: &str,
    request_id: &str,
    request: &ChatCompletionRequest,
    _permit: OwnedSemaphorePermit,
) -> Result<CommandResult, ApiError> {
    let mut command = build_command(config, prompt, request);
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .current_dir(config.binary.parent().unwrap_or_else(|| Path::new(".")))
        .envs(build_runtime_env(&config.binary));

    let mut child = command.spawn().map_err(|error| {
        error!(
            request_id = request_id,
            binary = %config.binary.display(),
            "failed to spawn llama-cli: {error}"
        );
        ApiError::inference("failed to start inference process")
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ApiError::inference("failed to capture inference output"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| ApiError::inference("failed to capture inference output"))?;

    let stdout_task = spawn_collector(stdout);
    let stderr_task = spawn_collector(stderr);
    let timeout_duration = Duration::from_secs(config.timeout_secs);

    let status = match tokio::time::timeout(timeout_duration, child.wait()).await {
        Ok(Ok(status)) => status,
        Ok(Err(error)) => {
            warn!(
                request_id = request_id,
                "failed waiting on llama-cli: {error}"
            );
            let _ = join_collector(stdout_task).await;
            let _ = join_collector(stderr_task).await;
            return Err(ApiError::inference("inference process failed"));
        }
        Err(_) => {
            warn!(
                request_id = request_id,
                timeout_secs = config.timeout_secs,
                "llama-cli timed out"
            );
            let _ = child.kill().await;
            let _ = child.wait().await;
            let _ = join_collector(stdout_task).await;
            let _ = join_collector(stderr_task).await;
            return Err(ApiError::inference("inference timed out"));
        }
    };

    let stdout = String::from_utf8_lossy(&join_collector(stdout_task).await?).to_string();
    let stderr = String::from_utf8_lossy(&join_collector(stderr_task).await?).to_string();

    debug!(request_id = request_id, stdout = %stdout, stderr = %stderr, "llama-cli raw output");

    if !status.success() {
        warn!(
            request_id = request_id,
            exit_code = status.code().unwrap_or(-1),
            stdout = %stdout,
            stderr = %stderr,
            "llama-cli exited unsuccessfully"
        );
        return Err(ApiError::inference("inference process failed"));
    }

    Ok(CommandResult { stdout, stderr })
}

fn build_command(config: &Config, prompt: &str, request: &ChatCompletionRequest) -> Command {
    let mut command = Command::new(&config.binary);
    command
        .arg("-m")
        .arg(&config.model)
        .arg("-p")
        .arg(prompt)
        .arg("-n")
        .arg(request.max_tokens.to_string())
        .arg("-t")
        .arg(config.threads.to_string())
        .arg("-c")
        .arg(config.context_size.to_string())
        .arg("--temp")
        .arg(request.temperature.to_string())
        .arg("--top-p")
        .arg(request.top_p.to_string())
        .arg("--repeat-penalty")
        .arg(DEFAULT_REPEAT_PENALTY)
        .arg("--simple-io")
        .arg("--no-display-prompt")
        .arg("--single-turn")
        .arg("--log-disable")
        .arg("--no-warmup");
    command
}

fn build_runtime_env(binary: &Path) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    if let Some(parent) = binary.parent() {
        let lib_dir = parent.display().to_string();
        for key in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH"] {
            let value = std::env::var(key)
                .ok()
                .filter(|current| !current.is_empty())
                .map(|current| format!("{lib_dir}:{}", current))
                .unwrap_or_else(|| lib_dir.clone());
            pairs.push((key.to_string(), value));
        }
    }
    pairs
}

async fn stream_stdout<R>(
    mut reader: R,
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    stream_meta: &StreamResponseMeta,
) -> Result<bool, ApiError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buffer = [0_u8; STREAM_READ_BUFFER_BYTES];
    let mut sent_any = false;

    loop {
        let read = reader.read(&mut buffer).await.map_err(|error| {
            ApiError::inference(format!("failed to read process stream: {error}"))
        })?;

        if read == 0 {
            return Ok(sent_any);
        }

        let content = clean_stream_chunk(&buffer[..read]);
        if content.is_empty() {
            continue;
        }

        sent_any = true;

        if send_chunk_event(
            tx,
            stream_meta,
            ChatMessageDelta {
                role: None,
                content: Some(content),
            },
            None,
        )
        .await
        .is_err()
        {
            return Err(ApiError::inference("stream client disconnected"));
        }
    }
}

fn spawn_collector<R>(mut reader: R) -> JoinHandle<Result<Vec<u8>, ApiError>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).await.map_err(|error| {
            ApiError::inference(format!("failed to read process stream: {error}"))
        })?;
        Ok(buffer)
    })
}

async fn join_collector(
    handle: JoinHandle<Result<Vec<u8>, ApiError>>,
) -> Result<Vec<u8>, ApiError> {
    handle
        .await
        .map_err(|error| ApiError::inference(format!("reader task failed: {error}")))?
}

fn clean_stream_chunk(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn render_prompt(messages: &[ChatMessageInput]) -> String {
    let system_messages: Vec<&str> = messages
        .iter()
        .filter(|message| matches!(message.role, ChatRole::System))
        .map(|message| message.content.trim())
        .collect();
    let mut sections = Vec::new();

    if !system_messages.is_empty() {
        sections.push(format!("System:\n{}", system_messages.join("\n\n")));
    }

    for message in messages {
        let label = match message.role {
            ChatRole::System => continue,
            ChatRole::User => "User",
            ChatRole::Assistant => "Assistant",
        };
        sections.push(format!("{label}:\n{}", message.content.trim()));
    }

    sections.push("Assistant:".to_string());
    sections.join("\n\n")
}

fn extract_assistant_content(
    stdout: &str,
    prompt: &str,
    stop: Option<&StopSequence>,
) -> Option<(String, FinishReason)> {
    let cleaned = clean_cli_text(stdout);
    let prompt_lines: Vec<&str> = prompt
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let mut output_lines = Vec::new();
    let mut remaining_prompt_lines = prompt_lines.as_slice();

    for raw_line in cleaned.lines() {
        let line = raw_line.trim();
        if line.is_empty() || is_noise_line(line) {
            continue;
        }

        if let Some(rest) = line.strip_prefix('>') {
            let echoed = rest.trim();
            if echoed == prompt.trim() || remaining_prompt_lines.first().copied() == Some(echoed) {
                if remaining_prompt_lines.first().copied() == Some(echoed) {
                    remaining_prompt_lines = &remaining_prompt_lines[1..];
                }
                continue;
            }
        }

        if remaining_prompt_lines.first().copied() == Some(line) {
            remaining_prompt_lines = &remaining_prompt_lines[1..];
            continue;
        }

        output_lines.push(line.to_string());
    }

    let mut content = output_lines.join("\n").trim().to_string();
    if content.is_empty() {
        return None;
    }

    let finish_reason = apply_stop_sequences(&mut content, stop);
    if content.trim().is_empty() {
        return None;
    }

    Some((content, finish_reason))
}

fn clean_cli_text(text: &str) -> String {
    strip_ansi(text).replace("\r\n", "\n").replace('\r', "\n")
}

fn apply_stop_sequences(content: &mut String, stop: Option<&StopSequence>) -> FinishReason {
    if let Some(stop) = stop {
        let earliest = stop
            .sequences()
            .into_iter()
            .filter_map(|value| content.find(value))
            .min();
        if let Some(index) = earliest {
            content.truncate(index);
            *content = content.trim().to_string();
            return FinishReason::Stop;
        }
    }

    FinishReason::Stop
}

fn is_noise_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let prefixes = [
        "loading model...",
        "build      :",
        "model      :",
        "modalities :",
        "available commands:",
        "main:",
        "common:",
        "system info",
        "llama_model_loader:",
        "llama_context:",
        "llama_kv_cache",
        "llama_perf_",
        "load time",
        "sample time",
        "prompt eval",
        "eval time",
        "total time",
        "[ prompt:",
        "exiting...",
    ];

    if prefixes.iter().any(|prefix| lower.starts_with(prefix)) {
        return true;
    }

    if matches!(line, "/exit" | "/regen" | "/clear")
        || line.starts_with("/read ")
        || line.starts_with("/glob ")
    {
        return true;
    }

    line.chars()
        .all(|ch| matches!(ch, '▄' | '█' | '▀' | ' ' | '|' | '_' | '-' | '='))
}

fn strip_ansi(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut state = AnsiState::Normal;

    for ch in input.chars() {
        match state {
            AnsiState::Normal => {
                if ch == '\u{1b}' {
                    state = AnsiState::Escape;
                } else {
                    output.push(ch);
                }
            }
            AnsiState::Escape => {
                if ch == '[' {
                    state = AnsiState::Csi;
                } else if ('@'..='~').contains(&ch) {
                    state = AnsiState::Normal;
                } else {
                    state = AnsiState::Normal;
                }
            }
            AnsiState::Csi => {
                if ('@'..='~').contains(&ch) {
                    state = AnsiState::Normal;
                }
            }
        }
    }

    output
}

#[derive(Copy, Clone)]
enum AnsiState {
    Normal,
    Escape,
    Csi,
}

async fn send_chunk_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    stream_meta: &StreamResponseMeta,
    delta: ChatMessageDelta,
    finish_reason: Option<FinishReason>,
) -> Result<(), ()> {
    let chunk = ChatCompletionChunkResponse {
        id: stream_meta.request_id.clone(),
        object: "chat.completion.chunk",
        created: stream_meta.created,
        model: stream_meta.model.clone(),
        choices: vec![ChatCompletionChunkChoice {
            index: 0,
            delta,
            finish_reason,
        }],
    };

    let data = serde_json::to_string(&chunk).map_err(|_| ())?;
    tx.send(Ok(Event::default().event("chunk").data(data)))
        .await
        .map_err(|_| ())
}

async fn send_error_event(
    tx: &mpsc::Sender<Result<Event, Infallible>>,
    error: ApiError,
) -> Result<(), ()> {
    let data = serde_json::to_string(&ErrorEnvelope {
        error: ErrorBody {
            message: error.message,
            error_type: error.error_type,
            code: error.code,
        },
    })
    .map_err(|_| ())?;

    tx.send(Ok(Event::default().event("error").data(data)))
        .await
        .map_err(|_| ())
}

async fn send_done_event(tx: &mpsc::Sender<Result<Event, Infallible>>) -> Result<(), ()> {
    tx.send(Ok(Event::default().data("[DONE]")))
        .await
        .map_err(|_| ())
}

struct CommandResult {
    stdout: String,
    #[allow(dead_code)]
    stderr: String,
}

#[cfg(test)]
mod tests {
    use super::{
        FinishReason, StopSequence, clean_cli_text, extract_assistant_content, render_prompt,
        strip_ansi,
    };
    use crate::models::{ChatMessageInput, ChatRole};

    #[test]
    fn strip_ansi_removes_escape_sequences() {
        let cleaned = strip_ansi("\u{1b}[31mhello\u{1b}[0m world");
        assert_eq!(cleaned, "hello world");
    }

    #[test]
    fn clean_cli_text_normalizes_line_endings() {
        assert_eq!(clean_cli_text("a\r\nb\rc"), "a\nb\nc");
    }

    #[test]
    fn extractor_strips_banner_prompt_and_footer_noise() {
        let prompt = render_prompt(&[
            ChatMessageInput {
                role: ChatRole::System,
                content: "You are concise.".to_string(),
            },
            ChatMessageInput {
                role: ChatRole::User,
                content: "Say hello".to_string(),
            },
        ]);

        let stdout = "\u{1b}[32mLoading model...\u{1b}[0m\r\n\
build      : 1\r\n\
▄▀█ █▀\r\n\
System:\r\n\
You are concise.\r\n\
User:\r\n\
Say hello\r\n\
Assistant:\r\n\
Hello! I'm here to help with your questions and concerns.\r\n\
llama_perf_context_print:        load time = 1.23 ms\r\n\
Exiting...\r\n";

        let (content, finish_reason) =
            extract_assistant_content(stdout, &prompt, None).expect("content");
        assert_eq!(
            content,
            "Hello! I'm here to help with your questions and concerns."
        );
        assert_eq!(finish_reason, FinishReason::Stop);
    }

    #[test]
    fn extractor_removes_echoed_prompt_lines() {
        let prompt = "System:\nRules\n\nUser:\nHi\n\nAssistant:".to_string();
        let stdout = "System:\nRules\n\nUser:\nHi\n\nAssistant:\nHello there";
        let (content, _) = extract_assistant_content(stdout, &prompt, None).expect("content");
        assert_eq!(content, "Hello there");
    }

    #[test]
    fn extractor_applies_stop_sequences() {
        let prompt = "User:\nHi\n\nAssistant:".to_string();
        let stdout = "Hello there<END>ignored";
        let (content, finish_reason) = extract_assistant_content(
            stdout,
            &prompt,
            Some(&StopSequence::Single("<END>".to_string())),
        )
        .expect("content");
        assert_eq!(content, "Hello there");
        assert_eq!(finish_reason, FinishReason::Stop);
    }

    #[test]
    fn extractor_returns_none_for_empty_output() {
        let prompt = "User:\nHi\n\nAssistant:".to_string();
        let stdout = "Loading model...\nExiting...\n";
        assert!(extract_assistant_content(stdout, &prompt, None).is_none());
    }
}
