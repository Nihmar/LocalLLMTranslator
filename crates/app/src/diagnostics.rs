//! Diagnostics bundle: a ZIP a user can hand over after a failure.
//!
//! It contains the log files and a JSON report (versions, sidecar and worker state, queue
//! counts, failed jobs, LLM call errors). It deliberately excludes the database, the
//! project files, the book text and every prompt/response body: those stay local, and the
//! archive must be safe to attach to a bug report.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::Result;
use crate::logging;
use crate::resources::EndpointUsage;
use crate::sidecar::SidecarStatus;

const REPORT_NAME: &str = "report.json";
/// Per-file cap when a log grew large; the tail is the interesting part.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// How many rotated log files the bundle carries, newest first.
const MAX_LOG_FILES: usize = 5;

/// Nullable job fields, so the report says "no error" instead of inventing one.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct FailedJob {
    pub id: String,
    pub project_id: String,
    pub kind: String,
    pub attempts: i64,
    pub max_attempts: i64,
    pub last_error: Option<String>,
    pub finished_at: Option<String>,
}

/// One failed model call. The prompt and the response are **not** selected.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct LlmCallError {
    pub role: String,
    pub model: String,
    pub error: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueCount {
    pub state: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsReport {
    pub app_version: String,
    pub generated_at: String,
    pub os: String,
    pub arch: String,
    pub log_dir: String,
    /// Explicit for whoever opens the archive: no book text, prompt or database inside.
    pub note: String,
    pub sidecar: SidecarStatus,
    pub worker_running: bool,
    pub worker_paused: bool,
    pub queue: Vec<QueueCount>,
    pub endpoints: Vec<EndpointUsage>,
    pub failed_jobs: Vec<FailedJob>,
    pub llm_call_errors: Vec<LlmCallError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiagnosticsOutcome {
    pub output_path: String,
    pub bytes: u64,
    pub files: usize,
}

/// Build the report. The caller gathers the live values (sidecar, worker, queue) and this
/// function adds the persisted ones; nothing here touches the book text.
#[allow(clippy::too_many_arguments)]
pub async fn build_report(
    pool: &sqlx::SqlitePool,
    data_dir: &Path,
    sidecar: SidecarStatus,
    worker_running: bool,
    worker_paused: bool,
    queue: Vec<QueueCount>,
    endpoints: Vec<EndpointUsage>,
) -> Result<DiagnosticsReport> {
    let failed_jobs = sqlx::query_as::<_, FailedJob>(
        "SELECT id, project_id, kind, attempts, max_attempts, last_error, finished_at \
         FROM job WHERE state = 'failed' ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(pool)
    .await?;
    let llm_call_errors = sqlx::query_as::<_, LlmCallError>(
        "SELECT role, model, error, created_at FROM llm_call \
         WHERE error IS NOT NULL ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(pool)
    .await?;

    Ok(DiagnosticsReport {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        generated_at: crate::db::now(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        log_dir: logging::log_dir(data_dir).to_string_lossy().to_string(),
        note: "No book text, prompt, response, glossary value or database is included.".to_string(),
        sidecar,
        worker_running,
        worker_paused,
        queue,
        endpoints,
        failed_jobs,
        llm_call_errors,
    })
}

/// Write the report and the newest log files as a ZIP under `<data_dir>/diagnostics/`.
pub fn write_bundle(data_dir: &Path, report: &DiagnosticsReport) -> Result<DiagnosticsOutcome> {
    let dir = data_dir.join("diagnostics");
    std::fs::create_dir_all(&dir)?;
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let target = dir.join(format!("llmtz-diagnostics-{stamp}.zip"));
    let temp = dir.join(format!(".diagnostics-{}.tmp", crate::db::new_id()));

    let result = (|| -> Result<usize> {
        let mut files = 0;
        {
            let mut zip = zip::ZipWriter::new(File::create(&temp)?);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            zip.start_file(REPORT_NAME, options)?;
            zip.write_all(&serde_json::to_vec_pretty(report)?)?;
            files += 1;

            for path in newest_logs(&logging::log_dir(data_dir), MAX_LOG_FILES) {
                let Some(name) = path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                else {
                    continue;
                };
                zip.start_file(format!("logs/{name}"), options)?;
                zip.write_all(&read_tail(&path, MAX_LOG_BYTES)?)?;
                files += 1;
            }
            zip.finish()?;
        }
        Ok(files)
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if std::fs::rename(&temp, &target).is_err() {
        // Windows refuses to replace an existing destination with `rename`.
        let _ = std::fs::remove_file(&target);
        std::fs::rename(&temp, &target)?;
    }
    let bytes = std::fs::metadata(&target)?.len();
    Ok(DiagnosticsOutcome {
        output_path: target.to_string_lossy().to_string(),
        bytes,
        files: result.unwrap_or(0),
    })
}

/// The newest `.log` files in `dir`, at most `limit` of them.
fn newest_logs(dir: &Path, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file() && path.extension().is_some_and(|extension| extension == "log")
        })
        .collect();
    files.sort();
    if files.len() > limit {
        files.drain(..files.len() - limit);
    }
    files
}

/// Read at most the last `max_bytes` of a file, starting after the first newline of the
/// window so the report never begins with half a line.
fn read_tail(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if len <= max_bytes {
        let mut bytes = Vec::with_capacity(len as usize);
        file.read_to_end(&mut bytes)?;
        return Ok(bytes);
    }
    file.seek(SeekFrom::End(-(max_bytes as i64)))?;
    let mut bytes = Vec::with_capacity(max_bytes as usize);
    file.read_to_end(&mut bytes)?;
    if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
        bytes.drain(..=newline);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> DiagnosticsReport {
        DiagnosticsReport {
            app_version: "0.1.0".to_string(),
            generated_at: "2026-01-01T00:00:00.000Z".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            log_dir: "/tmp/logs".to_string(),
            note: "test".to_string(),
            sidecar: SidecarStatus {
                state: crate::sidecar::SidecarState::Running,
                pid: Some(1),
                attempts: 0,
                message: None,
            },
            worker_running: true,
            worker_paused: false,
            queue: vec![QueueCount {
                state: "pending".to_string(),
                count: 3,
            }],
            endpoints: Vec::new(),
            failed_jobs: vec![FailedJob {
                id: "j1".to_string(),
                project_id: "p1".to_string(),
                kind: "translate_chunk".to_string(),
                attempts: 3,
                max_attempts: 3,
                last_error: Some("endpoint refused".to_string()),
                finished_at: None,
            }],
            llm_call_errors: Vec::new(),
        }
    }

    #[test]
    fn the_bundle_carries_the_report_and_the_newest_logs() {
        let dir = tempfile::tempdir().expect("tempdir");
        let logs = logging::log_dir(dir.path());
        std::fs::create_dir_all(&logs).expect("logs dir");
        std::fs::write(logs.join("llmtz.2026-01-01.log"), "old\n").expect("write");
        std::fs::write(logs.join("llmtz.2026-01-02.log"), "new\n").expect("write");
        std::fs::write(logs.join("notes.txt"), "ignored\n").expect("write");

        let outcome = write_bundle(dir.path(), &report()).expect("bundle");
        assert!(Path::new(&outcome.output_path).is_file());
        assert_eq!(outcome.files, 3, "report + two logs");

        let file = File::open(&outcome.output_path).expect("open");
        let mut zip = zip::ZipArchive::new(file).expect("zip");
        let mut names: Vec<String> = (0..zip.len())
            .map(|index| zip.by_index(index).expect("entry").name().to_string())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "logs/llmtz.2026-01-01.log",
                "logs/llmtz.2026-01-02.log",
                "report.json"
            ]
        );

        let mut text = String::new();
        zip.by_name("report.json")
            .expect("report")
            .read_to_string(&mut text)
            .expect("read");
        assert!(text.contains("\"failed_jobs\""));
        assert!(text.contains("endpoint refused"));
        // The report must never carry prompt or response bodies.
        assert!(!text.contains("prompt_text"));
        assert!(!text.contains("response_text"));
    }

    #[test]
    fn read_tail_keeps_the_end_of_a_large_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("big.log");
        let mut content = String::new();
        for index in 0..1000 {
            content.push_str(&format!("line {index}\n"));
        }
        std::fs::write(&path, &content).expect("write");

        let tail = read_tail(&path, 64).expect("tail");
        let text = String::from_utf8(tail).expect("utf8");
        assert!(text.len() <= 64);
        assert!(text.ends_with("line 999\n"));
        assert!(!text.starts_with("line 0\n"));
    }

    #[test]
    fn newest_logs_limits_and_sorts() {
        let dir = tempfile::tempdir().expect("tempdir");
        for day in ["2026-01-01", "2026-01-02", "2026-01-03"] {
            std::fs::write(dir.path().join(format!("llmtz.{day}.log")), "x").expect("write");
        }
        let files = newest_logs(dir.path(), 2);
        assert_eq!(files.len(), 2);
        assert!(files[0].ends_with("llmtz.2026-01-02.log"));
        assert!(files[1].ends_with("llmtz.2026-01-03.log"));
    }
}
