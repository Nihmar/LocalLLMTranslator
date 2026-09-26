//! JSON-RPC 2.0 over NDJSON on stdio: framing, id correlation, typed API.
//!
//! One request per line, one response per line, correlated by a numeric `id`.
//! Out-of-band `"method": "progress"` notifications are surfaced through an
//! [`EventSink`] so the Tauri layer can re-emit them as UI events.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt};

use crate::error::{AppError, Result};
use crate::sidecar::supervisor::Supervisor;

/// Default per-request timeout. Ingestion of a large PDF is the slowest call.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Maximum buffered bytes for a single unterminated line (16 MiB).
const MAX_LINE_LEN: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Splits a byte stream into complete newline-delimited lines.
///
/// Handles lines that arrive split across reads (partial lines are buffered
/// until their terminator arrives) and `\r\n` endings.
#[derive(Debug)]
pub struct LineFramer {
    buf: Vec<u8>,
}

impl Default for LineFramer {
    fn default() -> Self {
        Self::new()
    }
}

impl LineFramer {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed bytes; returns every line completed by this chunk.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            // Drop the trailing '\n' and an optional '\r'.
            let content = &line[..line.len() - 1];
            let text = String::from_utf8_lossy(content)
                .trim_end_matches('\r')
                .to_string();
            lines.push(text);
        }
        if self.buf.len() > MAX_LINE_LEN {
            tracing::warn!("dropping oversized sidecar line buffer");
            self.buf.clear();
        }
        lines
    }

    /// Bytes buffered but not yet terminated by a newline.
    pub fn pending_len(&self) -> usize {
        self.buf.len()
    }
}

// ---------------------------------------------------------------------------
// Message parsing
// ---------------------------------------------------------------------------

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RpcErrorObject {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcErrorObject {
    /// Errors whose side effect is nil, because the sidecar is stateless: the
    /// request can always be re-issued on a fresh process.
    pub fn is_retryable(&self) -> bool {
        matches!(self.code, -32603 | 1001 | 1002 | 1003)
    }
}

impl From<RpcErrorObject> for AppError {
    fn from(e: RpcErrorObject) -> Self {
        let retryable = e.is_retryable();
        // Fold the JSON-RPC `data` payload into the message: a pandoc failure (code
        // 1002) carries its build log there, and dropping it left the failure
        // undiagnosable.
        let message = match e.data {
            Some(data) if !data.is_null() => format!("{} [data: {data}]", e.message),
            _ => e.message,
        };
        AppError::Sidecar {
            code: e.code,
            message,
            retryable,
        }
    }
}

/// A parsed line coming from the sidecar.
#[derive(Debug)]
pub enum Incoming {
    Response {
        id: u64,
        result: std::result::Result<Value, RpcErrorObject>,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// Anything that is neither: comments, blank lines, malformed JSON. Ignored
    /// rather than fatal, so a single bad line cannot kill the reader.
    Ignored,
}

/// Parse one NDJSON line from the sidecar.
pub fn parse_incoming(line: &str) -> Incoming {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Incoming::Ignored;
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Incoming::Ignored;
    };
    let Some(object) = value.as_object() else {
        return Incoming::Ignored;
    };
    // Server -> client notifications carry a method and no id.
    if let Some(method) = object.get("method").and_then(Value::as_str) {
        let params = object.get("params").cloned().unwrap_or(Value::Null);
        return Incoming::Notification {
            method: method.to_string(),
            params,
        };
    }
    let Some(id) = object.get("id").and_then(Value::as_u64) else {
        return Incoming::Ignored;
    };
    if let Some(error) = object.get("error") {
        return match serde_json::from_value::<RpcErrorObject>(error.clone()) {
            Ok(err) => Incoming::Response {
                id,
                result: Err(err),
            },
            Err(_) => Incoming::Ignored,
        };
    }
    let result = object.get("result").cloned().unwrap_or(Value::Null);
    Incoming::Response {
        id,
        result: Ok(result),
    }
}

/// Encode a request as a single NDJSON line (newline included).
pub fn encode_request_line(id: u64, method: &str, params: &Value) -> String {
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut line = serde_json::to_string(&request).unwrap_or_else(|_| "{}".to_string());
    line.push('\n');
    line
}

// ---------------------------------------------------------------------------
// Pending request table
// ---------------------------------------------------------------------------

type ResponseSender = tokio::sync::oneshot::Sender<std::result::Result<Value, RpcErrorObject>>;

/// Outstanding requests, keyed by numeric id.
#[derive(Default)]
pub struct Pending {
    map: parking_lot::Mutex<HashMap<u64, ResponseSender>>,
}

impl Pending {
    pub fn insert(&self, id: u64, tx: ResponseSender) {
        self.map.lock().insert(id, tx);
    }

    pub fn remove(&self, id: u64) -> Option<ResponseSender> {
        self.map.lock().remove(&id)
    }

    /// Deliver a response. Returns false when the id is unknown (or the waiter
    /// was dropped, e.g. after a timeout).
    pub fn resolve(&self, id: u64, result: std::result::Result<Value, RpcErrorObject>) -> bool {
        match self.map.lock().remove(&id) {
            Some(tx) => tx.send(result).is_ok(),
            None => false,
        }
    }

    /// Fail every outstanding request. Returns how many were failed.
    pub fn fail_all(&self, err: &RpcErrorObject) -> usize {
        let mut map = self.map.lock();
        let count = map.len();
        for (_, tx) in map.drain() {
            let _ = tx.send(Err(err.clone()));
        }
        count
    }

    pub fn len(&self) -> usize {
        self.map.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Callback invoked for every out-of-band notification (e.g. `progress`).
pub type EventSink = Arc<dyn Fn(&str, &Value) + Send + Sync>;

/// Read and dispatch lines until EOF. Malformed lines are logged and skipped.
pub async fn read_loop<R: AsyncBufRead + Unpin>(reader: R, pending: Arc<Pending>, sink: EventSink) {
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match parse_incoming(&line) {
                Incoming::Response { id, result } => {
                    if !pending.resolve(id, result) {
                        tracing::debug!(id, "sidecar response for unknown or timed-out id");
                    }
                }
                Incoming::Notification { method, params } => sink(&method, &params),
                Incoming::Ignored => {
                    tracing::warn!(line = %shorten(&line), "ignoring malformed sidecar line");
                }
            },
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "sidecar stdout read error");
                break;
            }
        }
    }
}

fn shorten(line: &str) -> String {
    if line.len() <= 200 {
        line.to_string()
    } else {
        let mut end = 200;
        while end > 0 && !line.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &line[..end])
    }
}

// ---------------------------------------------------------------------------
// Typed results mirrored from AGENTS.md
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PingResult {
    #[serde(default)]
    pub pong: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub python: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DetectFormatResult {
    pub format: String,
    #[serde(default)]
    pub backends: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChapterInfo {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub level: i64,
    #[serde(default)]
    pub order: i64,
    #[serde(default)]
    pub block_first: Option<i64>,
    #[serde(default)]
    pub block_last: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IngestResult {
    #[serde(default)]
    pub markdown_path: String,
    #[serde(default)]
    pub metadata: serde_json::Map<String, Value>,
    #[serde(default)]
    pub chapters: Vec<ChapterInfo>,
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Absolute path of the directory holding the extracted media, or `None`
    /// when the source document carries none (see the frozen `ingest` contract).
    #[serde(default)]
    pub assets_dir: Option<String>,
    /// The media hrefs exactly as they appear in the Markdown, relative to
    /// `markdown_path`, e.g. `assets/harbour.png`.
    #[serde(default)]
    pub assets: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ParseDocumentResult {
    #[serde(default)]
    pub blocks: Vec<crate::db::models::BlockIr>,
    #[serde(default)]
    pub chapters: Vec<crate::db::models::ChapterIr>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BuildChunksResult {
    #[serde(default)]
    pub chunks: Vec<crate::db::models::ChunkIr>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrepareTextResult {
    #[serde(default)]
    pub llm_text: String,
    #[serde(default)]
    pub placeholders: Vec<(u32, String)>,
    #[serde(default)]
    pub used_blocks: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReinjectResult {
    #[serde(default)]
    pub blocks_md: Vec<String>,
    #[serde(default)]
    pub placeholders_ok: bool,
    #[serde(default)]
    pub missing: Vec<u32>,
    #[serde(default)]
    pub duplicated: Vec<u32>,
    #[serde(default)]
    pub block_count_ok: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SidecarFinding {
    pub kind: String,
    pub severity: String,
    #[serde(default)]
    pub block_id: Option<String>,
    #[serde(default)]
    pub details: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QaCheckResult {
    #[serde(default)]
    pub findings: Vec<SidecarFinding>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PandocUnit {
    pub path: String,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PandocParams {
    pub units: Vec<PandocUnit>,
    pub metadata: Value,
    pub output_path: String,
    pub output_format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Directories the sidecar hands to pandoc via `--resource-path` so relative
    /// targets (e.g. an image href `assets/harbour.png`) resolve. Omitted when
    /// empty, keeping the wire form identical to the pre-M2 contract.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_path: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PandocResult {
    #[serde(default)]
    pub output_path: String,
    #[serde(default)]
    pub log: String,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EstimateTokensResult {
    #[serde(default)]
    pub counts: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Typed client
// ---------------------------------------------------------------------------

/// Async, typed façade over the sidecar. Cheap to clone (holds an `Arc`).
#[derive(Clone)]
pub struct SidecarClient {
    supervisor: Arc<Supervisor>,
}

impl SidecarClient {
    pub fn new(supervisor: Arc<Supervisor>) -> Self {
        Self { supervisor }
    }

    pub fn supervisor(&self) -> &Arc<Supervisor> {
        &self.supervisor
    }

    /// Whether the sidecar process is already running. A caller doing a
    /// best-effort check (the review placeholder guard) uses this to skip
    /// instead of paying a spawn it does not need.
    pub fn is_running(&self) -> bool {
        self.supervisor.is_running()
    }

    /// Generic call; prefer a typed method below.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.supervisor.call(method, params).await
    }

    async fn call_typed<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<T> {
        let value = self.supervisor.call(method, params).await?;
        serde_json::from_value(value).map_err(AppError::from)
    }

    pub async fn ping(&self) -> Result<PingResult> {
        self.call_typed("ping", serde_json::json!({})).await
    }

    pub async fn detect_format(&self, path: &str) -> Result<DetectFormatResult> {
        self.call_typed("detect_format", serde_json::json!({ "path": path }))
            .await
    }

    pub async fn ingest(
        &self,
        path: &str,
        work_dir: &str,
        pdf_backend: Option<&str>,
    ) -> Result<IngestResult> {
        let mut params = serde_json::json!({ "path": path, "work_dir": work_dir });
        if let Some(backend) = pdf_backend {
            params["pdf_backend"] = Value::String(backend.to_string());
        }
        self.call_typed("ingest", params).await
    }

    pub async fn parse_document(&self, markdown_path: &str) -> Result<ParseDocumentResult> {
        self.call_typed(
            "parse_document",
            serde_json::json!({ "markdown_path": markdown_path }),
        )
        .await
    }

    pub async fn build_chunks(
        &self,
        blocks: &[crate::db::models::BlockIr],
        budget_tokens: usize,
    ) -> Result<BuildChunksResult> {
        self.call_typed(
            "build_chunks",
            serde_json::json!({ "blocks": blocks, "budget_tokens": budget_tokens }),
        )
        .await
    }

    pub async fn prepare_text(
        &self,
        block_ids: Option<&[String]>,
        text: &str,
    ) -> Result<PrepareTextResult> {
        let mut params = serde_json::json!({ "text": text });
        if let Some(ids) = block_ids {
            params["block_ids"] = serde_json::json!(ids);
        }
        self.call_typed("prepare_text", params).await
    }

    pub async fn reinject(
        &self,
        text: &str,
        placeholders: &[(u32, String)],
        expected_blocks: usize,
    ) -> Result<ReinjectResult> {
        self.call_typed(
            "reinject",
            serde_json::json!({
                "text": text,
                "placeholders": placeholders,
                "expected_blocks": expected_blocks,
            }),
        )
        .await
    }

    pub async fn qa_check(
        &self,
        source_text: &str,
        target_text: &str,
        glossary: &Value,
        placeholders: &[(u32, String)],
    ) -> Result<QaCheckResult> {
        self.call_typed(
            "qa_check",
            serde_json::json!({
                "source_text": source_text,
                "target_text": target_text,
                "glossary": glossary,
                "placeholders": placeholders,
            }),
        )
        .await
    }

    pub async fn pandoc_build(&self, params: PandocParams) -> Result<PandocResult> {
        self.call_typed("pandoc_build", serde_json::to_value(params)?)
            .await
    }

    pub async fn estimate_tokens(&self, texts: &[String]) -> Result<Vec<usize>> {
        let result: EstimateTokensResult = self
            .call_typed("estimate_tokens", serde_json::json!({ "texts": texts }))
            .await?;
        Ok(result.counts)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncWriteExt, BufReader};

    #[test]
    fn framer_buffers_partial_lines() {
        let mut framer = LineFramer::new();
        assert!(framer.push(b"{\"id\":1").is_empty());
        assert_eq!(framer.pending_len(), 7);
        let lines = framer.push(b"}\n{\"id\":2}\npar");
        assert_eq!(lines, vec!["{\"id\":1}", "{\"id\":2}"]);
        assert_eq!(framer.pending_len(), 3);
        let lines = framer.push(b"tial\n");
        assert_eq!(lines, vec!["partial"]);
        assert_eq!(framer.pending_len(), 0);
    }

    #[test]
    fn framer_handles_crlf_and_blank_lines() {
        let mut framer = LineFramer::new();
        let lines = framer.push(b"a\r\n\r\nb\n");
        assert_eq!(lines, vec!["a", "", "b"]);
    }

    #[test]
    fn parse_incoming_correlates_ids_and_errors() {
        match parse_incoming(r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#) {
            Incoming::Response { id, result } => {
                assert_eq!(id, 7);
                assert_eq!(result.unwrap()["ok"], true);
            }
            other => panic!("unexpected: {other:?}"),
        }
        match parse_incoming(r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no"}}"#) {
            Incoming::Response { id, result } => {
                assert_eq!(id, 3);
                assert_eq!(result.unwrap_err().code, -32601);
            }
            other => panic!("unexpected: {other:?}"),
        }
        match parse_incoming(r#"{"jsonrpc":"2.0","method":"progress","params":{"done":1}}"#) {
            Incoming::Notification { method, params } => {
                assert_eq!(method, "progress");
                assert_eq!(params["done"], 1);
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn malformed_lines_are_ignored_not_fatal() {
        assert!(matches!(parse_incoming("not json"), Incoming::Ignored));
        assert!(matches!(parse_incoming("{broken json"), Incoming::Ignored));
        assert!(matches!(parse_incoming(""), Incoming::Ignored));
        assert!(matches!(parse_incoming("[1,2,3]"), Incoming::Ignored));
        // An id that is not a number cannot be correlated.
        assert!(matches!(
            parse_incoming(r#"{"jsonrpc":"2.0","id":"x","result":{}}"#),
            Incoming::Ignored
        ));
    }

    #[test]
    fn encode_request_is_one_ndjson_line() {
        let line = encode_request_line(1, "ping", &serde_json::json!({}));
        assert!(line.ends_with('\n'));
        assert_eq!(line.matches('\n').count(), 1);
        let parsed: Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(parsed["id"], 1);
        assert_eq!(parsed["method"], "ping");
        assert_eq!(parsed["jsonrpc"], "2.0");
    }

    #[test]
    fn ingest_result_parses_the_media_contract() {
        // A document without media: the sidecar omits or nulls both keys.
        let bare: IngestResult = serde_json::from_value(serde_json::json!({
            "markdown_path": "/work/document.md"
        }))
        .unwrap();
        assert!(bare.assets_dir.is_none());
        assert!(bare.assets.is_empty());

        let with_media: IngestResult = serde_json::from_value(serde_json::json!({
            "markdown_path": "/work/document.md",
            "assets_dir": "/work/assets",
            "assets": ["assets/harbour.png"]
        }))
        .unwrap();
        assert_eq!(with_media.assets_dir.as_deref(), Some("/work/assets"));
        assert_eq!(with_media.assets, vec!["assets/harbour.png".to_string()]);
    }

    #[test]
    fn pandoc_params_omit_an_empty_resource_path() {
        let params = PandocParams {
            units: Vec::new(),
            metadata: serde_json::json!({}),
            output_path: "/out/book.epub".into(),
            output_format: "epub".into(),
            ..Default::default()
        };
        let value = serde_json::to_value(&params).unwrap();
        assert!(
            value.get("resource_path").is_none(),
            "an empty resource_path must not be serialized"
        );

        let params = PandocParams {
            resource_path: vec!["/work".into()],
            ..params
        };
        let value = serde_json::to_value(&params).unwrap();
        assert_eq!(value["resource_path"], serde_json::json!(["/work"]));
    }

    #[test]
    fn rpc_error_maps_to_app_error_with_retryability() {
        let not_found = AppError::from(RpcErrorObject {
            code: -32601,
            message: "method not found".into(),
            data: None,
        });
        assert!(!not_found.retryable());
        assert!(not_found.to_string().contains("-32601"));

        let internal = AppError::from(RpcErrorObject {
            code: 1001,
            message: "ingestion failed".into(),
            data: None,
        });
        assert!(internal.retryable());

        // A pandoc failure carries its build log in `data`; it must survive into
        // the message, or the failure is undiagnosable.
        let pandoc = AppError::from(RpcErrorObject {
            code: 1002,
            message: "pandoc failed".into(),
            data: Some(serde_json::json!({ "log": "! LaTeX Error: Unicode character ⟦" })),
        });
        assert!(pandoc.retryable());
        let text = pandoc.to_string();
        assert!(text.contains("pandoc failed"));
        assert!(text.contains("LaTeX Error"));
    }

    #[tokio::test]
    async fn read_loop_correlates_ids_and_survives_malformed_lines() {
        let (mut writer, reader) = tokio::io::duplex(8192);
        let pending = Arc::new(Pending::default());
        let (tx1, rx1) = tokio::sync::oneshot::channel();
        let (tx2, rx2) = tokio::sync::oneshot::channel();
        pending.insert(1, tx1);
        pending.insert(2, tx2);

        let events = Arc::new(parking_lot::Mutex::new(Vec::<(String, Value)>::new()));
        let sink: EventSink = {
            let events = events.clone();
            Arc::new(move |method: &str, params: &Value| {
                events.lock().push((method.to_string(), params.clone()));
            })
        };

        let pending_for_loop = pending.clone();
        let handle = tokio::spawn(async move {
            read_loop(BufReader::new(reader), pending_for_loop, sink).await;
        });

        // A malformed line first: it must not kill the reader.
        writer.write_all(b"this is not json\n").await.unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"progress\",\"params\":{\"done\":1,\"total\":2}}\n")
            .await
            .unwrap();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32601,\"message\":\"nope\"}}\n")
            .await
            .unwrap();
        // Close the write end so the read loop terminates.
        drop(writer);
        handle.await.unwrap();

        assert_eq!(rx2.await.unwrap().unwrap()["ok"], true);
        assert_eq!(rx1.await.unwrap().unwrap_err().code, -32601);
        let events = events.lock();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "progress");
        assert_eq!(events[0].1["total"], 2);
    }
}
