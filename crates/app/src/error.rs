//! Domain error type for the control plane.
//!
//! `thiserror` is used for domain errors; `anyhow` is only allowed at the very
//! edge of the command handlers (see `AGENTS.md`). The type is `Serialize` so it
//! can be returned directly from Tauri commands.

use thiserror::Error;

/// Convenience alias used throughout the crate.
pub type Result<T> = std::result::Result<T, AppError>;

/// Every fallible operation in the control plane funnels through this type.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),

    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("archive error: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("template error: {0}")]
    Template(#[from] minijinja::Error),

    #[error("sidecar request timed out after {0:?}")]
    SidecarTimeout(std::time::Duration),

    /// The sidecar is stateless, so any failure here is safe to retry: the
    /// request is re-issued on the fresh process.
    #[error("sidecar unavailable (retryable): {0}")]
    SidecarUnavailable(String),

    #[error("sidecar error {code}: {message}")]
    Sidecar {
        code: i64,
        message: String,
        retryable: bool,
    },

    #[error("endpoint returned {status}: {body}")]
    Endpoint { status: u16, body: String },

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("job error: {0}")]
    Job(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    /// Errors that may be re-issued without side effects. The Python sidecar is
    /// stateless and `llama-server` calls are pure request/response, so transport
    /// level failures are always retryable.
    pub fn retryable(&self) -> bool {
        match self {
            AppError::SidecarTimeout(_) | AppError::SidecarUnavailable(_) | AppError::Http(_) => {
                true
            }
            AppError::Sidecar { retryable, .. } => *retryable,
            _ => false,
        }
    }

    /// Stable machine readable code, surfaced to the UI alongside the message.
    pub fn code(&self) -> &'static str {
        match self {
            AppError::Db(_) => "db",
            AppError::Migration(_) => "migration",
            AppError::Http(_) => "http",
            AppError::Io(_) => "io",
            AppError::Json(_) => "json",
            AppError::Zip(_) => "zip",
            AppError::Template(_) => "template",
            AppError::SidecarTimeout(_) => "sidecar_timeout",
            AppError::SidecarUnavailable(_) => "sidecar_unavailable",
            AppError::Sidecar { .. } => "sidecar",
            AppError::Endpoint { .. } => "endpoint",
            AppError::NotFound(_) => "not_found",
            AppError::Invalid(_) => "invalid",
            AppError::Job(_) => "job",
            AppError::Other(_) => "other",
        }
    }
}

impl serde::Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("AppError", 3)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.serialize_field("retryable", &self.retryable())?;
        state.end()
    }
}
