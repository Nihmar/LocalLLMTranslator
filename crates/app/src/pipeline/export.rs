//! Export: per-chapter Markdown units, `metadata.yaml`, Pandoc build.

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

    let document_id: Option<String> =
        sqlx::query_scalar("SELECT id FROM document WHERE project_id = ?1 LIMIT 1")
            .bind(&request.project_id)
            .fetch_optional(pool)
            .await?;
    let document_id =
        document_id.ok_or_else(|| AppError::NotFound("project has no ingested document".into()))?;

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
    let _ = write_metadata_yaml(&output_dir, &metadata)?;

    let output_path = request.output_path.clone().unwrap_or_else(|| {
        output_dir
            .join(export_filename(&project.name, &request.output_format))
            .to_string_lossy()
            .to_string()
    });

    let driver = PandocDriver::new(deps.sidecar.clone());
    let result = driver
        .build(
            units.clone(),
            metadata,
            &output_path,
            &request.output_format,
            request.template.clone(),
            request.css.clone(),
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
