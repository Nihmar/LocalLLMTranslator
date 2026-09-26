//! Series bundles: export and import of a series canon (PLAN.md §9.5, S4).
//!
//! The archive is a ZIP with `manifest.json`, `series.json` (the series row, its glossary,
//! the aliases and the memory values) and, for a bundle that carries the member books, one
//! `project.sqlite` snapshot plus a `projects/<id>/` tree per book. Import **merges**: a term
//! that is missing locally is added, the same rendering only brings its note/kind up to
//! date, and a different rendering is never overwritten — the local row is flagged
//! `conflict` when it was not approved yet, and the member books are checked for conflicts
//! against the incoming rendering. Memory values are taken only when the incoming revision
//! is newer. A book whose id already exists locally is skipped, so importing twice is safe.

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::bundle;
use crate::db::models::{Series, SeriesGlossaryTerm, SeriesGlossaryVariant, SeriesMemory};
use crate::db::{new_id, repo};
use crate::error::{AppError, Result};

/// Bundle format version; bump when the layout changes. Version 1 carried the canon only;
/// version 2 may also carry the member books.
pub const FORMAT_VERSION: u32 = 2;

const MANIFEST_NAME: &str = "manifest.json";
const SERIES_NAME: &str = "series.json";
const DATABASE_NAME: &str = "project.sqlite";

/// Caps for extraction, shared with the project bundle.
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesBundleManifest {
    pub format_version: u32,
    pub app_version: String,
    pub exported_at: String,
    pub series_id: String,
    pub series_name: String,
    /// Member books carried by the archive, in `series_order`.
    #[serde(default)]
    pub books: Vec<BundleBook>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleBook {
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub series_order: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesBundle {
    pub series: Series,
    pub terms: Vec<SeriesGlossaryTerm>,
    pub variants: Vec<SeriesGlossaryVariant>,
    pub memory: Vec<SeriesMemory>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesExportOutcome {
    pub output_path: String,
    pub bytes: u64,
    pub terms: usize,
    pub variants: usize,
    pub books: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeriesImportOutcome {
    pub series: Series,
    pub terms_added: usize,
    pub terms_updated: usize,
    /// Terms the bundle rendered differently from the local canon (never overwritten).
    pub conflicts: usize,
    pub variants_added: usize,
    pub memory_updated: usize,
    /// Member books carried by the bundle and copied locally.
    pub books_imported: usize,
    /// Member books the bundle carried but that already existed locally.
    pub books_skipped: usize,
}

/// Write the series canon (and its member books, when it has any) to `output_path`,
/// defaulting to `<data_dir>/series/<name>.llmtsz`.
pub async fn export_series(
    pool: &SqlitePool,
    data_dir: &Path,
    series_id: &str,
    output_path: Option<&str>,
) -> Result<SeriesExportOutcome> {
    let series = repo::get_series(pool, series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {series_id}")))?;
    let projects = repo::list_projects_for_series(pool, series_id).await?;
    let target = match output_path {
        Some(path) => PathBuf::from(path),
        None => data_dir
            .join("series")
            .join(format!("{}.llmtsz", sanitize(&series.name))),
    };
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let bundle = SeriesBundle {
        series: series.clone(),
        terms: repo::list_series_terms(pool, series_id).await?,
        variants: repo::list_variants_for_series(pool, series_id).await?,
        memory: repo::list_series_memory(pool, series_id).await?,
    };
    let manifest = SeriesBundleManifest {
        format_version: FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at: crate::db::now(),
        series_id: series.id.clone(),
        series_name: series.name.clone(),
        books: projects
            .iter()
            .map(|project| BundleBook {
                project_id: project.id.clone(),
                name: project.name.clone(),
                series_order: project.series_order,
            })
            .collect(),
    };
    let project_dirs: Vec<(String, PathBuf)> = projects
        .iter()
        .map(|project| {
            (
                project.id.clone(),
                data_dir.join("projects").join(&project.id),
            )
        })
        .collect();

    // One snapshot for every member book: the archive stays proportional to the canon,
    // not to the number of books.
    let snapshot = if project_dirs.is_empty() {
        None
    } else {
        Some(bundle::snapshot_database(pool, data_dir).await?)
    };

    let target_clone = target.clone();
    let snapshot_clone = snapshot.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        write_bundle(
            &target_clone,
            &manifest,
            &bundle,
            &project_dirs,
            snapshot_clone.as_deref(),
        )
    })
    .await
    .map_err(|error| AppError::Other(anyhow::anyhow!("series zip task panicked: {error}")));
    if let Some(snapshot) = &snapshot {
        let _ = tokio::fs::remove_file(snapshot).await;
    }
    let (terms, variants, books) = outcome??;
    let bytes = tokio::fs::metadata(&target).await?.len();
    Ok(SeriesExportOutcome {
        output_path: target.to_string_lossy().to_string(),
        bytes,
        terms,
        variants,
        books,
    })
}

/// Merge a series bundle into the database, never overwriting a different rendering and
/// never overwriting an existing book. Works with a version 1 bundle (canon only).
pub async fn import_series(
    pool: &SqlitePool,
    data_dir: &Path,
    archive_path: &str,
) -> Result<SeriesImportOutcome> {
    let archive = PathBuf::from(archive_path);
    if !archive.is_file() {
        return Err(AppError::NotFound(format!("archive {archive_path}")));
    }

    // Stage inside the data dir so the final renames stay on one filesystem.
    let staging = data_dir.join(format!("series-import-{}", new_id()));
    tokio::fs::create_dir_all(&staging).await?;
    let archive_clone = archive.clone();
    let staging_clone = staging.clone();
    let extract_result = tokio::task::spawn_blocking(move || {
        bundle::extract_archive_with_limits(
            &archive_clone,
            &staging_clone,
            MAX_ARCHIVE_ENTRIES,
            MAX_ARCHIVE_BYTES,
        )
    })
    .await
    .map_err(|error| AppError::Other(anyhow::anyhow!("series unzip task panicked: {error}")))?;
    if let Err(error) = extract_result {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(error);
    }

    let result = import_staged(pool, data_dir, &staging).await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result
}

async fn import_staged(
    pool: &SqlitePool,
    data_dir: &Path,
    staging: &Path,
) -> Result<SeriesImportOutcome> {
    let manifest: SeriesBundleManifest = read_json_file(&staging.join(MANIFEST_NAME))
        .await
        .map_err(|error| {
            AppError::Invalid(format!(
                "the archive is not a series bundle (manifest.json missing or invalid): {error}"
            ))
        })?;
    let bundle: SeriesBundle =
        read_json_file(&staging.join(SERIES_NAME))
            .await
            .map_err(|error| {
                AppError::Invalid(format!(
                    "the archive is not a series bundle (series.json missing or invalid): {error}"
                ))
            })?;
    if manifest.format_version == 0 || manifest.format_version > FORMAT_VERSION {
        return Err(AppError::Invalid(format!(
            "series bundle format version {} is not supported (expected up to {FORMAT_VERSION})",
            manifest.format_version
        )));
    }
    if manifest.series_id != bundle.series.id {
        return Err(AppError::Invalid(
            "the bundle manifest does not match its series payload".into(),
        ));
    }

    // ---- series row: keep the local creation time, take the newer fields ----------
    let series = match repo::get_series(pool, &bundle.series.id).await? {
        None => {
            repo::upsert_series(pool, &bundle.series).await?;
            bundle.series.clone()
        }
        Some(local) => {
            if bundle.series.updated_at > local.updated_at {
                let updated = Series {
                    created_at: local.created_at.clone(),
                    ..bundle.series.clone()
                };
                repo::upsert_series(pool, &updated).await?;
                updated
            } else {
                local
            }
        }
    };

    // ---- member books (version 2 bundles only) ------------------------------------
    let mut books_imported = 0;
    let mut books_skipped = 0;
    let snapshot = staging.join(DATABASE_NAME);
    if !manifest.books.is_empty() && snapshot.is_file() {
        for book in &manifest.books {
            if repo::get_project(pool, &book.project_id).await?.is_some() {
                books_skipped += 1;
                continue;
            }
            let from = staging.join("projects").join(&book.project_id);
            let project_dir = data_dir.join("projects").join(&book.project_id);
            tokio::fs::create_dir_all(&project_dir).await?;
            for name in ["work", "output", "prompts"] {
                let source = from.join(name);
                if source.is_dir() {
                    tokio::fs::rename(&source, project_dir.join(name)).await?;
                }
            }
            if let Err(error) = bundle::copy_project_rows(
                pool,
                &snapshot,
                &book.project_id,
                &project_dir.join("work"),
                &project_dir.join("prompts"),
            )
            .await
            {
                let _ = tokio::fs::remove_dir_all(&project_dir).await;
                return Err(error);
            }
            repo::set_project_series(pool, &book.project_id, Some(&series.id), book.series_order)
                .await?;
            books_imported += 1;
        }
    }

    // ---- glossary: match by source, never overwrite a different rendering ---------
    let mut local_ids: HashMap<String, String> = HashMap::new();
    let mut terms_added = 0;
    let mut terms_updated = 0;
    let mut conflicts = 0;
    for term in &bundle.terms {
        match repo::get_series_term_by_source(pool, &series.id, &term.source).await? {
            None => {
                repo::upsert_series_term(pool, term).await?;
                local_ids.insert(term.id.clone(), term.id.clone());
                terms_added += 1;
                // A member book rendering it differently is the interesting case.
                if let Err(error) = crate::pipeline::glossary::flag_series_conflicts(
                    pool,
                    &series.id,
                    &term.source,
                    &term.target,
                )
                .await
                {
                    tracing::warn!(%error, "could not flag series conflicts after import");
                }
            }
            Some(local) => {
                local_ids.insert(term.id.clone(), local.id.clone());
                if local.target.eq_ignore_ascii_case(&term.target) {
                    if local.note != term.note || local.kind != term.kind {
                        let merged = SeriesGlossaryTerm {
                            note: term.note.clone(),
                            kind: term.kind.clone(),
                            revision: local.revision + 1,
                            ..local.clone()
                        };
                        repo::upsert_series_term(pool, &merged).await?;
                        terms_updated += 1;
                    }
                } else {
                    if local.status != "approved" {
                        repo::set_series_term_status(pool, &local.id, "conflict").await?;
                    }
                    conflicts += 1;
                }
            }
        }
    }

    // ---- aliases: union, mapped onto the local term ids ---------------------------
    let mut variants_added = 0;
    for variant in &bundle.variants {
        let Some(local_term_id) = local_ids.get(&variant.term_id) else {
            continue;
        };
        repo::upsert_series_variant(
            pool,
            &SeriesGlossaryVariant {
                id: new_id(),
                term_id: local_term_id.clone(),
                text: variant.text.clone(),
            },
        )
        .await?;
        variants_added += 1;
    }

    // ---- memory: an incoming revision wins only when it is newer ------------------
    let local_memory: HashMap<String, i64> = repo::list_series_memory(pool, &series.id)
        .await?
        .into_iter()
        .map(|row| (row.key, row.revision))
        .collect();
    let mut memory_updated = 0;
    for row in &bundle.memory {
        let newer = local_memory
            .get(&row.key)
            .is_none_or(|revision| row.revision > *revision);
        if newer {
            repo::set_series_memory(pool, &series.id, &row.key, &row.value).await?;
            memory_updated += 1;
        }
    }

    Ok(SeriesImportOutcome {
        series,
        terms_added,
        terms_updated,
        conflicts,
        variants_added,
        memory_updated,
        books_imported,
        books_skipped,
    })
}

// ---------------------------------------------------------------------------
// ZIP helpers (blocking)
// ---------------------------------------------------------------------------

/// Returns `(terms, variants, books)` written.
fn write_bundle(
    target: &Path,
    manifest: &SeriesBundleManifest,
    bundle: &SeriesBundle,
    project_dirs: &[(String, PathBuf)],
    snapshot: Option<&Path>,
) -> Result<(usize, usize, usize)> {
    // The archive is written next to the target and renamed into place, so a crash
    // never leaves a half-written bundle. `tempfile` is a dev-dependency only, so the
    // sibling name is built from a random id.
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let temp = parent.join(format!(".series-{}.tmp", crate::db::new_id()));
    let result = (|| -> Result<()> {
        let mut zip = zip::ZipWriter::new(File::create(&temp)?);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        zip.start_file(MANIFEST_NAME, options)?;
        zip.write_all(&serde_json::to_vec_pretty(manifest)?)?;
        zip.start_file(SERIES_NAME, options)?;
        zip.write_all(&serde_json::to_vec_pretty(bundle)?)?;

        if let Some(snapshot) = snapshot {
            zip.start_file(DATABASE_NAME, options)?;
            let mut database = File::open(snapshot)?;
            std::io::copy(&mut database, &mut zip)?;
        }
        for (project_id, dir) in project_dirs {
            bundle::add_tree(
                &mut zip,
                dir,
                &format!("projects/{project_id}"),
                options,
                None,
            )?;
        }
        zip.finish()?;
        Ok(())
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if std::fs::rename(&temp, target).is_err() {
        // Windows refuses to replace an existing destination with `rename`.
        let _ = std::fs::remove_file(target);
        std::fs::rename(&temp, target)?;
    }
    Ok((
        bundle.terms.len(),
        bundle.variants.len(),
        project_dirs.len(),
    ))
}

async fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let text = tokio::fs::read_to_string(path).await?;
    Ok(serde_json::from_str(&text)?)
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
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
    use crate::db::models::{GlossaryTerm, Project};
    use crate::db::{connect_temp_file, new_id, now};

    async fn seed_series(pool: &SqlitePool) -> Series {
        let timestamp = now();
        let series = Series {
            id: "s1".to_string(),
            name: "The Saga".to_string(),
            source_lang: Some("en".to_string()),
            target_lang: Some("it".to_string()),
            settings_json: "{}".to_string(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        repo::upsert_series(pool, &series).await.expect("series");
        series
    }

    async fn seed_term(pool: &SqlitePool, source: &str, target: &str, status: &str) -> String {
        let id = new_id();
        repo::upsert_series_term(
            pool,
            &SeriesGlossaryTerm {
                id: id.clone(),
                series_id: "s1".to_string(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                source: source.to_string(),
                target: target.to_string(),
                note: None,
                kind: "term".to_string(),
                origin: "manual".to_string(),
                revision: 1,
                status: status.to_string(),
            },
        )
        .await
        .expect("term");
        id
    }

    /// A member book: the row, a `document.md` under `work/` and a prompts dir.
    async fn seed_book(data_dir: &Path, project_id: &str, series_id: &str) -> Project {
        let timestamp = now();
        let project = Project {
            id: project_id.to_string(),
            name: "Book One".to_string(),
            source_path: "/source/book.epub".to_string(),
            source_hash: "h".to_string(),
            source_format: "epub".to_string(),
            source_lang: Some("en".to_string()),
            target_lang: "it".to_string(),
            doc_title: None,
            doc_author: None,
            series_id: Some(series_id.to_string()),
            series_order: Some(1),
            prompts_snapshot_dir: None,
            settings_json: "{}".to_string(),
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        let work = data_dir.join("projects").join(project_id).join("work");
        std::fs::create_dir_all(&work).expect("work dir");
        std::fs::write(work.join("document.md"), "# Chapter One\n").expect("document");
        std::fs::create_dir_all(data_dir.join("projects").join(project_id).join("prompts"))
            .expect("prompts dir");
        project
    }

    #[tokio::test]
    async fn a_round_trip_moves_the_canon_to_an_empty_database() {
        let (source_pool, source_dir) = connect_temp_file().await.expect("pool");
        seed_series(&source_pool).await;
        let term_id = seed_term(&source_pool, "keeper", "custode", "approved").await;
        repo::upsert_series_variant(
            &source_pool,
            &SeriesGlossaryVariant {
                id: new_id(),
                term_id,
                text: "the Keeper".to_string(),
            },
        )
        .await
        .expect("variant");
        repo::set_series_memory(&source_pool, "s1", "style_guide", "Registro alto")
            .await
            .expect("memory");

        let archive = source_dir.path().join("saga.llmtsz");
        let exported = export_series(
            &source_pool,
            source_dir.path(),
            "s1",
            Some(&archive.to_string_lossy()),
        )
        .await
        .expect("export");
        assert_eq!(exported.terms, 1);
        assert_eq!(exported.books, 0);
        assert!(archive.is_file());

        let (target_pool, target_dir) = connect_temp_file().await.expect("pool");
        let imported = import_series(&target_pool, target_dir.path(), &archive.to_string_lossy())
            .await
            .expect("import");
        assert_eq!(imported.terms_added, 1);
        assert_eq!(imported.variants_added, 1);
        assert_eq!(imported.memory_updated, 1);

        let terms = repo::list_series_terms(&target_pool, "s1")
            .await
            .expect("terms");
        assert_eq!(terms.len(), 1);
        assert_eq!(terms[0].target, "custode");
        let variants = repo::list_variants_for_series(&target_pool, "s1")
            .await
            .expect("variants");
        assert_eq!(variants.len(), 1);
        assert_eq!(
            repo::get_series_memory(&target_pool, "s1", "style_guide")
                .await
                .expect("memory")
                .as_deref(),
            Some("Registro alto")
        );
    }

    #[tokio::test]
    async fn a_series_bundle_carries_its_member_books() {
        let (source_pool, source_dir) = connect_temp_file().await.expect("pool");
        let series = seed_series(&source_pool).await;
        let project = seed_book(source_dir.path(), "p1", &series.id).await;
        repo::insert_project(&source_pool, &project)
            .await
            .expect("project");
        seed_term(&source_pool, "keeper", "custode", "approved").await;

        let archive = source_dir.path().join("saga.llmtsz");
        let exported = export_series(
            &source_pool,
            source_dir.path(),
            &series.id,
            Some(&archive.to_string_lossy()),
        )
        .await
        .expect("export");
        assert_eq!(exported.books, 1);

        let (target_pool, target_dir) = connect_temp_file().await.expect("pool");
        let imported = import_series(&target_pool, target_dir.path(), &archive.to_string_lossy())
            .await
            .expect("import");
        assert_eq!(imported.books_imported, 1);
        assert_eq!(imported.books_skipped, 0);

        // The book exists locally, belongs to the series and points at local paths.
        let stored = repo::get_project(&target_pool, "p1")
            .await
            .expect("get")
            .expect("project");
        assert_eq!(stored.series_id.as_deref(), Some("s1"));
        let local_work = target_dir.path().join("projects").join("p1").join("work");
        assert!(
            local_work.join("document.md").is_file(),
            "the book's work directory must move with the bundle"
        );
        let document = repo::get_document_for_project(&target_pool, "p1")
            .await
            .expect("document");
        if let Some(document) = document {
            assert!(document
                .markdown_path
                .starts_with(target_dir.path().to_str().unwrap()));
        }

        // Importing the same bundle again skips the existing book instead of failing.
        let again = import_series(&target_pool, target_dir.path(), &archive.to_string_lossy())
            .await
            .expect("second import");
        assert_eq!(again.books_imported, 0);
        assert_eq!(again.books_skipped, 1);
    }

    #[tokio::test]
    async fn importing_never_overwrites_a_different_rendering() {
        let (source_pool, source_dir) = connect_temp_file().await.expect("pool");
        seed_series(&source_pool).await;
        seed_term(&source_pool, "keeper", "custode", "approved").await;
        let archive = source_dir.path().join("saga.llmtsz");
        export_series(
            &source_pool,
            source_dir.path(),
            "s1",
            Some(&archive.to_string_lossy()),
        )
        .await
        .expect("export");

        // The local canon renders the same term differently and is not approved yet.
        let (target_pool, target_dir) = connect_temp_file().await.expect("pool");
        seed_series(&target_pool).await;
        seed_term(&target_pool, "keeper", "guardiano", "candidate").await;

        let imported = import_series(&target_pool, target_dir.path(), &archive.to_string_lossy())
            .await
            .expect("import");
        assert_eq!(imported.terms_added, 0);
        assert_eq!(imported.conflicts, 1);

        let terms = repo::list_series_terms(&target_pool, "s1")
            .await
            .expect("terms");
        assert_eq!(terms.len(), 1, "no duplicate row");
        assert_eq!(terms[0].target, "guardiano", "the local rendering stays");
        assert_eq!(terms[0].status, "conflict", "and is flagged");
    }

    #[tokio::test]
    async fn importing_flags_member_books_that_render_the_new_term_differently() {
        let (source_pool, source_dir) = connect_temp_file().await.expect("pool");
        seed_series(&source_pool).await;
        seed_term(&source_pool, "keeper", "custode", "approved").await;
        let archive = source_dir.path().join("saga.llmtsz");
        export_series(
            &source_pool,
            source_dir.path(),
            "s1",
            Some(&archive.to_string_lossy()),
        )
        .await
        .expect("export");

        // The target has the series plus one book that renders "keeper" as "guardiano".
        let (target_pool, target_dir) = connect_temp_file().await.expect("pool");
        let series = seed_series(&target_pool).await;
        let timestamp = now();
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, source_lang, \
             target_lang, series_id, series_order, settings_json, created_at, updated_at) \
             VALUES ('p1','book','/x','h','epub','en','it',?1,1,'{}',?2,?2)",
        )
        .bind(&series.id)
        .bind(&timestamp)
        .execute(&target_pool)
        .await
        .expect("project");
        repo::upsert_glossary_term(
            &target_pool,
            &GlossaryTerm {
                id: new_id(),
                project_id: "p1".to_string(),
                source_lang: Some("en".to_string()),
                target_lang: Some("it".to_string()),
                source: "keeper".to_string(),
                target: "guardiano".to_string(),
                note: None,
                kind: "term".to_string(),
                origin: "manual".to_string(),
                revision: 1,
                status: "approved".to_string(),
            },
        )
        .await
        .expect("project term");

        import_series(&target_pool, target_dir.path(), &archive.to_string_lossy())
            .await
            .expect("import");

        let findings = repo::list_qa_findings(&target_pool, "p1")
            .await
            .expect("findings");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "glossary_conflict");
    }

    #[tokio::test]
    async fn a_corrupt_archive_is_rejected_cleanly() {
        let (pool, dir) = connect_temp_file().await.expect("pool");
        let bogus = dir.path().join("bogus.llmtsz");
        std::fs::write(&bogus, b"this is not a zip").expect("write");
        let error = import_series(&pool, dir.path(), &bogus.to_string_lossy())
            .await
            .expect_err("must be rejected");
        assert!(
            error.to_string().contains("series bundle")
                || error.to_string().contains("zip")
                || error.to_string().contains("invalid"),
            "{error}"
        );
    }
}
