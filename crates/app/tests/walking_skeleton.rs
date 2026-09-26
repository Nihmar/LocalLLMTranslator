//! Walking-skeleton end-to-end test for milestone M1 (PLAN.md section 13).
//!
//! It drives the *real* control plane against the *real* Python sidecar and the
//! deterministic `tools/fake_llama_server.py`, with no Tauri `AppHandle` and no
//! mocks of our own code:
//!
//! ```text
//! make_fixtures.py -> EPUB
//!   -> detect_format -> ingest -> parse_document -> build_chunks  (run_ingest)
//!   -> translate every chunk through the worker pool + LlamaClient (fake server)
//!   -> export EPUB + PDF through the sidecar's pandoc bridge
//! ```
//!
//! and then re-runs the same work after simulating an abrupt crash, asserting
//! that a resumed run is lossless and identical to the uninterrupted one, and
//! that re-running a finished pipeline is idempotent.
//!
//! # Compliant answers and the `needs_review` path
//!
//! `tools/fake_llama_server.py` returns only the translated passage by default,
//! like a compliant instruction-following model, so the shipped prompt aligns
//! block-for-block and every chunk ends `done`. `--echo-prompt-prefix` restores
//! the old behaviour of echoing the non-translatable preface that precedes the
//! `PASSAGE TO TRANSLATE:` marker; the extra blocks then make the pipeline refuse
//! to align by force and mark the chunk `needs_review` with a NULL `target_md`
//! (PLAN.md section 15), while the raw reply is preserved in
//! `llm_call.response_text`. `echoed_preface_marks_the_chunk_needs_review`
//! exercises exactly that path.
//!
//! The exported EPUB is asserted to be free of placeholder tokens: `render_chunks`
//! writes `chunk.target_md`, which the pipeline now stores as the validated,
//! placeholder-free composition of the chunk's blocks.
//!
//! # M2: media, footnotes and tables
//!
//! The fixture EPUB embeds a real PNG behind a `<figure><img>`, a table with a
//! caption and alignment classes, and two footnotes. The ingest must extract the
//! image under `<work_dir>/assets/` and reference it as `assets/...`; `run_export`
//! hands pandoc that directory through `resource_path`, so the exported EPUB must
//! contain the image bytes and the exported PDF the footnote and table text.
//! `assert_ingest_assets` and `assert_export_media_and_structure` cover this.
//!
//! Environment gate: the test skips cleanly (early return + `eprintln!`) when
//! `sidecar/.venv/bin/python`, `pandoc` or `/bin/sh` are missing; when no PDF
//! engine is on `PATH` it skips only the PDF assertion.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use sqlx::SqlitePool;

use app_lib::db::models::{Block, Job, LlmEndpoint, Project, RoleBinding};
use app_lib::db::{self, now, repo};
use app_lib::error::{AppError, Result as AppResult};
use app_lib::events::{sidecar_event_sink, EventEmitter, NullEmitter};
use app_lib::llm::LlamaClient;
use app_lib::pipeline::export::{run_export, ExportRequest};
use app_lib::pipeline::ingest::run_ingest;
use app_lib::pipeline::translate::run_translate_chunk;
use app_lib::pipeline::PipelineDeps;
use app_lib::resources::ResourceGovernor;
use app_lib::scheduler::queue::{self, NewJob};
use app_lib::scheduler::{JobDispatcher, WorkerPool};
use app_lib::sidecar::{ParseDocumentResult, SidecarClient, SpawnSpec, Supervisor};

/// The `«`/`»` wrap the fake translation puts around every translated word:
/// its presence proves the text came out of the fake model.
const TRANSLATION_MARKER: &str = "\u{00ab}";

/// Opening delimiter of a placeholder token (`⟦12⟧`); it must never reach an
/// exported artifact, because a real link/emphasis/code literal stands in its place.
const PLACEHOLDER_OPEN: &str = "\u{27e6}";

/// Hard ceiling for the fake server to answer `/health`.
const SERVER_READY: Duration = Duration::from_secs(20);
/// Per-request ceiling handed to the sidecar supervisor.
const SIDECAR_TIMEOUT: Duration = Duration::from_secs(120);
/// Ceiling for one whole pipeline stage (ingest / translate / export).
const PIPELINE_TIMEOUT: Duration = Duration::from_secs(180);
/// Ceiling for the worker pool to drain the job queue.
const WORK_DEADLINE: Duration = Duration::from_secs(120);
/// Ceiling for generating the fixtures.
const FIXTURE_TIMEOUT: Duration = Duration::from_secs(180);
/// Delay the fake server adds before answering a chat request, so that a run can
/// be interrupted deterministically after the first chunk completes.
const MODEL_DELAY_MS: u64 = 150;

type LogSink = Arc<Mutex<Vec<String>>>;

// ---------------------------------------------------------------------------
// Guards: both child processes are killed on every exit path, assertion
// failures included.
// ---------------------------------------------------------------------------

/// Kills and reaps a child process when dropped.
struct ChildGuard {
    label: &'static str,
    child: Option<Child>,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // A child that already exited returns an error here; that is fine.
            if let Err(error) = child.kill() {
                eprintln!(
                    "[walking_skeleton] note: kill {} returned {error}",
                    self.label
                );
            }
            let _ = child.wait();
        }
    }
}

/// Shuts the sidecar supervisor (and its child process) down when dropped.
struct SupervisorGuard {
    supervisor: Arc<Supervisor>,
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        self.supervisor.shutdown();
    }
}

// ---------------------------------------------------------------------------
// The dispatcher: real queue + real pipeline, no mocks.
// ---------------------------------------------------------------------------

/// Executes claimed jobs by calling the real pipeline functions. It mirrors what
/// `app_lib::PipelineDispatcher` does for `translate_chunk`, but is constructible
/// from an integration test (the production one has no public constructor).
struct TestDispatcher {
    deps: PipelineDeps,
}

#[async_trait]
impl JobDispatcher for TestDispatcher {
    async fn dispatch(&self, job: &Job) -> AppResult<()> {
        let payload: Value = serde_json::from_str(&job.payload_json).unwrap_or(Value::Null);
        match job.kind.as_str() {
            "translate_chunk" => {
                let chunk_id = payload
                    .get("chunk_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| {
                        AppError::Invalid("translate_chunk job is missing chunk_id".into())
                    })?;
                run_translate_chunk(&self.deps, Some(&job.id), &chunk_id).await?;
                Ok(())
            }
            other => Err(AppError::Invalid(format!(
                "the test dispatcher does not handle job kind '{other}'"
            ))),
        }
    }
}

// ---------------------------------------------------------------------------
// The test
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn walking_skeleton_end_to_end() -> Result<()> {
    // ---- environment gate -------------------------------------------------
    let root = repo_root()?;
    let venv_python = root.join("sidecar/.venv/bin/python");
    if !venv_python.is_file() {
        eprintln!(
            "[walking_skeleton] SKIP: {} is missing; the sidecar interpreter is not installed. \
             Run `uv sync --all-groups` inside sidecar/.",
            venv_python.display()
        );
        return Ok(());
    }
    if find_in_path("pandoc").is_none() {
        eprintln!(
            "[walking_skeleton] SKIP: `pandoc` is not on PATH; EPUB/PDF export is unavailable."
        );
        return Ok(());
    }
    if !Path::new("/bin/sh").exists() {
        eprintln!("[walking_skeleton] SKIP: /bin/sh is missing; cannot launch the sidecar.");
        return Ok(());
    }
    let pdf_engine = find_pdf_engine();
    if pdf_engine.is_none() {
        eprintln!(
            "[walking_skeleton] note: no PDF engine on PATH; the PDF artifact assertion will be skipped."
        );
    }

    // ---- fixtures ---------------------------------------------------------
    let fixture_dir = tempfile::tempdir().context("fixture temp dir")?;
    let epub = build_fixture(
        &venv_python,
        &root.join("tools/make_fixtures.py"),
        fixture_dir.path(),
    )
    .await?;
    let epub_str = epub.to_string_lossy().to_string();

    // ---- deterministic fake llama-server on an ephemeral port -------------
    let (fake_url, _fake_guard, fake_log) =
        start_fake_server(&venv_python, &root.join("tools/fake_llama_server.py"), &[])?;
    wait_for_health(&fake_url, SERVER_READY).await?;
    eprintln!("[walking_skeleton] fake llama-server ready at {fake_url}");

    // ---- the real sidecar through the real supervisor ---------------------
    let supervisor = start_sidecar(&root.join("sidecar"), &venv_python)?;
    let _supervisor_guard = SupervisorGuard {
        supervisor: supervisor.clone(),
    };
    let client = SidecarClient::new(supervisor.clone());
    let ping = tokio::time::timeout(SIDECAR_TIMEOUT, client.ping()).await??;
    ensure!(ping.pong, "sidecar ping did not return pong");
    eprintln!("[walking_skeleton] sidecar ready, version {}", ping.version);

    // =======================================================================
    // Run A: the uninterrupted pipeline, the reference result.
    // =======================================================================
    let dir_a = tempfile::tempdir().context("run A data dir")?;
    let pool_a = db::connect(&dir_a.path().join("app.sqlite")).await?;
    let project_a = seed_environment(&pool_a, "A", &epub, &fake_url).await?;
    let deps_a = PipelineDeps::new(
        pool_a.clone(),
        client.clone(),
        ResourceGovernor::default(),
        dir_a.path().to_path_buf(),
    );

    let ingest_a = with_timeout(
        run_ingest(&deps_a, &project_a, &epub_str, None, None),
        "run A ingest",
    )
    .await??;

    let document_a = ingest_a.document_id.clone();
    assert_persistence(&pool_a, &client, &document_a, &ingest_a.markdown_path).await?;

    // M2: the fixture EPUB embeds a real PNG referenced by `<figure><img>`; ingest
    // must extract it under `assets/`, rewrite the Markdown href and leave the
    // file on disk next to `document.md`.
    assert_ingest_assets(&ingest_a.markdown_path, &ingest_a.assets).await?;

    // Translate every chunk through the real worker pool / queue.
    let jobs_a = enqueue_translate_jobs(&pool_a, &project_a).await?;
    ensure!(
        jobs_a.len() >= 2,
        "expected several chunks, got {}",
        jobs_a.len()
    );
    run_pool_to_completion(&pool_a, &deps_a, jobs_a.len(), "run A").await?;

    let translations_a = assert_translation_complete(&pool_a, &project_a, &document_a).await?;
    assert_no_structural_findings(&pool_a, &project_a).await?;
    assert_structure_preserved(&pool_a, &client, &document_a, &translations_a).await?;
    assert_fake_server_reached(&pool_a, jobs_a.len(), &fake_log).await?;

    // Export both artifacts through the real export path.
    let (epub_path, pdf_path) =
        export_book(&deps_a, &project_a, &venv_python, pdf_engine.is_some()).await?;

    // M2 acceptance: the image, the footnotes and the table must be present and
    // intact in the exported artifacts, not merely the translated prose.
    assert_export_media_and_structure(
        &epub_path,
        pdf_path.as_deref(),
        &venv_python,
        &fixture_dir.path().join("harbour.png"),
    )
    .await?;

    let blocks_a = block_signature(&pool_a, &document_a).await?;

    eprintln!("[walking_skeleton] run A complete: {} chunks", jobs_a.len());

    // =======================================================================
    // Run B: crash halfway, apply the boot recovery, resume, compare.
    // =======================================================================
    let dir_b = tempfile::tempdir().context("run B data dir")?;
    let db_b_path = dir_b.path().join("app.sqlite");
    let pool_b = db::connect(&db_b_path).await?;
    let project_b = seed_environment(&pool_b, "B", &epub, &fake_url).await?;
    let deps_b = PipelineDeps::new(
        pool_b.clone(),
        client.clone(),
        ResourceGovernor::default(),
        dir_b.path().to_path_buf(),
    );

    let ingest_b = with_timeout(
        run_ingest(&deps_b, &project_b, &epub_str, None, None),
        "run B ingest",
    )
    .await??;
    let document_b = ingest_b.document_id.clone();

    let jobs_b = enqueue_translate_jobs(&pool_b, &project_b).await?;
    let job_of: BTreeMap<String, String> = jobs_b.iter().cloned().collect();
    ensure!(
        jobs_b.len() >= 2,
        "expected several chunks, got {}",
        jobs_b.len()
    );

    // Translate part of it, then stop abruptly.
    interrupt_after_first_chunk(&pool_b, &deps_b, &project_b, WORK_DEADLINE).await?;

    // Deterministically reproduce the state a SIGKILL leaves: one chunk mid-run
    // with its job still leased/running against a live lease.
    let (victim_chunk, victim_job) = force_crash_remnant(&pool_b, &project_b, &job_of).await?;

    // "The process died": close the pool, reopen it, and run the boot recovery
    // the app performs in `build_state` (requeue in-flight jobs, reset running
    // chunks).
    pool_b.close().await;
    let pool_b = db::connect(&db_b_path).await?;
    let requeued = queue::requeue_in_flight(&pool_b).await?;
    let reset_chunks = repo::reset_running_chunks(&pool_b, None).await?;
    ensure!(
        requeued >= 1,
        "boot recovery requeued no in-flight job (the crash remnant was not reproduced)"
    );
    ensure!(
        reset_chunks >= 1,
        "boot recovery reset no running chunk (the crash remnant was not reproduced)"
    );
    eprintln!(
        "[walking_skeleton] crash recovery: requeued {requeued} job(s), reset {reset_chunks} chunk(s)"
    );

    let recovered_chunk = repo::get_chunk(&pool_b, &victim_chunk)
        .await?
        .ok_or_else(|| anyhow!("victim chunk {victim_chunk} disappeared"))?;
    ensure!(
        recovered_chunk.status == "pending",
        "boot recovery left the interrupted chunk in status {}",
        recovered_chunk.status
    );
    let recovered_job = queue::get_job(&pool_b, &victim_job)
        .await?
        .ok_or_else(|| anyhow!("victim job {victim_job} disappeared"))?;
    ensure!(
        recovered_job.state == "pending",
        "boot recovery left the interrupted job in state {}",
        recovered_job.state
    );

    // Finish the work with a fresh pool, exactly as a restarted process would.
    let deps_b = PipelineDeps::new(
        pool_b.clone(),
        client.clone(),
        ResourceGovernor::default(),
        dir_b.path().to_path_buf(),
    );
    run_pool_to_completion(&pool_b, &deps_b, jobs_b.len(), "resume").await?;

    let translations_b = assert_translation_complete(&pool_b, &project_b, &document_b).await?;
    assert_no_structural_findings(&pool_b, &project_b).await?;
    assert_structure_preserved(&pool_b, &client, &document_b, &translations_b).await?;

    // Nothing was lost and the result is identical to the uninterrupted run.
    let blocks_b = block_signature(&pool_b, &document_b).await?;
    ensure!(
        blocks_a == blocks_b,
        "the resumed run persisted a different set of blocks than the uninterrupted run"
    );
    ensure!(
        translations_a == translations_b,
        "the resumed run produced different translations than the uninterrupted run"
    );

    // Export the resumed project too: the resumed pipeline must still export.
    let resumed_epub = export_one(&deps_b, &project_b, "epub").await?;
    assert_zip_magic(&resumed_epub.output_path)?;

    eprintln!("[walking_skeleton] run B (resumed) matches run A");

    // =======================================================================
    // Idempotency: re-running a completed pipeline duplicates nothing.
    // =======================================================================
    let chunks_before = repo::list_chunks(&pool_b, &document_b).await?;
    let re_ingest = with_timeout(
        run_ingest(&deps_b, &project_b, &epub_str, None, None),
        "idempotency re-ingest",
    )
    .await??;
    let document_re = re_ingest.document_id.clone();

    let blocks_re = repo::list_blocks(&pool_b, &document_re).await?;
    ensure!(
        blocks_re.len() == blocks_a.len(),
        "re-ingesting changed the block count ({} -> {})",
        blocks_a.len(),
        blocks_re.len()
    );
    let signature_re = block_signature(&pool_b, &document_re).await?;
    ensure!(
        signature_re == blocks_a,
        "re-ingesting did not reproduce the same deterministic blocks"
    );
    let chunks_re = repo::list_chunks(&pool_b, &document_re).await?;
    ensure!(
        chunks_re.len() == chunks_before.len(),
        "re-ingesting changed the chunk count ({} -> {})",
        chunks_before.len(),
        chunks_re.len()
    );

    // M3 acceptance: re-translating already completed chunks is served by the
    // block-level translation memory, so the model is not called again.
    let calls_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM llm_call WHERE role = 'translator'")
            .fetch_one(&pool_b)
            .await?;
    for chunk in &chunks_re {
        with_timeout(
            run_translate_chunk(&deps_b, None, &chunk.id),
            "idempotency re-translate",
        )
        .await??;
    }
    let calls_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM llm_call WHERE role = 'translator'")
            .fetch_one(&pool_b)
            .await?;
    ensure!(
        calls_after == calls_before,
        "re-running completed chunks called the model again ({calls_before} -> {calls_after}): \
         the translation memory did not reuse the accepted blocks"
    );
    let translations_re = assert_translation_complete(&pool_b, &project_b, &document_re).await?;
    ensure!(
        translations_re == translations_b,
        "re-running the completed pipeline changed the translations"
    );

    eprintln!("[walking_skeleton] idempotency check passed (no duplicated blocks or translations)");

    // Keep the artifact paths alive so their existence is asserted, not just the
    // intermediate values.
    ensure!(Path::new(&epub_path).is_file(), "run A EPUB disappeared");
    if let Some(pdf) = pdf_path {
        ensure!(Path::new(&pdf).is_file(), "run A PDF disappeared");
    }

    Ok(())
}

/// A model that echoes the prompt preface answers irregularly: the pipeline must
/// refuse to align by force, mark the chunk `needs_review` and leave `target_md`
/// NULL (PLAN.md section 15), while keeping the raw reply for audit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn echoed_preface_marks_the_chunk_needs_review() -> Result<()> {
    let root = repo_root()?;
    let venv_python = root.join("sidecar/.venv/bin/python");
    if !venv_python.is_file() {
        eprintln!(
            "[walking_skeleton] SKIP: {} is missing; the sidecar interpreter is not installed.",
            venv_python.display()
        );
        return Ok(());
    }
    if !Path::new("/bin/sh").exists() {
        eprintln!("[walking_skeleton] SKIP: /bin/sh is missing; cannot launch the sidecar.");
        return Ok(());
    }

    let fixture_dir = tempfile::tempdir().context("fixture temp dir")?;
    let epub = build_fixture(
        &venv_python,
        &root.join("tools/make_fixtures.py"),
        fixture_dir.path(),
    )
    .await?;

    // The fault: the model echoes the non-translatable preface ahead of the passage.
    let (fake_url, _fake_guard, _fake_log) = start_fake_server(
        &venv_python,
        &root.join("tools/fake_llama_server.py"),
        &["--echo-prompt-prefix"],
    )?;
    wait_for_health(&fake_url, SERVER_READY).await?;

    let supervisor = start_sidecar(&root.join("sidecar"), &venv_python)?;
    let _supervisor_guard = SupervisorGuard {
        supervisor: supervisor.clone(),
    };
    let client = SidecarClient::new(supervisor.clone());

    let data_dir = tempfile::tempdir().context("needs_review data dir")?;
    let pool = db::connect(&data_dir.path().join("app.sqlite")).await?;
    let project_id = seed_environment(&pool, "echo", &epub, &fake_url).await?;
    let deps = PipelineDeps::new(
        pool.clone(),
        client.clone(),
        ResourceGovernor::default(),
        data_dir.path().to_path_buf(),
    );

    let ingest = with_timeout(
        run_ingest(&deps, &project_id, &epub.to_string_lossy(), None, None),
        "echo ingest",
    )
    .await??;
    let document = ingest.document_id.clone();

    let chunks = repo::list_chunks(&pool, &document).await?;
    let chunk = chunks
        .first()
        .ok_or_else(|| anyhow!("the echoed run produced no chunks"))?;

    let outcome = with_timeout(
        run_translate_chunk(&deps, None, &chunk.id),
        "echo translate",
    )
    .await??;
    ensure!(
        outcome.status == "needs_review",
        "an echoed preface must mark the chunk needs_review, got status {}",
        outcome.status
    );

    let stored = repo::get_chunk(&pool, &chunk.id)
        .await?
        .ok_or_else(|| anyhow!("chunk {} disappeared", chunk.id))?;
    ensure!(
        stored.status == "needs_review",
        "the persisted chunk status is {}, expected needs_review",
        stored.status
    );
    ensure!(
        stored.target_md.is_none(),
        "a needs_review chunk must have a NULL target_md, got {:?}",
        stored.target_md
    );

    // The rejected answer is preserved for audit, not thrown away.
    let preserved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM llm_call WHERE role = 'translator' \
         AND response_text IS NOT NULL AND response_text <> ''",
    )
    .fetch_one(&pool)
    .await?;
    ensure!(
        preserved >= 1,
        "the raw rejected response was not preserved in llm_call"
    );

    eprintln!("[walking_skeleton] echoed preface correctly produced a needs_review chunk");
    Ok(())
}

// ---------------------------------------------------------------------------
// Environment helpers
// ---------------------------------------------------------------------------

/// The repository root, derived from the crate manifest directory at compile time.
fn repo_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest
        .join("../..")
        .canonicalize()
        .with_context(|| format!("resolving the repository root from {}", manifest.display()))?;
    Ok(root)
}

/// Search `PATH` for an executable.
fn find_in_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The first PDF engine pandoc can use, if any is installed.
fn find_pdf_engine() -> Option<String> {
    [
        "pdflatex",
        "xelatex",
        "lualatex",
        "tectonic",
        "wkhtmltopdf",
        "weasyprint",
        "prince",
    ]
    .into_iter()
    .find(|name| find_in_path(name).is_some())
    .map(str::to_string)
}

/// Wrap a pipeline future in a hard timeout so the test can never hang.
async fn with_timeout<F: std::future::Future>(future: F, what: &str) -> Result<F::Output> {
    match tokio::time::timeout(PIPELINE_TIMEOUT, future).await {
        Ok(output) => Ok(output),
        Err(_) => bail!("{what} timed out after {PIPELINE_TIMEOUT:?}"),
    }
}

// ---------------------------------------------------------------------------
// Fixtures and child processes
// ---------------------------------------------------------------------------

/// Generate the small fixture EPUB into `out_dir` and return its path.
async fn build_fixture(python: &Path, script: &Path, out_dir: &Path) -> Result<PathBuf> {
    let output = tokio::time::timeout(
        FIXTURE_TIMEOUT,
        tokio::process::Command::new(python)
            .arg(script)
            .arg("--out-dir")
            .arg(out_dir)
            .arg("--small-only")
            .arg("--quiet")
            .output(),
    )
    .await
    .context("make_fixtures.py timed out")??;
    ensure!(
        output.status.success(),
        "make_fixtures.py failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let epub = out_dir.join("content.epub");
    ensure!(
        epub.is_file(),
        "fixture {} was not produced",
        epub.display()
    );
    Ok(epub)
}

/// Start the fake llama-server on an ephemeral port and return its base URL, a
/// kill-on-drop guard and the shared stderr log.
///
/// `extra_args` append fault-injection flags (for example `--echo-prompt-prefix`).
fn start_fake_server(
    python: &Path,
    script: &Path,
    extra_args: &[&str],
) -> Result<(String, ChildGuard, LogSink)> {
    let mut child = Command::new(python)
        .arg(script)
        .arg("--port")
        .arg("0")
        .arg("--delay-ms")
        .arg(MODEL_DELAY_MS.to_string())
        .args(extra_args)
        // `FAKE_LLAMA_LOG` makes the stdlib HTTP handler log every request line
        // to stderr, which proves the control plane actually reached the server.
        .env("FAKE_LLAMA_LOG", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawning the fake llama-server")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("fake server stdout was not captured"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("fake server stderr was not captured"))?;

    let log: LogSink = Arc::new(Mutex::new(Vec::new()));
    {
        let log = log.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
            {
                log.lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .push(line);
            }
        });
    }

    let guard = ChildGuard {
        label: "fake-llama-server",
        child: Some(child),
    };
    let port = read_announced_port(stdout, SERVER_READY)?;
    Ok((format!("http://127.0.0.1:{port}"), guard, log))
}

/// Read the `... listening on http://127.0.0.1:PORT` banner, with a bound so a
/// server that fails to start cannot hang the test.
fn read_announced_port(stdout: std::process::ChildStdout, timeout: Duration) -> Result<u16> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut first = String::new();
        let _ = reader.read_line(&mut first);
        let _ = tx.send(first);
        // Keep draining so the child never blocks on a full stdout pipe.
        let mut rest = String::new();
        while reader.read_line(&mut rest).map(|n| n > 0).unwrap_or(false) {
            rest.clear();
        }
    });
    let banner = rx
        .recv_timeout(timeout)
        .context("the fake server did not announce its port in time")?;
    banner
        .rsplit(':')
        .next()
        .and_then(|port| port.trim().parse::<u16>().ok())
        .ok_or_else(|| anyhow!("could not parse a port from the fake server banner {banner:?}"))
}

/// Poll `/health` until it answers, bounded by `deadline`.
async fn wait_for_health(base_url: &str, deadline: Duration) -> Result<()> {
    let client = LlamaClient::new(base_url)?;
    let start = Instant::now();
    loop {
        if let Ok(Ok(status)) = tokio::time::timeout(Duration::from_secs(2), client.health()).await
        {
            if status.ok {
                return Ok(());
            }
        }
        if start.elapsed() >= deadline {
            bail!("the fake server /health did not become ready within {deadline:?}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Start the sidecar through the real [`Supervisor`].
///
/// The supervisor spawns `program` with `args` and has no `current_dir` option,
/// so the interpreter is launched through a shell that `cd`s into `sidecar/`
/// (the package must be importable: `python -m llmtranslator_sidecar`).
fn start_sidecar(sidecar_dir: &Path, python: &Path) -> Result<Arc<Supervisor>> {
    let command = format!(
        "cd {} && exec {} -m llmtranslator_sidecar",
        shell_quote(sidecar_dir),
        shell_quote(python),
    );
    let spec = SpawnSpec::new("/bin/sh", vec!["-c".to_string(), command]);
    let emitter: Arc<dyn EventEmitter> = Arc::new(NullEmitter);
    let sink = sidecar_event_sink(emitter.clone());
    Ok(Supervisor::new(spec, sink, emitter, Some(SIDECAR_TIMEOUT)))
}

/// Single-quote a path for `/bin/sh`.
fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

// ---------------------------------------------------------------------------
// Database setup
// ---------------------------------------------------------------------------

/// Insert a project, an `llm_endpoint` pointing at the fake server and the
/// `translator` role binding. Returns the project id.
async fn seed_environment(
    pool: &SqlitePool,
    name: &str,
    source: &Path,
    base_url: &str,
) -> Result<String> {
    let project_id = db::new_id();
    let endpoint_id = db::new_id();
    let timestamp = now();

    repo::insert_project(
        pool,
        &Project {
            id: project_id.clone(),
            name: format!("skeleton-{name}"),
            source_path: source.to_string_lossy().to_string(),
            source_hash: "fixture-hash".to_string(),
            source_format: "epub".to_string(),
            source_lang: Some("en".to_string()),
            target_lang: "it".to_string(),
            doc_title: None,
            doc_author: None,
            prompts_snapshot_dir: None,
            settings_json: "{}".to_string(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        },
    )
    .await?;

    repo::upsert_endpoint(
        pool,
        &LlmEndpoint {
            id: endpoint_id.clone(),
            name: format!("fake-{name}"),
            base_url: base_url.to_string(),
            api_key_ref: None,
            max_concurrency: Some(1),
            notes: None,
            last_health_at: None,
            last_health_ok: None,
            props_json: None,
        },
    )
    .await?;

    repo::upsert_role_binding(
        pool,
        &RoleBinding {
            id: db::new_id(),
            endpoint_id,
            role: "translator".to_string(),
            model: "fake-model".to_string(),
            params_json: "{}".to_string(),
            priority: 0,
        },
    )
    .await?;

    Ok(project_id)
}

/// The document id of `project_id`.
async fn document_id(pool: &SqlitePool, project_id: &str) -> Result<String> {
    let id: Option<String> =
        sqlx::query_scalar("SELECT id FROM document WHERE project_id = ?1 LIMIT 1")
            .bind(project_id)
            .fetch_optional(pool)
            .await?;
    id.ok_or_else(|| anyhow!("project {project_id} has no document"))
}

// ---------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------

/// Every block the extractor produced is persisted with a deterministic id, and
/// the chunks cover the blocks exactly.
async fn assert_persistence(
    pool: &SqlitePool,
    client: &SidecarClient,
    document_id: &str,
    markdown_path: &str,
) -> Result<()> {
    let stored = repo::list_blocks(pool, document_id).await?;
    let parsed = parse_document_bounded(client, markdown_path).await?;

    ensure!(
        stored.len() == parsed.blocks.len(),
        "persisted {} blocks but the extractor produced {}",
        stored.len(),
        parsed.blocks.len()
    );
    for (index, (persisted, extracted)) in stored.iter().zip(parsed.blocks.iter()).enumerate() {
        let expected_id = format!("b{index:06}");
        ensure!(
            persisted.id == expected_id,
            "block #{index} has id {} instead of the deterministic {expected_id}",
            persisted.id
        );
        ensure!(
            persisted.id == extracted.id,
            "persisted block {} does not match the extractor's {}",
            persisted.id,
            extracted.id
        );
        ensure!(
            persisted.kind == extracted.kind,
            "block {} kind {} != extractor kind {}",
            persisted.id,
            persisted.kind,
            extracted.kind
        );
        ensure!(
            persisted.order_index == index as i64,
            "block {} has order {} but sits at index {index}",
            persisted.id,
            persisted.order_index
        );
    }

    let stored_chapters = repo::list_chapters(pool, document_id).await?;
    ensure!(
        stored_chapters.len() == parsed.chapters.len(),
        "persisted {} chapters but the extractor produced {}",
        stored_chapters.len(),
        parsed.chapters.len()
    );

    // Chunks: every block appears in exactly one chunk.
    let chunks = repo::list_chunks(pool, document_id).await?;
    ensure!(
        chunks.len() >= 2,
        "expected several chunks, got {}",
        chunks.len()
    );
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for chunk in &chunks {
        let ids: Vec<String> = serde_json::from_str(&chunk.block_ids_json)?;
        for id in ids {
            ensure!(
                seen.insert(id.clone()),
                "block {id} appears in more than one chunk"
            );
        }
    }
    let all_ids: BTreeSet<String> = stored.iter().map(|b| b.id.clone()).collect();
    ensure!(
        seen == all_ids,
        "the chunks' block_ids do not cover the persisted blocks exactly"
    );
    Ok(())
}

/// The chunk is `done`, and every translatable block has a placeholder-safe
/// `block_translation` row. Returns the `block_id -> text_md` map.
async fn assert_translation_complete(
    pool: &SqlitePool,
    project_id: &str,
    document_id: &str,
) -> Result<BTreeMap<String, String>> {
    let chunks = repo::list_chunks(pool, document_id).await?;
    ensure!(!chunks.is_empty(), "the project has no chunks");
    for chunk in &chunks {
        ensure!(
            chunk.status == "done",
            "chunk {} ended in status {} (error: {:?})",
            chunk.id,
            chunk.status,
            chunk.error
        );
    }

    let blocks = repo::list_blocks(pool, document_id).await?;
    let translatable: Vec<&Block> = blocks.iter().filter(|b| b.translatable).collect();
    let translations = block_translation_map(pool, document_id).await?;
    for block in &translatable {
        ensure!(
            translations.contains_key(&block.id),
            "translatable block {} has no block_translation row",
            block.id
        );
    }
    ensure!(
        translations.len() == translatable.len(),
        "found {} block_translation rows for {} translatable blocks",
        translations.len(),
        translatable.len()
    );

    let lost: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM block_translation bt \
         JOIN chunk c ON c.id = bt.chunk_id \
         JOIN document d ON d.id = c.document_id \
         WHERE d.project_id = ?1 AND bt.placeholders_ok = 0",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?;
    ensure!(
        lost == 0,
        "{lost} block translations lost their placeholders"
    );

    Ok(translations)
}

/// No placeholder breakage anywhere: neither a `placeholder_broken` QA finding
/// nor any QA finding at all (a clean alignment produces none).
async fn assert_no_structural_findings(pool: &SqlitePool, project_id: &str) -> Result<()> {
    let findings = repo::list_qa_findings(pool, project_id).await?;
    let broken: Vec<&str> = findings
        .iter()
        .filter(|f| f.kind == "placeholder_broken")
        .map(|f| f.chunk_id.as_deref().unwrap_or("<none>"))
        .collect();
    ensure!(
        broken.is_empty(),
        "placeholder_broken QA finding(s) on chunk(s): {broken:?}"
    );
    // The fake model returns deliberately pseudo-translations, so advisory
    // findings (untranslated runs, length anomalies) are expected; a clean run
    // must not report a structural defect.
    let structural: Vec<&str> = findings
        .iter()
        .filter(|f| matches!(f.kind.as_str(), "empty" | "markdown_malformed"))
        .map(|f| f.kind.as_str())
        .collect();
    ensure!(
        structural.is_empty(),
        "structural QA finding(s) on a clean run: {structural:?}"
    );
    Ok(())
}

/// The structure survives: re-parsing the translated document yields the same
/// sequence of block kinds as the source. This is the M1 acceptance criterion.
async fn assert_structure_preserved(
    pool: &SqlitePool,
    client: &SidecarClient,
    document_id: &str,
    translations: &BTreeMap<String, String>,
) -> Result<()> {
    let blocks = repo::list_blocks(pool, document_id).await?;
    let mut parts = Vec::with_capacity(blocks.len());
    for block in &blocks {
        // `render(blocks, translations)`: the translation when present (only for
        // translatable blocks), otherwise the untouched source slice.
        match translations.get(&block.id) {
            Some(text) => parts.push(text.clone()),
            None => parts.push(block.source_md.clone()),
        }
    }
    let document = format!("{}\n", parts.join("\n\n"));

    let dir = tempfile::tempdir().context("translated-document temp dir")?;
    let path = dir.path().join("translated.md");
    std::fs::write(&path, document).context("writing the reconstructed translated document")?;
    let reparsed = parse_document_bounded(client, &path.to_string_lossy()).await?;

    let source_kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
    let translated_kinds: Vec<&str> = reparsed.blocks.iter().map(|b| b.kind.as_str()).collect();
    ensure!(
        source_kinds == translated_kinds,
        "the translated document's block-kind sequence changed: source {source_kinds:?}, translated {translated_kinds:?}"
    );
    ensure!(
        reparsed.blocks.len() == blocks.len(),
        "the translated document has {} blocks but the source has {}",
        reparsed.blocks.len(),
        blocks.len()
    );
    Ok(())
}

/// The fake server was really reached: audit rows exist and the server logged
/// the chat requests.
async fn assert_fake_server_reached(
    pool: &SqlitePool,
    chunk_count: usize,
    fake_log: &LogSink,
) -> Result<()> {
    let calls: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM llm_call WHERE role = 'translator'")
        .fetch_one(pool)
        .await?;
    ensure!(
        calls >= chunk_count as i64,
        "expected at least {chunk_count} translator llm_call audit rows, found {calls}"
    );
    let answered: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM llm_call WHERE role = 'translator' \
         AND response_text IS NOT NULL AND response_text <> ''",
    )
    .fetch_one(pool)
    .await?;
    ensure!(
        answered >= chunk_count as i64,
        "only {answered} of {chunk_count} translator calls recorded a response"
    );

    let logged = fake_log
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .any(|line| line.contains("POST /v1/chat/completions"));
    ensure!(
        logged,
        "the fake server never logged a chat-completions request; the endpoint was not reached"
    );

    // The context budget reads `/props` and the counter uses `/tokenize` when the
    // server exposes it; a run that never logged a tokenize request silently
    // fell back to the heuristic on both sides.
    let tokenizer_used = fake_log
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .iter()
        .any(|line| line.contains("POST /tokenize"));
    ensure!(
        tokenizer_used,
        "the fake server never logged a /tokenize request; the exact token counter is not wired"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// M2: media, footnotes and tables
// ---------------------------------------------------------------------------

/// Asset hrefs (`assets/...`) referenced by Markdown image/link syntax.
fn referenced_assets(markdown: &str) -> Vec<String> {
    const MARKER: &str = "](assets/";
    let mut found = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = markdown[cursor..].find(MARKER) {
        // `offset` is the `]`; the href starts two bytes later, at the `a`.
        let href_start = cursor + offset + 2;
        let rest = &markdown[href_start..];
        let target = rest.split(')').next().unwrap_or(rest).trim();
        // A link may carry an optional `"title"`; the href is its first token.
        let href = target.split_whitespace().next().unwrap_or(target);
        found.push(href.to_string());
        cursor = href_start + 1;
    }
    found
}

/// Every asset the ingest extracted is referenced as `assets/...` in the produced
/// Markdown and exists on disk next to `document.md`.
async fn assert_ingest_assets(markdown_path: &str, assets: &[String]) -> Result<()> {
    let markdown = std::fs::read_to_string(markdown_path)
        .with_context(|| format!("reading the extracted markdown {markdown_path}"))?;
    let referenced = referenced_assets(&markdown);
    ensure!(
        !referenced.is_empty(),
        "the extracted markdown does not reference its media as `assets/...`: {markdown}"
    );
    ensure!(
        !assets.is_empty(),
        "the fixture embeds an image, but ingest reported no assets"
    );

    let base = Path::new(markdown_path)
        .parent()
        .ok_or_else(|| anyhow!("markdown path {markdown_path} has no parent directory"))?;
    for href in &referenced {
        let resolved = base.join(href);
        ensure!(
            resolved.is_file(),
            "the markdown references {href:?} but {} does not exist",
            resolved.display()
        );
    }
    // The declared list is the same set of hrefs the Markdown carries.
    for href in assets {
        ensure!(
            referenced.contains(href),
            "ingest declared asset {href:?}, but the markdown never references it"
        );
    }
    eprintln!(
        "[walking_skeleton] ingest extracted {} asset(s), all referenced and present on disk",
        assets.len()
    );
    Ok(())
}

/// Footnotes, the aligned table and the embedded image survive into the exported
/// artifacts.
async fn assert_export_media_and_structure(
    epub_path: &str,
    pdf_path: Option<&str>,
    python: &Path,
    fixture_image: &Path,
) -> Result<()> {
    // Footnotes: pandoc renders `[^n]` as a real `<aside epub:type="footnote">`
    // and keeps the (translated) note text inside it.
    ensure!(
        epub_contains(epub_path, python, "epub:type=\"footnote\"").await?,
        "the exported EPUB contains no footnote: the source footnotes were dropped"
    );
    // The table is rendered as a table, not flattened into paragraphs.
    ensure!(
        epub_contains(epub_path, python, "<table").await?,
        "the exported EPUB contains no <table>: the source table was dropped"
    );
    // The fake model wraps words but leaves these unchanged, so the substring
    // proves the footnote body and the table cells reached the output.
    for needle in ["storms", "lanterns", "Wicks", "Element"] {
        ensure!(
            epub_contains(epub_path, python, needle).await?,
            "the exported EPUB is missing {needle:?} (the footnote text or a table cell)"
        );
    }

    // The image must be embedded verbatim: the alt text alone would still pass a
    // string check, so the fixture's bytes are looked for inside the archive.
    ensure!(
        epub_embeds_file(epub_path, python, fixture_image).await?,
        "the exported EPUB does not embed the fixture image bytes ({}): the image was dropped",
        fixture_image.display()
    );

    if let Some(pdf) = pdf_path {
        match pdf_text(pdf, python).await? {
            Some(text) => {
                ensure!(
                    text.contains("storms") && text.contains("lanterns"),
                    "the exported PDF is missing the footnote text: footnotes were dropped"
                );
                ensure!(
                    text.contains("Wicks") && text.contains("Element"),
                    "the exported PDF is missing the table content: the table was dropped"
                );
            }
            None => eprintln!(
                "[walking_skeleton] note: PyMuPDF is unavailable; the PDF text assertion is skipped"
            ),
        }
    }
    Ok(())
}

/// Whether the EPUB embeds a media entry whose bytes equal `reference`'s bytes.
///
/// The fixture image must be embedded verbatim, so the alt text alone never
/// satisfies the assertion.
async fn epub_embeds_file(path: &str, python: &Path, reference: &Path) -> Result<bool> {
    const SCRIPT: &str = r#"
import sys, zipfile
with open(sys.argv[2], "rb") as handle:
    data = handle.read()
with zipfile.ZipFile(sys.argv[1]) as archive:
    found = any(archive.read(name) == data for name in archive.namelist())
sys.stdout.write("FOUND" if found else "MISSING")
"#;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(python)
            .arg("-c")
            .arg(SCRIPT)
            .arg(path)
            .arg(reference)
            .output(),
    )
    .await
    .context("reading the EPUB media timed out")??;
    ensure!(
        output.status.success(),
        "reading the EPUB media failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "FOUND")
}

/// The extracted text of a PDF, or `None` when PyMuPDF is unavailable.
async fn pdf_text(path: &str, python: &Path) -> Result<Option<String>> {
    const SCRIPT: &str = r#"
import sys
try:
    import pymupdf
except ImportError:
    try:
        import fitz as pymupdf
    except ImportError:
        sys.exit(3)
doc = pymupdf.open(sys.argv[1])
text = "\n".join(page.get_text() for page in doc)
sys.stdout.write(text.replace("-\n", "").replace("\n", " "))
"#;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(python)
            .arg("-c")
            .arg(SCRIPT)
            .arg(path)
            .output(),
    )
    .await
    .context("reading the PDF text timed out")??;
    if output.status.code() == Some(3) {
        return Ok(None);
    }
    ensure!(
        output.status.success(),
        "reading the PDF text failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(Some(String::from_utf8_lossy(&output.stdout).to_string()))
}

// ---------------------------------------------------------------------------
// Translation driving
// ---------------------------------------------------------------------------

/// Enqueue one `translate_chunk` job per chunk, in chunk order. Returns the
/// `chunk_id -> job_id` mapping.
async fn enqueue_translate_jobs(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Vec<(String, String)>> {
    let document = document_id(pool, project_id).await?;
    let chunks = repo::list_chunks(pool, &document).await?;
    let mut pairs = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        let job = NewJob::new(
            project_id,
            "translate_chunk",
            json!({ "chunk_id": chunk.id }),
        )
        .with_priority(chunk.order_index);
        let job_id = queue::enqueue(pool, &job).await?;
        pairs.push((chunk.id.clone(), job_id));
    }
    Ok(pairs)
}

/// Run the worker pool until every job finishes, bounded by [`WORK_DEADLINE`].
async fn run_pool_to_completion(
    pool: &SqlitePool,
    deps: &PipelineDeps,
    expected_jobs: usize,
    label: &str,
) -> Result<()> {
    let worker = spawn_pool(pool, deps);
    worker.start();
    let result = wait_for_all_jobs(pool, expected_jobs, WORK_DEADLINE, label).await;
    worker.cancel();
    result
}

/// Build and start a single-worker pool that dispatches through the real
/// pipeline.
fn spawn_pool(pool: &SqlitePool, deps: &PipelineDeps) -> WorkerPool {
    let dispatcher = Arc::new(TestDispatcher { deps: deps.clone() });
    let emitter: Arc<dyn EventEmitter> = Arc::new(NullEmitter);
    WorkerPool::new(pool.clone(), dispatcher, emitter, 1, 1)
}

/// Start a pool, wait until at least one chunk is `done`, then stop it abruptly
/// (the worker loops are aborted mid-flight).
async fn interrupt_after_first_chunk(
    pool: &SqlitePool,
    deps: &PipelineDeps,
    project_id: &str,
    deadline: Duration,
) -> Result<()> {
    let document = document_id(pool, project_id).await?;
    let worker = spawn_pool(pool, deps);
    worker.start();

    let start = Instant::now();
    loop {
        let (done,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM chunk WHERE document_id = ?1 AND status = 'done'")
                .bind(&document)
                .fetch_one(pool)
                .await?;
        if done >= 1 {
            break;
        }
        if start.elapsed() >= deadline {
            worker.cancel();
            bail!("no chunk completed before the interruption deadline");
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    worker.cancel();
    drop(worker);
    // Let the aborted tasks actually stop before the pool is torn down.
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok(())
}

/// Recreate, deterministically, the leftovers of a `SIGKILL`: one chunk left
/// `running` with its job still `leased`/`running` against a live lease.
async fn force_crash_remnant(
    pool: &SqlitePool,
    project_id: &str,
    job_of: &BTreeMap<String, String>,
) -> Result<(String, String)> {
    let document = document_id(pool, project_id).await?;
    let chunks = repo::list_chunks(pool, &document).await?;
    let victim = chunks
        .iter()
        .find(|chunk| chunk.status != "done")
        .ok_or_else(|| anyhow!("every chunk already finished; cannot simulate an interruption"))?;

    repo::set_chunk_status(pool, &victim.id, "running").await?;
    let job_id = job_of
        .get(&victim.id)
        .cloned()
        .ok_or_else(|| anyhow!("no job recorded for chunk {}", victim.id))?;
    sqlx::query(
        "UPDATE job SET state = 'running', lease_owner = 'crashed-worker', lease_expires_at = ?2 \
         WHERE id = ?1",
    )
    .bind(&job_id)
    .bind(db::now_plus_secs(90))
    .execute(pool)
    .await?;

    Ok((victim.id.clone(), job_id))
}

/// Poll the job table until every job is `done`, bounded by `deadline`.
async fn wait_for_all_jobs(
    pool: &SqlitePool,
    expected: usize,
    deadline: Duration,
    label: &str,
) -> Result<()> {
    let start = Instant::now();
    loop {
        let states = queue::count_by_state(pool).await?;
        let count = |name: &str| -> i64 {
            states
                .iter()
                .find(|(state, _)| state == name)
                .map(|(_, n)| *n)
                .unwrap_or(0)
        };
        let done = count("done");
        let failed = count("failed");
        let pending = count("pending");
        let leased = count("leased");
        let running = count("running");

        ensure!(failed == 0, "{label}: {failed} job(s) ended failed");
        if done as usize == expected && pending == 0 && leased == 0 && running == 0 {
            return Ok(());
        }
        if start.elapsed() >= deadline {
            bail!(
                "{label}: jobs did not finish in {deadline:?} \
                 (done={done}, pending={pending}, leased={leased}, running={running})"
            );
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Run the real export path for EPUB and (when an engine exists) PDF.
async fn export_book(
    deps: &PipelineDeps,
    project_id: &str,
    venv_python: &Path,
    pdf_engine: bool,
) -> Result<(String, Option<String>)> {
    let epub = export_one(deps, project_id, "epub").await?;
    assert_zip_magic(&epub.output_path)?;
    let has_text = epub_contains(&epub.output_path, venv_python, TRANSLATION_MARKER).await?;
    ensure!(
        has_text,
        "the exported EPUB does not contain the translated text ({} marker)",
        TRANSLATION_MARKER
    );
    // `render_chunks` writes `chunk.target_md`, which the pipeline stores as the
    // validated, placeholder-free composition of the chunk's blocks. A raw `⟦`
    // in the XHTML would mean a placeholder leaked into the artifact.
    let has_placeholder = epub_contains(&epub.output_path, venv_python, PLACEHOLDER_OPEN).await?;
    ensure!(
        !has_placeholder,
        "the exported EPUB contains a raw placeholder ({PLACEHOLDER_OPEN}); the translated \
         text never went through reinjection"
    );

    let pdf = if pdf_engine {
        // With a PDF engine installed the build must succeed: a LaTeX failure on a
        // placeholder token would mean the rendered units were not cleaned up, and
        // that is a real defect, not a skip.
        let pdf = export_one(deps, project_id, "pdf").await?;
        assert_pdf_magic(&pdf.output_path)?;
        Some(pdf.output_path)
    } else {
        eprintln!("[walking_skeleton] SKIP: PDF artifact assertion (no PDF engine on PATH)");
        None
    };

    Ok((epub.output_path, pdf))
}

/// Run one export and return its outcome.
async fn export_one(
    deps: &PipelineDeps,
    project_id: &str,
    format: &str,
) -> Result<app_lib::pipeline::export::ExportOutcome> {
    let request = ExportRequest {
        project_id: project_id.to_string(),
        output_format: format.to_string(),
        output_path: None,
        template: None,
        css: None,
    };
    let outcome = with_timeout(run_export(deps, &request), "export").await??;
    ensure!(
        Path::new(&outcome.output_path).is_file(),
        "export did not produce {}",
        outcome.output_path
    );
    Ok(outcome)
}

/// Assert the file starts with the ZIP local-file-header magic.
fn assert_zip_magic(path: &str) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    ensure!(
        bytes.starts_with(b"PK\x03\x04"),
        "{path} does not start with the ZIP magic"
    );
    Ok(())
}

/// Assert the file starts with `%PDF-`.
fn assert_pdf_magic(path: &str) -> Result<()> {
    let mut file = std::fs::File::open(path).with_context(|| format!("opening {path}"))?;
    let mut magic = [0u8; 5];
    file.read_exact(&mut magic)
        .with_context(|| format!("reading the {path} header"))?;
    ensure!(&magic == b"%PDF-", "{path} does not start with %PDF");
    Ok(())
}

/// Whether the EPUB (a ZIP) contains `needle` in any of its HTML/XML parts.
///
/// Uses the venv interpreter's stdlib `zipfile`, since the crate has no ZIP
/// reader and the parts are compressed.
async fn epub_contains(path: &str, python: &Path, needle: &str) -> Result<bool> {
    const SCRIPT: &str = r#"
import sys, zipfile
path, needle = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(path) as archive:
    text = "\n".join(
        archive.read(name).decode("utf-8", "replace")
        for name in archive.namelist()
        if name.lower().endswith((".xhtml", ".html", ".htm", ".xml"))
    )
sys.stdout.write("FOUND" if needle in text else "MISSING")
"#;
    let output = tokio::time::timeout(
        Duration::from_secs(30),
        tokio::process::Command::new(python)
            .arg("-c")
            .arg(SCRIPT)
            .arg(path)
            .arg(needle)
            .output(),
    )
    .await
    .context("reading the EPUB timed out")??;
    ensure!(
        output.status.success(),
        "reading the EPUB failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8_lossy(&output.stdout).trim() == "FOUND")
}

// ---------------------------------------------------------------------------
// Small shared helpers
// ---------------------------------------------------------------------------

/// Parse a Markdown file through the sidecar, bounded by the sidecar timeout.
async fn parse_document_bounded(
    client: &SidecarClient,
    markdown_path: &str,
) -> Result<ParseDocumentResult> {
    let parsed = tokio::time::timeout(SIDECAR_TIMEOUT, client.parse_document(markdown_path))
        .await
        .context("parse_document timed out")??;
    Ok(parsed)
}

/// `block_id -> text_md` for every block translation of `document_id`.
async fn block_translation_map(
    pool: &SqlitePool,
    document_id: &str,
) -> Result<BTreeMap<String, String>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT bt.block_id, bt.text_md FROM block_translation bt \
         JOIN chunk c ON c.id = bt.chunk_id \
         WHERE c.document_id = ?1",
    )
    .bind(document_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The ordered `(id, kind, order_index)` signature of a document's blocks, used
/// to compare two runs.
async fn block_signature(
    pool: &SqlitePool,
    document_id: &str,
) -> Result<Vec<(String, String, i64)>> {
    let blocks = repo::list_blocks(pool, document_id).await?;
    Ok(blocks
        .into_iter()
        .map(|b| (b.id, b.kind, b.order_index))
        .collect())
}

#[test]
fn referenced_assets_finds_relative_media_hrefs() {
    let markdown = "intro\n\n![The harbour at dawn](assets/harbour.png)\n\n\
                    [plate](assets/plate-2.jpg \"Plate 2\")\n";
    assert_eq!(
        referenced_assets(markdown),
        vec![
            "assets/harbour.png".to_string(),
            "assets/plate-2.jpg".to_string()
        ]
    );
    // An absolute or external link is not an extracted asset.
    assert!(referenced_assets("![x](https://example.org/x.png)").is_empty());
    assert!(referenced_assets("no media here").is_empty());
}
