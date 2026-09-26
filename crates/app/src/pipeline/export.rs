//! Export: per-chapter Markdown units, `metadata.yaml`, Pandoc build.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::PipelineDeps;
use crate::db::models::Chunk;
use crate::db::repo;
use crate::error::{AppError, Result};
use crate::pandoc::{write_metadata_yaml, BookMetadata, PandocDriver};
use crate::sidecar::PandocUnit;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRequest {
    pub project_id: String,
    /// `pdf` | `epub` | `docx`.
    pub output_format: String,
    #[serde(default)]
    pub output_path: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub css: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportOutcome {
    pub output_path: String,
    pub units: usize,
    pub log: String,
    pub duration_ms: Option<u64>,
}

/// Build per-chapter Markdown units, a `metadata.yaml`, then invoke the sidecar's
/// `pandoc_build`.
pub async fn run_export(deps: &PipelineDeps, request: &ExportRequest) -> Result<ExportOutcome> {
    let pool = &deps.pool;
    let project = repo::get_project(pool, &request.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {}", request.project_id)))?;

    let document = repo::get_document_for_project(pool, &request.project_id)
        .await?
        .ok_or_else(|| AppError::NotFound("project has no ingested document".into()))?;
    let document_id = document.id.clone();

    let chapters = repo::list_chapters(pool, &document_id).await?;
    let chunks = repo::list_chunks(pool, &document_id).await?;

    // Refuse to export when nothing has been translated at all: silently
    // emitting the source text produced untranslated "translations".
    let translated_chunks = chunks.iter().filter(|c| chunk_has_translation(c)).count();
    if translated_chunks == 0 {
        return Err(AppError::Invalid(
            "nothing to export: no chunk has been translated yet".into(),
        ));
    }
    // Work that will fall back to the source text. The renderer writes one chunk
    // at a time and substitutes `chunk.source_md` whenever a chunk has no usable
    // `target_md`, so the honest unit is the chunk — not the block. A block can be
    // covered by its chunk's translation without having its own
    // `block_translation` row, which is why block-based counting drifted from the
    // actual output. This uses the same predicate as `render_chunks`.
    let untranslated_chunks = chunks.len().saturating_sub(translated_chunks);

    let output_dir = deps.output_dir(&request.project_id);
    let units_dir = output_dir.join("units");
    tokio::fs::create_dir_all(&units_dir).await?;

    let mut units: Vec<PandocUnit> = Vec::new();
    let mut index = 0usize;

    // Chunks that belong to no chapter (preamble/front matter).
    let preamble: Vec<_> = chunks.iter().filter(|c| c.chapter_id.is_none()).collect();
    if !preamble.is_empty() {
        let path = units_dir.join(format!("unit-{index:04}.md"));
        tokio::fs::write(&path, render_chunks(&preamble)).await?;
        units.push(PandocUnit {
            path: path.to_string_lossy().to_string(),
            title: "Front matter".to_string(),
        });
        index += 1;
    }

    for chapter in &chapters {
        let chapter_chunks: Vec<_> = chunks
            .iter()
            .filter(|c| c.chapter_id.as_deref() == Some(chapter.id.as_str()))
            .collect();
        if chapter_chunks.is_empty() {
            continue;
        }
        let path = units_dir.join(format!("unit-{index:04}.md"));
        tokio::fs::write(&path, render_chunks(&chapter_chunks)).await?;
        units.push(PandocUnit {
            path: path.to_string_lossy().to_string(),
            title: chapter.title.clone(),
        });
        index += 1;
    }

    if units.is_empty() {
        return Err(AppError::Invalid("project has no translated chunks".into()));
    }

    let mut metadata = BookMetadata::new(
        project
            .doc_title
            .clone()
            .unwrap_or_else(|| project.name.clone()),
    );
    metadata.author = project.doc_author.clone();
    metadata.language = Some(project.target_lang.clone());
    // The source document's own metadata (front matter) reaches the output too,
    // without overriding what the project row already set.
    match serde_json::from_str::<serde_json::Value>(&document.front_matter_json) {
        Ok(serde_json::Value::Object(front_matter)) => metadata.merge_front_matter(&front_matter),
        Ok(_) => {}
        Err(error) => tracing::warn!(%error, "ignoring unparsable document front matter"),
    }
    let _ = write_metadata_yaml(&output_dir, &metadata)?;

    let output_path = request.output_path.clone().unwrap_or_else(|| {
        output_dir
            .join(export_filename(&project.name, &request.output_format))
            .to_string_lossy()
            .to_string()
    });

    // Pandoc resolves the units' relative image hrefs (e.g. `assets/harbour.png`)
    // against the directory that holds `document.md` and the extracted assets dir.
    let resource_path = resource_paths(&document.markdown_path);

    let driver = PandocDriver::new(deps.sidecar.clone());
    let result = driver
        .build(
            units.clone(),
            metadata,
            &output_path,
            &request.output_format,
            request.template.clone(),
            request.css.clone(),
            resource_path,
        )
        .await?;

    // Report, in the build log, how much of the book is not translated. Counting
    // chunks (not blocks) keeps this in step with what `render_chunks` emitted.
    let mut log = result.log;
    log.push('\n');
    log.push_str(&format!(
        "[export] translated chunks: {translated_chunks}/{}; chunks rendered from source: {untranslated_chunks}\n",
        chunks.len()
    ));

    Ok(ExportOutcome {
        output_path: if result.output_path.is_empty() {
            output_path
        } else {
            result.output_path
        },
        units: units.len(),
        log,
        duration_ms: result.duration_ms,
    })
}

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
fn chunk_has_translation(chunk: &Chunk) -> bool {
    translated_text(chunk).is_some()
}

/// Default output filename: `<sanitized project name>.<format>`.
fn export_filename(project_name: &str, output_format: &str) -> String {
    format!("{}.{}", sanitize(project_name), output_format)
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
}
