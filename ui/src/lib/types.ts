/**
 * Domain types for the `UI -> Tauri` boundary.
 *
 * Every shape here mirrors the Rust control plane one-to-one: the command signatures and
 * the DTOs in `crates/app/src/commands/*.rs`, and the row types in
 * `crates/app/src/db/models.rs`. Rust is the source of truth; when a shape and this file
 * disagree, the file is wrong.
 *
 * Two conventions from the Rust side are reproduced faithfully:
 *
 * 1. Serde uses the **declared field names unchanged**: there is no `rename_all`, so every
 *    field is `snake_case`, exactly as the database columns and the JSON-RPC payloads.
 * 2. A Rust `Option<T>` serialises as `T | null`; `serde(default)` fields that are not
 *    `skip_serializing_if` are still emitted, so the `| null` is real, not just "may be absent".
 *
 * `lib/ipc.ts` is the only module allowed to mention a command name; `lib/events.ts` the only
 * one allowed to mention an event name.
 */

/** JSON value that survived deserialisation but is not modelled further by the UI. */
export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { readonly [key: string]: JsonValue };
export type JsonObject = { readonly [key: string]: JsonValue };

// --- enums shared with the backend ---------------------------------------------------------

/**
 * `source_format` is a free-form `String` column on the Rust side; the UI keeps this union for
 * the extension-based estimate it shows before the sidecar reports the authoritative value.
 */
export type SourceFormat = "epub" | "pdf" | "markdown" | "unknown";

/** PDF extraction backend accepted by `ingest_start` (`PLAN.md` §1). */
export type PdfBackend = "auto" | "pymupdf4llm" | "marker";

/** `role_binding.role` (`PLAN.md` §5). */
export type Role = "translator" | "editor" | "proofreader" | "orchestrator";

/** `chunk.status` (`PLAN.md` §5). */
export type ChunkStatus = "pending" | "running" | "done" | "failed" | "needs_review";

/** `job.state` (`PLAN.md` §5). */
export type JobState = "pending" | "leased" | "running" | "done" | "failed" | "cancelled";

/** `job.kind` (`PLAN.md` §5). */
export type JobKind =
  | "ingest"
  | "translate_chunk"
  | "summarize"
  | "book_recon"
  | "edit_chunk"
  | "proofread_chunk"
  | "qa_scan"
  | "export_unit";

/** `block.kind`, as in `PLAN.md` §4.1 and §12.1. */
export type BlockKind =
  | "heading"
  | "para"
  | "list"
  | "blockquote"
  | "table"
  | "code"
  | "figure"
  | "footnote_def"
  | "hr"
  | "html";

/** Log levels emitted on `log://line`, matching the Rust `tracing` levels. */
export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

/**
 * Sidecar supervisor state (`crates/app/src/sidecar/supervisor.rs::SidecarState`, serde
 * `snake_case`). Reported by `sidecar_status` and on `sidecar://status`.
 */
import type { SidecarState } from "./generated/SidecarState.ts";
export type { SidecarState };

/**
 * Why the resource governor capped the parallel degree
 * (`crates/app/src/resources/vram.rs::ParallelReason`, serde `snake_case`).
 */
import type { ParallelReason } from "./generated/ParallelReason.ts";
export type { ParallelReason };

/** Output formats offered by the Pandoc driver. */
export type ExportFormat = "pdf" | "epub" | "docx";

/** Generic acknowledgement returned by the mutating commands (`commands::Ack`). */
import type { Ack } from "./generated/Ack.ts";
export type { Ack };
// --- projects ------------------------------------------------------------------------------

/** Row of `project` (`db::models::Project`); `settings_json` is the raw column. */
import type { Project } from "./generated/Project.ts";
export type { Project };

/** Request body of `project_create` (`commands::CreateProjectRequest`). */
import type { CreateProjectRequest } from "./generated/CreateProjectRequest.ts";
export type { CreateProjectRequest };
/** Row of `chapter` (`db::models::Chapter`). */
import type { Chapter } from "./generated/Chapter.ts";
export type { Chapter };

/** Result of `project_get` (`commands::project::ProjectDetail`). */
import type { ProjectDetail } from "./generated/ProjectDetail.ts";
export type { ProjectDetail };
/** Request body of `project_export` (`commands::project::ExportBundleRequest`). */
import type { ExportBundleRequest } from "./generated/ExportBundleRequest.ts";
export type { ExportBundleRequest };
/** Result of `project_export` (`pipeline::bundle::ExportBundleOutcome`). */
import type { ExportBundleOutcome } from "./generated/ExportBundleOutcome.ts";
export type { ExportBundleOutcome };
/** Request body of `project_import` (`commands::project::ImportBundleRequest`). */
import type { ImportBundleRequest } from "./generated/ImportBundleRequest.ts";
export type { ImportBundleRequest };
// --- LLM endpoints -------------------------------------------------------------------------

/** Row of `llm_endpoint` (`db::models::LlmEndpoint`); `props_json` is the raw `/props` body. */
import type { LlmEndpoint as Endpoint } from "./generated/LlmEndpoint.ts";
export type { Endpoint };

/** Request body of `endpoint_upsert` (`commands::endpoint::EndpointUpsert`); `id` absent = create. */
import type { EndpointUpsert } from "./generated/EndpointUpsert.ts";
export type { EndpointUpsert };
/** Request body of `endpoint_models` (`commands::endpoint::EndpointModelsRequest`). */
import type { EndpointModelsRequest } from "./generated/EndpointModelsRequest.ts";
export type { EndpointModelsRequest };
/** Result of a single `GET /health` probe (`llm::health::EndpointHealth`). */
import type { EndpointHealth } from "./generated/EndpointHealth.ts";
export type { EndpointHealth };
/** `GET /props` (`llm::types::Props`). */
import type { Props } from "./generated/Props.ts";
export type { Props };
/** One entry of `GET /v1/models` (`llm::types::ModelInfo`). */
import type { ModelInfo } from "./generated/ModelInfo.ts";
export type { ModelInfo };
/** Result of `endpoint_test` (`commands::endpoint::EndpointTestResult`). */
import type { EndpointTestResult } from "./generated/EndpointTestResult.ts";
export type { EndpointTestResult };
// --- role bindings -------------------------------------------------------------------------

/** Row of `role_binding` (`db::models::RoleBinding`); `params_json` is the raw column. */
import type { RoleBinding } from "./generated/RoleBinding.ts";
export type { RoleBinding };

/** Request body of `role_binding_set` (`commands::role_binding::RoleBindingSet`). */
import type { RoleBindingSet } from "./generated/RoleBindingSet.ts";
export type { RoleBindingSet };
// --- ingestion -----------------------------------------------------------------------------

/** Request body of `ingest_start` (`commands::ingest::IngestStartRequest`). */
import type { IngestStartRequest } from "./generated/IngestStartRequest.ts";
export type { IngestStartRequest };
/** Result of `document_inspect` (`sidecar::DetectFormatResult`). */
import type { DetectFormatResult as DocumentInspection } from "./generated/DetectFormatResult.ts";
export type { DocumentInspection };
/** Result of `ingest_start` (`commands::ingest::JobStarted`). */
import type { JobStarted } from "./generated/JobStarted.ts";
export type { JobStarted };
// --- translation control -------------------------------------------------------------------

/** Request body of `translation_start` (`commands::translation::TranslationStartRequest`). */
import type { TranslationStartRequest } from "./generated/TranslationStartRequest.ts";
export type { TranslationStartRequest };
/** Result of `translation_start` (`commands::translation::TranslationStartResult`). */
import type { TranslationStartResult } from "./generated/TranslationStartResult.ts";
export type { TranslationStartResult };
// --- book reconnaissance (PLAN.md §9.4) -----------------------------------------------------

/**
 * One profile value with its provenance (`pipeline::recon::ProfileField`). `basis` is one of
 * `from_text`, `metadata`, `inferred` (or `user` for a value typed by hand): an `inferred`
 * field is shown as such and is not confirmed by default.
 */
import type { ProfileField } from "./generated/ProfileField.ts";
export type { ProfileField };
/** A name the profile proposes for the glossary (`pipeline::recon::ProperNoun`). */
import type { ProperNoun } from "./generated/ProperNoun.ts";
export type { ProperNoun };
/** Where the candidate came from (`pipeline::recon::ProfileProvenance`). */
import type { ProfileProvenance } from "./generated/ProfileProvenance.ts";
export type { ProfileProvenance };
/** The candidate book profile (`pipeline::recon::BookProfile`). */
import type { BookProfile } from "./generated/BookProfile.ts";
export type { BookProfile };
/** Row of `glossary_term` (`db::models::GlossaryTerm`). */
import type { GlossaryTerm } from "./generated/GlossaryTerm.ts";
export type { GlossaryTerm };

/** Result of `recon_get` and `recon_confirm` (`pipeline::recon::ReconSnapshot`). */
import type { ReconSnapshot } from "./generated/ReconSnapshot.ts";
export type { ReconSnapshot };
/** `keep` = the source's dash, `quotes` = target-language quotation marks. */
export type DialogueStyle = "keep" | "quotes";

/** Request body of `project_set_dialogue_style` (`commands::recon::DialogueStyleRequest`). */
import type { DialogueStyleRequest } from "./generated/DialogueStyleRequest.ts";
export type { DialogueStyleRequest };
/** Request body of `recon_start` (`commands::recon::ReconStartRequest`). */
import type { ReconStartRequest } from "./generated/ReconStartRequest.ts";
export type { ReconStartRequest };
/** One accepted proper noun in `recon_confirm` (`pipeline::recon::ConfirmedTerm`). */
import type { ConfirmedTerm } from "./generated/ConfirmedTerm.ts";
export type { ConfirmedTerm };
/** Request body of `recon_confirm` (`pipeline::recon::ConfirmRequest`). */
import type { ConfirmRequest as ReconConfirmRequest } from "./generated/ConfirmRequest.ts";
export type { ReconConfirmRequest };
/** Request body of `glossary_upsert` (`commands::glossary::GlossaryUpsert`). */
import type { GlossaryUpsert as GlossaryUpsertRequest } from "./generated/GlossaryUpsert.ts";
export type { GlossaryUpsertRequest };
// --- review and QA (PLAN.md §11.4) ---------------------------------------------------------

/** Row of `suggestion` (`db::models::Suggestion`); `original`/`proposed` are raw strings. */
import type { Suggestion } from "./generated/Suggestion.ts";
export type { Suggestion };

/** Row of `qa_finding` (`db::models::QaFinding`); `details_json` is the raw column. */
import type { QaFinding } from "./generated/QaFinding.ts";
export type { QaFinding };
import type { QaFindingView } from "./generated/QaFindingView.ts";
export type { QaFindingView };

/** Request body of `review_start` (`commands::review::ReviewStartRequest`). */
import type { ReviewStartRequest } from "./generated/ReviewStartRequest.ts";
export type { ReviewStartRequest };
/** Result of `review_start` (`commands::review::ReviewStartResult`). */
import type { ReviewStartResult } from "./generated/ReviewStartResult.ts";
export type { ReviewStartResult };
/** Request body of `suggestion_list` (`commands::review::SuggestionListRequest`). */
import type { SuggestionListRequest } from "./generated/SuggestionListRequest.ts";
export type { SuggestionListRequest };
/** Request body of `suggestion_history` (`commands::review::SuggestionHistoryRequest`). */
import type { SuggestionHistoryRequest } from "./generated/SuggestionHistoryRequest.ts";
export type { SuggestionHistoryRequest };
/** Request body of `qa_report` (`commands::review::QaReportRequest`). */
import type { QaReportRequest } from "./generated/QaReportRequest.ts";
export type { QaReportRequest };
// --- jobs ----------------------------------------------------------------------------------

/** Row of `job` (`db::models::Job`); `payload_json` is the raw column. */
import type { Job } from "./generated/Job.ts";
export type { Job };
import type { JobView } from "./generated/JobView.ts";
export type { JobView };

/** Request body of `job_list` (`commands::jobs::JobListRequest`). */
import type { JobListRequest } from "./generated/JobListRequest.ts";
export type { JobListRequest };
/** Request body of `job_cancel` (`commands::jobs::JobCancelRequest`). */
import type { JobCancelRequest } from "./generated/JobCancelRequest.ts";
export type { JobCancelRequest };
/** Result of `job_cancel` (`commands::jobs::JobCancelResult`). */
import type { JobCancelResult } from "./generated/JobCancelResult.ts";
export type { JobCancelResult };
// --- chunks and blocks ---------------------------------------------------------------------

/** Row of `chunk` (`db::models::Chunk`); the `*_json` columns are raw strings. */
import type { Chunk } from "./generated/Chunk.ts";
export type { Chunk };
import type { ChunkView } from "./generated/ChunkView.ts";
export type { ChunkView };

/** Request body of `chunk_list` (`commands::chunks::ChunkListRequest`). */
import type { ChunkListRequest } from "./generated/ChunkListRequest.ts";
export type { ChunkListRequest };
/** Row of `block` (`db::models::Block`); `attrs_json` is the raw column. */
import type { Block } from "./generated/Block.ts";
export type { Block };

/** Row of `block_translation` (`db::models::BlockTranslation`). */
import type { BlockTranslation } from "./generated/BlockTranslation.ts";
export type { BlockTranslation };

/** Row of `llm_call` (`db::models::LlmCall`); `params_json` is the raw column. */
import type { LlmCall } from "./generated/LlmCall.ts";
export type { LlmCall };

/** Result of `chunk_get` (`commands::chunks::ChunkDetail`). */
import type { ChunkDetail } from "./generated/ChunkDetail.ts";
export type { ChunkDetail };
// --- metrics -------------------------------------------------------------------------------

/** VRAM reading, in bytes (`resources::vram::VramInfo`). */
import type { VramInfo } from "./generated/VramInfo.ts";
export type { VramInfo };
/** One row of the queue-depth histogram (`commands::metrics::JobCount`). */
import type { JobCount } from "./generated/JobCount.ts";
export type { JobCount };
/**
 * Per-role LLM capacity (`resources::endpoints::EndpointUsage`, PLAN.md §10). The UI shows it so
 * a capped sub-agent is explained instead of looking slow.
 */
import type { EndpointUsage } from "./generated/EndpointUsage.ts";
export type { EndpointUsage };
/**
 * Result of `metrics_get` (`commands::metrics::Metrics`): a point-in-time snapshot of
 * machine resources, queue depth and worker state. `metrics://tick` carries a closely related
 * payload (see `MetricsTickEvent`).
 */
import type { Metrics } from "./generated/Metrics.ts";
export type { Metrics };
// --- sidecar -------------------------------------------------------------------------------

/** Result of `sidecar_status` and payload of `sidecar://status` (`sidecar::supervisor::SidecarStatus`). */
import type { SidecarStatus } from "./generated/SidecarStatus.ts";
export type { SidecarStatus };
// --- export --------------------------------------------------------------------------------

/** Request body of `export_build` (`pipeline::export::ExportRequest`). */
import type { ExportRequest } from "./generated/ExportRequest.ts";
export type { ExportRequest };
/** Result of `export_build` (`pipeline::export::ExportOutcome`). */
import type { ExportOutcome } from "./generated/ExportOutcome.ts";
export type { ExportOutcome };
/** One entry of the build history (`pipeline::export::ExportBuildRecord`). */
import type { ExportBuildRecord } from "./generated/ExportBuildRecord.ts";
export type { ExportBuildRecord };
/** Request body of `export_preview` (`pipeline::export::ExportPreviewRequest`). */
import type { ExportPreviewRequest } from "./generated/ExportPreviewRequest.ts";
export type { ExportPreviewRequest };
/** One composed unit of an `export_preview` (`pipeline::export::PreviewUnit`). */
import type { PreviewUnit } from "./generated/PreviewUnit.ts";
export type { PreviewUnit };
/** Result of `export_preview` (`pipeline::export::ExportPreview`). */
import type { ExportPreview } from "./generated/ExportPreview.ts";
export type { ExportPreview };
// --- event payloads ------------------------------------------------------------------------

/**
 * Payload of `job://progress`.
 *
 * The control plane emits the serialized [`Job`] row — exactly the shape `job_list` returns —
 * at every transition it owns: `pending` on enqueue, `leased`/`running` on claim, then `done`,
 * `failed` or `cancelled`. Views still treat it as an **invalidation trigger** and refetch
 * through commands, because a job event says nothing about the other rows.
 */
export type JobProgressEvent = JobView;

/**
 * Payload of `metrics://tick` (`crates/app/src/lib.rs::spawn_metrics_ticker`). It is close to,
 * but not identical to, the `metrics_get` result: it carries the sidecar status inline and no
 * `vram` / `sidecar_in_flight` fields.
 */
import type { MetricsTick as MetricsTickEvent } from "./generated/MetricsTick.ts";
export type { MetricsTickEvent };
/**
 * Payload of `log://line` (`crates/app/src/events.rs::LogLine`).
 *
 * `source` is the emitter — `sidecar` for the sidecar's stderr, `worker` for job failures. The
 * control plane does not populate `project_id` today, so the view treats a missing value as a
 * global line rather than dropping it.
 */
import type { LogLine as LogLineEvent } from "./generated/LogLine.ts";
export type { LogLineEvent };
/** Payload of `export://progress` (`commands::export`, one of a `started` / `done` ack). */
import type { ExportProgress as ExportProgressEvent } from "./generated/ExportProgress.ts";
export type { ExportProgressEvent };
// --- diagnostics ---------------------------------------------------------------------------

/** Result of `diagnostics_paths` (`commands::misc::DiagnosticsPaths`). */
import type { DiagnosticsPaths } from "./generated/DiagnosticsPaths.ts";
export type { DiagnosticsPaths };
/** Result of `diagnostics_export` (`diagnostics::DiagnosticsOutcome`). */
import type { DiagnosticsOutcome } from "./generated/DiagnosticsOutcome.ts";
export type { DiagnosticsOutcome };
// --- series (PLAN.md §9.5) ------------------------------------------------------------------

/** Row of `series` (`db::models::Series`); the language pair is shared by its books. */
import type { Series } from "./generated/Series.ts";
export type { Series };

/** Row of `series_glossary_term` (`db::models::SeriesGlossaryTerm`). */
import type { SeriesGlossaryTerm } from "./generated/SeriesGlossaryTerm.ts";
export type { SeriesGlossaryTerm };

/** Row of `series_glossary_variant` (`db::models::SeriesGlossaryVariant`). */
import type { SeriesGlossaryVariant } from "./generated/SeriesGlossaryVariant.ts";
export type { SeriesGlossaryVariant };

/** Row of `series_memory` (`db::models::SeriesMemory`). */
import type { SeriesMemory } from "./generated/SeriesMemory.ts";
export type { SeriesMemory };

/** Result of `series_get` (`commands::series::SeriesDetail`). */
import type { SeriesDetail } from "./generated/SeriesDetail.ts";
export type { SeriesDetail };
/** Request body of `series_create` (`commands::series::SeriesCreate`). */
import type { SeriesCreate as SeriesCreateRequest } from "./generated/SeriesCreate.ts";
export type { SeriesCreateRequest };
/** Request body of `series_update` (`commands::series::SeriesUpdate`). */
import type { SeriesUpdate as SeriesUpdateRequest } from "./generated/SeriesUpdate.ts";
export type { SeriesUpdateRequest };
/** Request body of `project_set_series` (`commands::series::ProjectSetSeries`). */
import type { ProjectSetSeries as ProjectSetSeriesRequest } from "./generated/ProjectSetSeries.ts";
export type { ProjectSetSeriesRequest };
/** Request body of `series_glossary_upsert` (`commands::series::SeriesGlossaryUpsert`). */
import type { SeriesGlossaryUpsert as SeriesGlossaryUpsertRequest } from "./generated/SeriesGlossaryUpsert.ts";
export type { SeriesGlossaryUpsertRequest };
/** Request body of `series_variant_upsert` (`commands::series::SeriesVariantUpsert`). */
import type { SeriesVariantUpsert as SeriesVariantUpsertRequest } from "./generated/SeriesVariantUpsert.ts";
export type { SeriesVariantUpsertRequest };
/** Request body of `series_promote_term` (`commands::series::SeriesPromote`). */
import type { SeriesPromote as SeriesPromoteRequest } from "./generated/SeriesPromote.ts";
export type { SeriesPromoteRequest };
/** Outcome of a promotion (`pipeline::glossary::ProposalOutcome`, serde `snake_case`). */
import type { ProposalOutcome } from "./generated/ProposalOutcome.ts";
export type { ProposalOutcome };

/** Result of `series_promote_term` (`commands::series::PromoteOutcome`). */
import type { PromoteOutcome } from "./generated/PromoteOutcome.ts";
export type { PromoteOutcome };
/** Request body of `series_export` (`commands::series::SeriesExportRequest`). */
import type { SeriesExportRequest } from "./generated/SeriesExportRequest.ts";
export type { SeriesExportRequest };
/** Result of `series_export` (`pipeline::series_bundle::SeriesExportOutcome`). */
import type { SeriesExportOutcome } from "./generated/SeriesExportOutcome.ts";
export type { SeriesExportOutcome };
/** Request body of `series_import` (`commands::series::SeriesImportRequest`). */
import type { SeriesImportRequest } from "./generated/SeriesImportRequest.ts";
export type { SeriesImportRequest };
/** Result of `series_import` (`pipeline::series_bundle::SeriesImportOutcome`). */
import type { SeriesImportOutcome } from "./generated/SeriesImportOutcome.ts";
export type { SeriesImportOutcome };
/** Result of `series_qa_scan` (`commands::series::SeriesQaScanResult`). */
import type { SeriesQaScanResult } from "./generated/SeriesQaScanResult.ts";
export type { SeriesQaScanResult };
/** Request body of `series_recon_start` (`commands::series::SeriesReconStart`). */
import type { SeriesReconStart as SeriesReconStartRequest } from "./generated/SeriesReconStart.ts";
export type { SeriesReconStartRequest };
/** One character/term of the candidate series profile (`pipeline::series_recon::SeriesCharacter`). */
import type { SeriesCharacter as SeriesReconCharacter } from "./generated/SeriesCharacter.ts";
export type { SeriesReconCharacter };
/** Where the candidate series profile came from (`pipeline::series_recon::SeriesProfileProvenance`). */
import type { SeriesProfileProvenance as SeriesReconProvenance } from "./generated/SeriesProfileProvenance.ts";
export type { SeriesReconProvenance };
/**
 * Candidate series profile, stored in `series_memory['series_profile']`.
 * `rejected` lists the sources the user refused: a later reconnaissance skips them.
 */
import type { SeriesProfile } from "./generated/SeriesProfile.ts";
export type { SeriesProfile };
/** One character the user accepted from the candidate (`pipeline::series_recon::ConfirmedCharacter`). */
import type { ConfirmedCharacter as ConfirmedSeriesCharacter } from "./generated/ConfirmedCharacter.ts";
export type { ConfirmedSeriesCharacter };
/** Request body of `series_recon_confirm` (`pipeline::series_recon::ConfirmRequest`). */
import type { SeriesConfirmRequest } from "./generated/SeriesConfirmRequest.ts";
export type { SeriesConfirmRequest };
/** Result of `series_recon_confirm` (`pipeline::series_recon::ConfirmOutcome`). */
import type { SeriesConfirmOutcome } from "./generated/SeriesConfirmOutcome.ts";
export type { SeriesConfirmOutcome };