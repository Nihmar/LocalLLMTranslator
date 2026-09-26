//! Sidecar supervisor: process lifecycle, restart with backoff, status events.
//!
//! Transport is stdio NDJSON (see [`super::rpc`]). The sidecar is stateless, so
//! when it dies the in-flight requests are failed with a retryable error and the
//! process is restarted on a small exponential backoff.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, oneshot};

use super::rpc::{
    encode_request_line, read_loop, EventSink, Pending, RpcErrorObject, DEFAULT_TIMEOUT,
};
use crate::error::{AppError, Result};

/// How many consecutive restart attempts before giving up.
const MAX_RESTART_ATTEMPTS: u32 = 5;

/// Environment variable that overrides the sidecar location (development).
pub const SIDECAR_ENV: &str = "LLMTRANSLATOR_SIDECAR";

/// How to launch the Python sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl SpawnSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }
}

/// Resolve the sidecar launch command.
///
/// Order: `LLMTRANSLATOR_SIDECAR` (dev override) -> the bundled resource path ->
/// `python -m llmtranslator_sidecar` as a last-resort development fallback.
pub fn resolve_spawn_spec(bundled: Option<&Path>) -> SpawnSpec {
    if let Ok(override_path) = std::env::var(SIDECAR_ENV) {
        let trimmed = override_path.trim();
        if !trimmed.is_empty() {
            return SpawnSpec::new(trimmed.to_string(), Vec::new());
        }
    }
    if let Some(path) = bundled {
        if path.exists() {
            return SpawnSpec::new(path.to_string_lossy().to_string(), Vec::new());
        }
    }
    SpawnSpec::new(
        "python".to_string(),
        vec!["-m".to_string(), "llmtranslator_sidecar".to_string()],
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SidecarState {
    Stopped,
    Starting,
    Running,
    Restarting,
    Failed,
}

/// Reported through the `sidecar://status` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SidecarStatus {
    pub state: SidecarState,
    pub pid: Option<u32>,
    pub attempts: u32,
    pub message: Option<String>,
}

impl SidecarStatus {
    fn stopped() -> Self {
        Self {
            state: SidecarState::Stopped,
            pid: None,
            attempts: 0,
            message: None,
        }
    }

    fn starting() -> Self {
        Self {
            state: SidecarState::Starting,
            pid: None,
            attempts: 0,
            message: None,
        }
    }

    fn running(pid: Option<u32>) -> Self {
        Self {
            state: SidecarState::Running,
            pid,
            attempts: 0,
            message: None,
        }
    }

    fn restarting(attempts: u32, message: Option<String>) -> Self {
        Self {
            state: SidecarState::Restarting,
            pid: None,
            attempts,
            message,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        Self {
            state: SidecarState::Failed,
            pid: None,
            attempts: MAX_RESTART_ATTEMPTS,
            message: Some(message.into()),
        }
    }
}

/// Owns the sidecar child process and the request/response transport.
pub struct Supervisor {
    spec: SpawnSpec,
    timeout: Duration,
    pending: Arc<Pending>,
    sink: EventSink,
    writer: parking_lot::Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    running: Arc<AtomicBool>,
    next_id: AtomicU64,
    start_lock: tokio::sync::Mutex<()>,
    status: parking_lot::Mutex<SidecarStatus>,
    status_tx: broadcast::Sender<SidecarStatus>,
}

impl Supervisor {
    /// Build a supervisor. The returned `Arc` is shared by the client and the
    /// restart tasks.
    pub fn new(spec: SpawnSpec, sink: EventSink, timeout: Option<Duration>) -> Arc<Self> {
        let (status_tx, _) = broadcast::channel(32);
        Arc::new(Self {
            spec,
            timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
            pending: Arc::new(Pending::default()),
            sink,
            writer: parking_lot::Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
            next_id: AtomicU64::new(0),
            start_lock: tokio::sync::Mutex::new(()),
            status: parking_lot::Mutex::new(SidecarStatus::stopped()),
            status_tx,
        })
    }

    pub fn spec(&self) -> &SpawnSpec {
        &self.spec
    }

    pub fn status(&self) -> SidecarStatus {
        self.status.lock().clone()
    }

    /// Subscribe to status changes (used to forward `sidecar://status` events).
    pub fn subscribe_status(&self) -> broadcast::Receiver<SidecarStatus> {
        self.status_tx.subscribe()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    fn set_status(&self, status: SidecarStatus) {
        *self.status.lock() = status.clone();
        // Broadcast errors are fine: it means nobody is listening.
        let _ = self.status_tx.send(status);
    }

    /// Issue a request and await its response, with a hard timeout.
    pub async fn call(self: &Arc<Self>, method: &str, params: Value) -> Result<Value> {
        self.ensure_running().await?;

        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.insert(id, tx);

        let line = encode_request_line(id, method, &params);
        if !self.write(line.into_bytes()) {
            self.pending.remove(id);
            return Err(AppError::SidecarUnavailable(
                "sidecar is not accepting requests".into(),
            ));
        }

        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(error.into()),
            // The sender was dropped: the process exited after we wrote.
            Ok(Err(_)) => Err(AppError::SidecarUnavailable(
                "sidecar connection closed before the response arrived".into(),
            )),
            Err(_) => {
                // Drop the waiter so a late response does not leak.
                self.pending.remove(id);
                Err(AppError::SidecarTimeout(self.timeout))
            }
        }
    }

    fn write(&self, bytes: Vec<u8>) -> bool {
        match &*self.writer.lock() {
            Some(tx) => tx.send(bytes).is_ok(),
            None => false,
        }
    }

    /// Ensure a live process, starting it if necessary. Concurrent callers are
    /// serialised so only one process is ever spawned.
    pub async fn ensure_running(self: &Arc<Self>) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        let _guard = self.start_lock.lock().await;
        if self.is_running() {
            return Ok(());
        }
        self.spawn_once().await
    }

    /// Start the process once (no retry). Shared by the initial start and the
    /// restart loop.
    async fn spawn_once(self: &Arc<Self>) -> Result<()> {
        self.set_status(SidecarStatus::starting());

        let mut command = Command::new(&self.spec.program);
        command.args(&self.spec.args);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().map_err(|error| {
            AppError::SidecarUnavailable(format!(
                "failed to spawn sidecar program '{}': {error}",
                self.spec.program
            ))
        })?;

        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| AppError::SidecarUnavailable("sidecar stdin unavailable".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| AppError::SidecarUnavailable("sidecar stdout unavailable".into()))?;
        let stderr = child.stderr.take();

        // Writer.
        let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        tokio::spawn(writer_task(stdin, rx));

        // Reader.
        let pending = self.pending.clone();
        let sink = self.sink.clone();
        tokio::spawn(async move {
            read_loop(BufReader::new(stdout), pending, sink).await;
        });

        // stderr -> logs.
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "sidecar", "{line}");
                }
            });
        }

        *self.writer.lock() = Some(tx);
        self.running.store(true, Ordering::SeqCst);
        self.set_status(SidecarStatus::running(pid));

        // Waiter: owns the child and reacts to its exit. `handle_exit` is
        // synchronous so this future's `Send`-ness does not depend on the
        // restart path (which awaits `spawn_once` again).
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let status = child.wait().await;
            if let Some(this) = weak.upgrade() {
                let code = status.ok().and_then(|s| s.code());
                this.handle_exit(code);
            }
        });

        Ok(())
    }

    fn handle_exit(self: &Arc<Self>, code: Option<i32>) {
        // Guard against double handling.
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        *self.writer.lock() = None;

        let error = RpcErrorObject {
            code: 1001,
            message: format!("sidecar exited (code {code:?})"),
            data: None,
        };
        let failed = self.pending.fail_all(&error);
        tracing::warn!(
            failed,
            "sidecar exited; in-flight requests failed with a retryable error"
        );

        self.set_status(SidecarStatus::restarting(
            0,
            Some(format!("exited with code {code:?}")),
        ));
        let this = self.clone();
        tokio::spawn(async move {
            this.restart_loop().await;
        });
    }

    async fn restart_loop(self: &Arc<Self>) {
        let mut delay = Duration::from_millis(250);
        for attempt in 1..=MAX_RESTART_ATTEMPTS {
            tokio::time::sleep(delay).await;
            if self.is_running() {
                return;
            }
            match self.spawn_once().await {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(attempt, %error, "sidecar restart failed");
                    self.set_status(SidecarStatus::restarting(attempt, Some(error.to_string())));
                    delay = (delay * 2).min(Duration::from_secs(10));
                }
            }
        }
        self.set_status(SidecarStatus::failed(
            "sidecar could not be restarted after repeated attempts",
        ));
    }

    /// Stop the process (best effort) and mark the supervisor stopped.
    pub fn shutdown(&self) {
        self.running.store(false, Ordering::SeqCst);
        *self.writer.lock() = None;
        self.pending.fail_all(&RpcErrorObject {
            code: 1001,
            message: "sidecar supervisor shutting down".into(),
            data: None,
        });
        self.set_status(SidecarStatus::stopped());
    }

    /// Outstanding request count (diagnostics).
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }
}

async fn writer_task(mut stdin: ChildStdin, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = rx.recv().await {
        if stdin.write_all(&bytes).await.is_err() {
            break;
        }
        if stdin.flush().await.is_err() {
            break;
        }
    }
    // Dropping `stdin` closes the pipe, signalling EOF to the sidecar.
}

/// The bundled sidecar resource directory/file, if the app is running from a
/// packaged bundle. Kept as a free function so it can be unit tested.
pub fn bundled_sidecar_path(resource_dir: Option<&PathBuf>) -> Option<PathBuf> {
    let dir = resource_dir?;
    let candidates = ["llmtranslator_sidecar", "llmtranslator_sidecar.exe"];
    for name in candidates {
        let direct = dir.join(name);
        if direct.exists() {
            return Some(direct);
        }
    }
    let nested = dir.join("llmtranslator_sidecar");
    if nested.exists() {
        return Some(nested);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_spec_resolution_order() {
        // Keep both assertions in one test: they mutate the same process-wide
        // environment variable, so running them concurrently would be racy.
        std::env::set_var(SIDECAR_ENV, "/tmp/custom-sidecar");
        let overridden = resolve_spawn_spec(None);
        std::env::remove_var(SIDECAR_ENV);

        assert_eq!(overridden.program, "/tmp/custom-sidecar");
        assert!(overridden.args.is_empty());

        let fallback = resolve_spawn_spec(None);
        assert_eq!(fallback.program, "python");
        assert_eq!(fallback.args, vec!["-m", "llmtranslator_sidecar"]);
    }

    #[test]
    fn bundled_path_is_none_when_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(bundled_sidecar_path(Some(&dir.path().to_path_buf())).is_none());
    }
}
