//! Worker pool: claims jobs, bounds concurrency with a semaphore, heartbeats the
//! lease, and can be started / paused / cancelled from the Tauri commands.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::SqlitePool;
use tokio::sync::Semaphore;

use super::{lease, queue};
use crate::db::models::Job;
use crate::error::Result;

/// How a job is executed. Implemented by the pipeline in the app layer; tests
/// supply a stub.
#[async_trait]
pub trait JobDispatcher: Send + Sync + 'static {
    async fn dispatch(&self, job: &Job) -> Result<()>;
}

/// Bound on the number of concurrent job executions for an endpoint. The permit
/// count comes from the resource governor (`resources`/`props`).
pub struct WorkerPool {
    inner: Arc<Inner>,
}

struct Inner {
    pool: SqlitePool,
    dispatcher: Arc<dyn JobDispatcher>,
    owner: String,
    permits: Arc<Semaphore>,
    worker_count: usize,
    paused: AtomicBool,
    cancelled: AtomicBool,
    running: AtomicBool,
    workers: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl WorkerPool {
    /// `max_parallel` is the endpoint concurrency cap; `worker_count` is how many
    /// claim loops run (>= `max_parallel` is typical).
    pub fn new(
        pool: SqlitePool,
        dispatcher: Arc<dyn JobDispatcher>,
        max_parallel: usize,
        worker_count: usize,
    ) -> Self {
        let owner = format!("worker-{}", crate::db::new_id());
        Self {
            inner: Arc::new(Inner {
                pool,
                dispatcher,
                owner,
                permits: Arc::new(Semaphore::new(max_parallel.max(1))),
                worker_count: worker_count.max(1),
                paused: AtomicBool::new(false),
                cancelled: AtomicBool::new(false),
                running: AtomicBool::new(false),
                workers: parking_lot::Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn is_running(&self) -> bool {
        self.inner.running.load(Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.inner.paused.load(Ordering::SeqCst)
    }

    /// Current concurrency cap.
    pub fn max_parallel(&self) -> usize {
        self.inner.permits.available_permits()
    }

    /// Start the worker loops. Idempotent: calling it while running is a no-op.
    pub fn start(&self) {
        if self.inner.running.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.cancelled.store(false, Ordering::SeqCst);
        self.inner.paused.store(false, Ordering::SeqCst);

        let worker_count = self.inner.worker_count.max(1);
        let mut handles = self.inner.workers.lock();

        // The lease reaper gets its own task: if it shared the claim loop, waiting
        // for the next reap interval would stall that worker between jobs.
        let reaper_inner = self.inner.clone();
        handles.push(tokio::spawn(async move { reap_loop(reaper_inner).await }));

        for _ in 0..worker_count {
            let inner = self.inner.clone();
            handles.push(tokio::spawn(async move { worker_loop(inner).await }));
        }
    }

    /// Stop claiming new jobs; running jobs finish.
    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::SeqCst);
    }

    /// Resume claiming.
    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::SeqCst);
    }

    /// Stop the pool and abort the worker loops. Running jobs are left to the
    /// reaper (their lease will expire while `cancelled` is true is fine: the
    /// reaper returns them to `pending`).
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        self.inner.running.store(false, Ordering::SeqCst);
        let mut handles = self.inner.workers.lock();
        for handle in handles.drain(..) {
            handle.abort();
        }
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Returns expired leases to `pending` on its own schedule.
async fn reap_loop(inner: Arc<Inner>) {
    let mut ticker = tokio::time::interval(Duration::from_secs(15));
    loop {
        // The first tick completes immediately, so the pool reaps once at start-up.
        ticker.tick().await;
        if inner.cancelled.load(Ordering::SeqCst) {
            break;
        }
        if let Err(error) = queue::reap_expired(&inner.pool).await {
            tracing::warn!(%error, "lease reaper failed");
        }
    }
}

async fn worker_loop(inner: Arc<Inner>) {
    loop {
        if inner.cancelled.load(Ordering::SeqCst) {
            break;
        }
        if inner.paused.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        }

        // Reserve execution capacity before claiming, so a claimed job is never
        // left waiting for a permit while its lease ticks down.
        let Ok(permit) = inner.permits.clone().acquire_owned().await else {
            break;
        };

        match queue::claim(&inner.pool, &inner.owner).await {
            Ok(Some(job)) => {
                run_job(&inner, job).await;
                drop(permit);
            }
            Ok(None) => {
                drop(permit);
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(error) => {
                drop(permit);
                tracing::warn!(%error, "job claim failed");
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

async fn run_job(inner: &Arc<Inner>, job: Job) {
    if let Err(error) = queue::mark_running(&inner.pool, &job.id).await {
        tracing::warn!(job_id = %job.id, %error, "could not mark job running");
        return;
    }
    let heartbeat = lease::spawn_heartbeat(
        inner.pool.clone(),
        job.id.clone(),
        inner.owner.clone(),
        lease::HEARTBEAT_INTERVAL,
    );

    let outcome = inner.dispatcher.dispatch(&job).await;
    heartbeat.stop();

    match outcome {
        Ok(()) => {
            if let Err(error) = queue::complete(&inner.pool, &job.id).await {
                tracing::warn!(job_id = %job.id, %error, "could not complete job");
            }
        }
        Err(error) => {
            let message = error.to_string();
            match queue::retry_or_fail(&inner.pool, &job.id, &message).await {
                Ok(state) => {
                    tracing::warn!(job_id = %job.id, state, %message, "job attempt failed");
                }
                Err(record_error) => {
                    tracing::error!(job_id = %job.id, %record_error, "could not record job failure");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect_temp_file, now};
    use std::sync::atomic::AtomicU32;

    struct CountingDispatcher {
        executions: Arc<AtomicU32>,
        fail_from: u32,
    }

    #[async_trait]
    impl JobDispatcher for CountingDispatcher {
        async fn dispatch(&self, _job: &Job) -> Result<()> {
            let n = self.executions.fetch_add(1, Ordering::SeqCst);
            if n >= self.fail_from {
                return Err(crate::error::AppError::Job("stub failure".into()));
            }
            Ok(())
        }
    }

    async fn seed_project(pool: &SqlitePool) {
        sqlx::query(
            "INSERT INTO project (id, name, source_path, source_hash, source_format, target_lang, \
             settings_json, created_at, updated_at) VALUES ('p','p','/x','h','epub','it','{}',?1,?1)",
        )
        .bind(now())
        .execute(pool)
        .await
        .expect("project");
    }

    #[tokio::test]
    async fn pool_processes_jobs_and_marks_them_done() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        seed_project(&pool).await;
        for _ in 0..5 {
            queue::enqueue(
                &pool,
                &queue::NewJob::new("p", "translate_chunk", serde_json::Value::Null),
            )
            .await
            .expect("enqueue");
        }

        let executions = Arc::new(AtomicU32::new(0));
        let dispatcher = Arc::new(CountingDispatcher {
            executions: executions.clone(),
            fail_from: u32::MAX,
        });
        let worker = WorkerPool::new(pool.clone(), dispatcher, 2, 2);
        worker.start();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let states = queue::count_by_state(&pool).await.expect("count");
            let done = states
                .iter()
                .find(|(state, _)| state == "done")
                .map(|(_, n)| *n)
                .unwrap_or(0);
            if done == 5 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "jobs did not finish"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(executions.load(Ordering::SeqCst), 5);
        worker.cancel();
    }

    #[tokio::test]
    async fn failed_jobs_are_retried_then_marked_failed() {
        let (pool, _dir) = connect_temp_file().await.expect("pool");
        seed_project(&pool).await;
        queue::enqueue(
            &pool,
            &queue::NewJob::new("p", "translate_chunk", serde_json::Value::Null),
        )
        .await
        .expect("enqueue");

        let dispatcher = Arc::new(CountingDispatcher {
            executions: Arc::new(AtomicU32::new(0)),
            fail_from: 0,
        });
        let worker = WorkerPool::new(pool.clone(), dispatcher, 1, 1);
        worker.start();

        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let states = queue::count_by_state(&pool).await.expect("count");
            if states.iter().any(|(state, _)| state == "failed") {
                break;
            }
            assert!(tokio::time::Instant::now() < deadline, "job never failed");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        worker.cancel();
    }
}
