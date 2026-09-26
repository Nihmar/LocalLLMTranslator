//! Export: per-chapter Markdown units, `metadata.yaml`, Pandoc build.

use serde::{Deserialize, Serialize};

use super::PipelineDeps;
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
            .join(format!(
                "{} .{}",
                sanitize(&project.name),
                request.output_format
            ))
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

    Ok(ExportOutcome {
        output_path: if result.output_path.is_empty() {
            output_path
        } else {
            result.output_path
        },
        units: units.len(),
        log: result.log,
        duration_ms: result.duration_ms,
    })
}

/// Render chunk outputs (translated when available) into one Markdown unit.
fn render_chunks(chunks: &[&crate::db::models::Chunk]) -> String {
    let mut out = String::new();
    for chunk in chunks {
        match &chunk.target_md {
            Some(text) if !text.is_empty() => out.push_str(text),
            _ => out.push_str(&chunk.source_md),
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
}
