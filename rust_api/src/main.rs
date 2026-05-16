mod config;
mod inference;
mod models;

use std::net::SocketAddr;

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State, rejection::JsonRejection},
    response::{IntoResponse, Response, sse::Sse},
    routing::{get, post},
};
use tower_http::trace::TraceLayer;
use tracing::info;

use crate::{
    config::Config,
    inference::{ApiError, AppState, collect_inference, runtime_ready, start_streaming_inference},
    models::{ChatCompletionRequest, HealthResponse},
};

const BODY_LIMIT_BYTES: usize = 16 * 1024;

#[tokio::main]
async fn main() {
    init_tracing();

    let config = Config::from_env();
    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("invalid listen address");
    let state = AppState::new(config);
    let app = app(state);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind tcp listener");

    info!(
        "rust_api listening on http://{}",
        listener.local_addr().unwrap()
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .expect("server error");
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(DefaultBodyLimit::max(BODY_LIMIT_BYTES))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let ok = runtime_ready(&state.config);
    Json(HealthResponse {
        ok,
        status: if ok { "ready" } else { "error" },
        model: state.config.model_alias.clone(),
    })
}

async fn chat_completions(
    State(state): State<AppState>,
    request: Result<Json<ChatCompletionRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request.map_err(|rejection| match rejection {
        JsonRejection::JsonDataError(error) => ApiError::invalid_request(error.body_text()),
        JsonRejection::JsonSyntaxError(error) => ApiError::invalid_request(error.body_text()),
        JsonRejection::MissingJsonContentType(error) => {
            ApiError::invalid_request(error.body_text())
        }
        other => ApiError::invalid_request(other.body_text()),
    })?;

    if matches!(request.stream, Some(true)) {
        let stream = start_streaming_inference(state, request).await?;
        return Ok(Sse::new(stream).into_response());
    }

    let response = collect_inference(state, request).await?;
    Ok(Json(
        serde_json::to_value(response)
            .map_err(|error| ApiError::inference(format!("failed to encode response: {error}")))?,
    )
    .into_response())
}

fn init_tracing() {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,tower_http=info,axum=info".into());

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{SignalKind, signal};
        signal(SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };

    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::util::ServiceExt;

    use super::*;
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[tokio::test]
    async fn chat_completions_returns_openai_style_response_without_internal_fields() {
        let fixture = TestFixture::new(
            "#!/bin/sh\nprintf 'Hello from the Pi-safe API.'\n",
            "smollm2-135m-instruct",
        );
        let app = app(fixture.state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"messages":[{"role":"user","content":"Hello"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_json(response).await;
        assert_eq!(body["object"], "chat.completion");
        assert_eq!(body["model"], "smollm2-135m-instruct");
        assert_eq!(body["choices"][0]["message"]["role"], "assistant");
        assert_eq!(
            body["choices"][0]["message"]["content"],
            "Hello from the Pi-safe API."
        );
        assert!(body.get("stdout").is_none());
        assert!(body.get("stderr").is_none());
        assert!(body.get("output").is_none());
        assert!(body.get("binary").is_none());
        assert!(body.get("duration_ms").is_none());
        assert!(body.get("exit_code").is_none());
    }

    #[tokio::test]
    async fn chat_completions_streams_sse_chunks() {
        let fixture = TestFixture::new(
            "#!/bin/sh\nprintf 'Hel'; sleep 0.1; printf 'lo'\n",
            "smollm2-135m-instruct",
        );
        let app = app(fixture.state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"messages":[{"role":"user","content":"Hello"}],"stream":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.starts_with("text/event-stream"));

        let body = to_text(response).await;
        assert!(body.contains("\"object\":\"chat.completion.chunk\""));
        assert!(body.contains("\"role\":\"assistant\""));
        assert!(body.contains("\"content\":\"Hel\""));
        assert!(body.contains("\"content\":\"lo\""));
        assert!(body.contains("\"finish_reason\":\"stop\""));
        assert!(body.contains("data: [DONE]"));
    }

    #[tokio::test]
    async fn chat_completions_rejects_invalid_requests() {
        let fixture = TestFixture::new("#!/bin/sh\nprintf 'ignored'\n", "smollm2-135m-instruct");
        let app = app(fixture.state());

        let cases = [
            r#"{"messages":[]}"#,
            r#"{"messages":[{"role":"user","content":""}]}"#,
            r#"{"messages":[{"role":"tool","content":"bad"}]}"#,
            r#"{"messages":[{"role":"user","content":"ok"}],"max_tokens":257}"#,
        ];

        for case in cases {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/chat/completions")
                        .header("content-type", "application/json")
                        .body(Body::from(case))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn chat_completions_rejects_wrong_model_alias() {
        let fixture = TestFixture::new("#!/bin/sh\nprintf 'ignored'\n", "smollm2-135m-instruct");
        let app = app(fixture.state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"model":"wrong-alias","messages":[{"role":"user","content":"Hello"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn busy_server_returns_429() {
        let fixture = TestFixture::new(
            "#!/bin/sh\nsleep 1\nprintf 'done'\n",
            "smollm2-135m-instruct",
        );
        let state = fixture.state();
        let first_app = app(state.clone());
        let busy_app = app(state);

        let first = tokio::spawn(async move {
            first_app
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri("/v1/chat/completions")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            r#"{"messages":[{"role":"user","content":"Hello"}]}"#,
                        ))
                        .unwrap(),
                )
                .await
                .unwrap()
        });

        tokio::time::sleep(Duration::from_millis(100)).await;

        let second = busy_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"messages":[{"role":"user","content":"Hello"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let _ = first.await.unwrap();
    }

    #[tokio::test]
    async fn busy_server_returns_429_while_stream_is_open() {
        let fixture = TestFixture::new(
            "#!/bin/sh\nprintf 'start'; sleep 1; printf ' end'\n",
            "smollm2-135m-instruct",
        );
        let state = fixture.state();
        let first_app = app(state.clone());
        let busy_app = app(state);

        let streaming_response = first_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"messages":[{"role":"user","content":"Hello"}],"stream":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(streaming_response.status(), StatusCode::OK);
        tokio::time::sleep(Duration::from_millis(100)).await;

        let second = busy_app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/chat/completions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"messages":[{"role":"user","content":"Hello"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        let _ = to_text(streaming_response).await;
    }

    #[tokio::test]
    async fn health_is_minimal_and_does_not_leak_paths() {
        let fixture = TestFixture::new("#!/bin/sh\nprintf 'ok'\n", "smollm2-135m-instruct");
        let app = app(fixture.state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_json(response).await;
        let body_text = serde_json::to_string(&body).unwrap();
        assert_eq!(body["ok"], true);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["model"], "smollm2-135m-instruct");
        assert!(!body_text.contains("/"));
        assert!(!body_text.contains("llama-cli"));
        assert!(!body_text.contains(".gguf"));
    }

    async fn to_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = to_bytes(response.into_body(), BODY_LIMIT_BYTES * 2)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    async fn to_text(response: axum::response::Response) -> String {
        let bytes = to_bytes(response.into_body(), BODY_LIMIT_BYTES * 4)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    struct TestFixture {
        dir: PathBuf,
        binary: PathBuf,
        model: PathBuf,
        model_alias: String,
    }

    impl TestFixture {
        fn new(script: &str, model_alias: &str) -> Self {
            let dir = unique_test_dir();
            fs::create_dir_all(&dir).unwrap();

            let binary = dir.join("llama-cli");
            let model = dir.join("model.gguf");
            fs::write(&binary, script).unwrap();
            fs::write(&model, b"test-model").unwrap();
            set_executable(&binary);

            Self {
                dir,
                binary,
                model,
                model_alias: model_alias.to_string(),
            }
        }

        fn state(&self) -> AppState {
            AppState::new(Config {
                binary: self.binary.clone(),
                model: self.model.clone(),
                model_alias: self.model_alias.clone(),
                host: "127.0.0.1".to_string(),
                port: 0,
                threads: 2,
                timeout_secs: 5,
                context_size: 128,
                max_concurrency: 1,
            })
        }
    }

    impl Drop for TestFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn unique_test_dir() -> PathBuf {
        let seq = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("rust-api-tests-{seq}"))
    }

    fn set_executable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(path, permissions).unwrap();
        }
    }
}
