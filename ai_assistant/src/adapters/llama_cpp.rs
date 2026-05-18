use std::{path::Path, process::Command};

use crate::{
    config::LlmConfig,
    util::{json_escape, truncate},
};

#[derive(Clone, Debug)]
pub struct LlamaCppAdapter {
    config: LlmConfig,
}

impl LlamaCppAdapter {
    pub fn new(config: LlmConfig) -> Self {
        Self { config }
    }

    pub fn health_check(&self) -> Result<bool, String> {
        if self.local_cli_ready() && !self.config.prefer_http {
            return Ok(true);
        }
        let output = Command::new("curl")
            .args([
                "-sS",
                "--max-time",
                &self.config.timeout_secs.to_string(),
                &self.config.health_endpoint,
            ])
            .output()
            .map_err(|error| format!("failed to run curl health check: {error}"))?;
        if output.status.success() {
            return Ok(true);
        }
        Ok(self.local_cli_ready())
    }

    pub fn infer(&self, prompt: &str, stream: bool) -> Result<String, String> {
        if self.local_cli_ready() && !self.config.prefer_http {
            return self.infer_local_cli(prompt);
        }

        self.infer_http("", prompt, stream)
    }

    pub fn infer_chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        stream: bool,
    ) -> Result<String, String> {
        if self.local_cli_ready() && !self.config.prefer_http {
            return self.infer_local_cli_chat(system_prompt, user_prompt);
        }

        self.infer_http(system_prompt, user_prompt, stream)
    }

    fn infer_http(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        stream: bool,
    ) -> Result<String, String> {
        let payload = if system_prompt.trim().is_empty() {
            format!(
                "{{\"model\":\"{}\",\"messages\":[{{\"role\":\"user\",\"content\":\"{}\"}}],\"stream\":{}}}",
                json_escape(&self.config.model),
                json_escape(user_prompt),
                if stream { "true" } else { "false" }
            )
        } else {
            format!(
                "{{\"model\":\"{}\",\"messages\":[{{\"role\":\"system\",\"content\":\"{}\"}},{{\"role\":\"user\",\"content\":\"{}\"}}],\"stream\":{}}}",
                json_escape(&self.config.model),
                json_escape(system_prompt),
                json_escape(user_prompt),
                if stream { "true" } else { "false" }
            )
        };

        let mut last_error = String::new();
        for _ in 0..=self.config.retries {
            let mut command = Command::new("curl");
            command.args([
                "-sS",
                "--connect-timeout",
                "2",
                "--max-time",
                &self.config.timeout_secs.to_string(),
                "-H",
                "content-type: application/json",
                "-d",
                &payload,
                &self.config.endpoint,
            ]);
            if stream {
                command.arg("-N");
            }
            match command.output() {
                Ok(output) if output.status.success() => {
                    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if stream {
                        return Ok(raw);
                    }
                    if let Some(content) = extract_assistant_content(&raw) {
                        return Ok(content);
                    }
                    return Ok(raw);
                }
                Ok(output) => {
                    last_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
                }
                Err(error) => {
                    last_error = error.to_string();
                }
            }
        }

        if self.local_cli_ready() {
            return self.infer_local_cli_chat(system_prompt, user_prompt);
        }

        Err(format!(
            "llama.cpp request failed after {} attempts: {}",
            self.config.retries + 1,
            truncate(&last_error, 240)
        ))
    }

    fn infer_local_cli(&self, prompt: &str) -> Result<String, String> {
        let output = Command::new(&self.config.binary_path)
            .args([
                "-m",
                &self.config.model_path,
                "--single-turn",
                "--no-display-prompt",
                "--simple-io",
                "--threads",
                &self.config.threads.to_string(),
                "--ctx-size",
                &self.config.context_size.to_string(),
                "--n-predict",
                &self.config.predict_tokens.to_string(),
                "--prompt",
                prompt,
            ])
            .output()
            .map_err(|error| format!("failed to run local llama-cli fallback: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "local llama-cli fallback failed: {}",
                truncate(&String::from_utf8_lossy(&output.stderr), 240)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Err("local llama-cli fallback returned empty output".to_string());
        }
        let cleaned = clean_local_cli_output(&stdout);
        if cleaned.is_empty() {
            Ok(stdout)
        } else {
            Ok(cleaned)
        }
    }

    fn infer_local_cli_chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, String> {
        let mut command = Command::new(&self.config.binary_path);
        command.args([
            "-m",
            &self.config.model_path,
            "--conversation",
            "--single-turn",
            "--no-display-prompt",
            "--simple-io",
            "--threads",
            &self.config.threads.to_string(),
            "--ctx-size",
            &self.config.context_size.to_string(),
            "--n-predict",
            &self.config.predict_tokens.to_string(),
        ]);
        if !system_prompt.trim().is_empty() {
            command.args(["--system-prompt", system_prompt]);
        }
        command.args(["--prompt", user_prompt]);
        let output = command
            .output()
            .map_err(|error| format!("failed to run local llama-cli chat mode: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "local llama-cli chat mode failed: {}",
                truncate(&String::from_utf8_lossy(&output.stderr), 240)
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            return Err("local llama-cli chat mode returned empty output".to_string());
        }
        let cleaned = clean_local_cli_output(&stdout);
        if cleaned.is_empty() {
            Ok(stdout)
        } else {
            Ok(cleaned)
        }
    }

    fn local_cli_ready(&self) -> bool {
        Path::new(&self.config.binary_path).exists() && Path::new(&self.config.model_path).exists()
    }
}

fn extract_assistant_content(response: &str) -> Option<String> {
    if let Some(index) = response.find("\"content\":\"") {
        let tail = &response[index + "\"content\":\"".len()..];
        let mut value = String::new();
        let mut escaped = false;
        for ch in tail.chars() {
            if escaped {
                value.push(match ch {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '"' => '"',
                    '\\' => '\\',
                    other => other,
                });
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => return Some(value),
                other => value.push(other),
            }
        }
    }
    None
}

fn clean_local_cli_output(raw: &str) -> String {
    let normalized = raw.replace('\r', "");

    let after_prompt = if let Some(index) = normalized.find("\n> ") {
        &normalized[index + 3..]
    } else if let Some(stripped) = normalized.strip_prefix("> ") {
        stripped
    } else {
        normalized.as_str()
    };

    let mut candidate = if let Some(index) = after_prompt.find("\n\n") {
        &after_prompt[index + 2..]
    } else {
        after_prompt
    };

    let mut cut_index = candidate.len();
    for marker in ["\n[ Prompt:", "\n> ", "\nExiting...", "\nConnection to "] {
        if let Some(index) = candidate.find(marker) {
            cut_index = cut_index.min(index);
        }
    }
    candidate = &candidate[..cut_index];

    candidate
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty()
                && !trimmed.starts_with("Loading model")
                && !trimmed.starts_with("build      :")
                && !trimmed.starts_with("model      :")
                && !trimmed.starts_with("modalities :")
                && trimmed != "available commands:"
                && !trimmed.starts_with('/')
                && trimmed.chars().any(|ch| ch.is_ascii_alphanumeric())
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use crate::{config::LlmConfig, util::unique_temp_dir};

    use super::{LlamaCppAdapter, clean_local_cli_output, extract_assistant_content};

    #[test]
    fn extracts_content_from_chat_completion() {
        let response = r#"{"choices":[{"message":{"role":"assistant","content":"hello"}}]}"#;
        assert_eq!(
            extract_assistant_content(response).as_deref(),
            Some("hello")
        );
    }

    #[test]
    fn falls_back_to_local_llama_cli_when_http_is_unavailable() {
        let root = unique_temp_dir("assistant-llama-cli");
        let script = root.join("llama-cli");
        let model = root.join("model.gguf");
        fs::write(&script, "#!/bin/sh\nprintf 'local fallback response\\n'\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&model, "mock").unwrap();

        let adapter = LlamaCppAdapter::new(LlmConfig {
            prefer_http: false,
            endpoint: "http://127.0.0.1:9/v1/chat/completions".into(),
            health_endpoint: "http://127.0.0.1:9/health".into(),
            model: "mock".into(),
            binary_path: script.to_string_lossy().to_string(),
            model_path: model.to_string_lossy().to_string(),
            threads: 1,
            context_size: 64,
            predict_tokens: 16,
            timeout_secs: 1,
            retries: 0,
            stream: false,
        });

        let response = adapter.infer("hello", false).unwrap();
        assert_eq!(response, "local fallback response");
    }

    #[test]
    fn strips_llama_cli_transcript_noise_from_local_output() {
        let raw = r#"Loading model...

▄▄ ▄▄
██ ██

build      : b9146-320a6a44a
model      : SmolLM2-135M-Instruct.Q4_K_M.gguf
modalities : text

available commands:
  /exit or Ctrl+C     stop or exit

> ## Identity Layer
Assistant: Kumo
... (truncated)

Augmenting assistant

[ Prompt: 10.4 t/s | Generation: 4.4 t/s ]

Exiting..."#;

        assert_eq!(clean_local_cli_output(raw), "Augmenting assistant");
    }
}
