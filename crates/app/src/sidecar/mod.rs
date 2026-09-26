//! Python sidecar bridge: JSON-RPC 2.0 over NDJSON on stdio.

pub mod rpc;
pub mod supervisor;

pub use rpc::{
    BuildChunksResult, ChapterInfo, DetectFormatResult, EstimateTokensResult, EventSink,
    IngestResult, PandocParams, PandocResult, PandocUnit, ParseDocumentResult, PingResult,
    PrepareTextResult, QaCheckResult, ReinjectResult, SidecarClient, SidecarFinding,
};
pub use supervisor::{
    bundled_sidecar_path, resolve_spawn_spec, SidecarState, SidecarStatus, SpawnSpec, Supervisor,
};
