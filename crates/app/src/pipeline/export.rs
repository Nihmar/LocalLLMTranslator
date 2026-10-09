//! Export: per-chapter Markdown units, `metadata.yaml`, Pandoc build, preview
//! and build history (PLAN.md §11.5).
//!
//! Rendering is deterministic: the units are composed from the chunk table and
//! hashed together with the metadata and the template/CSS/filters in use, so an
//! unchanged build can be skipped (and reported as such) instead of re-running
//! pandoc. A `chapter_id` scopes the build to one standalone chapter — the
//! selective rebuild — and `force` bypasses the skip.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::PipelineDeps;
use crate::db::models::{Chapter, Chunk, Document, Project};
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::pandoc::{write_metadata_yaml, BookMetadata, PandocBuild, PandocDriver};
use crate::sidecar::PandocUnit;
use crate::util::{sha256_hex, sha256_hex_str};

/// Project memory key holding the last successful build state.
pub const MEMORY_STATE_KEY: &str = "export_state";
/// Project memory key holding the recent build records.
pub const MEMORY_HISTORY_KEY: &str = "export_history";
/// How many build records the history keeps.
pub const HISTORY_LIMIT: usize = 10;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct ExportRequest {
    pub project_id: String,
    /// `pdf` | `epub` | `docx` (also `html` for previews).
    pub output_format: String,
    /// Absolute destination; when absent the project output directory is used.
    #[serde(default)]
    pub output_path: Option<String>,
    /// Absolute template override; absent means the format's default from the
    /// pandoc assets directory.
    #[serde(default)]
    pub template: Option<String>,
    /// Absolute CSS override; absent means the format's default.
    #[serde(default)]
    pub css: Option<String>,
    /// Include the table of contents (default true).
    #[serde(default = "default_true")]
    pub toc: bool,
    /// Build only this chapter into a standalone file.
    #[serde(default)]
    pub chapter_id: Option<String>,
    /// Bypass the unchanged-build skip.
    #[serde(default)]
    pub force: bool,
    /// Export even though some chunks in scope have no translation: they are rendered
    /// from the source, so the book mixes languages. Off by default, the user confirms.
    #[serde(default)]
    pub allow_untranslated: bool,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ExportOutcome {
    pub output_path: String,
    pub units: usize,
    pub log: String,
    pub duration_ms: Option<u64>,
    /// True when the build was skipped because nothing changed.
    pub from_cache: bool,
    /// Unit keys rebuilt since the previous build.
    pub changed_units: Vec<String>,
    /// Unit keys reused from the previous build.
    pub reused_units: usize,
    /// Id of the history record written for this attempt.
    pub build_id: String,
}

/// One entry of the build history (PLAN.md §11.5).
#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
pub struct ExportBuildRecord {
    pub id: String,
    pub output_path: String,
    pub output_format: String,
    #[serde(default)]
    pub chapter_id: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub css: Option<String>,
    pub toc: bool,
    pub units: usize,
    #[serde(default)]
    pub changed_units: Vec<String>,
    #[serde(default)]
    pub reused_units: usize,
    pub from_cache: bool,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    pub built_at: String,
}

/// Persisted state of the last successful build, used for the skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportState {
    build_hash: String,
    output_path: String,
    output_format: String,
    #[serde(default)]
    chapter_id: Option<String>,
    #[serde(default)]
    unit_hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, optional_fields = nullable)]
pub struct ExportPreviewRequest {
    pub project_id: String,
    #[serde(default)]
    pub chapter_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct PreviewUnit {
    pub key: String,
    pub title: String,
    pub markdown: String,
    pub chunks: usize,
    pub untranslated: usize,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ExportPreview {
    pub metadata_yaml: String,
    pub units: Vec<PreviewUnit>,
    pub total_chunks: usize,
    pub untranslated_chunks: usize,
}

/// Payload of `export://progress`, emitted before and after a build. The optional
/// fields depend on `state`; `from_cache` arrives with the `done` ack.
#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct ExportProgress {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_cache: Option<bool>,
}

/// A composed Markdown unit: the preamble or one chapter.
#[derive(Debug, Clone)]
struct ComposedUnit {
    key: String,
    title: String,
    markdown: String,
    chunks: usize,
    untranslated: usize,
}

/// Rendering options resolved for one build.
#[derive(Debug, Clone)]
struct RenderOptions {
    template: Option<String>,
    css: Option<String>,
    toc: bool,
    lua_filters: Vec<String>,
    top_level_division: Option<String>,
}

impl RenderOptions {
    /// Everything about the options that invalidates a cached build. The file
    /// *contents* are hashed, not the paths: editing the template must rebuild.
    fn signature(&self) -> String {
        let mut parts = vec![
            format!("toc={}", self.toc),
            format!(
                "top={}",
                self.top_level_division.as_deref().unwrap_or_default()
            ),
        ];
        for path in [self.template.as_deref(), self.css.as_deref()] {
            parts.push(file_signature(path));
        }
        for filter in &self.lua_filters {
            parts.push(file_signature(Some(filter)));
        }
        parts.join("|")
    }
}

fn file_signature(path: Option<&str>) -> String {
    match path {
        Some(path) => match std::fs::read(path) {
            Ok(bytes) => sha256_hex(&bytes),
            Err(_) => format!("missing:{path}"),
        },
        None => "none".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Build
// ---------------------------------------------------------------------------

/// Build the per-chapter Markdown units, a `metadata.yaml`, then invoke the
/// sidecar's `pandoc_build`.
pub async fn run_export(deps: &PipelineDeps, request: &ExportRequest) -> Result<ExportOutcome> {
    let pool = &deps.pool;
    let project = repo::get_project(pool, &request.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", request.project_id)))?;

    let document = repo::get_document_for_project(pool, &request.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound("project has no ingested document".into()))?;

    let chapters = repo::list_chapters(pool, &document.id).await?;
    let chunks = repo::list_chunks(pool, &document.id).await?;
    let units = compose_units(&chunks, &chapters, request.chapter_id.as_deref());

    // Refuse to export when nothing in scope has been translated at all:
    // silently emitting the source text produced untranslated "translations".
    let total_chunks: usize = units.iter().map(|unit| unit.chunks).sum();
    let untranslated_chunks: usize = units.iter().map(|unit| unit.untranslated).sum();
    let translated_chunks = total_chunks.saturating_sub(untranslated_chunks);
    if total_chunks == 0 || translated_chunks == 0 {
        return Err(AppError::Invalid(if request.chapter_id.is_some() {
            "nothing to export: the selected chapter has no translated chunk".into()
        } else {
            "nothing to export: no chunk has been translated yet".into()
        }));
    }
    if untranslated_chunks > 0 && !request.allow_untranslated {
        return Err(AppError::Invalid(format!(
            "{untranslated_chunks} of {total_chunks} chunks have no translation and would be \
             exported in the source language; confirm to export anyway"
        )));
    }

    let metadata = book_metadata(&project, &document);
    let options = resolve_render_options(deps, request);
    let output_dir = deps.output_dir(&request.project_id);
    let units_dir = output_dir.join("units");
    tokio::fs::create_dir_all(&units_dir).await?;
    let _ = write_metadata_yaml(&output_dir, &metadata)?;

    let output_path = request
        .output_path
        .clone()
        .unwrap_or_else(|| default_output_path(&output_dir, &project, &chapters, request));

    // ---- hashing and the unchanged-build skip -----------------------------
    let unit_hashes: BTreeMap<String, String> = units
        .iter()
        .map(|unit| (unit.key.clone(), unit_hash(unit)))
        .collect();
    let build_hash = sha256_hex_str(&format!(
        "{}\n{}\n{}\n{}\n{}",
        request.output_format,
        output_path,
        metadata.to_yaml(),
        options.signature(),
        unit_hashes.values().cloned().collect::<Vec<_>>().join(",")
    ));
    let previous = load_state(pool, &request.project_id).await?;
    let changed_units = changed_unit_keys(&units, previous.as_ref());
    let reused_units = units.len().saturating_sub(changed_units.len());

    if !request.force {
        if let Some(state) = &previous {
            if state.build_hash == build_hash && Path::new(&state.output_path).is_file() {
                let record = ExportBuildRecord {
                    id: crate::db::new_id(),
                    output_path: state.output_path.clone(),
                    output_format: request.output_format.clone(),
                    chapter_id: request.chapter_id.clone(),
                    template: options.template.clone(),
                    css: options.css.clone(),
                    toc: options.toc,
                    units: units.len(),
                    changed_units: Vec::new(),
                    reused_units: units.len(),
                    from_cache: true,
                    duration_ms: None,
                    built_at: crate::db::now(),
                };
                append_history(pool, &request.project_id, &record).await?;
                return Ok(ExportOutcome {
                    output_path: state.output_path.clone(),
                    units: units.len(),
                    log: format!(
                        "[export] unchanged build skipped: {} unit(s) reused\n",
                        units.len()
                    ),
                    duration_ms: None,
                    from_cache: true,
                    changed_units: Vec::new(),
                    reused_units: units.len(),
                    build_id: record.id,
                });
            }
        }
    }

    // ---- write the units and run pandoc -----------------------------------
    let mut pandoc_units: Vec<PandocUnit> = Vec::with_capacity(units.len());
    for (index, unit) in units.iter().enumerate() {
        let path = units_dir.join(format!("unit-{index:04}.md"));
        tokio::fs::write(&path, &unit.markdown).await?;
        pandoc_units.push(PandocUnit {
            path: path.to_string_lossy().to_string(),
            title: unit.title.clone(),
        });
    }

    // Pandoc resolves the units' relative image hrefs (e.g. `assets/harbour.png`)
    // against the directory that holds `document.md` and the extracted assets dir.
    let resource_path = resource_paths(&document.markdown_path);

    let driver = PandocDriver::new(deps.sidecar.clone());
    let result = driver
        .build(PandocBuild {
            units: pandoc_units,
            metadata: metadata.clone(),
            output_path: output_path.clone(),
            output_format: request.output_format.clone(),
            template: options.template.clone(),
            css: options.css.clone(),
            resource_path,
            toc: options.toc,
            lua_filters: options.lua_filters.clone(),
            top_level_division: options.top_level_division.clone(),
        })
        .await?;

    let final_output = if result.output_path.is_empty() {
        output_path
    } else {
        result.output_path
    };

    // Report, in the build log, how much of the book is not translated. Counting
    // chunks (not blocks) keeps this in step with what `render_chunks` emitted.
    let mut log = result.log;
    log.push('\n');
    log.push_str(&format!(
        "[export] translated chunks: {translated_chunks}/{total_chunks}; chunks rendered from source: {untranslated_chunks}\n"
    ));

    save_state(
        pool,
        &request.project_id,
        &ExportState {
            build_hash,
            output_path: final_output.clone(),
            output_format: request.output_format.clone(),
            chapter_id: request.chapter_id.clone(),
            unit_hashes,
        },
    )
    .await?;

    let record = ExportBuildRecord {
        id: crate::db::new_id(),
        output_path: final_output.clone(),
        output_format: request.output_format.clone(),
        chapter_id: request.chapter_id.clone(),
        template: options.template.clone(),
        css: options.css.clone(),
        toc: options.toc,
        units: units.len(),
        changed_units: changed_units.clone(),
        reused_units,
        from_cache: false,
        duration_ms: result.duration_ms,
        built_at: crate::db::now(),
    };
    append_history(pool, &request.project_id, &record).await?;

    Ok(ExportOutcome {
        output_path: final_output,
        units: units.len(),
        log,
        duration_ms: result.duration_ms,
        from_cache: false,
        changed_units,
        reused_units,
        build_id: record.id,
    })
}

/// Compose the same units the build would write, without invoking pandoc.
pub async fn run_export_preview(
    deps: &PipelineDeps,
    request: &ExportPreviewRequest,
) -> Result<ExportPreview> {
    let pool = &deps.pool;
    let project = repo::get_project(pool, &request.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", request.project_id)))?;
    let document = repo::get_document_for_project(pool, &request.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound("project has no ingested document".into()))?;
    let chapters = repo::list_chapters(pool, &document.id).await?;
    let chunks = repo::list_chunks(pool, &document.id).await?;
    let units = compose_units(&chunks, &chapters, request.chapter_id.as_deref());
    if units.is_empty() {
        return Err(AppError::Invalid(
            "nothing to preview: no chunk has been translated yet".into(),
        ));
    }

    let total_chunks: usize = units.iter().map(|unit| unit.chunks).sum();
    let untranslated_chunks: usize = units.iter().map(|unit| unit.untranslated).sum();
    Ok(ExportPreview {
        metadata_yaml: book_metadata(&project, &document).to_yaml(),
        units: units
            .into_iter()
            .map(|unit| {
                let markdown = preview_markdown(&unit);
                PreviewUnit {
                    key: unit.key,
                    title: unit.title,
                    markdown,
                    chunks: unit.chunks,
                    untranslated: unit.untranslated,
                }
            })
            .collect(),
        total_chunks,
        untranslated_chunks,
    })
}

/// The markdown a unit contributes to the built document: pandoc receives `# <title>`
/// followed by the body (the sidecar prepends the same heading when it combines the
/// units), so the preview shows exactly that instead of a body with no heading.
fn preview_markdown(unit: &ComposedUnit) -> String {
    if unit.title.trim().is_empty() {
        return unit.markdown.clone();
    }
    if unit.markdown.trim().is_empty() {
        return format!("# {}\n", unit.title);
    }
    format!("# {}\n\n{}", unit.title, unit.markdown)
}

/// Recent build records, newest first.
pub async fn export_history(
    pool: &sqlx::SqlitePool,
    project_id: &str,
) -> Result<Vec<ExportBuildRecord>> {
    repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;
    Ok(load_history(pool, project_id).await)
}

// ---------------------------------------------------------------------------
// Composition and defaults
// ---------------------------------------------------------------------------

/// Per-chapter units, in document order; a `chapter_filter` keeps only that
/// chapter (a preamble never belongs to a standalone chapter build).
fn compose_units(
    chunks: &[Chunk],
    chapters: &[Chapter],
    chapter_filter: Option<&str>,
) -> Vec<ComposedUnit> {
    let mut units = Vec::new();

    if chapter_filter.is_none() {
        let preamble: Vec<&Chunk> = chunks.iter().filter(|c| c.chapter_id.is_none()).collect();
        if !preamble.is_empty() {
            units.push(composed("preamble", "Front matter", &preamble));
        }
    }

    for chapter in chapters {
        if chapter_filter.is_some_and(|id| id != chapter.id) {
            continue;
        }
        let chapter_chunks: Vec<&Chunk> = chunks
            .iter()
            .filter(|c| c.chapter_id.as_deref() == Some(chapter.id.as_str()))
            .collect();
        if chapter_chunks.is_empty() {
            continue;
        }
        units.push(composed(&chapter.id, &chapter.title, &chapter_chunks));
    }
    units
}

fn composed(key: &str, title: &str, chunks: &[&Chunk]) -> ComposedUnit {
    let (title, markdown) = split_title(title, render_chunks(chunks));
    ComposedUnit {
        key: key.to_string(),
        title,
        markdown,
        chunks: chunks.len(),
        untranslated: chunks
            .iter()
            .filter(|chunk| !chunk_has_translation(chunk))
            .count(),
    }
}

/// Split a composed unit into its title and body.
///
/// A chapter unit opens with the chapter's heading block, whose translation is the title the
/// reader should see. Using it as the unit title is what keeps the exported heading single
/// (pandoc adds `# <title>` when it combines the units) *and* translated: the `chapter.title`
/// column still holds the source-language heading. When the unit does not open with a heading,
/// the fallback title is used and the body is left untouched.
fn split_title(fallback_title: &str, markdown: String) -> (String, String) {
    let Some((first, rest)) = markdown.split_once('\n') else {
        return (fallback_title.to_string(), markdown);
    };
    let candidate = first.trim();
    let after_hashes = candidate.trim_start_matches('#');
    let hash_count = candidate.len() - after_hashes.len();
    // CommonMark ATX heading: one to six `#`, then a space or end of line. `#hashtag`
    // is a paragraph, not a heading.
    if !(1..=6).contains(&hash_count) || !(after_hashes.is_empty() || after_hashes.starts_with(' '))
    {
        return (fallback_title.to_string(), markdown);
    }
    let title = after_hashes.trim();
    if title.is_empty() {
        return (fallback_title.to_string(), markdown);
    }
    (title.to_string(), rest.trim_start().to_string())
}

/// Book metadata: the project row wins over the source document's front matter.
fn book_metadata(project: &Project, document: &Document) -> BookMetadata {
    let mut metadata = BookMetadata::new(
        project
            .doc_title
            .clone()
            .unwrap_or_else(|| project.name.clone()),
    );
    metadata.author = project.doc_author.clone();
    metadata.language = Some(project.target_lang.clone());
    match serde_json::from_str::<serde_json::Value>(&document.front_matter_json) {
        Ok(serde_json::Value::Object(front_matter)) => metadata.merge_front_matter(&front_matter),
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "ignoring unparsable document front matter"),
    }
    metadata
}

/// The template, CSS, filters and division a build uses when the request does
/// not override them.
fn resolve_render_options(deps: &PipelineDeps, request: &ExportRequest) -> RenderOptions {
    let format = request.output_format.trim().to_lowercase();
    let assets = deps.pandoc_dir.as_deref();
    RenderOptions {
        template: request
            .template
            .clone()
            .or_else(|| default_template(assets, &format)),
        css: request.css.clone().or_else(|| default_css(assets, &format)),
        toc: request.toc,
        lua_filters: default_filters(assets, &format),
        // A book's top-level heading is a chapter in LaTeX/PDF.
        top_level_division: matches!(format.as_str(), "pdf" | "latex" | "tex")
            .then(|| "chapter".to_string()),
    }
}

fn default_template(assets: Option<&Path>, format: &str) -> Option<String> {
    let name = match format {
        "pdf" | "latex" | "tex" => "book.tex",
        "html" | "html5" | "epub" => "book.html",
        _ => return None,
    };
    crate::pandoc::assets_file(assets, "templates", name)
}

fn default_css(assets: Option<&Path>, format: &str) -> Option<String> {
    match format {
        "html" | "html5" | "epub" => crate::pandoc::assets_file(assets, "styles", "book.css"),
        _ => None,
    }
}

fn default_filters(assets: Option<&Path>, format: &str) -> Vec<String> {
    let mut names = vec!["footnotes.lua", "tables.lua"];
    if matches!(format, "html" | "html5" | "epub") {
        names.push("epub_cleanup.lua");
    }
    names
        .into_iter()
        .filter_map(|name| crate::pandoc::assets_file(assets, "filters", name))
        .collect()
}

/// Default output filename: `<sanitized project name>.<format>`, or
/// `<project>-<chapter>.<format>` for a standalone chapter build.
fn default_output_path(
    output_dir: &Path,
    project: &Project,
    chapters: &[Chapter],
    request: &ExportRequest,
) -> String {
    let stem = match request
        .chapter_id
        .as_deref()
        .and_then(|id| chapters.iter().find(|chapter| chapter.id == id))
    {
        Some(chapter) if !chapter.title.is_empty() => {
            format!("{}-{}", project.name, chapter.title)
        }
        Some(chapter) => format!("{}-{}", project.name, chapter.id),
        None => project.name.clone(),
    };
    output_dir
        .join(export_filename(&stem, &request.output_format))
        .to_string_lossy()
        .to_string()
}

/// Stable content hash of a unit.
fn unit_hash(unit: &ComposedUnit) -> String {
    sha256_hex_str(&format!("{}\n{}\n{}", unit.key, unit.title, unit.markdown))
}

/// Unit keys whose content differs from the previous build.
fn changed_unit_keys(units: &[ComposedUnit], previous: Option<&ExportState>) -> Vec<String> {
    units
        .iter()
        .filter(|unit| {
            previous
                .and_then(|state| state.unit_hashes.get(&unit.key))
                .is_none_or(|hash| hash != &unit_hash(unit))
        })
        .map(|unit| unit.key.clone())
        .collect()
}

async fn load_state(pool: &sqlx::SqlitePool, project_id: &str) -> Result<Option<ExportState>> {
    Ok(repo::get_memory(pool, project_id, MEMORY_STATE_KEY)
        .await?
        .and_then(|json| serde_json::from_str(&json).ok()))
}

async fn save_state(pool: &sqlx::SqlitePool, project_id: &str, state: &ExportState) -> Result<()> {
    repo::set_memory(
        pool,
        project_id,
        MEMORY_STATE_KEY,
        &serde_json::to_string(state)?,
    )
    .await
}

async fn load_history(pool: &sqlx::SqlitePool, project_id: &str) -> Vec<ExportBuildRecord> {
    repo::get_memory(pool, project_id, MEMORY_HISTORY_KEY)
        .await
        .ok()
        .flatten()
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default()
}

async fn append_history(
    pool: &sqlx::SqlitePool,
    project_id: &str,
    record: &ExportBuildRecord,
) -> Result<()> {
    let mut history = load_history(pool, project_id).await;
    history.insert(0, record.clone());
    history.truncate(HISTORY_LIMIT);
    repo::set_memory(
        pool,
        project_id,
        MEMORY_HISTORY_KEY,
        &serde_json::to_string(&history)?,
    )
    .await
}

// ---------------------------------------------------------------------------
// Rendering helpers
// ---------------------------------------------------------------------------

/// The translated text to render for a chunk, or `None` when it has no usable
/// translation.
///
/// Single predicate for "has a translation": a missing or whitespace-only
/// `target_md` counts as untranslated. Both the progress count and the renderer
/// go through it, so the reported numbers cannot drift from the emitted output.
fn translated_text(chunk: &Chunk) -> Option<&str> {
    chunk
        .target_md
        .as_deref()
        .filter(|text| !text.trim().is_empty())
}

/// Whether a chunk carries translated output.
pub(crate) fn chunk_has_translation(chunk: &Chunk) -> bool {
    translated_text(chunk).is_some()
}

/// Default output filename: `<sanitized name>.<format>`.
fn export_filename(name: &str, output_format: &str) -> String {
    format!("{}.{}", sanitize(name), output_format)
}

/// Directories pandoc searches to resolve the units' relative targets.
///
/// The directory holding the extracted `document.md` comes first, because a
/// media href such as `assets/harbour.png` is relative to it; the extracted
/// `assets/` sibling is added only when the sidecar actually wrote media there.
fn resource_paths(markdown_path: &str) -> Vec<String> {
    let markdown_dir = Path::new(markdown_path)
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut paths = vec![markdown_dir.to_string_lossy().to_string()];
    let assets_dir = markdown_dir.join("assets");
    if assets_dir.is_dir() {
        paths.push(assets_dir.to_string_lossy().to_string());
    }
    paths
}

/// Render chunk outputs (translated when available) into one Markdown unit.
///
/// Uses [`translated_text`] — the same predicate as the progress count — so a
/// whitespace-only target falls back to the source exactly as it is counted.
fn render_chunks(chunks: &[&crate::db::models::Chunk]) -> String {
    let mut out = String::new();
    for chunk in chunks {
        match translated_text(chunk) {
            Some(text) => out.push_str(text),
            None => out.push_str(&chunk.source_md),
        }
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::Chunk;

    fn chunk(id: &str, target: Option<&str>, source: &str) -> Chunk {
        Chunk {
            id: id.into(),
            document_id: "d".into(),
            chapter_id: None,
            order_index: 0,
            block_ids_json: "[]".into(),
            source_md: source.into(),
            token_estimate: 0,
            context_json: "{}".into(),
            flags_json: "[]".into(),
            status: "done".into(),
            prompt_hash: None,
            model_id: None,
            params_json: None,
            context_manifest_json: None,
            target_md: target.map(str::to_string),
            error: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn chapter(id: &str, order: i64, title: &str) -> Chapter {
        Chapter {
            id: id.into(),
            document_id: "d".into(),
            order_index: order,
            title: title.into(),
            level: 1,
            block_first: 0,
            block_last: 0,
            summary: None,
            summary_model: None,
            summary_hash: None,
            status: "pending".into(),
        }
    }

    #[test]
    fn unit_prefers_translated_text() {
        let a = chunk("c1", Some("# Ciao\n"), "# Hello\n");
        let b = chunk("c2", None, "# Raw\n");
        let rendered = render_chunks(&[&a, &b]);
        assert!(rendered.contains("# Ciao"));
        assert!(rendered.contains("# Raw"));
        assert!(!rendered.contains("# Hello"));
    }

    #[test]
    fn sanitize_replaces_unsafe_characters() {
        assert_eq!(sanitize("My Book: v2"), "My_Book__v2");
    }

    #[test]
    fn export_filename_has_no_stray_space_before_extension() {
        assert_eq!(export_filename("Book", "epub"), "Book.epub");
        assert_eq!(export_filename("My Book", "pdf"), "My_Book.pdf");
    }

    #[test]
    fn compose_units_splits_per_chapter_and_honours_the_filter() {
        let mut first = chunk("c1", Some("Uno"), "One");
        first.chapter_id = Some("ch1".into());
        let mut second = chunk("c2", None, "Two");
        second.chapter_id = Some("ch1".into());
        let mut third = chunk("c3", Some("Tre"), "Three");
        third.chapter_id = Some("ch2".into());
        let preamble = chunk("c0", Some("Intro"), "Front");
        let chunks = [preamble, first, second, third];
        let chapters = [chapter("ch1", 1, "One"), chapter("ch2", 2, "Two")];

        let all = compose_units(&chunks, &chapters, None);
        assert_eq!(all.len(), 3, "preamble + two chapters");
        assert_eq!(all[0].key, "preamble");
        assert_eq!(all[0].chunks, 1);
        assert_eq!(all[1].key, "ch1");
        assert_eq!(all[1].chunks, 2);
        assert_eq!(all[1].untranslated, 1);
        assert_eq!(all[2].key, "ch2");

        let only = compose_units(&chunks, &chapters, Some("ch2"));
        assert_eq!(only.len(), 1);
        assert_eq!(only[0].key, "ch2");
        assert_eq!(only[0].title, "Two");
        assert!(only[0].markdown.contains("Tre"));
    }

    #[test]
    fn compose_units_takes_the_translated_heading_as_the_title_and_removes_it() {
        let mut chapter_one = chunk(
            "c1",
            Some("## Chapter One: The Harbour\n\nIl porto era quieto.\n"),
            "## Chapter One: The Harbour\n\nThe harbour was quiet.\n",
        );
        chapter_one.chapter_id = Some("ch1".into());
        let chunks = [chapter_one];
        let chapters = [chapter("ch1", 1, "Chapter One: The Harbour")];

        let units = compose_units(&chunks, &chapters, None);
        assert_eq!(units.len(), 1);
        // The title is the *translated* heading, and the body no longer repeats it:
        // the sidecar prepends `# <title>` when it combines the units, so keeping
        // the heading in both places produced two headings and two TOC entries.
        assert_eq!(units[0].title, "Chapter One: The Harbour");
        assert!(!units[0].markdown.contains("Chapter One"));
        assert!(units[0].markdown.contains("Il porto era quieto."));
    }

    #[test]
    fn compose_units_keeps_the_source_title_when_the_unit_has_no_heading() {
        let mut plain = chunk(
            "c1",
            Some("Il porto era quieto.\n"),
            "The harbour was quiet.\n",
        );
        plain.chapter_id = Some("ch1".into());
        let chunks = [plain];
        let chapters = [chapter("ch1", 1, "Chapter One")];

        let units = compose_units(&chunks, &chapters, None);
        assert_eq!(units[0].title, "Chapter One");
        assert_eq!(units[0].markdown.trim(), "Il porto era quieto.");
    }

    #[test]
    fn a_hashtag_is_not_a_heading_for_the_title_split() {
        let (title, body) = split_title("Fallback", "#hashtag sentence\n\nBody".to_string());
        assert_eq!(title, "Fallback");
        assert!(body.starts_with("#hashtag"));
    }

    #[test]
    fn preview_markdown_rebuilds_the_heading_pandoc_will_add() {
        let unit = ComposedUnit {
            key: "ch1".into(),
            title: "Il porto".into(),
            markdown: "Il porto era quieto.".into(),
            chunks: 1,
            untranslated: 0,
        };
        assert_eq!(
            preview_markdown(&unit),
            "# Il porto\n\nIl porto era quieto."
        );

        let untitled = ComposedUnit {
            title: String::new(),
            ..unit
        };
        assert_eq!(preview_markdown(&untitled), "Il porto era quieto.");
    }

    #[test]
    fn changed_unit_keys_flags_new_and_edited_units() {
        let unit = |markdown: &str| ComposedUnit {
            key: "ch1".into(),
            title: "One".into(),
            markdown: markdown.into(),
            chunks: 1,
            untranslated: 0,
        };
        let units = [
            unit("Uno"),
            ComposedUnit {
                key: "ch2".into(),
                ..unit("Due")
            },
        ];

        // No previous state: everything changed.
        assert_eq!(changed_unit_keys(&units, None), vec!["ch1", "ch2"]);

        // Previous hashes for the same content: nothing changed.
        let previous = ExportState {
            build_hash: "h".into(),
            output_path: "/out.epub".into(),
            output_format: "epub".into(),
            chapter_id: None,
            unit_hashes: units
                .iter()
                .map(|unit| (unit.key.clone(), unit_hash(unit)))
                .collect(),
        };
        assert!(changed_unit_keys(&units, Some(&previous)).is_empty());

        // Editing one unit and adding another flags exactly those.
        let edited = [
            unit("Uno modificato"),
            ComposedUnit {
                key: "ch3".into(),
                ..unit("Tre")
            },
        ];
        assert_eq!(
            changed_unit_keys(&edited, Some(&previous)),
            vec!["ch1", "ch3"]
        );
    }

    #[test]
    fn render_options_signature_tracks_file_contents() {
        let dir = tempfile::tempdir().expect("temp dir");
        let template = dir.path().join("book.html");
        std::fs::write(&template, "A").expect("write");
        let template_str = template.to_string_lossy().to_string();

        let options = RenderOptions {
            template: Some(template_str.clone()),
            css: None,
            toc: true,
            lua_filters: Vec::new(),
            top_level_division: None,
        };
        let first = options.signature();
        std::fs::write(&template, "B").expect("write");
        let second = options.signature();
        assert_ne!(
            first, second,
            "editing the template must invalidate the build"
        );
    }

    #[test]
    fn resolve_render_options_picks_the_defaults_for_a_format() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::create_dir_all(dir.path().join("templates")).expect("templates");
        std::fs::create_dir_all(dir.path().join("filters")).expect("filters");
        std::fs::create_dir_all(dir.path().join("styles")).expect("styles");
        std::fs::write(dir.path().join("templates/book.tex"), "tex").expect("template");
        std::fs::write(dir.path().join("filters/footnotes.lua"), "-- f").expect("filter");
        std::fs::write(dir.path().join("filters/epub_cleanup.lua"), "-- e").expect("filter");

        let request = ExportRequest {
            project_id: "p".into(),
            output_format: "pdf".into(),
            output_path: None,
            template: None,
            css: None,
            toc: true,
            chapter_id: None,
            force: false,
            allow_untranslated: false,
        };
        // A helper that only reads `deps.pandoc_dir` would need the whole struct,
        // so the pure pieces are exercised through a bare call.
        let options = RenderOptions {
            template: default_template(Some(dir.path()), "pdf"),
            css: default_css(Some(dir.path()), "pdf"),
            toc: request.toc,
            lua_filters: default_filters(Some(dir.path()), "pdf"),
            top_level_division: Some("chapter".into()),
        };
        assert!(options
            .template
            .is_some_and(|path| path.ends_with("book.tex")));
        assert!(options.css.is_none(), "PDF has no CSS");
        assert_eq!(options.lua_filters.len(), 1, "epub_cleanup is EPUB-only");
        assert!(options.lua_filters[0].ends_with("footnotes.lua"));

        let epub_filters = default_filters(Some(dir.path()), "epub");
        assert_eq!(epub_filters.len(), 2);
    }

    #[test]
    fn resource_paths_includes_the_assets_dir_only_when_it_exists() {
        let dir = tempfile::tempdir().expect("temp dir");
        let work = dir.path().join("work");
        std::fs::create_dir_all(&work).expect("work dir");
        let markdown = work.join("document.md");
        std::fs::write(&markdown, "![x](assets/harbour.png)\n").expect("markdown");
        let markdown_path = markdown.to_string_lossy().to_string();

        // Without extracted media, only the directory holding the markdown is searched.
        assert_eq!(
            resource_paths(&markdown_path),
            vec![work.to_string_lossy().to_string()]
        );

        // With `<work>/assets` present, it is appended so `assets/...` hrefs resolve.
        std::fs::create_dir_all(work.join("assets")).expect("assets dir");
        assert_eq!(
            resource_paths(&markdown_path),
            vec![
                work.to_string_lossy().to_string(),
                work.join("assets").to_string_lossy().to_string(),
            ]
        );
    }

    #[test]
    fn chunk_has_translation_rejects_missing_and_blank_targets() {
        assert!(chunk_has_translation(&chunk("c1", Some("Ciao"), "Hello")));
        assert!(!chunk_has_translation(&chunk("c2", None, "Hello")));
        assert!(!chunk_has_translation(&chunk("c3", Some("   "), "Hello")));
    }

    #[test]
    fn render_and_count_agree_on_blank_targets() {
        // A whitespace-only target is untranslated for both the predicate and the
        // renderer, so the reported count matches what is actually written.
        let blank = chunk("c1", Some("  \n"), "# Source\n");
        assert!(!chunk_has_translation(&blank));
        assert_eq!(render_chunks(&[&blank]).trim(), "# Source");

        let translated = chunk("c2", Some("# Ciao\n"), "# Hello\n");
        assert!(chunk_has_translation(&translated));
        assert_eq!(render_chunks(&[&translated]).trim(), "# Ciao");
    }

    #[test]
    fn default_output_path_names_the_chapter_for_a_scoped_build() {
        let project = Project {
            id: "p".into(),
            name: "My Book".into(),
            source_path: "s".into(),
            source_hash: "h".into(),
            source_format: "epub".into(),
            source_lang: None,
            target_lang: "it".into(),
            doc_title: None,
            doc_author: None,
            series_id: None,
            series_order: None,
            prompts_snapshot_dir: None,
            settings_json: "{}".into(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        let chapters = [chapter("ch1", 1, "The Siege")];
        let scoped = ExportRequest {
            project_id: "p".into(),
            output_format: "pdf".into(),
            output_path: None,
            template: None,
            css: None,
            toc: true,
            chapter_id: Some("ch1".into()),
            force: false,
            allow_untranslated: false,
        };
        let path = default_output_path(Path::new("/out"), &project, &chapters, &scoped);
        assert!(path.ends_with("My_Book-The_Siege.pdf"), "got {path}");

        let whole = ExportRequest {
            chapter_id: None,
            ..scoped
        };
        let path = default_output_path(Path::new("/out"), &project, &chapters, &whole);
        assert!(path.ends_with("My_Book.pdf"), "got {path}");
    }
}
