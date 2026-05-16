use serde::{Deserialize, Serialize};

use crate::config::Config;

const DEFAULT_MAX_TOKENS: u32 = 64;
const MAX_MAX_TOKENS: u32 = 256;
const DEFAULT_TEMPERATURE: f32 = 0.7;
const DEFAULT_TOP_P: f32 = 0.95;

#[derive(Debug, Clone, Deserialize)]
pub struct ChatCompletionRequest {
    #[serde(default)]
    pub model: Option<String>,
    pub messages: Vec<ChatMessageInput>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    #[serde(default = "default_top_p")]
    pub top_p: f32,
    #[serde(default)]
    pub stop: Option<StopSequence>,
    #[serde(default)]
    pub stream: Option<bool>,
}

impl ChatCompletionRequest {
    pub fn validate(&self, config: &Config) -> Result<(), String> {
        if let Some(model) = &self.model {
            if model != &config.model_alias {
                return Err(format!(
                    "model must match configured alias '{}'",
                    config.model_alias
                ));
            }
        }

        if self.messages.is_empty() {
            return Err("messages must contain at least one item".to_string());
        }

        for message in &self.messages {
            if message.content.trim().is_empty() {
                return Err("message content must be non-empty".to_string());
            }
        }

        if self.max_tokens == 0 {
            return Err("max_tokens must be greater than 0".to_string());
        }

        if self.max_tokens > MAX_MAX_TOKENS {
            return Err(format!(
                "max_tokens must be less than or equal to {MAX_MAX_TOKENS}"
            ));
        }

        if self.temperature < 0.0 {
            return Err("temperature must be non-negative".to_string());
        }

        if !(0.0 < self.top_p && self.top_p <= 1.0) {
            return Err("top_p must be greater than 0 and less than or equal to 1".to_string());
        }

        if let Some(stop) = &self.stop {
            let invalid = stop.sequences().iter().any(|value| value.trim().is_empty());
            if invalid {
                return Err("stop sequences must be non-empty".to_string());
            }
        }

        Ok(())
    }

    pub fn effective_model<'a>(&'a self, config: &'a Config) -> &'a str {
        self.model.as_deref().unwrap_or(&config.model_alias)
    }
}

fn default_max_tokens() -> u32 {
    DEFAULT_MAX_TOKENS
}

fn default_temperature() -> f32 {
    DEFAULT_TEMPERATURE
}

fn default_top_p() -> f32 {
    DEFAULT_TOP_P
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct ChatMessageInput {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum StopSequence {
    Single(String),
    Multiple(Vec<String>),
}

impl StopSequence {
    pub fn sequences(&self) -> Vec<&str> {
        match self {
            Self::Single(value) => vec![value.as_str()],
            Self::Multiple(values) => values.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatCompletionChoice {
    pub index: u32,
    pub message: ChatMessageOutput,
    pub finish_reason: FinishReason,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatMessageOutput {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatCompletionChunkResponse {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatCompletionChunkChoice {
    pub index: u32,
    pub delta: ChatMessageDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatMessageDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<ChatRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    #[allow(dead_code)]
    Length,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HealthResponse {
    pub ok: bool,
    pub status: &'static str,
    pub model: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorEnvelope {
    pub error: ErrorBody,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ErrorBody {
    pub message: String,
    #[serde(rename = "type")]
    pub error_type: String,
    pub code: String,
}
