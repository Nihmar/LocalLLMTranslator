//! HTTP + SSE client for an external `llama-server`.

use std::collections::HashMap;
use std::pin::Pin;
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;

use super::types::*;
use crate::error::{AppError, Result};

type ByteStream = Pin<Box<dyn Stream<Item = reqwest::Result<Bytes>> + Send + 'static>>;

/// How long a chat stream may stay silent before it is treated as stalled.
///
/// A server that accepts the request and then stops emitting SSE events would otherwise
/// block the job forever (the lease heartbeat keeps it alive). This is an *idle* bound, so
/// a slow but steadily-streaming generation is never killed.
pub const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// How many `/tokenize` requests a single chunk may have in flight.
const TOKENIZE_CONCURRENCY: usize = 4;

/// Client bound to a single `llama-server` base URL (without the `/v1` suffix).
#[derive(Clone)]
pub struct LlamaClient {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
    idle_timeout: Duration,
}

impl LlamaClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()?;
        Ok(Self {
            base_url: normalize_base(base_url.into()),
            api_key: None,
            http,
            idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
        })
    }

    /// Override the idle timeout of a chat stream (used by the tests).
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key.filter(|k| !k.is_empty());
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        let rb = self.http.request(method, url);
        match &self.api_key {
            Some(key) => rb.bearer_auth(key),
            None => rb,
        }
    }

    /// `GET /health`. Returns the parsed status even on non-2xx responses.
    pub async fn health(&self) -> Result<HealthStatus> {
        let resp = self.request(reqwest::Method::GET, "/health").send().await?;
        let code = resp.status().as_u16();
        let ok = resp.status().is_success();
        let status = match resp.text().await {
            Ok(text) if !text.is_empty() => serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| v.get("status").and_then(|s| s.as_str()).map(str::to_owned)),
            _ => None,
        };
        Ok(HealthStatus { ok, status, code })
    }

    /// `GET /props`.
    pub async fn props(&self) -> Result<Props> {
        let resp = self.request(reqwest::Method::GET, "/props").send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(AppError::Endpoint {
                status: status.as_u16(),
                body: truncate(&text, 512),
            });
        }
        let props: Props = serde_json::from_str(&text).unwrap_or_default();
        Ok(props)
    }

    /// `GET /v1/models`.
    pub async fn models(&self) -> Result<Vec<ModelInfo>> {
        let resp = self
            .request(reqwest::Method::GET, "/v1/models")
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(AppError::Endpoint {
                status: status.as_u16(),
                body: truncate(&text, 512),
            });
        }
        let parsed: ModelsResponse = serde_json::from_str(&text).unwrap_or_default();
        Ok(parsed.data)
    }

    /// `POST /tokenize`. Returns `None` when the endpoint does not implement the
    /// route (some builds return 404/405/501), so callers can fall back to the
    /// heuristic estimate.
    pub async fn tokenize(&self, text: &str) -> Result<Option<Vec<u32>>> {
        let resp = self
            .request(reqwest::Method::POST, "/tokenize")
            .json(&serde_json::json!({ "content": text }))
            .send()
            .await?;
        let status = resp.status();
        if matches!(status.as_u16(), 404 | 405 | 501) {
            return Ok(None);
        }
        let text_body = resp.text().await?;
        if !status.is_success() {
            return Err(AppError::Endpoint {
                status: status.as_u16(),
                body: truncate(&text_body, 512),
            });
        }
        #[derive(Deserialize)]
        struct Tokens {
            #[serde(default)]
            tokens: Vec<u32>,
        }
        let parsed: Tokens = serde_json::from_str(&text_body).unwrap_or(Tokens { tokens: vec![] });
        if parsed.tokens.is_empty() {
            Ok(None)
        } else {
            Ok(Some(parsed.tokens))
        }
    }

    /// Exact token counts via `/tokenize`, one entry per text the server
    /// answered for. Texts the endpoint refuses are simply absent, so the
    /// caller's counter can fall back to the heuristic estimate.
    ///
    /// The requests are issued concurrently (bounded) instead of one after the
    /// other: a chunk asks for every prompt piece, and a long book would otherwise
    /// wait for a sequential round trip per piece before each translation.
    pub async fn token_counts(&self, texts: &[&str]) -> HashMap<String, usize> {
        // Each future owns a cloned client and its text, so the stream is `Send` and the
        // requests run up to `TOKENIZE_CONCURRENCY` at a time.
        let pending: Vec<_> = texts
            .iter()
            .copied()
            .filter(|text| !text.is_empty())
            .map(|text| {
                let client = self.clone();
                let text = text.to_string();
                async move {
                    match client.tokenize(&text).await {
                        Ok(Some(tokens)) => Some((text, tokens.len())),
                        _ => None,
                    }
                }
            })
            .collect();
        stream::iter(pending)
            .buffer_unordered(TOKENIZE_CONCURRENCY)
            .filter_map(|entry| async move { entry })
            .collect()
            .await
    }

    /// `POST /v1/chat/completions` with `stream: true`, parsed as SSE.
    ///
    /// The returned stream is boxed so callers can hold it across `.await`
    /// without pinning concerns.
    pub async fn chat_stream(
        &self,
        req: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<Delta>> + Send + 'static>>> {
        let resp = self
            .request(reqwest::Method::POST, "/v1/chat/completions")
            .json(&req)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Endpoint {
                status: status.as_u16(),
                body: truncate(&body, 512),
            });
        }
        let state = SseState {
            inner: Box::pin(resp.bytes_stream()),
            buf: Vec::new(),
            raw: String::new(),
            finished: false,
            idle: self.idle_timeout,
        };
        let stream = stream::unfold(state, |mut st| async move {
            if st.finished {
                return None;
            }
            loop {
                if let Some(pos) = st.buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = st.buf.drain(..=pos).collect();
                    let line = String::from_utf8_lossy(&line);
                    let line = line.trim_end_matches(['\r', '\n']);
                    let Some(payload) = line.strip_prefix("data:") else {
                        continue;
                    };
                    let payload = payload.trim();
                    if payload == "[DONE]" {
                        st.finished = true;
                        let raw = std::mem::take(&mut st.raw);
                        return Some((
                            Ok(Delta {
                                done: true,
                                raw: Some(raw),
                                ..Default::default()
                            }),
                            st,
                        ));
                    }
                    if payload.is_empty() {
                        continue;
                    }
                    st.raw.push_str(payload);
                    st.raw.push('\n');
                    match serde_json::from_str::<StreamChunk>(payload) {
                        Ok(chunk) => {
                            let delta = chunk.into_delta();
                            // A metadata-only event (no content, no reasoning, no
                            // finish reason) is not worth surfacing on its own.
                            if delta.content.is_empty()
                                && delta.reasoning.is_empty()
                                && delta.finish_reason.is_none()
                                && delta.usage.is_none()
                            {
                                continue;
                            }
                            return Some((Ok(delta), st));
                        }
                        Err(e) => {
                            tracing::debug!(error = %e, "skipping malformed SSE data line");
                            continue;
                        }
                    }
                } else {
                    match tokio::time::timeout(st.idle, st.inner.next()).await {
                        Err(_) => {
                            st.finished = true;
                            return Some((Err(AppError::LlmTimeout(st.idle)), st));
                        }
                        Ok(Some(Ok(chunk))) => st.buf.extend_from_slice(&chunk),
                        Ok(Some(Err(e))) => {
                            st.finished = true;
                            return Some((Err(e.into()), st));
                        }
                        Ok(None) => {
                            st.finished = true;
                            // Flush a trailing, newline-less data line.
                            if !st.buf.is_empty() {
                                let trailing = String::from_utf8_lossy(&st.buf).trim().to_string();
                                st.buf.clear();
                                if let Some(p) = trailing.strip_prefix("data:") {
                                    let p = p.trim();
                                    if !p.is_empty() && p != "[DONE]" {
                                        st.raw.push_str(p);
                                        st.raw.push('\n');
                                    }
                                }
                            }
                            let raw = std::mem::take(&mut st.raw);
                            return Some((
                                Ok(Delta {
                                    done: true,
                                    raw: Some(raw),
                                    ..Default::default()
                                }),
                                st,
                            ));
                        }
                    }
                }
            }
        });
        Ok(Box::pin(stream))
    }
}

struct SseState {
    inner: ByteStream,
    buf: Vec<u8>,
    raw: String,
    finished: bool,
    idle: Duration,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<StreamChoice>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct StreamChoice {
    #[serde(default)]
    delta: Option<StreamDeltaBody>,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct StreamDeltaBody {
    #[serde(default)]
    content: Option<String>,
    /// Thinking of a reasoning model: `llama-server` streams it separately from the
    /// answer, so it must not be dropped (it is the only clue when an answer is empty).
    #[serde(default)]
    reasoning_content: Option<String>,
}

impl StreamChunk {
    fn into_delta(self) -> Delta {
        let StreamChunk { choices, usage } = self;
        let choice = choices.into_iter().next();
        let (content, reasoning, finish_reason) = match choice {
            Some(c) => (
                c.delta
                    .as_ref()
                    .and_then(|d| d.content.clone())
                    .unwrap_or_default(),
                c.delta
                    .as_ref()
                    .and_then(|d| d.reasoning_content.clone())
                    .unwrap_or_default(),
                c.finish_reason,
            ),
            None => (String::new(), String::new(), None),
        };
        Delta {
            content,
            reasoning,
            finish_reason,
            usage,
            raw: None,
            done: false,
        }
    }
}

fn normalize_base(url: String) -> String {
    url.trim_end_matches('/').to_string()
}

/// Truncate an error body so it stays log-friendly.
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sse_body() -> String {
        [
            r#"data: {"choices":[{"delta":{"content":"Ciao "}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{"content":"mondo"}}]}"#,
            "",
            r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":2}}"#,
            "",
            "data: [DONE]",
            "",
        ]
        .join("\n")
    }

    #[tokio::test]
    async fn parses_sse_stream_into_deltas() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(sse_body()),
            )
            .mount(&server)
            .await;

        let client = LlamaClient::new(server.uri()).expect("client");
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        let mut stream = client.chat_stream(req).await.expect("stream");

        let mut content = String::new();
        let mut raw = String::new();
        let mut finish = None;
        let mut usage = None;
        let mut saw_done = false;
        while let Some(item) = stream.next().await {
            let delta = item.expect("delta");
            if delta.done {
                saw_done = true;
                raw = delta.raw.clone().unwrap_or_default();
            } else {
                content.push_str(&delta.content);
                if delta.finish_reason.is_some() {
                    finish = delta.finish_reason.clone();
                }
                if delta.usage.is_some() {
                    usage = delta.usage.clone();
                }
            }
        }
        assert_eq!(content, "Ciao mondo");
        assert_eq!(finish.as_deref(), Some("stop"));
        assert_eq!(usage.and_then(|u| u.completion_tokens), Some(2));
        assert!(saw_done);
        assert!(raw.contains("\"content\":\"Ciao \""));
    }

    /// A reasoning model streams `reasoning_content` before any `content`: those events
    /// carry no answer but must reach the caller, otherwise a budget exhausted while
    /// thinking looks like a server that answered nothing at all.
    #[tokio::test]
    async fn keeps_reasoning_content_separate_from_content() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/event-stream")
                    .set_body_string(
                        [
                            r#"data: {"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
                            r#"data: {"choices":[{"delta":{"reasoning_content":"peso "}}]}"#,
                            r#"data: {"choices":[{"delta":{"reasoning_content":"bene"}}]}"#,
                            r#"data: {"choices":[{"delta":{},"finish_reason":"length"}]}"#,
                            "data: [DONE]",
                            "",
                        ]
                        .join("\n\n"),
                    ),
            )
            .mount(&server)
            .await;

        let client = LlamaClient::new(server.uri()).expect("client");
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        let mut stream = client.chat_stream(req).await.expect("stream");

        let mut content = String::new();
        let mut reasoning = String::new();
        let mut finish = None;
        while let Some(item) = stream.next().await {
            let delta = item.expect("delta");
            if delta.done {
                break;
            }
            content.push_str(&delta.content);
            reasoning.push_str(&delta.reasoning);
            if delta.finish_reason.is_some() {
                finish = delta.finish_reason;
            }
        }
        assert_eq!(content, "");
        assert_eq!(reasoning, "peso bene");
        assert_eq!(finish.as_deref(), Some("length"));
    }

    /// The request body carries the per-request chat-template variables verbatim.
    #[tokio::test]
    async fn serialises_chat_template_kwargs() {
        let mut req = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        req.chat_template_kwargs = Some(serde_json::json!({ "enable_thinking": false }));
        let body = serde_json::to_value(&req).expect("serialize");
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);

        let plain = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        let body = serde_json::to_value(&plain).expect("serialize");
        assert!(body.get("chat_template_kwargs").is_none());
    }

    /// A server that sends one SSE event and then stalls must not hang the caller: the
    /// idle timeout turns it into a retryable error instead.
    #[tokio::test]
    async fn a_stalled_stream_times_out_instead_of_hanging() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut head = [0u8; 1024];
            let _ = socket.read(&mut head).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
                      data: {\"choices\":[{\"delta\":{\"content\":\"Ciao\"}}]}\n\n",
                )
                .await
                .expect("write");
            // Stay silent from here on: the body never ends.
            tokio::time::sleep(Duration::from_secs(30)).await;
        });

        let client = LlamaClient::new(format!("http://{addr}"))
            .expect("client")
            .with_idle_timeout(Duration::from_millis(150));
        let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
        let mut stream = client.chat_stream(req).await.expect("stream");

        let first = stream.next().await.expect("first item").expect("delta");
        assert_eq!(first.content, "Ciao");
        let stalled = stream.next().await.expect("second item");
        assert!(
            matches!(stalled, Err(AppError::LlmTimeout(_))),
            "expected an idle timeout, got {stalled:?}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn token_counts_covers_every_non_empty_text() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/tokenize"))
            .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"tokens":[1,2,3]}"#))
            .mount(&server)
            .await;
        let client = LlamaClient::new(server.uri()).expect("client");
        let counts = client.token_counts(&["a", "", "b c"]).await;
        assert_eq!(counts.len(), 2, "empty texts are skipped");
        assert_eq!(counts.get("a"), Some(&3));
        assert_eq!(counts.get("b c"), Some(&3));
    }

    #[tokio::test]
    async fn tokenize_returns_none_when_not_implemented() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/tokenize"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let client = LlamaClient::new(server.uri()).expect("client");
        assert_eq!(client.tokenize("hello").await.expect("call"), None);
    }

    #[tokio::test]
    async fn props_parses_known_fields() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/props"))
            .respond_with(ResponseTemplate::new(200).set_body_string(
                r#"{"total_slots":4,"n_ctx":32768,"model_path":"/m.gguf","other":1}"#,
            ))
            .mount(&server)
            .await;
        let client = LlamaClient::new(server.uri()).expect("client");
        let props = client.props().await.expect("props");
        assert_eq!(props.total_slots, Some(4));
        assert_eq!(props.n_ctx, Some(32768));
        assert_eq!(props.model_path.as_deref(), Some("/m.gguf"));
    }
}
