//! Lease helpers: background heartbeat while a job is executing.

use std::time::Duration;

use sqlx::SqlitePool;
use tokio::sync::watch;

use super::queue;

/// Renew well inside the 90 second lease.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);

/// Handle to a running heartbeat task. Dropping it stops the heartbeat.
pub struct LeaseHandle {
    cancel: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
}

impl LeaseHandle {
    /// Stop the heartbeat (idempotent).
    pub fn stop(self) {
        let _ = self.cancel.send(true);
        self.handle.abort();
    }
}

/// Spawn a task that renews the lease of `job_id` every `interval` until stopped
/// or until the lease is lost.
pub fn spawn_heartbeat(
    pool: SqlitePool,
    job_id: String,
    owner: String,
    interval: Duration,
) -> LeaseHandle {
    let (cancel, mut rx) = watch::channel(false);
    let handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; skip it so we do not renew before
        // the job has even started.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    match queue::heartbeat(&pool, &job_id, &owner).await {
                        Ok(true) => {}
                        Ok(false) => {
                            tracing::warn!(job_id, "job lease lost; stopping heartbeat");
                            break;
                        }
                        Err(error) => {
                            tracing::warn!(job_id, %error, "lease heartbeat failed");
                        }
                    }
                }
                _ = rx.changed() => break,
            }
        }
    });
    LeaseHandle { cancel, handle }
}
