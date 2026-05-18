use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use ai_assistant::{adapters::llama_cpp::LlamaCppAdapter, config::LlmConfig};

#[test]
fn llama_cpp_connectivity_mock_round_trip() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 2048];
        let _ = stream.read(&mut buffer).unwrap();
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"offline hello"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let adapter = LlamaCppAdapter::new(LlmConfig {
        prefer_http: true,
        endpoint: format!("http://{address}/v1/chat/completions"),
        health_endpoint: format!("http://{address}/health"),
        model: "mock".into(),
        binary_path: "/nonexistent/llama-cli".into(),
        model_path: "/nonexistent/model.gguf".into(),
        threads: 1,
        context_size: 64,
        predict_tokens: 16,
        timeout_secs: 5,
        retries: 0,
        stream: false,
    });

    let response = adapter.infer("hello", false).unwrap();
    handle.join().unwrap();

    assert_eq!(response, "offline hello");
}

#[test]
fn llama_cpp_chat_payload_includes_system_and_user_messages() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0u8; 4096];
        let size = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..size]);
        assert!(request.contains("\"role\":\"system\""));
        assert!(request.contains("\"content\":\"Stay local\""));
        assert!(request.contains("\"role\":\"user\""));
        assert!(request.contains("\"content\":\"hello\""));
        let body = r#"{"choices":[{"message":{"role":"assistant","content":"offline hello"}}]}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let adapter = LlamaCppAdapter::new(LlmConfig {
        prefer_http: true,
        endpoint: format!("http://{address}/v1/chat/completions"),
        health_endpoint: format!("http://{address}/health"),
        model: "mock".into(),
        binary_path: "/nonexistent/llama-cli".into(),
        model_path: "/nonexistent/model.gguf".into(),
        threads: 1,
        context_size: 64,
        predict_tokens: 16,
        timeout_secs: 5,
        retries: 0,
        stream: false,
    });

    let response = adapter.infer_chat("Stay local", "hello", false).unwrap();
    handle.join().unwrap();

    assert_eq!(response, "offline hello");
}
