//! Process logging: stdout for development plus a daily-rotated file under the app data
//! directory.
//!
//! The file is what a user can hand over after a failure, so it must exist in a packaged
//! app (where stdout goes nowhere) and be readable while the process runs. The writer is a
//! small `MakeWriter` implementation instead of a background appender: the volume is low
//! (events, not prompts) and flushing per event keeps the file current mid-run.
//!
//! What is **not** logged: book text, prompt or response bodies, glossary values. Those stay
//! in the `llm_call` audit table, which the diagnostics bundle deliberately excludes.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// Directory holding the log files, relative to the app data dir.
pub const LOG_DIR: &str = "logs";
/// Base name of the log file; the date is inserted before the extension.
const LOG_STEM: &str = "llmtz";

/// Where the log files live.
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(LOG_DIR)
}

/// Install the global subscriber: stdout plus the daily file.
///
/// Idempotent: the file writer is created once per process. If the log file cannot be
/// opened the process keeps running with stdout only, reported on stderr.
pub fn init(data_dir: &Path) {
    install(data_dir, true);
}

/// Like [`init`], but the console stays clean: the CLI writes its result to stdout and its
/// progress to stderr, so the tracing output goes to the file only.
pub fn init_file_only(data_dir: &Path) {
    install(data_dir, false);
}

fn install(data_dir: &Path, stdout: bool) {
    static FILE_WRITER: OnceLock<Option<Arc<DailyFileWriter>>> = OnceLock::new();

    let dir = log_dir(data_dir);
    let writer = FILE_WRITER
        .get_or_init(|| match DailyFileWriter::new(dir.clone()) {
            Ok(writer) => Some(Arc::new(writer)),
            Err(error) => {
                eprintln!("[logging] file log disabled ({dir:?}): {error}");
                None
            }
        })
        .clone();

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = stdout.then(tracing_subscriber::fmt::layer);
    let file_layer = writer.clone().map(|writer| {
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(writer)
    });
    // A second call (tests, or a reload) must not panic.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .try_init();
}

/// Current log file name for a given day, exposed so the diagnostics bundle and the tests
/// agree with the writer on the layout.
pub fn log_file_name(date: &str) -> String {
    format!("{LOG_STEM}.{date}.log")
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn open_log_file(dir: &Path, date: &str) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(log_file_name(date)))
}

struct FileState {
    date: String,
    file: File,
}

/// A file writer that rolls over to a new file when the local date changes.
struct DailyFileWriter {
    dir: PathBuf,
    state: Mutex<FileState>,
}

impl DailyFileWriter {
    fn new(dir: PathBuf) -> io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        let date = today();
        let file = open_log_file(&dir, &date)?;
        Ok(Self {
            dir,
            state: Mutex::new(FileState { date, file }),
        })
    }

    fn write_event(&self, buf: &[u8]) -> io::Result<usize> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let date = today();
        if state.date != date {
            state.file = open_log_file(&self.dir, &date)?;
            state.date = date;
        }
        state.file.write_all(buf)?;
        state.file.flush()?;
        Ok(buf.len())
    }

    fn flush(&self) -> io::Result<()> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.file.flush()
    }
}

impl Write for &DailyFileWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_event(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        DailyFileWriter::flush(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_dir_is_under_the_data_dir() {
        assert_eq!(log_dir(Path::new("/data")), PathBuf::from("/data/logs"));
    }

    #[test]
    fn the_writer_appends_and_rolls_over_by_date() {
        let dir = tempfile::tempdir().expect("tempdir");
        let writer = DailyFileWriter::new(dir.path().to_path_buf()).expect("writer");
        writer.write_event(b"first\n").expect("write");
        writer.write_event(b"second\n").expect("write");
        let path = dir.path().join(log_file_name(&today()));
        let content = std::fs::read_to_string(&path).expect("read");
        assert_eq!(content, "first\nsecond\n");

        // A stale date forces a new file instead of appending to the old one.
        {
            let mut state = writer.state.lock().expect("lock");
            state.date = "1999-01-01".to_string();
        }
        writer.write_event(b"third\n").expect("write");
        let content = std::fs::read_to_string(&path).expect("read");
        assert_eq!(content, "first\nsecond\nthird\n");
    }

    /// The global install must be idempotent: calling it twice cannot panic.
    #[test]
    fn init_is_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        init(dir.path());
        init(dir.path());
        tracing::info!(target: "logging-test", "hello from the test");
    }
}
