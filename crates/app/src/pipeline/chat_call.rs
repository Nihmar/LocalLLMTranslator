//! One chat call on any role: optional structured output, SSE streaming, audit.
//!
//! Shared by the book reconnaissance (PLAN.md §9.4), the rolling summaries (§8),
//! the bilingual editor and the proofreader (§8): all of them stream from
//! `llama-server` and must leave the same `llm_call` audit trail. The translation
//! path uses it too. Only the `response_format` differs — a JSON schema for the
//! structured passes, `None` for a plain text answer like the proofreader's.

use std::time::Instant;

use futures_util::StreamExt;
use serde_json::Value;

use super::PipelineDeps;
use crate::db::repo;
use crate::error::Result;
use crate::llm::{ChatMessage, ChatRequest, LlamaClient, ResponseFormat};

/// Everything one call needs. `params_json` carries the role binding's sampling
/// parameters; `default_max_tokens` applies when they do not set one (`None`
/// leaves the server default).
pub struct ChatCall<'a> {
    pub job_id: Option<&'a str>,
    /// The chunk the call belongs to, when there is one (reconnaissance and
    /// summary calls have none).
    pub chunk_id: Option<&'a str>,
    pub role: &'a str,
    pub endpoint_id: &'a str,
    pub base_url: &'a str,
    pub model: &'a str,
    pub params_json: &'a str,
    pub prompt_hash: &'a str,
    pub system: &'a str,
    pub user: &'a str,
    /// `Some` for the structured passes, `None` for a plain text answer.
    pub response_format: Option<ResponseFormat>,
    pub seed: i64,
    pub default_max_tokens: Option<u32>,
}

/// Run the call and return the accumulated content. Success and failure are
/// both recorded in `llm_call`, so a job can be audited after the fact.
pub async fn run_chat_call(deps: &PipelineDeps, call: &ChatCall<'_>) -> Result<String> {
    let client = LlamaClient::new(call.base_url)?;
    let params: Value = serde_json::from_str(call.params_json).unwrap_or(Value::Null);

    let mut request = ChatRequest::new(
        call.model,
        vec![
            ChatMessage::system(call.system),
            ChatMessage::user(call.user),
        ],
    );
    request.seed = Some(call.seed);
    request.temperature = params
        .get("temperature")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    request.top_p = params
        .get("top_p")
        .and_then(Value::as_f64)
        .map(|value| value as f32);
    request.max_tokens = params
        .get("max_tokens")
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .or(call.default_max_tokens);
    request.response_format = call.response_format.clone();
    if let Some(grammar) = params.get("grammar").and_then(Value::as_str) {
        request.grammar = Some(grammar.to_string());
    }

    let started = Instant::now();
    let mut content = String::new();
    let mut finish_reason = None;
    let mut usage = None;

    let stream_result: Result<()> = async {
        let mut stream = client.chat_stream(request).await?;
        while let Some(item) = stream.next().await {
            let delta = item?;
            if delta.done {
                break;
            }
            content.push_str(&delta.content);
            if delta.finish_reason.is_some() {
                finish_reason = delta.finish_reason;
            }
            if delta.usage.is_some() {
                usage = delta.usage;
            }
        }
        Ok(())
    }
    .await;
    let latency_ms = started.elapsed().as_millis() as i64;

    match stream_result {
        Ok(()) => {
            repo::insert_llm_call(
                &deps.pool,
                call.job_id,
                call.chunk_id,
                call.role,
                Some(call.endpoint_id),
                call.model,
                call.params_json,
                Some(call.seed),
                call.prompt_hash,
                Some(call.user),
                Some(&content),
                finish_reason.as_deref(),
                usage
                    .as_ref()
                    .and_then(|value| value.prompt_tokens)
                    .map(|value| value as i64),
                usage
                    .as_ref()
                    .and_then(|value| value.completion_tokens)
                    .map(|value| value as i64),
                Some(latency_ms),
                1,
                None,
            )
            .await?;
            Ok(content)
        }
        Err(error) => {
            repo::insert_llm_call(
                &deps.pool,
                call.job_id,
                call.chunk_id,
                call.role,
                Some(call.endpoint_id),
                call.model,
                call.params_json,
                None,
                call.prompt_hash,
                Some(call.user),
                None,
                None,
                None,
                None,
                Some(latency_ms),
                1,
                Some(&error.to_string()),
            )
            .await?;
            Err(error)
        }
    }
}
