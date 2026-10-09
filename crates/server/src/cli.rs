//! `llmtz translate` and `llmtz export`: the control plane without a UI, for scripting
//! (issue #17, step 6).
//!
//! Both commands build the same state the desktop app and `llmtz serve` use, then call the
//! same command functions the HTTP API dispatches to. stdout carries only the result (the
//! output path, or the project id with `--no-export`), so a pipe gets a clean value;
//! progress and warnings go to stderr.

use std::path::{Path, PathBuf};
use std::time::Duration;

use app_lib::commands::export::export_build;
use app_lib::commands::CreateProjectRequest;
use app_lib::commands::{endpoint, ingest, project, role_binding, translation};
use app_lib::db::repo;
use app_lib::error::AppError;
use app_lib::pipeline::export::{ExportOutcome, ExportRequest};
use app_lib::scheduler::queue;
use app_lib::AppState;
use clap::Args;
use serde_json::Value;

/// Poll interval while a stage runs.
const POLL: Duration = Duration::from_millis(700);

#[derive(Debug, Args)]
pub struct TranslateArgs {
    /// Document to translate (EPUB, PDF or Markdown). Relative paths are resolved here.
    #[arg(
        value_name = "FILE",
        conflicts_with = "project",
        required_unless_present = "project"
    )]
    pub file: Option<PathBuf>,
    /// Resume an existing project instead of creating one.
    #[arg(long, value_name = "ID")]
    pub project: Option<String>,
    /// Target language (ISO 639-1), e.g. `it`.
    #[arg(long, value_name = "LANG", required_unless_present = "project")]
    pub to: Option<String>,
    /// Source language; detected from the text when omitted.
    #[arg(long, value_name = "LANG")]
    pub from: Option<String>,
    /// Book name; defaults to the file stem.
    #[arg(long)]
    pub name: Option<String>,
    /// llama-server base URL, e.g. `http://127.0.0.1:8080`.
    #[arg(long)]
    pub endpoint: Option<String>,
    /// Model name the endpoint exposes, e.g. `qwen2.5-32b-instruct`.
    #[arg(long)]
    pub model: Option<String>,
    /// Output format (`pdf`, `epub`, `docx`).
    #[arg(long, default_value = "epub")]
    pub format: String,
    /// Destination file; defaults to the project output directory.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Stop after translating; print the project id instead of building a file.
    #[arg(long)]
    pub no_export: bool,
    /// Export even though some chunks have no translation.
    #[arg(long)]
    pub allow_untranslated: bool,
    /// Data directory; defaults to the desktop app's.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    /// Project id (see `llmtz translate --no-export` or the library page).
    pub project: String,
    /// Output format (`pdf`, `epub`, `docx`).
    #[arg(long, default_value = "epub")]
    pub format: String,
    /// Destination file; defaults to the project output directory.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// Export even though some chunks have no translation.
    #[arg(long)]
    pub allow_untranslated: bool,
    /// Build again even when nothing changed.
    #[arg(long)]
    pub force: bool,
    /// Leave the table of contents out.
    #[arg(long)]
    pub no_toc: bool,
    /// Data directory; defaults to the desktop app's.
    #[arg(long)]
    pub data_dir: Option<PathBuf>,
}

/// Create (or resume) a project, translate it and build the output.
pub async fn translate(args: TranslateArgs) -> anyhow::Result<()> {
    let data_dir = args
        .data_dir
        .clone()
        .unwrap_or_else(crate::default_data_dir);
    app_lib::logging::init_file_only(&data_dir);
    let state = crate::build_headless_state(data_dir).await?;
    let outcome = run_translate(&state, &args).await;
    crate::shutdown_state(&state);
    outcome
}

/// Build one output file of an existing project.
pub async fn export(args: ExportArgs) -> anyhow::Result<()> {
    let data_dir = args
        .data_dir
        .clone()
        .unwrap_or_else(crate::default_data_dir);
    app_lib::logging::init_file_only(&data_dir);
    let state = crate::build_headless_state(data_dir).await?;
    let outcome = run_export(&state, &args).await;
    crate::shutdown_state(&state);
    outcome
}

async fn run_translate(state: &AppState, args: &TranslateArgs) -> anyhow::Result<()> {
    ensure_translator(state, args.endpoint.as_deref(), args.model.as_deref()).await?;

    let project_id = match &args.project {
        Some(project_id) => {
            repo::get_project(&state.pool, project_id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
            eprintln!("[translate] riprendo il progetto {project_id}");
            project_id.clone()
        }
        None => {
            let file = args
                .file
                .as_deref()
                .ok_or_else(|| AppError::Invalid("a file or --project is required".into()))?;
            let path = std::fs::canonicalize(file)
                .map_err(|error| AppError::Invalid(format!("{}: {error}", file.display())))?;
            let target = args
                .to
                .as_deref()
                .ok_or_else(|| AppError::Invalid("--to is required".into()))?;
            create_and_ingest(state, &path, target, args).await?
        }
    };

    eprintln!("[translate] traduzione in corso…");
    translation::translation_start(
        state,
        translation::TranslationStartRequest {
            project_id: Some(project_id.clone()),
            only_retry: false,
        },
    )
    .await?;
    wait_for_idle(state, &project_id).await?;

    let chunks = repo::list_chunks_by_project(&state.pool, &project_id, None).await?;
    let failed = chunks
        .iter()
        .filter(|chunk| chunk.status == "failed")
        .count();
    if failed > 0 {
        anyhow::bail!(
            "{failed} chunk non riusciti: riprova con gli stessi argomenti o esamina `{}/logs`",
            state.data_dir.display()
        );
    }
    let translated = chunks
        .iter()
        .filter(|chunk| {
            chunk
                .target_md
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty())
        })
        .count();
    eprintln!("[translate] {translated}/{} chunk tradotti", chunks.len());

    if args.no_export {
        println!("{project_id}");
        return Ok(());
    }
    let outcome = build(
        state,
        &project_id,
        &args.format,
        args.output.as_deref(),
        args.allow_untranslated,
        false,
        true,
    )
    .await?;
    eprintln!("[translate] file generato");
    println!("{}", outcome.output_path);
    Ok(())
}

async fn run_export(state: &AppState, args: &ExportArgs) -> anyhow::Result<()> {
    repo::get_project(&state.pool, &args.project)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", args.project)))?;
    let outcome = build(
        state,
        &args.project,
        &args.format,
        args.output.as_deref(),
        args.allow_untranslated,
        args.force,
        !args.no_toc,
    )
    .await?;
    if outcome.from_cache {
        eprintln!("[export] nessuna modifica: output riutilizzato");
    } else {
        eprintln!("[export] {} unità impaginate", outcome.units);
    }
    println!("{}", outcome.output_path);
    Ok(())
}

/// Inspect the file, create the project and run the ingestion to completion.
async fn create_and_ingest(
    state: &AppState,
    path: &Path,
    target: &str,
    args: &TranslateArgs,
) -> anyhow::Result<String> {
    let path_text = path.to_string_lossy().to_string();
    // Detection pre-fills title, author and language exactly like the new-book form; a
    // failure is not fatal, the ingestion would report the real problem.
    let detected = match ingest::document_inspect(state, path_text.clone()).await {
        Ok(info) => {
            eprintln!(
                "[translate] rilevato: {}{}",
                info.format.to_uppercase(),
                info.metadata
                    .language
                    .as_deref()
                    .map(|language| format!(" · {language}"))
                    .unwrap_or_default()
            );
            Some(info)
        }
        Err(error) => {
            eprintln!("[translate] ispezione non disponibile ({error})");
            None
        }
    };
    let metadata = detected.as_ref().map(|info| info.metadata.clone());
    let source_lang = args
        .from
        .clone()
        .or_else(|| metadata.as_ref().and_then(|hints| hints.language.clone()));
    let name = args.name.clone().unwrap_or_else(|| {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().to_string())
            .unwrap_or_else(|| "Libro".to_string())
    });

    let created = project::project_create(
        state,
        CreateProjectRequest {
            name,
            source_path: path_text,
            target_lang: target.to_string(),
            source_lang,
            source_format: detected.as_ref().map(|info| info.format.clone()),
            doc_title: metadata.as_ref().and_then(|hints| hints.title.clone()),
            doc_author: metadata.as_ref().and_then(|hints| hints.author.clone()),
            settings: None,
            series_id: None,
            series_order: None,
        },
    )
    .await?;
    eprintln!("[translate] progetto {} creato", created.id);

    ingest::ingest_start(
        state,
        ingest::IngestStartRequest {
            project_id: created.id.clone(),
            source_path: None,
            pdf_backend: None,
        },
    )
    .await?;
    wait_for_idle(state, &created.id).await?;
    let jobs = queue::list_jobs(&state.pool, Some(created.id.as_str()), None, 100).await?;
    if let Some(job) = jobs
        .iter()
        .find(|job| job.kind == "ingest" && job.state == "failed")
    {
        anyhow::bail!(
            "ingestione fallita: {}",
            job.last_error.as_deref().unwrap_or("errore sconosciuto")
        );
    }
    let chunks = repo::list_chunks_by_project(&state.pool, &created.id, None).await?;
    eprintln!("[translate] ingestione completata: {} chunk", chunks.len());
    Ok(created.id)
}

/// Bind the translator role from `--endpoint`/`--model`, or check an existing binding.
async fn ensure_translator(
    state: &AppState,
    endpoint_url: Option<&str>,
    model: Option<&str>,
) -> anyhow::Result<()> {
    match (endpoint_url, model) {
        (Some(url), Some(model)) => {
            let url = url.trim_end_matches('/').to_string();
            let existing = repo::list_endpoints(&state.pool)
                .await?
                .into_iter()
                .find(|endpoint| endpoint.base_url == url);
            let endpoint_id = match existing {
                Some(endpoint) => endpoint.id,
                None => {
                    endpoint::endpoint_upsert(
                        state,
                        endpoint::EndpointUpsert {
                            id: None,
                            name: url.clone(),
                            base_url: url,
                            api_key_ref: None,
                            max_concurrency: None,
                            notes: None,
                        },
                    )
                    .await?
                    .id
                }
            };
            role_binding::role_binding_set(
                state,
                role_binding::RoleBindingSet {
                    id: None,
                    role: "translator".to_string(),
                    endpoint_id,
                    model: model.to_string(),
                    params: Value::Null,
                    priority: None,
                },
            )
            .await?;
            eprintln!("[translate] endpoint e ruolo traduttore configurati");
            Ok(())
        }
        (None, None) => {
            if repo::role_binding_for(&state.pool, "translator")
                .await?
                .is_none()
            {
                anyhow::bail!(
                    "nessun ruolo traduttore configurato: passa --endpoint e --model, \
                     oppure configuralo dalla pagina Modelli dell'app"
                );
            }
            Ok(())
        }
        _ => anyhow::bail!("--endpoint e --model vanno indicati insieme"),
    }
}

/// Wait until the project has no pending/leased/running job, printing chunk progress.
async fn wait_for_idle(state: &AppState, project_id: &str) -> anyhow::Result<()> {
    let mut last = String::new();
    loop {
        let jobs = queue::list_jobs(&state.pool, Some(project_id), None, 1000).await?;
        let active = jobs
            .iter()
            .any(|job| matches!(job.state.as_str(), "pending" | "leased" | "running"));
        let chunks = repo::list_chunks_by_project(&state.pool, project_id, None).await?;
        if !chunks.is_empty() {
            let done = chunks
                .iter()
                .filter(|chunk| {
                    chunk
                        .target_md
                        .as_deref()
                        .is_some_and(|text| !text.trim().is_empty())
                })
                .count();
            let line = format!("{done}/{} chunk", chunks.len());
            if line != last {
                eprintln!("[translate] {line}");
                last = line;
            }
        }
        if !active {
            return Ok(());
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Call `export_build` and return the outcome.
#[allow(clippy::too_many_arguments)]
async fn build(
    state: &AppState,
    project_id: &str,
    format: &str,
    output: Option<&Path>,
    allow_untranslated: bool,
    force: bool,
    toc: bool,
) -> anyhow::Result<ExportOutcome> {
    export_build(
        state,
        ExportRequest {
            project_id: project_id.to_string(),
            output_format: format.to_string(),
            output_path: output.map(|path| path.to_string_lossy().to_string()),
            template: None,
            css: None,
            toc,
            chapter_id: None,
            force,
            allow_untranslated,
        },
    )
    .await
    .map_err(anyhow::Error::from)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<crate::Cli, clap::Error> {
        crate::Cli::try_parse_from(args)
    }

    #[test]
    fn translate_requires_a_file_or_a_project_and_a_target() {
        // Neither the file nor --project: refused.
        assert!(parse(&["llmtz", "translate"]).is_err());
        // A target is required when the document comes from a file.
        assert!(parse(&["llmtz", "translate", "book.epub"]).is_err());
        // The two sources are mutually exclusive.
        assert!(parse(&[
            "llmtz",
            "translate",
            "book.epub",
            "--to",
            "it",
            "--project",
            "p1"
        ])
        .is_err());
        // Both accepted forms parse.
        assert!(parse(&["llmtz", "translate", "book.epub", "--to", "it"]).is_ok());
        assert!(parse(&["llmtz", "translate", "--project", "p1"]).is_ok());
    }

    #[test]
    fn export_defaults_to_epub_and_keeps_the_toc() {
        let cli = parse(&["llmtz", "export", "p1"]).expect("export args");
        match cli.command {
            crate::Command::Export(args) => {
                assert_eq!(args.format, "epub");
                assert!(!args.no_toc);
                assert!(!args.force);
            }
            _ => panic!("expected the export command"),
        }
    }
}
