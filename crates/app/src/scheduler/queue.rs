//! Job queue: enqueue, claim, heartbeat, complete/fail, reap.
//!
//! The claim statement is the one specified in PLAN.md section 6.

use serde_json::Value;
use sqlx::SqlitePool;

use crate::db::models::Job;
use crate::db::{new_id, now, now_plus_secs};
use crate::error::Result;

/// Lease duration in seconds (PLAN.md section 6).
pub const LEASE_SECS: i64 = 90;

/// The exact claim SQL from PLAN.md section 6:
/// `UPDATE job SET state='leased', lease_owner=?, lease_expires_at=now+90s
///  WHERE id = (SELECT id FROM job WHERE state='pending' AND run_after<=now
///  ORDER BY priority, created_at LIMIT 1) RETURNING *`.
///
/// `run_after IS NULL` is treated as immediately eligible, and `now` is a bound
/// parameter because SQLite has no clock function with a stable format.
pub const CLAIM_SQL: &str = r#"
UPDATE job SET state = 'leased', lease_owner = ?1, lease_expires_at = ?2
WHERE id = (
    SELECT id FROM job
    WHERE state = 'pending' AND (run_after IS NULL OR run_after <= ?3)
    ORDER BY priority, created_at
    LIMIT 1
)
RETURNING *"#;

/// A job to enqueue.
#[derive(Debug, Clone)]
pub struct NewJob {
    pub project_id: String,
    pub kind: String,
    pub payload: Value,
    pub priority: i64,
    pub max_attempts: i64,
    pub run_after: Option<String>,
}

impl NewJob {
    pub fn new(project_id: impl Into<String>, kind: impl Into<String>, payload: Value) -> Self {
        Self {
            project_id: project_id.into(),
            kind: kind.into(),
            payload,
            priority: 100,
            max_attempts: 3,
            // Eligible immediately.
            run_after: Some(now()),
        }
    }

    pub fn with_priority(mut self, priority: i64) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_run_after(mut self, run_after: Option<String>) -> Self {
        self.run_after = run_after;
        self
    }
}

/// Insert a new pending job and return its id.
pub async fn enqueue(pool: &SqlitePool, job: &NewJob) -> Result<String> {
    let id = new_id();
    sqlx::query(
        "INSERT INTO job (id, project_id, kind, payload_json, priority, state, attempts, \
         max_attempts, run_after, created_at) VALUES (?1,?2,?3,?4,?5,'pending',0,?6,?7,?8)",
    )
    .bind(id.as_str())
    .bind(job.project_id.as_str())
    .bind(job.kind.as_str())
    .bind(serde_json::to_string(&job.payload)?)
    .bind(job.priority)
    .bind(job.max_attempts)
    .bind(job.run_after.as_deref())
    .bind(now())
    .execute(pool)
    .await?;
    Ok(id)
}

/// Claim the next eligible job, or `None` when the queue is empty.
pub async fn claim(pool: &SqlitePool, owner: &str) -> Result<Option<Job>> {
    let expires_at = now_plus_secs(LEASE_SECS);
    let claimed = sqlx::query_as::<_, Job>(CLAIM_SQL)
        .bind(owner)
        .bind(expires_at)
        .bind(now())
        .fetch_optional(pool)
        .await?;
    Ok(claimed)
}

/// Move a claimed job to `running` (called when a worker actually picks it up).
pub async fn mark_running(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE job SET state='running', started_at=COALESCE(started_at, ?2) WHERE id=?1")
        .bind(id)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(())
}

/// Renew the lease while the job runs. Returns false if the lease was lost.
pub async fn heartbeat(pool: &SqlitePool, id: &str, owner: &str) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE job SET lease_expires_at = ?3 WHERE id = ?1 AND lease_owner = ?2 \
         AND state IN ('leased','running')",
    )
    .bind(id)
    .bind(owner)
    .bind(now_plus_secs(LEASE_SECS))
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// Mark a job done. Idempotent: re-running it leaves the row in `done`.
pub async fn complete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE job SET state='done', finished_at=?2, lease_owner=NULL, lease_expires_at=NULL, \
         last_error=NULL WHERE id=?1",
    )
    .bind(id)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a job failed permanently.
pub async fn fail(pool: &SqlitePool, id: &str, error: &str) -> Result<()> {
    sqlx::query(
        "UPDATE job SET state='failed', finished_at=?2, last_error=?3, lease_owner=NULL, \
         lease_expires_at=NULL, attempts=attempts+1 WHERE id=?1",
    )
    .bind(id)
    .bind(now())
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// Requeue a failed attempt, or fail it when the attempt budget is exhausted.
/// Returns the resulting state (`pending` or `failed`).
pub async fn retry_or_fail(pool: &SqlitePool, id: &str, error: &str) -> Result<String> {
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE job SET \
           state = CASE WHEN attempts + 1 < max_attempts THEN 'pending' ELSE 'failed' END, \
           attempts = attempts + 1, last_error = ?2, lease_owner = NULL, lease_expires_at = NULL, \
           run_after = ?3, \
           finished_at = CASE WHEN attempts + 1 < max_attempts THEN NULL ELSE ?4 END \
         WHERE id = ?1 RETURNING state",
    )
    .bind(id)
    .bind(error)
    .bind(now())
    .bind(now())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0).unwrap_or_else(|| "missing".to_string()))
}

/// Return expired `leased`/`running` jobs to `pending`, incrementing `attempts`.
/// Returns the number of jobs reaped.
pub async fn reap_expired(pool: &SqlitePool) -> Result<u64> {
    let res = sqlx::query(
        "UPDATE job SET state='pending', lease_owner=NULL, lease_expires_at=NULL, \
         attempts=attempts+1, last_error='lease expired' \
         WHERE state IN ('leased','running') AND lease_expires_at IS NOT NULL \
         AND lease_expires_at < ?1",
    )
    .bind(now())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Cancel a job that has not finished yet.
pub async fn cancel(pool: &SqlitePool, id: &str) -> Result<bool> {
    let res = sqlx::query(
        "UPDATE job SET state='cancelled', finished_at=?2, lease_owner=NULL, lease_expires_at=NULL \
         WHERE id=?1 AND state IN ('pending','leased','running')",
    )
    .bind(id)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(res.rows_affected() > 0)
}

/// List jobs, optionally filtered by state and/or project.
pub async fn list_jobs(
    pool: &SqlitePool,
    project_id: Option<&str>,
    state: Option<&str>,
    limit: i64,
) -> Result<Vec<Job>> {
    let rows = sqlx::query_as::<_, Job>(
        "SELECT * FROM job WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR state = ?2) \
         ORDER BY created_at DESC LIMIT ?3",
    )
    .bind(project_id)
    .bind(state)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Count jobs grouped by state (used by `metrics_get`).
pub async fn count_by_state(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT state, COUNT(*) FROM job GROUP BY state")
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::connect_temp_file;
    use std::collections::HashSet;

    async fn insert_test_project(pool: &SqlitePool, id: &str) {
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, target_lang, \
             settings_json, created_at, updated_at) VALUES (?1,'p','/x','h','epub','it','{}',?2,?2)",
        )
        .bind(id)
        .bind(now())
        .execute(pool)
        .await
        .expect("project");
    }

    #[tokio::test]
    async fn claim_respects_priority_order() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        insert_test_project(&pool, "proj").await;

        enqueue(
            &pool,
            &NewJob::new("proj", "translate_chunk", Value::Null).with_priority(100),
        )
        .await
        .expect("enqueue");
        enqueue(
            &pool,
            &NewJob::new("proj", "translate_chunk", Value::Null).with_priority(10),
        )
        .await
        .expect("enqueue");
        enqueue(
            &pool,
            &NewJob::new("proj", "translate_chunk", Value::Null).with_priority(50),
        )
        .await
        .expect("enqueue");

        let first = claim(&pool, "w1").await.expect("claim").expect("some");
        assert_eq!(first.priority, 10);
        let second = claim(&pool, "w1").await.expect("claim").expect("some");
        assert_eq!(second.priority, 50);
        let third = claim(&pool, "w1").await.expect("claim").expect("some");
        assert_eq!(third.priority, 100);
        assert!(claim(&pool, "w1").await.expect("claim").is_none());
    }

    #[tokio::test]
    async fn reap_returns_expired_lease_and_increments_attempts() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        insert_test_project(&pool, "proj").await;
        let id = enqueue(&pool, &NewJob::new("proj", "translate_chunk", Value::Null))
            .await
            .expect("enqueue");

        let claimed = claim(&pool, "w1").await.expect("claim").expect("some");
        assert_eq!(claimed.state, "leased");
        assert_eq!(claimed.attempts, 0);

        // Force the lease into the past.
        sqlx::query("UPDATE job SET state='running', lease_expires_at='2000-01-01T00:00:00.000Z' WHERE id=?1")
            .bind(id.as_str())
            .execute(&pool)
            .await
            .expect("expire");

        let reaped = reap_expired(&pool).await.expect("reap");
        assert_eq!(reaped, 1);

        let job = sqlx::query_as::<_, Job>("SELECT * FROM job WHERE id=?1")
            .bind(id.as_str())
            .fetch_one(&pool)
            .await
            .expect("get");
        assert_eq!(job.state, "pending");
        assert_eq!(job.attempts, 1);
        assert!(job.lease_owner.is_none());
    }

    #[tokio::test]
    async fn reaping_ignores_live_leases() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        insert_test_project(&pool, "proj").await;
        enqueue(&pool, &NewJob::new("proj", "translate_chunk", Value::Null))
            .await
            .expect("enqueue");
        claim(&pool, "w1").await.expect("claim");
        assert_eq!(reap_expired(&pool).await.expect("reap"), 0);
    }

    #[tokio::test]
    async fn heartbeat_renews_and_retry_respects_max_attempts() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        insert_test_project(&pool, "proj").await;
        let id = enqueue(&pool, &NewJob::new("proj", "translate_chunk", Value::Null))
            .await
            .expect("enqueue");
        claim(&pool, "w1").await.expect("claim");
        assert!(heartbeat(&pool, &id, "w1").await.expect("hb"));
        assert!(!heartbeat(&pool, &id, "someone-else").await.expect("hb"));

        // max_attempts defaults to 3: 0 -> 1 -> 2 requeue, 3 -> failed.
        assert_eq!(
            retry_or_fail(&pool, &id, "boom").await.expect("retry"),
            "pending"
        );
        assert_eq!(
            retry_or_fail(&pool, &id, "boom").await.expect("retry"),
            "pending"
        );
        assert_eq!(
            retry_or_fail(&pool, &id, "boom").await.expect("retry"),
            "failed"
        );
    }

    #[tokio::test]
    async fn re_running_a_completed_job_is_idempotent() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        insert_test_project(&pool, "proj").await;
        let id = enqueue(&pool, &NewJob::new("proj", "translate_chunk", Value::Null))
            .await
            .expect("enqueue");
        complete(&pool, &id).await.expect("complete");
        complete(&pool, &id).await.expect("complete again");
        let job = sqlx::query_as::<_, Job>("SELECT * FROM job WHERE id=?1")
            .bind(id.as_str())
            .fetch_one(&pool)
            .await
            .expect("get");
        assert_eq!(job.state, "done");
        // A done job is not claimable again.
        assert!(claim(&pool, "w1").await.expect("claim").is_none());
    }

    #[tokio::test]
    async fn concurrent_claims_never_return_the_same_job() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        insert_test_project(&pool, "proj").await;
        const JOBS: usize = 40;
        for _ in 0..JOBS {
            enqueue(&pool, &NewJob::new("proj", "translate_chunk", Value::Null))
                .await
                .expect("enqueue");
        }

        let mut handles = Vec::new();
        for worker in 0..8 {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                let owner = format!("w{worker}");
                let mut claimed = Vec::new();
                while let Some(job) = claim(&pool, &owner).await.expect("claim") {
                    claimed.push(job.id);
                }
                claimed
            }));
        }

        let mut all = Vec::new();
        for handle in handles {
            all.extend(handle.await.expect("join"));
        }
        let unique: HashSet<_> = all.iter().cloned().collect();
        assert_eq!(all.len(), JOBS, "every job must be claimed exactly once");
        assert_eq!(unique.len(), JOBS, "no job may be claimed twice");
    }
}
