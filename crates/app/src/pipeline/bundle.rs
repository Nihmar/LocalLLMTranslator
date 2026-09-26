//! `.llmtz` project bundles: export and import (PLAN.md §6).
//!
//! The archive is a ZIP with `manifest.json`, a `VACUUM INTO` snapshot of the app
//! database, the project's `work/` directory (Markdown + assets), its `output/`
//! directory and the `prompts/` snapshot. Import extracts the files under the
//! project data directory, copies the project-owned rows out of the attached
//! archive database and rewrites the absolute paths to the local locations; an id
//! that already exists is rejected, and no other project's rows are touched.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, SqlitePool};

use crate::db::models::Project;
use crate::db::{new_id, now, repo};
use crate::error::{AppError, Result};

/// Bundle format version; bump when the layout changes.
pub const FORMAT_VERSION: u32 = 1;

const MANIFEST_NAME: &str = "manifest.json";
const DATABASE_NAME: &str = "project.sqlite";

/// Caps for extraction: a bundle is a project snapshot, not an arbitrary archive.
/// `enclosed_name` stops path traversal but not a decompression bomb.
const MAX_ARCHIVE_ENTRIES: usize = 20_000;
const MAX_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleManifest {
    pub format_version: u32,
    pub app_version: String,
    pub exported_at: String,
    pub project_id: String,
    pub project_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportBundleOutcome {
    pub output_path: String,
    pub bytes: u64,
    pub files: usize,
}

/// Write `project_id` to a `.llmtz` archive. `output_path` defaults to the
/// project output directory.
pub async fn export_project(
    pool: &SqlitePool,
    data_dir: &Path,
    project_id: &str,
    output_path: Option<&str>,
) -> Result<ExportBundleOutcome> {
    let project = repo::get_project(pool, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("project {project_id}")))?;

    let project_dir = data_dir.join("projects").join(project_id);
    let output_dir = project_dir.join("output");
    tokio::fs::create_dir_all(&output_dir).await?;
    let target = match output_path {
        Some(path) => PathBuf::from(path),
        None => output_dir.join(format!("{}.llmtz", sanitize(&project.name))),
    };
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // A consistent database snapshot; SQLite refuses an existing destination.
    let database_snapshot = snapshot_database(pool, data_dir).await?;

    let manifest = BundleManifest {
        format_version: FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        exported_at: now(),
        project_id: project_id.to_string(),
        project_name: project.name.clone(),
    };

    let project_dir_clone = project_dir.clone();
    let target_clone = target.clone();
    let snapshot_clone = database_snapshot.clone();
    let zip_result = tokio::task::spawn_blocking(move || {
        write_archive(
            &target_clone,
            &project_dir_clone,
            &snapshot_clone,
            &manifest,
        )
    })
    .await
    .map_err(|error| AppError::Other(anyhow::anyhow!("zip task panicked: {error}")))?;
    let _ = tokio::fs::remove_file(&database_snapshot).await;
    let files = zip_result?;

    let bytes = tokio::fs::metadata(&target).await?.len();
    Ok(ExportBundleOutcome {
        output_path: target.to_string_lossy().to_string(),
        bytes,
        files,
    })
}

/// A consistent `VACUUM INTO` snapshot of the database, inside `data_dir`.
///
/// Shared with the series bundle, which stores one snapshot for all the member books.
/// The caller owns the returned file and removes it when done.
pub(crate) async fn snapshot_database(pool: &SqlitePool, data_dir: &Path) -> Result<PathBuf> {
    let snapshot = data_dir.join(format!("export-{}.sqlite", new_id()));
    let _ = tokio::fs::remove_file(&snapshot).await;
    sqlx::query("VACUUM INTO ?1")
        .bind(snapshot.to_string_lossy().to_string())
        .execute(pool)
        .await?;
    Ok(snapshot)
}

/// Extract `archive_path` and import its project into the local database.
pub async fn import_project(
    pool: &SqlitePool,
    data_dir: &Path,
    archive_path: &str,
) -> Result<Project> {
    let archive = PathBuf::from(archive_path);
    if !archive.is_file() {
        return Err(AppError::NotFound(format!("archive {archive_path}")));
    }

    // Stage inside the data dir so the final rename stays on one filesystem.
    let staging = data_dir.join(format!("import-{}", new_id()));
    tokio::fs::create_dir_all(&staging).await?;
    let archive_clone = archive.clone();
    let staging_clone = staging.clone();
    let extract_result =
        tokio::task::spawn_blocking(move || extract_archive(&archive_clone, &staging_clone))
            .await
            .map_err(|error| AppError::Other(anyhow::anyhow!("unzip task panicked: {error}")))?;
    if let Err(error) = extract_result {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(error);
    }

    let result = import_staged(pool, data_dir, &staging).await;
    let _ = tokio::fs::remove_dir_all(&staging).await;
    result
}

async fn import_staged(pool: &SqlitePool, data_dir: &Path, staging: &Path) -> Result<Project> {
    let manifest_text = tokio::fs::read_to_string(staging.join(MANIFEST_NAME))
        .await
        .map_err(|_| {
            AppError::Invalid("the archive has no manifest.json; it is not a .llmtz bundle".into())
        })?;
    let manifest: BundleManifest = serde_json::from_str(&manifest_text)
        .map_err(|error| AppError::Invalid(format!("invalid bundle manifest: {error}")))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(AppError::Invalid(format!(
            "bundle format version {} is not supported (expected {FORMAT_VERSION})",
            manifest.format_version
        )));
    }
    let database = staging.join(DATABASE_NAME);
    if !database.is_file() {
        return Err(AppError::Invalid(
            "the archive has no project.sqlite; it is not a .llmtz bundle".into(),
        ));
    }

    let project_id = manifest.project_id.clone();
    let project_dir = data_dir.join("projects").join(&project_id);
    if project_dir.exists() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(AppError::Invalid(format!(
            "project {project_id} already exists; delete it before importing the bundle"
        )));
    }

    // Move the bundled directories into place before the rows point at them.
    tokio::fs::create_dir_all(&project_dir).await?;
    for name in ["work", "output", "prompts"] {
        let from = staging.join(name);
        if from.is_dir() {
            tokio::fs::rename(&from, project_dir.join(name)).await?;
        }
    }

    let local_work = project_dir.join("work");
    let local_prompts = project_dir.join("prompts");
    if let Err(error) =
        copy_project_rows(pool, &database, &project_id, &local_work, &local_prompts).await
    {
        let _ = tokio::fs::remove_dir_all(&project_dir).await;
        return Err(error);
    }

    repo::get_project(pool, &project_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("imported project {project_id}")))
}

/// Copy the project-owned rows from the attached archive database. `ATTACH` is
/// per-connection, so the whole copy runs on one pooled connection.
pub(crate) async fn copy_project_rows(
    pool: &SqlitePool,
    database: &Path,
    project_id: &str,
    local_work: &Path,
    local_prompts: &Path,
) -> Result<()> {
    let mut connection = pool.acquire().await?;
    sqlx::query("ATTACH DATABASE ?1 AS imported")
        .bind(database.to_string_lossy().to_string())
        .execute(&mut *connection)
        .await?;

    let result =
        copy_rows_in_transaction(&mut connection, project_id, local_work, local_prompts).await;
    let _ = sqlx::query("DETACH DATABASE imported")
        .execute(&mut *connection)
        .await;
    result
}

async fn copy_rows_in_transaction(
    connection: &mut SqliteConnection,
    project_id: &str,
    local_work: &Path,
    local_prompts: &Path,
) -> Result<()> {
    let archived: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM imported.project WHERE id = ?1")
        .bind(project_id)
        .fetch_one(&mut *connection)
        .await?;
    if archived == 0 {
        return Err(AppError::Invalid(format!(
            "the archive does not contain project {project_id}"
        )));
    }
    let local: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project WHERE id = ?1")
        .bind(project_id)
        .fetch_one(&mut *connection)
        .await?;
    if local > 0 {
        return Err(AppError::Invalid(format!(
            "project {project_id} already exists locally"
        )));
    }

    // The archived markdown path is absolute on the exporting machine; keep only
    // its file name and rebuild it against the local work directory.
    let archived_markdown: Option<String> = sqlx::query_scalar(
        "SELECT markdown_path FROM imported.document WHERE project_id = ?1 LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?;
    let markdown_name = archived_markdown
        .as_deref()
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "document.md".to_string());
    let local_markdown = local_work
        .join(&markdown_name)
        .to_string_lossy()
        .to_string();

    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let copy = async {
        // Foreign-key-safe order: parents before children. The statements are
        // literal on purpose: sqlx 0.9 refuses dynamically built SQL. `project` uses an
        // explicit column list with NULL series values so an archive written before the
        // series columns existed still imports (a `SELECT *` would not line up).
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, \
             source_lang, target_lang, doc_title, doc_author, series_id, series_order, \
             prompts_snapshot_dir, settings_json, created_at, updated_at) \
             SELECT id, name, source_path, source_hash, source_format, source_lang, target_lang, \
             doc_title, doc_author, NULL, NULL, prompts_snapshot_dir, settings_json, created_at, \
             updated_at FROM imported.project WHERE id = ?1",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query("INSERT INTO document SELECT * FROM imported.document WHERE project_id = ?1")
            .bind(project_id)
            .execute(&mut *connection)
            .await?;
        sqlx::query(
            "INSERT INTO chapter SELECT * FROM imported.chapter \
             WHERE document_id IN (SELECT id FROM imported.document WHERE project_id = ?1)",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT INTO block SELECT * FROM imported.block \
             WHERE document_id IN (SELECT id FROM imported.document WHERE project_id = ?1)",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT INTO chunk SELECT * FROM imported.chunk \
             WHERE document_id IN (SELECT id FROM imported.document WHERE project_id = ?1)",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT INTO block_translation SELECT * FROM imported.block_translation \
             WHERE chunk_id IN (SELECT id FROM imported.chunk WHERE document_id IN \
             (SELECT id FROM imported.document WHERE project_id = ?1))",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT INTO suggestion SELECT * FROM imported.suggestion \
             WHERE chunk_id IN (SELECT id FROM imported.chunk WHERE document_id IN \
             (SELECT id FROM imported.document WHERE project_id = ?1))",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query("INSERT INTO qa_finding SELECT * FROM imported.qa_finding WHERE project_id = ?1")
            .bind(project_id)
            .execute(&mut *connection)
            .await?;
        sqlx::query(
            "INSERT INTO glossary_term SELECT * FROM imported.glossary_term WHERE project_id = ?1",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;
        sqlx::query(
            "INSERT INTO project_memory SELECT * FROM imported.project_memory WHERE project_id = ?1",
        )
        .bind(project_id)
        .execute(&mut *connection)
        .await?;

        // Local paths and a clean export state: the archived ones point at the
        // exporting machine and the build cache is therefore meaningless.
        sqlx::query("UPDATE document SET markdown_path = ?2 WHERE project_id = ?1")
            .bind(project_id)
            .bind(&local_markdown)
            .execute(&mut *connection)
            .await?;
        sqlx::query("UPDATE project SET prompts_snapshot_dir = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(project_id)
            .bind(local_prompts.to_string_lossy().to_string())
            .bind(now())
            .execute(&mut *connection)
            .await?;
        sqlx::query("DELETE FROM project_memory WHERE project_id = ?1 AND key IN ('export_state', 'export_history')")
            .bind(project_id)
            .execute(&mut *connection)
            .await?;
        Ok::<(), AppError>(())
    }
    .await;

    match copy {
        Ok(()) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(())
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// ZIP helpers (blocking)
// ---------------------------------------------------------------------------

fn write_archive(
    target: &Path,
    project_dir: &Path,
    database: &Path,
    manifest: &BundleManifest,
) -> Result<usize> {
    let file = File::create(target)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let mut files = 0usize;

    let manifest_json = serde_json::to_vec_pretty(manifest)?;
    zip.start_file(MANIFEST_NAME, options)?;
    zip.write_all(&manifest_json)?;

    zip.start_file(DATABASE_NAME, options)?;
    let mut database_file = File::open(database)?;
    std::io::copy(&mut database_file, &mut zip)?;

    // `project_dir` holds work/ (with the assets), output/ and prompts/. The
    // target itself is skipped: it lives in output/ and would otherwise be added
    // to the archive while it is being written.
    files += add_tree(&mut zip, project_dir, "", options, Some(target))?;
    zip.finish()?;
    Ok(files)
}

/// Add a directory tree to the archive, skipping `exclude` and anything that is
/// not a regular file or directory.
pub(crate) fn add_tree(
    zip: &mut zip::ZipWriter<File>,
    root: &Path,
    prefix: &str,
    options: zip::write::SimpleFileOptions,
    exclude: Option<&Path>,
) -> Result<usize> {
    let mut files = 0usize;
    if !root.is_dir() {
        return Ok(files);
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root)?
        .filter_map(std::result::Result::ok)
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if exclude.is_some_and(|excluded| path == excluded) {
            continue;
        }
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let archive_name = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if path.is_dir() {
            zip.add_directory(format!("{archive_name}/"), options)?;
            files += add_tree(zip, &path, &archive_name, options, exclude)?;
        } else if path.is_file() {
            zip.start_file(archive_name, options)?;
            let mut source = File::open(&path)?;
            std::io::copy(&mut source, zip)?;
            files += 1;
        }
    }
    Ok(files)
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    extract_archive_with_limits(archive, destination, MAX_ARCHIVE_ENTRIES, MAX_ARCHIVE_BYTES)
}

/// Extract `archive` into `destination`, refusing an archive that is too large or holds
/// too many entries. The byte budget is enforced while copying, so a header that lies
/// about its entry size cannot smuggle a bomb past it.
pub(crate) fn extract_archive_with_limits(
    archive: &Path,
    destination: &Path,
    max_entries: usize,
    max_bytes: u64,
) -> Result<()> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;
    if zip.len() > max_entries {
        return Err(AppError::Invalid(format!(
            "the archive has {} entries; the supported maximum is {max_entries}",
            zip.len()
        )));
    }
    let mut extracted: u64 = 0;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        // `enclosed_name` rejects absolute paths and `..` traversal.
        let Some(relative) = entry.enclosed_name() else {
            return Err(AppError::Invalid(
                "the archive contains an unsafe path".into(),
            ));
        };
        let out = destination.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let allowed = max_bytes.saturating_sub(extracted);
        let mut target = File::create(&out)?;
        // Read at most one byte past the remaining budget: if it is consumed, the
        // archive is larger than the cap and the staging directory is discarded.
        let copied = std::io::copy(&mut (&mut entry).take(allowed + 1), &mut target)?;
        extracted = extracted.saturating_add(copied);
        if extracted > max_bytes {
            return Err(AppError::Invalid(format!(
                "the archive expands to more than {max_bytes} bytes; it is not a supported .llmtz bundle"
            )));
        }
    }
    Ok(())
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
    use std::io::Write;

    fn zip_with(entries: &[(&str, &[u8])]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bundle.zip");
        let mut writer = zip::ZipWriter::new(File::create(&path).expect("create"));
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in entries {
            writer.start_file(*name, options).expect("start");
            writer.write_all(body).expect("write");
        }
        writer.finish().expect("finish");
        (dir, path)
    }

    #[test]
    fn extraction_refuses_a_too_large_archive() {
        let (dir, archive) = zip_with(&[("work/document.md", b"0123456789")]);
        let destination = dir.path().join("out");

        // Five bytes are allowed: the copy reads one past the budget and fails.
        let error = extract_archive_with_limits(&archive, &destination, 10, 5)
            .expect_err("the byte cap must reject the archive");
        assert!(
            error.to_string().contains("expands to more than"),
            "{error}"
        );

        // The same archive is fine with a budget above its size.
        extract_archive_with_limits(&archive, &destination, 10, 64).expect("under the cap");
    }

    #[test]
    fn extraction_refuses_too_many_entries() {
        let (dir, archive) = zip_with(&[("a", b"a"), ("b", b"b"), ("c", b"c")]);
        let error = extract_archive_with_limits(&archive, &dir.path().join("out"), 2, 1024)
            .expect_err("the entry cap must reject the archive");
        assert!(error.to_string().contains("entries"), "{error}");
    }

    #[test]
    fn extraction_refuses_an_unsafe_path() {
        let (dir, archive) = zip_with(&[("../escape.txt", b"x")]);
        let error = extract_archive_with_limits(&archive, &dir.path().join("out"), 10, 1024)
            .expect_err("traversal must be rejected");
        assert!(error.to_string().contains("unsafe path"), "{error}");
        assert!(!dir.path().join("escape.txt").exists());
    }
}
