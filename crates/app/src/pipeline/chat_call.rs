//! One chat call on any role: optional structured output, SSE streaming, audit.
//!
//! Shared by the book reconnaissance (PLAN.md §9.4), the rolling summaries (§8),
//! the bilingual editor and the proofreader (§8): all of them stream from
//! `llama-server` and must leave the same `llm_call` audit trail. The translation
//! path uses it too. Only the `response_format` differs — a JSON schema for the
//! structured passes, `None` for the translator's plain text answer.

use std::time::Instant;

use futures_util::StreamExt;
use serde_json::Value;

use super::PipelineDeps;
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::llm::{ChatMessage, ChatRequest, LlamaClient, ResponseFormat};
use crate::util::sha256_hex_str;

/// Everything one call needs. `params_json` carries the role binding's sampling
/// parameters; `default_max_tokens` applies when they do not set one (`None`
/// leaves the server default). Besides the sampling values the binding may carry
/// `chat_template_kwargs`, forwarded verbatim to the server: it is how a role
/// turns a reasoning model's thinking off (`{"enable_thinking": false}`).
#[derive(Debug, Clone)]
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

/// Run the call and return the answer alone.
pub async fn run_chat_call(deps: &PipelineDeps, call: &ChatCall<'_>) -> Result<String> {
    Ok(run_chat_call_full(deps, call).await?.content)
}

/// What one call produced, beyond the text the callers parse: enough to explain a
/// reply that carried no answer without quoting it.
#[derive(Debug, Clone, Default)]
pub struct ChatAnswer {
    /// The answer as streamed — the thinking of a reasoning model is never in it.
    pub content: String,
    /// The thinking, when the model streams one (`reasoning_content`).
    pub reasoning: String,
    pub finish_reason: Option<String>,
    /// The completion budget the call asked for, the binding's `max_tokens`
    /// included when it set one. The server default is `None`.
    pub max_tokens: Option<u32>,
}

impl ChatAnswer {
    /// True when the server stopped because the completion budget ran out.
    pub fn stopped_on_budget(&self) -> bool {
        self.finish_reason.as_deref() == Some("length")
    }

    /// A short description of the outcome, for an error message: it never carries
    /// the text, so it is safe in the log file and in the UI.
    pub fn describe(&self) -> String {
        let stop = self.finish_reason.as_deref().unwrap_or("unknown");
        let budget = match self.max_tokens {
            Some(max) => max.to_string(),
            None => "server default".to_string(),
        };
        format!(
            "finish_reason={stop}, answer={} chars, reasoning={} chars, max_tokens={budget}",
            self.content.chars().count(),
            self.reasoning.chars().count(),
        )
    }
}

/// Does the text carry a JSON object at all? The parsers look for the outermost
/// `{...}`; a reply without one cannot be repaired by parsing differently.
pub fn contains_json_object(text: &str) -> bool {
    matches!((text.find('{'), text.rfind('}')), (Some(start), Some(end)) if end > start)
}

/// Told to the model when its first answer carried no JSON object.
const RETRY_NO_JSON: &str = "IMPORTANT: your previous answer contained no JSON object. \
Answer with a single JSON object only, matching the schema in the message above, and output \
nothing else.";

/// Ceiling for the budget a retry may ask for: the per-role defaults are small
/// (900-1500 tokens), which a reasoning model spends on its thinking alone.
const RETRY_MAX_TOKENS_CEILING: u32 = 8192;

/// Run a structured call, retrying it once when the answer carries no JSON object
/// at all. A reply that was merely truncated (a `{` without its closing brace) is
/// not retried: the same call would truncate again.
pub async fn run_structured_call<T>(
    deps: &PipelineDeps,
    call: &ChatCall<'_>,
    parse: impl Fn(&str) -> Result<T>,
) -> Result<(ChatAnswer, T)> {
    let answer = run_chat_call_full(deps, call).await?;
    if contains_json_object(&answer.content) {
        let value = parse(&answer.content).map_err(|error| with_outcome(error, &answer, false))?;
        return Ok((answer, value));
    }

    let user = format!("{}\n\n{RETRY_NO_JSON}", call.user);
    let prompt_hash = sha256_hex_str(&format!("{}\n\u{0}\n{user}", call.system));
    let retry = ChatCall {
        user: &user,
        prompt_hash: &prompt_hash,
        default_max_tokens: retry_budget(call, &answer),
        ..call.clone()
    };
    let retried = run_chat_call_full(deps, &retry).await?;
    let value = parse(&retried.content).map_err(|error| with_outcome(error, &retried, true))?;
    Ok((retried, value))
}

/// A rejected answer keeps its message and gains the outcome of the call: the stop
/// reason and the sizes say whether the model refused, ran out of budget or spent it
/// thinking, and none of that quotes the text.
fn with_outcome(error: AppError, answer: &ChatAnswer, after_retry: bool) -> AppError {
    match error {
        AppError::Invalid(message) => {
            let retried = if after_retry { ", retried once" } else { "" };
            AppError::Invalid(format!("{message} ({}{retried})", answer.describe()))
        }
        other => other,
    }
}

/// The budget of a retry: only a call that ran out of tokens gets a larger one, and
/// a `max_tokens` chosen by the binding is respected.
fn retry_budget(call: &ChatCall<'_>, answer: &ChatAnswer) -> Option<u32> {
    if !answer.stopped_on_budget() {
        return call.default_max_tokens;
    }
    let params: Value = serde_json::from_str(call.params_json).unwrap_or(Value::Null);
    if params.get("max_tokens").and_then(Value::as_u64).is_some() {
        return call.default_max_tokens;
    }
    call.default_max_tokens
        .map(|cap| cap.saturating_mul(2).min(RETRY_MAX_TOKENS_CEILING))
}

/// The `chat_template_kwargs` to send. A structured pass turns a reasoning model's
/// thinking off unless the binding says otherwise: the thinking spends the small
/// JSON budget before any answer (every orchestrator call of the real project ended
/// `finish_reason=length` until the binding set it by hand). A model without a
/// thinking switch ignores the variable.
fn template_kwargs(params: &Value, structured: bool) -> Option<Value> {
    let configured = params.get("chat_template_kwargs").cloned();
    if !structured {
        return configured;
    }
    let mut kwargs = match configured {
        Some(Value::Object(map)) => map,
        Some(other) => return Some(other),
        None => serde_json::Map::new(),
    };
    kwargs
        .entry("enable_thinking")
        .or_insert(Value::Bool(false));
    Some(Value::Object(kwargs))
}

/// Run one call and return everything it produced. Success and failure are both
/// recorded in `llm_call`, so a job can be audited after the fact.
pub async fn run_chat_call_full(deps: &PipelineDeps, call: &ChatCall<'_>) -> Result<ChatAnswer> {
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
    request.chat_template_kwargs = template_kwargs(&params, call.response_format.is_some());
    let max_tokens = request.max_tokens;

    let started = Instant::now();
    let mut content = String::new();
    // A reasoning model streams its thinking before the answer: kept apart, never
    // parsed as the answer (the translator would otherwise see the thinking as text).
    let mut reasoning = String::new();
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
            reasoning.push_str(&delta.reasoning);
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

    // One line per call in the log file: role, model, tokens and outcome, never the
    // prompt or the answer (those live in `llm_call`).
    match &stream_result {
        Ok(()) => tracing::info!(
            role = call.role,
            model = call.model,
            endpoint_id = call.endpoint_id,
            chunk_id = call.chunk_id.unwrap_or(""),
            latency_ms,
            finish_reason = finish_reason.as_deref().unwrap_or(""),
            prompt_tokens = usage
                .as_ref()
                .and_then(|value| value.prompt_tokens)
                .unwrap_or(0),
            completion_tokens = usage
                .as_ref()
                .and_then(|value| value.completion_tokens)
                .unwrap_or(0),
            answer_chars = content.chars().count(),
            reasoning_chars = reasoning.chars().count(),
            "llm call completed"
        ),
        Err(error) => tracing::warn!(
            role = call.role,
            model = call.model,
            endpoint_id = call.endpoint_id,
            chunk_id = call.chunk_id.unwrap_or(""),
            latency_ms,
            answer_chars = content.chars().count(),
            reasoning_chars = reasoning.chars().count(),
            %error,
            "llm call failed"
        ),
    }

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
                (!reasoning.is_empty()).then_some(reasoning.as_str()),
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
            Ok(ChatAnswer {
                content,
                reasoning,
                finish_reason,
                max_tokens,
            })
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
                (!reasoning.is_empty()).then_some(reasoning.as_str()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_calls_turn_thinking_off_unless_the_binding_decides() {
        let none = serde_json::json!({ "temperature": 0.2 });
        assert_eq!(
            template_kwargs(&none, true),
            Some(serde_json::json!({ "enable_thinking": false }))
        );
        // A plain text call (the translator) is left as configured.
        assert_eq!(template_kwargs(&none, false), None);

        let chosen = serde_json::json!({
            "chat_template_kwargs": { "enable_thinking": true, "other": 1 }
        });
        assert_eq!(
            template_kwargs(&chosen, true),
            Some(serde_json::json!({ "enable_thinking": true, "other": 1 }))
        );

        let other = serde_json::json!({ "chat_template_kwargs": { "other": 1 } });
        assert_eq!(
            template_kwargs(&other, true),
            Some(serde_json::json!({ "other": 1, "enable_thinking": false }))
        );
    }

    fn call(params_json: &str, default_max_tokens: Option<u32>) -> ChatCall<'_> {
        ChatCall {
            job_id: None,
            chunk_id: None,
            role: "editor",
            endpoint_id: "e",
            base_url: "http://127.0.0.1:1",
            model: "m",
            params_json,
            prompt_hash: "h",
            system: "s",
            user: "u",
            response_format: None,
            seed: 0,
            default_max_tokens,
        }
    }

    fn answer(finish_reason: &str, content: &str, reasoning: &str) -> ChatAnswer {
        ChatAnswer {
            content: content.to_string(),
            reasoning: reasoning.to_string(),
            finish_reason: Some(finish_reason.to_string()),
            max_tokens: Some(1500),
        }
    }

    #[test]
    fn a_json_object_needs_both_braces_in_order() {
        assert!(contains_json_object("{\"verdict\":\"ok\",\"issues\":[]}"));
        assert!(contains_json_object("```json\n{\"a\":1}\n```"));
        assert!(contains_json_object("{}"));
        // Prose, a lone brace and a reversed pair are all "no JSON object".
        assert!(!contains_json_object("I could not decide."));
        assert!(!contains_json_object(""));
        assert!(!contains_json_object("a { alone"));
        assert!(!contains_json_object("} {"));
    }

    #[test]
    fn a_budget_stop_doubles_the_default_but_never_the_bindings_cap() {
        let stopped = answer("length", "", "thinking");
        assert_eq!(retry_budget(&call("{}", Some(1500)), &stopped), Some(3000));
        assert_eq!(
            retry_budget(&call("{}", Some(8000)), &stopped),
            Some(RETRY_MAX_TOKENS_CEILING)
        );
        // An explicit max_tokens of the binding is respected.
        assert_eq!(
            retry_budget(&call("{\"max_tokens\":1500}", Some(1500)), &stopped),
            Some(1500)
        );
        // A stop that is not the budget keeps the same cap.
        let stopped_early = answer("stop", "prose", "");
        assert_eq!(
            retry_budget(&call("{}", Some(1500)), &stopped_early),
            Some(1500)
        );
    }

    #[test]
    fn the_description_reports_the_stop_the_sizes_and_the_budget() {
        let described = answer("length", "", "peso bene").describe();
        assert_eq!(
            described,
            "finish_reason=length, answer=0 chars, reasoning=9 chars, max_tokens=1500"
        );
        assert!(answer("length", "", "").stopped_on_budget());
        assert!(!answer("stop", "{}", "").stopped_on_budget());
    }
}
