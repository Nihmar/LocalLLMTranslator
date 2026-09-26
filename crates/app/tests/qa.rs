//! QA scan integration test against the **real** Python sidecar.
//!
//! The heuristics live in the sidecar, so this test starts it through the real
//! supervisor (gate: `sidecar/.venv/bin/python` and `/bin/sh` exist, otherwise it
//! skips cleanly) and drives a controlled chunk through the three states the
//! report must flag: a glossary mismatch, an untranslated chunk and an empty one.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{ensure, Context, Result};
use app_lib::db::models::GlossaryTerm;
use app_lib::db::repo;
use app_lib::events::{sidecar_event_sink, EventEmitter, NullEmitter};
use app_lib::pipeline::{qa, PipelineDeps};
use app_lib::resources::ResourceGovernor;
use app_lib::sidecar::{SidecarClient, SpawnSpec, Supervisor};

use common::{add_chunk_with_blocks, seed_project, set_block_translation};

const SIDECAR_TIMEOUT: Duration = Duration::from_secs(120);
const LONG_SOURCE: &str =
    "The harbour was quiet that morning and the keeper waited for the light to turn.";

struct SupervisorGuard {
    supervisor: Arc<Supervisor>,
}

impl Drop for SupervisorGuard {
    fn drop(&mut self) {
        self.supervisor.shutdown();
    }
}

fn repo_root() -> Result<PathBuf> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("../..")
        .canonicalize()
        .with_context(|| format!("resolving the repository root from {}", manifest.display()))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// Start the sidecar through the real supervisor (the package must be
/// importable: `python -m llmtranslator_sidecar`).
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn qa_scan_flags_a_controlled_chunk() -> Result<()> {
    let root = repo_root()?;
    let venv_python = root.join("sidecar/.venv/bin/python");
    if !venv_python.is_file() {
        eprintln!(
            "[qa] SKIP: {} is missing; run `uv sync --all-groups` inside sidecar/.",
            venv_python.display()
        );
        return Ok(());
    }
    if !Path::new("/bin/sh").exists() {
        eprintln!("[qa] SKIP: /bin/sh is missing; cannot launch the sidecar.");
        return Ok(());
    }

    let supervisor = start_sidecar(&root.join("sidecar"), &venv_python)?;
    let _guard = SupervisorGuard {
        supervisor: supervisor.clone(),
    };
    let client = SidecarClient::new(supervisor.clone());
    let ping = tokio::time::timeout(SIDECAR_TIMEOUT, client.ping()).await??;
    ensure!(ping.pong, "sidecar ping did not return pong");

    let dir = tempfile::tempdir()?;
    let pool = app_lib::db::connect(&dir.path().join("app.sqlite")).await?;
    let deps = PipelineDeps::new(
        pool.clone(),
        client,
        ResourceGovernor::default(),
        dir.path().to_path_buf(),
    );
    let seeded = seed_project(&pool, "qa", "http://127.0.0.1:1", false).await?;
    let chunk_id = add_chunk_with_blocks(
        &pool,
        &seeded.document_id,
        &seeded.chapter_id,
        1,
        &["b000001".to_string()],
        Some("Una traduzione italiana che non contiene il termine atteso."),
    )
    .await?;
    sqlx::query("UPDATE chunk SET source_md = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(&chunk_id)
        .bind("The harbour was quiet that morning.")
        .bind(app_lib::db::now())
        .execute(&pool)
        .await?;
    set_block_translation(
        &pool,
        "b000001",
        &chunk_id,
        "translator",
        "Una traduzione italiana che non contiene il termine atteso.",
    )
    .await?;
    repo::upsert_glossary_term(
        &pool,
        &GlossaryTerm {
            id: app_lib::db::new_id(),
            project_id: seeded.project_id.clone(),
            source_lang: Some("English".to_string()),
            target_lang: Some("Italian".to_string()),
            source: "harbour".to_string(),
            target: "baia".to_string(),
            note: None,
            kind: "term".to_string(),
            origin: "manual".to_string(),
            revision: 1,
            status: "approved".to_string(),
        },
    )
    .await?;

    // ---- a present translation with a glossary mismatch ---------------------
    qa::scan_chunk(&deps, &seeded.project_id, &chunk_id).await?;
    let findings = repo::list_qa_findings(&pool, &seeded.project_id).await?;
    ensure!(
        findings
            .iter()
            .any(|finding| finding.kind == "glossary_mismatch"),
        "expected a glossary_mismatch finding, got {:?}",
        findings.iter().map(|f| &f.kind).collect::<Vec<_>>()
    );
    ensure!(
        findings
            .iter()
            .find(|finding| finding.kind == "glossary_mismatch")
            .is_some_and(|finding| finding.severity == "major"),
        "a glossary mismatch is major"
    );

    // ---- an untranslated chunk ---------------------------------------------
    sqlx::query("UPDATE chunk SET source_md = ?2, target_md = ?2, updated_at = ?3 WHERE id = ?1")
        .bind(&chunk_id)
        .bind(LONG_SOURCE)
        .bind(app_lib::db::now())
        .execute(&pool)
        .await?;
    qa::scan_chunk(&deps, &seeded.project_id, &chunk_id).await?;
    let findings = repo::list_qa_findings(&pool, &seeded.project_id).await?;
    ensure!(
        findings
            .iter()
            .any(|finding| finding.kind == "untranslated"),
        "expected an untranslated finding, got {:?}",
        findings.iter().map(|f| &f.kind).collect::<Vec<_>>()
    );

    // ---- an empty translation ----------------------------------------------
    // The rescan path treats an empty `target_md` as "not translated yet" and
    // clears the findings, so the empty case goes through the inline path, the
    // one that sees the raw model answer.
    let chunk = repo::get_chunk(&pool, &chunk_id).await?.expect("chunk");
    qa::scan_translated_chunk(&deps, &seeded.project_id, &chunk, "").await?;
    let findings = repo::list_qa_findings_filtered(
        &pool,
        &seeded.project_id,
        Some("empty"),
        Some("critical"),
        None,
    )
    .await?;
    ensure!(
        !findings.is_empty(),
        "expected a critical empty finding for an empty translation"
    );

    // The report is filterable and a re-scan replaces the old findings.
    let all = repo::list_qa_findings(&pool, &seeded.project_id).await?;
    ensure!(
        all.len() == 1,
        "a re-scan must replace the previous findings, found {}",
        all.len()
    );

    // A rescanned chunk with no translation at all clears its findings.
    sqlx::query("UPDATE chunk SET target_md = NULL, updated_at = ?2 WHERE id = ?1")
        .bind(&chunk_id)
        .bind(app_lib::db::now())
        .execute(&pool)
        .await?;
    qa::scan_chunk(&deps, &seeded.project_id, &chunk_id).await?;
    let all = repo::list_qa_findings(&pool, &seeded.project_id).await?;
    ensure!(all.is_empty(), "an untranslated chunk has nothing to scan");
    Ok(())
}
