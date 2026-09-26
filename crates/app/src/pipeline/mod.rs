//! Pipeline: ingestion, translation and export orchestration.

pub mod export;
pub mod ingest;
pub mod json_call;
pub mod recon;
pub mod summarize;
pub mod translate;

use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::resources::ResourceGovernor;
use crate::sidecar::SidecarClient;

/// Shared dependencies handed to every pipeline stage.
#[derive(Clone)]
pub struct PipelineDeps {
    pub pool: SqlitePool,
    pub sidecar: SidecarClient,
    pub resources: ResourceGovernor,
    pub data_dir: PathBuf,
}

impl PipelineDeps {
    pub fn new(
        pool: SqlitePool,
        sidecar: SidecarClient,
        resources: ResourceGovernor,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            pool,
            sidecar,
            resources,
            data_dir,
        }
    }

    fn project_dir(&self, project_id: &str) -> PathBuf {
        self.data_dir.join("projects").join(project_id)
    }

    /// Working directory handed to the sidecar for one project.
    pub fn work_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("work")
    }

    /// Output directory for Pandoc builds.
    pub fn output_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("output")
    }

    /// Immutable copy of the prompts used for a project.
    pub fn prompts_dir(&self, project_id: &str) -> PathBuf {
        self.project_dir(project_id).join("prompts")
    }
}
