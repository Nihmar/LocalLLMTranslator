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

/// Client bound to a single `llama-server` base URL (without the `/v1` suffix).
#[derive(Clone)]
pub struct LlamaClient {
    base_url: String,
    api_key: Option<String>,
    http: reqwest::Client,
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
        })
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
    pub async fn token_counts(&self, texts: &[&str]) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for text in texts {
            if text.is_empty() {
                continue;
            }
            if let Ok(Some(tokens)) = self.tokenize(text).await {
                counts.insert((*text).to_string(), tokens.len());
            }
        }
        counts
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
                            // A metadata-only event (no content, no finish reason)
                            // is not worth surfacing on its own.
                            if delta.content.is_empty()
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
                    match st.inner.next().await {
                        Some(Ok(chunk)) => st.buf.extend_from_slice(&chunk),
                        Some(Err(e)) => {
                            st.finished = true;
                            return Some((Err(e.into()), st));
                        }
                        None => {
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
}

impl StreamChunk {
    fn into_delta(self) -> Delta {
        let StreamChunk { choices, usage } = self;
        let choice = choices.into_iter().next();
        let (content, finish_reason) = match choice {
            Some(c) => (
                c.delta
                    .as_ref()
                    .and_then(|d| d.content.clone())
                    .unwrap_or_default(),
                c.finish_reason,
            ),
            None => (String::new(), None),
        };
        Delta {
            content,
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
