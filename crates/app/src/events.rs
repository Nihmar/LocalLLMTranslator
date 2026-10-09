//! Decoupled UI event emitters.
//!
//! The control plane announces job state transitions (`job://progress`),
//! structured logs (`log://line`) and sidecar notifications without depending on
//! [`tauri::AppHandle`]: everything goes through the [`EventEmitter`] trait, so
//! the emitters can be driven by a recording sink in unit tests.
//!
//! `job://progress` carries the serialized [`Job`] row — exactly the shape
//! `job_list` returns — and is emitted at every transition the control plane
//! owns: `pending` (enqueue), `leased`/`running` (claim), `done`, `failed` and
//! `cancelled`.

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::db::models::Job;
use crate::sidecar::EventSink;

// Event names (frozen, PLAN.md §12.2).
pub const EVENT_JOB_PROGRESS: &str = "job://progress";
pub const EVENT_LOG_LINE: &str = "log://line";
pub const EVENT_METRICS_TICK: &str = "metrics://tick";
pub const EVENT_SIDECAR_STATUS: &str = "sidecar://status";
pub const EVENT_EXPORT_PROGRESS: &str = "export://progress";
/// Out-of-band progress notifications coming from the Python sidecar.
pub const EVENT_SIDECAR_PROGRESS: &str = "sidecar://progress";

/// A sink for UI events.
///
/// Implemented for [`AppHandle`] in production and by a recording sink in tests,
/// so the emitters never have to know about Tauri.
pub trait EventEmitter: Send + Sync + 'static {
    fn emit(&self, event: &str, payload: Value);
}

impl EventEmitter for AppHandle {
    fn emit(&self, event: &str, payload: Value) {
        if let Err(error) = Emitter::emit(self, event, payload) {
            tracing::debug!(%error, event, "failed to emit UI event");
        }
    }
}

/// An emitter that drops every event. Used in headless contexts and tests that
/// do not assert on emitted events.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEmitter;

impl EventEmitter for NullEmitter {
    fn emit(&self, _event: &str, _payload: Value) {}
}

/// Payload of a `log://line` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LogLine {
    pub ts: String,
    pub level: String,
    pub source: String,
    pub message: String,
}

impl LogLine {
    /// `level` is one of `info` | `warn` | `error`; `source` is `sidecar` or
    /// `worker`.
    pub fn new(level: &str, source: &str, message: impl Into<String>) -> Self {
        Self {
            ts: crate::db::now(),
            level: level.to_string(),
            source: source.to_string(),
            message: message.into(),
        }
    }
}

/// Emit the serialized [`Job`] row on `job://progress`.
pub fn emit_job(emitter: &dyn EventEmitter, job: &Job) {
    match serde_json::to_value(job) {
        Ok(payload) => emitter.emit(EVENT_JOB_PROGRESS, payload),
        Err(error) => tracing::warn!(job_id = %job.id, %error, "could not serialize job for event"),
    }
}

/// Emit a structured log line on `log://line`.
pub fn emit_log(emitter: &dyn EventEmitter, level: &str, source: &str, message: impl Into<String>) {
    match serde_json::to_value(LogLine::new(level, source, message)) {
        Ok(payload) => emitter.emit(EVENT_LOG_LINE, payload),
        Err(error) => tracing::warn!(%error, "could not serialize log line"),
    }
}

/// Map a sidecar notification method to a UI event sink.
///
/// `progress` becomes `sidecar://progress`; every other method becomes
/// `sidecar://<method>`. The raw params are forwarded unchanged.
pub fn sidecar_event_sink(emitter: Arc<dyn EventEmitter>) -> EventSink {
    Arc::new(move |method: &str, params: &Value| {
        let event = if method == "progress" {
            EVENT_SIDECAR_PROGRESS.to_string()
        } else {
            format!("sidecar://{method}")
        };
        emitter.emit(&event, params.clone());
    })
}

/// Classify a sidecar stderr line into `info` | `warn` | `error`.
///
/// stderr is unstructured text, so the level is inferred from a few well-known
/// markers and otherwise defaults to `info`.
pub fn classify_stderr_level(line: &str) -> &'static str {
    let lower = line.to_ascii_lowercase();
    if lower.contains("traceback")
        || lower.contains("error")
        || lower.contains("exception")
        || lower.contains("critical")
    {
        "error"
    } else if lower.contains("warn") {
        "warn"
    } else {
        "info"
    }
}

#[cfg(test)]
pub use test_support::RecordingEmitter;

#[cfg(test)]
mod test_support {
    use super::*;
    use parking_lot::Mutex;

    /// An [`EventEmitter`] that records `(event, payload)` pairs for assertions.
    #[derive(Default)]
    pub struct RecordingEmitter {
        events: Mutex<Vec<(String, Value)>>,
    }

    impl RecordingEmitter {
        pub fn new() -> Self {
            Self::default()
        }

        /// Snapshot of the recorded events.
        pub fn events(&self) -> Vec<(String, Value)> {
            self.events.lock().clone()
        }

        /// Events recorded for a given event name.
        pub fn events_named(&self, name: &str) -> Vec<Value> {
            self.events
                .lock()
                .iter()
                .filter(|(event, _)| event == name)
                .map(|(_, payload)| payload.clone())
                .collect()
        }
    }

    impl EventEmitter for RecordingEmitter {
        fn emit(&self, event: &str, payload: Value) {
            self.events.lock().push((event.to_string(), payload));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::now;
    use serde_json::json;

    fn sample_job(state: &str) -> Job {
        Job {
            id: "j1".into(),
            project_id: "p1".into(),
            kind: "translate_chunk".into(),
            payload_json: "{}".into(),
            priority: 100,
            state: state.into(),
            attempts: 0,
            max_attempts: 3,
            lease_owner: None,
            lease_expires_at: None,
            run_after: None,
            last_error: None,
            created_at: now(),
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn emit_job_uses_the_job_list_shape() {
        let rec = Arc::new(RecordingEmitter::new());
        emit_job(&*rec, &sample_job("running"));

        let events = rec.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "job://progress");
        // Same fields as the `Job` row returned by `job_list`.
        assert_eq!(events[0].1["id"], "j1");
        assert_eq!(events[0].1["kind"], "translate_chunk");
        assert_eq!(events[0].1["state"], "running");
        assert!(events[0].1.get("payload_json").is_some());
    }

    #[test]
    fn emit_log_payload_shape() {
        let rec = Arc::new(RecordingEmitter::new());
        emit_log(&*rec, "warn", "worker", "something happened");

        let events = rec.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].0, "log://line");
        let payload = &events[0].1;
        assert_eq!(payload["level"], "warn");
        assert_eq!(payload["source"], "worker");
        assert_eq!(payload["message"], "something happened");
        // RFC3339 timestamp.
        let ts = payload["ts"].as_str().expect("ts string");
        assert!(chrono::DateTime::parse_from_rfc3339(ts).is_ok());
    }

    #[test]
    fn sidecar_sink_routes_progress_and_other_methods() {
        let rec = Arc::new(RecordingEmitter::new());
        let sink = sidecar_event_sink(rec.clone());

        sink("progress", &json!({"job_id": "j1", "done": 3, "total": 10}));
        sink("something_else", &json!({"x": 1}));

        let events = rec.events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "sidecar://progress");
        assert_eq!(events[0].1["done"], 3);
        assert_eq!(events[1].0, "sidecar://something_else");
        // It must not masquerade as a job event.
        assert!(events.iter().all(|(event, _)| event != EVENT_JOB_PROGRESS));
    }

    #[test]
    fn classify_stderr_level_detects_severity() {
        assert_eq!(classify_stderr_level("INFO: ready"), "info");
        assert_eq!(classify_stderr_level("Warning: slow"), "warn");
        assert_eq!(classify_stderr_level("ERROR: boom"), "error");
        assert_eq!(
            classify_stderr_level("Traceback (most recent call last):"),
            "error"
        );
    }
}
