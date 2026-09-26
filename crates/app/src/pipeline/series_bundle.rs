//! Series bundles: export and import of a series canon (PLAN.md §9.5, S4).
//!
//! The archive is a ZIP with `manifest.json` and `series.json` (the series row, its
//! glossary, the aliases and the memory values). Import **merges**: a term that is missing
//! locally is added, the same rendering only brings its note/kind up to date, and a
//! different rendering is never overwritten — the local row is flagged `conflict` when it
//! was not approved yet, and the member books are checked for conflicts against the
//! incoming rendering. Memory values are taken only when the incoming revision is newer.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::db::models::{Series, SeriesGlossaryTerm, SeriesGlossaryVariant, SeriesMemory};
use crate::db::{new_id, repo};
use crate::error::{AppError, Result};

/// Bundle format version; bump when the layout changes.
pub const FORMAT_VERSION: u32 = 1;

const MANIFEST_NAME: &str = "manifest.json";
const SERIES_NAME: &str = "series.json";

/// Cap on each archive entry, so a crafted bundle cannot exhaust memory.
const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesBundleManifest {
    pub format_version: u32,
    pub app_version: String,
    pub exported_at: String,
    pub series_id: String,
    pub series_name: String,
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
}

/// Write the series canon to `output_path`, defaulting to `<data_dir>/series/<name>.llmtsz`.
pub async fn export_series(
    pool: &SqlitePool,
    data_dir: &Path,
    series_id: &str,
    output_path: Option<&str>,
) -> Result<SeriesExportOutcome> {
    let series = repo::get_series(pool, series_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("series {series_id}")))?;
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
    };

    let target_clone = target.clone();
    let outcome =
        tokio::task::spawn_blocking(move || write_bundle(&target_clone, &manifest, &bundle))
            .await
            .map_err(|error| {
                AppError::Other(anyhow::anyhow!("series zip task panicked: {error}"))
            })??;
    let bytes = tokio::fs::metadata(&target).await?.len();
    Ok(SeriesExportOutcome {
        output_path: target.to_string_lossy().to_string(),
        bytes,
        terms: outcome.0,
        variants: outcome.1,
    })
}

/// Merge a series bundle into the database, never overwriting a different rendering.
pub async fn import_series(pool: &SqlitePool, archive_path: &str) -> Result<SeriesImportOutcome> {
    let archive = PathBuf::from(archive_path);
    if !archive.is_file() {
        return Err(AppError::NotFound(format!("archive {archive_path}")));
    }
    let (manifest, bundle) = tokio::task::spawn_blocking(move || read_bundle(&archive))
        .await
        .map_err(|error| {
            AppError::Other(anyhow::anyhow!("series unzip task panicked: {error}"))
        })??;
    if manifest.format_version != FORMAT_VERSION {
        return Err(AppError::Invalid(format!(
            "series bundle format version {} is not supported (expected {FORMAT_VERSION})",
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
    })
}

// ---------------------------------------------------------------------------
// ZIP helpers (blocking)
// ---------------------------------------------------------------------------

/// Returns `(terms, variants)` written.
fn write_bundle(
    target: &Path,
    manifest: &SeriesBundleManifest,
    bundle: &SeriesBundle,
) -> Result<(usize, usize)> {
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
    Ok((bundle.terms.len(), bundle.variants.len()))
}

fn read_bundle(archive: &Path) -> Result<(SeriesBundleManifest, SeriesBundle)> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    let manifest: SeriesBundleManifest = read_json(&mut zip, MANIFEST_NAME).map_err(|error| {
        AppError::Invalid(format!(
            "the archive is not a series bundle (manifest.json missing or invalid): {error}"
        ))
    })?;
    let bundle: SeriesBundle = read_json(&mut zip, SERIES_NAME).map_err(|error| {
        AppError::Invalid(format!(
            "the archive is not a series bundle (series.json missing or invalid): {error}"
        ))
    })?;
    Ok((manifest, bundle))
}

fn read_json<R: Read + std::io::Seek, T: serde::de::DeserializeOwned>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
) -> Result<T> {
    let mut entry = zip
        .by_name(name)
        .map_err(|error| AppError::Invalid(format!("{name}: {error}")))?;
    let mut text = String::new();
    entry
        .by_ref()
        .take(MAX_ENTRY_BYTES)
        .read_to_string(&mut text)?;
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
    use crate::db::models::GlossaryTerm;
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
        assert!(archive.is_file());

        let (target_pool, _target_dir) = connect_temp_file().await.expect("pool");
        let imported = import_series(&target_pool, &archive.to_string_lossy())
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
        let (target_pool, _target_dir) = connect_temp_file().await.expect("pool");
        seed_series(&target_pool).await;
        seed_term(&target_pool, "keeper", "guardiano", "candidate").await;

        let imported = import_series(&target_pool, &archive.to_string_lossy())
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
        let (target_pool, _target_dir) = connect_temp_file().await.expect("pool");
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

        import_series(&target_pool, &archive.to_string_lossy())
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
        let error = import_series(&pool, &bogus.to_string_lossy())
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
