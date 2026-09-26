//! Wire types for the `llama-server` OpenAI-compatible API.

use serde::{Deserialize, Serialize};

/// A single chat message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: content.into(),
        }
    }
}

/// `response_format` payload. Supports `json_schema` (structured output) and the
/// generic `text` / `json_object` modes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ResponseFormat {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json_schema: Option<JsonSchemaSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonSchemaSpec {
    pub name: String,
    pub schema: serde_json::Value,
}

impl ResponseFormat {
    pub fn json_schema(name: impl Into<String>, schema: serde_json::Value) -> Self {
        Self {
            kind: "json_schema".into(),
            json_schema: Some(JsonSchemaSpec {
                name: name.into(),
                schema,
            }),
        }
    }

    pub fn json_object() -> Self {
        Self {
            kind: "json_object".into(),
            json_schema: None,
        }
    }
}

/// Chat completion request. `stream` is always serialised; the raw `grammar`
/// field carries a GBNF grammar when the caller prefers it over `response_format`.
#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResponseFormat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grammar: Option<String>,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: true,
            temperature: None,
            top_p: None,
            max_tokens: None,
            seed: None,
            response_format: None,
            grammar: None,
        }
    }
}

/// Token accounting reported by the server, when present.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
    #[serde(default)]
    pub completion_tokens: Option<u64>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
}

/// A streamed fragment. Content fragments arrive as they are produced; the
/// terminal delta (`done == true`) carries the accumulated raw response text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Delta {
    /// The content fragment of this event (empty for metadata-only events).
    pub content: String,
    /// Present when the server closed the completion.
    pub finish_reason: Option<String>,
    /// Present when the server reports usage.
    pub usage: Option<TokenUsage>,
    /// Raw SSE payload(s) accumulated so far; only set on the terminal delta.
    pub raw: Option<String>,
    /// True for the terminal delta.
    pub done: bool,
}

/// `GET /props`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Props {
    #[serde(default)]
    pub total_slots: Option<u32>,
    #[serde(default)]
    pub n_ctx: Option<u32>,
    #[serde(default)]
    pub model_path: Option<String>,
    #[serde(default)]
    pub default_generation_settings: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub owned_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelsResponse {
    #[serde(default)]
    pub data: Vec<ModelInfo>,
}

/// Result of `GET /health`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HealthStatus {
    pub ok: bool,
    #[serde(default)]
    pub status: Option<String>,
    pub code: u16,
}
