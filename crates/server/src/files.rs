//! File transfer for the browser client (`PLAN.md` §12.3, issue #17 step 4).
//!
//! The desktop shell hands the backend absolute paths from the native picker; a browser
//! cannot, so `POST /api/upload` stores a picked or dropped file under `<data_dir>/uploads/`
//! and returns the path the path-based commands expect, and `GET /api/download` streams an
//! artifact back. Downloads are confined to the data directory: a request may not read an
//! arbitrary file from the machine.

use std::path::{Path, PathBuf};

use app_lib::error::AppError;
use axum::body::Body;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tokio::fs::File;
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use crate::ServerState;

/// Largest accepted upload. An EPUB or PDF of a long novel stays well below this.
pub const MAX_UPLOAD_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    /// Original file name, used for the stored copy and the extension-based format hint.
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DownloadQuery {
    /// Absolute path of a file under the data directory.
    pub path: String,
}

/// `POST /api/upload?filename=…`: the body is the file itself.
pub async fn upload_file(
    State(server): State<ServerState>,
    Query(query): Query<UploadQuery>,
    body: axum::body::Bytes,
) -> Response {
    if body.is_empty() {
        return crate::error_response(AppError::Invalid("the uploaded file is empty".into()));
    }
    let name = sanitize_filename(query.filename.as_deref().unwrap_or("document"));
    let dir = server
        .state
        .data_dir
        .join("uploads")
        .join(Uuid::new_v4().simple().to_string());
    let path = dir.join(&name);
    if let Err(error) = tokio::fs::create_dir_all(&dir).await {
        return crate::error_response(AppError::Io(error));
    }
    if let Err(error) = tokio::fs::write(&path, &body).await {
        return crate::error_response(AppError::Io(error));
    }
    tracing::info!(path = %path.display(), bytes = body.len(), "uploaded a file");
    (
        StatusCode::OK,
        Json(json!({
            "path": path.to_string_lossy(),
            "name": name,
            "bytes": body.len(),
        })),
    )
        .into_response()
}

/// `GET /api/download?path=…`: stream a file that lives under the data directory.
pub async fn download_file(
    State(server): State<ServerState>,
    Query(query): Query<DownloadQuery>,
) -> Response {
    let resolved = match resolve_download(&server.state.data_dir, &query.path).await {
        Ok(path) => path,
        Err(error) => return crate::error_response(error),
    };
    let file = match File::open(&resolved).await {
        Ok(file) => file,
        Err(error) => return crate::error_response(AppError::Io(error)),
    };
    let name = resolved
        .file_name()
        .map(|name| name.to_string_lossy().replace('"', ""))
        .unwrap_or_else(|| "download".to_string());
    let body = Body::from_stream(ReaderStream::new(file));
    let headers: [(String, String); 2] = [
        (
            "content-type".to_string(),
            content_type(&resolved).to_string(),
        ),
        (
            "content-disposition".to_string(),
            format!("attachment; filename=\"{name}\""),
        ),
    ];
    (headers, body).into_response()
}

/// The real path of `requested`, only when it is a file under `data_dir`.
async fn resolve_download(data_dir: &Path, requested: &str) -> Result<PathBuf, AppError> {
    let base = tokio::fs::canonicalize(data_dir)
        .await
        .map_err(|_| AppError::Invalid("the data directory does not exist yet".into()))?;
    let candidate = PathBuf::from(requested);
    if !candidate.is_absolute() {
        return Err(AppError::Invalid(
            "the download path must be absolute".into(),
        ));
    }
    let resolved = tokio::fs::canonicalize(&candidate)
        .await
        .map_err(|_| AppError::NotFound(format!("file {requested}")))?;
    if !resolved.starts_with(&base) || !resolved.is_file() {
        return Err(AppError::Invalid(
            "only files inside the data directory can be downloaded".into(),
        ));
    }
    Ok(resolved)
}

/// The base name only, with shell-hostile characters replaced.
fn sanitize_filename(raw: &str) -> String {
    let base = raw
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(raw)
        .trim()
        .to_string();
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ' | '(' | ')') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches(['.', ' ']).to_string();
    if trimmed.is_empty() {
        "document".to_string()
    } else {
        trimmed
    }
}

fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("pdf") => "application/pdf",
        Some("epub") => "application/epub+zip",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("docx") => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        Some("zip" | "llmtz" | "llmtsz") => "application/zip",
        Some("md" | "markdown" | "txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::{content_type, resolve_download, sanitize_filename};
    use std::path::PathBuf;

    #[test]
    fn a_filename_keeps_its_base_and_loses_hostile_characters() {
        assert_eq!(sanitize_filename("../../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("D'un monde.epub"), "D_un monde.epub");
        assert_eq!(sanitize_filename("..."), "document");
        assert_eq!(sanitize_filename("libro (1).pdf"), "libro (1).pdf");
    }

    #[test]
    fn content_type_covers_the_export_formats() {
        assert_eq!(content_type(&PathBuf::from("a.pdf")), "application/pdf");
        assert_eq!(
            content_type(&PathBuf::from("a.epub")),
            "application/epub+zip"
        );
        assert_eq!(content_type(&PathBuf::from("a.llmtz")), "application/zip");
        assert_eq!(
            content_type(&PathBuf::from("a.unknown")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn downloads_stay_inside_the_data_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let inside = dir.path().join("output.epub");
        std::fs::write(&inside, "book").expect("write");
        let resolved = resolve_download(dir.path(), inside.to_str().expect("path"))
            .await
            .expect("inside");
        assert_eq!(resolved, std::fs::canonicalize(&inside).expect("canonical"));

        // A sibling file is refused even if its path is absolute.
        let outside = tempfile::tempdir().expect("temp dir");
        let foreign = outside.path().join("secret.txt");
        std::fs::write(&foreign, "secret").expect("write");
        assert!(
            resolve_download(dir.path(), foreign.to_str().expect("path"))
                .await
                .is_err()
        );
        // A relative path is refused, too.
        assert!(resolve_download(dir.path(), "relative.epub").await.is_err());
    }
}
