//! LLM layer: HTTP/SSE client for external `llama-server` processes.

pub mod client;
pub mod health;
pub mod types;

pub use client::LlamaClient;
pub use health::{probe, EndpointHealth};
pub use types::{
    ChatMessage, ChatRequest, Delta, HealthStatus, ModelInfo, Props, ResponseFormat, TokenUsage,
};
