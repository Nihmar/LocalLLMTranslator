//! SQLite persistence layer.
//!
//! Queries are runtime-checked (`sqlx::query`) on purpose: it keeps a live
//! database out of the build, as required by `AGENTS.md`. Migrations are
//! embedded from `migrations/`.

pub mod models;
pub mod repo;

use std::path::Path;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::error::Result;

/// The embedded migrator. Path is relative to this crate's manifest directory.
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Default connection-pool size.
const POOL_MAX_CONNECTIONS: u32 = 8;

/// Open (creating if needed) the SQLite database at `path`, enable foreign key
/// enforcement, and run pending migrations.
pub async fn connect(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(15));
    let pool = SqlitePoolOptions::new()
        .max_connections(POOL_MAX_CONNECTIONS)
        .acquire_timeout(Duration::from_secs(15))
        .connect_with(options)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// Open a private, single-connection in-memory database with migrations applied.
///
/// A single connection is mandatory for `:memory:` so that every query sees the
/// same database.
pub async fn connect_memory() -> Result<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .in_memory(true)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// Open a temporary *file* database. Needed by tests that must exercise real
/// concurrent connections (in-memory SQLite is per-connection).
#[cfg(test)]
pub async fn connect_temp_file() -> Result<(SqlitePool, tempfile::TempDir)> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("test.sqlite");
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(15));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .acquire_timeout(Duration::from_secs(15))
        .connect_with(options)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok((pool, dir))
}

/// RFC3339 UTC timestamp with millisecond precision and a trailing `Z`.
///
/// A single canonical format matters: the schema stores timestamps as `TEXT`
/// and ordering (`ORDER BY created_at`, lease expiry) relies on lexicographic
/// comparison.
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A canonical timestamp `secs` seconds in the future.
pub fn now_plus_secs(secs: i64) -> String {
    (chrono::Utc::now() + chrono::Duration::seconds(secs))
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// New random identifier (v4 UUID, hex without dashes).
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
