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
use crate::events::{classify_stderr_level, emit_log, EventEmitter};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SidecarState {
    Stopped,
    Starting,
    Running,
    Restarting,
    Failed,
}

/// Reported through the `sidecar://status` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
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
    /// Emits sidecar stderr as `log://line`.
    emitter: Arc<dyn EventEmitter>,
    writer: parking_lot::Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    /// Signal channel used to terminate the running child deterministically.
    kill: parking_lot::Mutex<Option<oneshot::Sender<()>>>,
    running: Arc<AtomicBool>,
    /// Set once the supervisor is shutting down: no restart, no new spawn.
    stopping: AtomicBool,
    next_id: AtomicU64,
    start_lock: tokio::sync::Mutex<()>,
    status: parking_lot::Mutex<SidecarStatus>,
    status_tx: broadcast::Sender<SidecarStatus>,
}

impl Supervisor {
    /// Build a supervisor. The returned `Arc` is shared by the client and the
    /// restart tasks.
    pub fn new(
        spec: SpawnSpec,
        sink: EventSink,
        emitter: Arc<dyn EventEmitter>,
        timeout: Option<Duration>,
    ) -> Arc<Self> {
        let (status_tx, _) = broadcast::channel(32);
        Arc::new(Self {
            spec,
            timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
            pending: Arc::new(Pending::default()),
            sink,
            emitter,
            writer: parking_lot::Mutex::new(None),
            kill: parking_lot::Mutex::new(None),
            running: Arc::new(AtomicBool::new(false)),
            stopping: AtomicBool::new(false),
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
        let changed = {
            let mut current = self.status.lock();
            let changed = current.state != status.state;
            *current = status.clone();
            changed
        };
        if changed {
            // State transitions belong in the log file: they explain why a request was
            // rejected or retried.
            tracing::info!(
                state = ?status.state,
                pid = status.pid,
                attempts = status.attempts,
                message = status.message.as_deref(),
                "sidecar status"
            );
        }
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
                // The process stopped answering. The sidecar is stateless and every
                // request is repeatable, so kill it and let the exit watcher restart a
                // fresh one: a hung call (a stuck pandoc, a wedged library) would
                // otherwise block the sequential loop for every later request too.
                self.restart_child(format!(
                    "request '{method}' timed out after {:?}",
                    self.timeout
                ));
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
    ///
    /// A spawn failure is terminal until the next attempt: the status is moved to
    /// [`SidecarState::Failed`] so `sidecar_status` and the UI banner show the
    /// reason instead of reporting a perpetual `Starting`.
    pub async fn ensure_running(self: &Arc<Self>) -> Result<()> {
        if self.is_running() {
            return Ok(());
        }
        let _guard = self.start_lock.lock().await;
        if self.is_running() {
            return Ok(());
        }
        match self.spawn_once().await {
            Ok(()) => Ok(()),
            Err(error) => {
                // `shutdown` already owns the terminal state in that case.
                if !self.stopping.load(Ordering::SeqCst) {
                    self.set_status(SidecarStatus::failed(error.to_string()));
                }
                Err(error)
            }
        }
    }

    /// Start the process once (no retry). Shared by the initial start and the
    /// restart loop.
    async fn spawn_once(self: &Arc<Self>) -> Result<()> {
        if self.stopping.load(Ordering::SeqCst) {
            return Err(AppError::SidecarUnavailable(
                "sidecar is shutting down".into(),
            ));
        }
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

        // stderr -> structured `log://line` events and the log file.
        if let Some(stderr) = stderr {
            let emitter = self.emitter.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    match classify_stderr_level(&line) {
                        "error" => tracing::error!(target: "sidecar", "{line}"),
                        "warn" => tracing::warn!(target: "sidecar", "{line}"),
                        _ => tracing::debug!(target: "sidecar", "{line}"),
                    }
                    emit_log(&*emitter, classify_stderr_level(&line), "sidecar", line);
                }
            });
        }

        *self.writer.lock() = Some(tx);
        self.running.store(true, Ordering::SeqCst);
        self.set_status(SidecarStatus::running(pid));

        // Waiter: owns the child and reacts to its exit. `handle_exit` is
        // synchronous so this future's `Send`-ness does not depend on the
        // restart path (which awaits `spawn_once` again).
        //
        // It also listens on the per-spawn kill channel so `shutdown` can
        // terminate the process deterministically instead of relying on
        // `kill_on_drop`, which only fires when this task is dropped. The
        // `child.wait()` future is confined to the `select!`, so `child` is free
        // to be borrowed again by `kill` once the select has resolved.
        let (kill_tx, kill_rx) = oneshot::channel::<()>();
        *self.kill.lock() = Some(kill_tx);
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let kill = tokio::select! {
                status = child.wait() => {
                    if let Some(this) = weak.upgrade() {
                        let code = status.ok().and_then(|s| s.code());
                        this.handle_exit(code);
                    }
                    false
                }
                requested = kill_rx => requested.is_ok(),
            };
            if kill {
                if let Err(error) = child.kill().await {
                    tracing::warn!(%error, "failed to kill sidecar process");
                }
                // Run the same exit path as a natural exit so a kill requested while
                // running (a timed-out request) restarts the process instead of
                // leaving the pool dead. During shutdown this is a no-op: `shutdown`
                // already cleared `running`, so `handle_exit` returns immediately.
                if let Some(this) = weak.upgrade() {
                    this.handle_exit(None);
                }
            }
        });

        // A `shutdown` may have landed between the entry check and publishing the
        // kill sender above; it would then have found nothing to signal and the
        // child we just spawned would stay alive unreaped (`kill_on_drop` never
        // fires because the supervisor lives in `AppState` for the whole process).
        // Re-check now that the sender exists and terminate the child if so.
        if self.abort_spawn_if_stopping() {
            return Err(AppError::SidecarUnavailable(
                "sidecar is shutting down".into(),
            ));
        }

        Ok(())
    }

    /// Terminate a child spawned while `shutdown` was in progress.
    ///
    /// Call this only after the per-spawn kill sender has been published, so the
    /// signal can actually reach the waiter. Returns `true` when the supervisor
    /// was stopping and the spawn was aborted (the child is killed by its waiter
    /// task reacting to the kill channel).
    fn abort_spawn_if_stopping(&self) -> bool {
        if !self.stopping.load(Ordering::SeqCst) {
            return false;
        }
        self.running.store(false, Ordering::SeqCst);
        *self.writer.lock() = None;
        if let Some(kill) = self.kill.lock().take() {
            let _ = kill.send(());
        }
        self.set_status(SidecarStatus::stopped());
        true
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

        if self.stopping.load(Ordering::SeqCst) {
            return;
        }
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
            if self.stopping.load(Ordering::SeqCst) || self.is_running() {
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

    /// Kill a child that stopped answering and let the exit watcher restart it.
    ///
    /// Called when a request times out. The sidecar is stateless, so dropping a
    /// wedged process costs nothing but a respawn; the alternative is leaving the
    /// sequential RPC loop blocked behind it. No-op while shutting down.
    pub fn restart_child(&self, reason: impl Into<String>) {
        if self.stopping.load(Ordering::SeqCst) {
            return;
        }
        self.set_status(SidecarStatus::restarting(0, Some(reason.into())));
        if let Some(kill) = self.kill.lock().take() {
            let _ = kill.send(());
        }
    }

    /// Stop accepting work, terminate the child process and mark the supervisor
    /// stopped. Idempotent.
    pub fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        *self.writer.lock() = None;
        self.pending.fail_all(&RpcErrorObject {
            code: 1001,
            message: "sidecar supervisor shutting down".into(),
            data: None,
        });
        // Terminate the child now: the waiter reacts to this signal by calling
        // `Child::kill`, so release does not depend on `kill_on_drop`.
        if let Some(kill) = self.kill.lock().take() {
            let _ = kill.send(());
        }
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

/// The bundled sidecar executable, if the app is running from a packaged
/// bundle. The Tauri resource is the PyInstaller `onedir` directory, so the
/// executable lives inside it (`<resources>/llmtranslator_sidecar/llmtranslator_sidecar`);
/// a single-file resource is also accepted. Kept as a free function so it can be
/// unit tested.
pub fn bundled_sidecar_path(resource_dir: Option<&PathBuf>) -> Option<PathBuf> {
    let dir = resource_dir?;
    let names: [&str; 2] = ["llmtranslator_sidecar", "llmtranslator_sidecar.exe"];

    // A single-file resource.
    for name in names {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
    }

    // The onedir layout: `<resources>/llmtranslator_sidecar/<exe>`.
    let nested_dir = dir.join("llmtranslator_sidecar");
    for name in names {
        let executable = nested_dir.join(name);
        if executable.is_file() {
            return Some(executable);
        }
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

    #[test]
    fn bundled_path_finds_the_onedir_executable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("llmtranslator_sidecar");
        std::fs::create_dir_all(&nested).expect("nested dir");
        let executable = nested.join("llmtranslator_sidecar");
        std::fs::write(&executable, b"").expect("touch");
        assert_eq!(
            bundled_sidecar_path(Some(&dir.path().to_path_buf())),
            Some(executable)
        );

        // The directory alone is not an executable.
        let empty = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(empty.path().join("llmtranslator_sidecar")).expect("dir");
        assert!(bundled_sidecar_path(Some(&empty.path().to_path_buf())).is_none());
    }

    fn test_supervisor() -> Arc<Supervisor> {
        let sink: EventSink = Arc::new(|_method: &str, _params: &Value| {});
        Supervisor::new(
            SpawnSpec::new("python", Vec::new()),
            sink,
            Arc::new(crate::events::NullEmitter),
            None,
        )
    }

    #[test]
    fn restart_child_signals_the_waiter_and_marks_restarting() {
        let supervisor = test_supervisor();
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        *supervisor.kill.lock() = Some(kill_tx);
        supervisor.running.store(true, Ordering::SeqCst);

        supervisor.restart_child("request 'pandoc_build' timed out");

        assert_eq!(supervisor.status().state, SidecarState::Restarting);
        assert!(
            supervisor
                .status()
                .message
                .is_some_and(|message| message.contains("timed out")),
            "the reason must reach the status event"
        );
        assert!(
            kill_rx.try_recv().is_ok(),
            "the stuck child must be signalled"
        );
        assert!(supervisor.kill.lock().is_none(), "the sender is consumed");
    }

    #[test]
    fn restart_child_is_a_noop_while_stopping() {
        let supervisor = test_supervisor();
        supervisor.stopping.store(true, Ordering::SeqCst);
        supervisor.restart_child("too late");
        assert_ne!(supervisor.status().state, SidecarState::Restarting);
    }

    #[tokio::test]
    async fn ensure_running_reports_a_failed_spawn_instead_of_starting_forever() {
        let sink: EventSink = Arc::new(|_method: &str, _params: &Value| {});
        let supervisor = Supervisor::new(
            SpawnSpec::new("/nonexistent/llmtranslator-sidecar", Vec::new()),
            sink,
            Arc::new(crate::events::NullEmitter),
            None,
        );

        let result = supervisor.ensure_running().await;
        assert!(result.is_err(), "a missing program must fail the spawn");
        let status = supervisor.status();
        assert_eq!(status.state, SidecarState::Failed);
        assert!(
            status.message.is_some(),
            "the reason must reach `sidecar_status` so the banner can show it"
        );
    }

    /// A script that hangs on its first run and answers `ping` on every later one.
    #[cfg(unix)]
    const HANGING_SIDECAR: &str = "#!/bin/sh\n\
if [ -f \"$1\" ]; then\n\
  while read -r line; do\n\
    id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9]*\\).*/\\1/p')\n\
    printf '{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{\"pong\":true,\"version\":\"test\"}}\\n' \"$id\"\n\
  done\n\
else\n\
  touch \"$1\"\n\
  sleep 60\n\
fi\n";

    #[cfg(unix)]
    #[tokio::test]
    async fn a_timed_out_request_restarts_the_child() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tempdir");
        let script = dir.path().join("fake-sidecar.sh");
        let marker = dir.path().join("first-run-done");
        std::fs::write(&script, HANGING_SIDECAR).expect("write script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let sink: EventSink = Arc::new(|_method: &str, _params: &Value| {});
        let supervisor = Supervisor::new(
            SpawnSpec::new(
                "/bin/sh",
                vec![
                    script.to_string_lossy().to_string(),
                    marker.to_string_lossy().to_string(),
                ],
            ),
            sink,
            Arc::new(crate::events::NullEmitter),
            Some(Duration::from_millis(300)),
        );

        // The first child hangs on the request: the call times out and the
        // supervisor kills it.
        let first = supervisor.call("ping", serde_json::json!({})).await;
        assert!(
            matches!(first, Err(AppError::SidecarTimeout(_))),
            "expected a timeout, got {first:?}"
        );

        // The restart loop spawns the script again, which now answers.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut recovered = None;
        while tokio::time::Instant::now() < deadline {
            match supervisor.call("ping", serde_json::json!({})).await {
                Ok(value) => {
                    recovered = Some(value);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        supervisor.shutdown();

        let value = recovered.expect("the supervisor never recovered from the timeout");
        assert_eq!(value["pong"], true);
    }

    #[test]
    fn abort_spawn_if_stopping_signals_a_child_spawned_during_shutdown() {
        let supervisor = test_supervisor();
        let (kill_tx, mut kill_rx) = oneshot::channel::<()>();
        *supervisor.kill.lock() = Some(kill_tx);
        supervisor.running.store(true, Ordering::SeqCst);
        // `shutdown` landed while the spawn was in flight: it found no kill
        // sender, so the freshly spawned child has not been signalled yet.
        supervisor.stopping.store(true, Ordering::SeqCst);

        assert!(supervisor.abort_spawn_if_stopping());
        assert!(!supervisor.is_running());
        assert_eq!(supervisor.status().state, SidecarState::Stopped);
        assert!(
            kill_rx.try_recv().is_ok(),
            "the child spawned during shutdown must be signalled"
        );
    }

    #[test]
    fn abort_spawn_if_stopping_is_a_noop_while_running() {
        let supervisor = test_supervisor();
        supervisor.running.store(true, Ordering::SeqCst);
        assert!(!supervisor.abort_spawn_if_stopping());
        assert!(supervisor.is_running());
    }
}
