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

/** `block.kind`, identical in AGENTS.md and `PLAN.md` §4.1. */
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
export type SidecarState = "stopped" | "starting" | "running" | "restarting" | "failed";

/**
 * Why the resource governor capped the parallel degree
 * (`crates/app/src/resources/vram.rs::ParallelReason`, serde `snake_case`).
 */
export type ParallelReason = "ok" | "vram_unknown" | "insufficient_headroom" | "slot_limited";

/** Output formats offered by the Pandoc driver. */
export type ExportFormat = "pdf" | "epub" | "docx";

/** Generic acknowledgement returned by the mutating commands (`commands::Ack`). */
export interface Ack {
  ok: boolean;
}

// --- projects ------------------------------------------------------------------------------

/** Row of `project` (`db::models::Project`); `settings_json` is the raw column. */
export interface Project {
  id: string;
  name: string;
  source_path: string;
  source_hash: string;
  source_format: string;
  source_lang: string | null;
  target_lang: string;
  doc_title: string | null;
  doc_author: string | null;
  prompts_snapshot_dir: string | null;
  settings_json: string;
  created_at: string;
  updated_at: string;
}

/** Request body of `project_create` (`commands::CreateProjectRequest`). */
export interface CreateProjectRequest {
  name: string;
  source_path: string;
  target_lang: string;
  source_lang?: string | null;
  source_format?: string | null;
  doc_title?: string | null;
  doc_author?: string | null;
  settings?: JsonValue;
}

/** Row of `chapter` (`db::models::Chapter`). */
export interface Chapter {
  id: string;
  document_id: string;
  order_index: number;
  title: string;
  level: number;
  block_first: number;
  block_last: number;
  summary: string | null;
  summary_model: string | null;
  summary_hash: string | null;
  status: string;
}

/** Result of `project_get` (`commands::project::ProjectDetail`). */
export interface ProjectDetail {
  project: Project;
  chapters: Chapter[];
  chunks_total: number;
  chunks_done: number;
}

/** Request body of `project_export` (`commands::project::ExportBundleRequest`). */
export interface ExportBundleRequest {
  project_id: string;
  /** Destination `.llmtz`; `null` writes it into the project output directory. */
  output_path?: string | null;
}

/** Result of `project_export` (`pipeline::bundle::ExportBundleOutcome`). */
export interface ExportBundleOutcome {
  output_path: string;
  bytes: number;
  files: number;
}

/** Request body of `project_import` (`commands::project::ImportBundleRequest`). */
export interface ImportBundleRequest {
  archive_path: string;
}

// --- LLM endpoints -------------------------------------------------------------------------

/** Row of `llm_endpoint` (`db::models::LlmEndpoint`); `props_json` is the raw `/props` body. */
export interface Endpoint {
  id: string;
  name: string;
  base_url: string;
  /** Name of the secret in the OS keyring — never the secret itself (`PLAN.md` §5). */
  api_key_ref: string | null;
  max_concurrency: number | null;
  notes: string | null;
  last_health_at: string | null;
  last_health_ok: boolean | null;
  props_json: string | null;
}

/** Request body of `endpoint_upsert` (`commands::endpoint::EndpointUpsert`); `id` absent = create. */
export interface EndpointUpsert {
  id?: string | null;
  name: string;
  base_url: string;
  api_key_ref?: string | null;
  max_concurrency?: number | null;
  notes?: string | null;
}

/** Request body of `endpoint_models` (`commands::endpoint::EndpointModelsRequest`). */
export interface EndpointModelsRequest {
  endpoint_id?: string | null;
  base_url?: string | null;
}

/** Result of a single `GET /health` probe (`llm::health::EndpointHealth`). */
export interface EndpointHealth {
  ok: boolean;
  status: string | null;
  code: number;
  checked_at: string;
}

/** `GET /props` (`llm::types::Props`). */
export interface Props {
  total_slots: number | null;
  n_ctx: number | null;
  model_path: string | null;
  default_generation_settings: JsonValue | null;
}

/** One entry of `GET /v1/models` (`llm::types::ModelInfo`). */
export interface ModelInfo {
  id: string;
  object: string | null;
  owned_by: string | null;
}

/** Result of `endpoint_test` (`commands::endpoint::EndpointTestResult`). */
export interface EndpointTestResult {
  health: EndpointHealth;
  props: Props | null;
  models: ModelInfo[];
}

// --- role bindings -------------------------------------------------------------------------

/** Row of `role_binding` (`db::models::RoleBinding`); `params_json` is the raw column. */
export interface RoleBinding {
  id: string;
  endpoint_id: string;
  role: string;
  model: string;
  params_json: string;
  priority: number;
}

/** Request body of `role_binding_set` (`commands::role_binding::RoleBindingSet`). */
export interface RoleBindingSet {
  id?: string | null;
  role: string;
  endpoint_id: string;
  model: string;
  params?: JsonValue;
  priority?: number | null;
}

// --- ingestion -----------------------------------------------------------------------------

/** Request body of `ingest_start` (`commands::ingest::IngestStartRequest`). */
export interface IngestStartRequest {
  project_id: string;
  /** Falls back to the project's `source_path` when omitted. */
  source_path?: string | null;
  pdf_backend?: string | null;
}

/** Result of `ingest_start` (`commands::ingest::JobStarted`). */
export interface JobStarted {
  job_id: string;
}

// --- translation control -------------------------------------------------------------------

/** Request body of `translation_start` (`commands::translation::TranslationStartRequest`). */
export interface TranslationStartRequest {
  project_id?: string | null;
  /** Only re-enqueue chunks that previously failed or need review. */
  only_retry?: boolean;
}

/** Result of `translation_start` (`commands::translation::TranslationStartResult`). */
export interface TranslationStartResult {
  enqueued: number;
  running: boolean;
}

// --- book reconnaissance (PLAN.md §9.4) -----------------------------------------------------

/**
 * One profile value with its provenance (`pipeline::recon::ProfileField`). `basis` is one of
 * `from_text`, `metadata`, `inferred` (or `user` for a value typed by hand): an `inferred`
 * field is shown as such and is not confirmed by default.
 */
export interface ProfileField<T> {
  value: T;
  basis: string;
}

/** A name the profile proposes for the glossary (`pipeline::recon::ProperNoun`). */
export interface ProperNoun {
  source: string;
  kind: string;
  note: string;
}

/** Where the candidate came from (`pipeline::recon::ProfileProvenance`). */
export interface ProfileProvenance {
  generated_at: string;
  model: string;
  prompt_hash: string;
  excerpt_blocks: number;
  metadata: boolean;
  pasted_chars: number;
}

/** The candidate book profile (`pipeline::recon::BookProfile`). */
export interface BookProfile {
  source_language: ProfileField<string>;
  genre: ProfileField<string>;
  audience: ProfileField<string>;
  era: ProfileField<string>;
  narrative_voice: ProfileField<string>;
  register: ProfileField<string>;
  style_notes: ProfileField<string[]>;
  themes: ProfileField<string[]>;
  synopsis: ProfileField<string>;
  proper_nouns: ProperNoun[];
  provenance: ProfileProvenance;
}

/** Row of `glossary_term` (`db::models::GlossaryTerm`). */
export interface GlossaryTerm {
  id: string;
  project_id: string;
  source_lang: string | null;
  target_lang: string | null;
  source: string;
  target: string;
  note: string | null;
  kind: string;
  origin: string;
  revision: number;
  status: string;
}

/** Result of `recon_get` and `recon_confirm` (`pipeline::recon::ReconSnapshot`). */
export interface ReconSnapshot {
  project_id: string;
  /** The last candidate, still unconfirmed; `null` when none was generated. */
  profile: BookProfile | null;
  /** Confirmed values the translator prompt already reads. */
  style_guide: string;
  synopsis: string;
  book_meta: JsonValue | null;
  glossary: GlossaryTerm[];
  /** Style-note candidates proposed by the summarizer; the user decides. */
  style_notes: string[];
  orchestrator_bound: boolean;
  /** Id of a pending/running `book_recon` job, when there is one. */
  running_job: string | null;
  /** Last failure of a `book_recon` job, when there is one. */
  last_error: string | null;
}

/** Request body of `recon_start` (`commands::recon::ReconStartRequest`). */
export interface ReconStartRequest {
  project_id: string;
  /** Text the user pasted themselves; the app never fetches a page. */
  pasted_text?: string | null;
}

/** One accepted proper noun in `recon_confirm` (`pipeline::recon::ConfirmedTerm`). */
export interface ConfirmedTerm {
  source: string;
  target?: string | null;
  kind: string;
  note?: string | null;
}

/** Request body of `recon_confirm` (`pipeline::recon::ConfirmRequest`). */
export interface ReconConfirmRequest {
  project_id: string;
  profile: BookProfile;
  /** Profile keys the user accepted; only those are written. */
  confirmed_fields: string[];
  /** Style guide assembled and edited in the UI. */
  style_guide: string;
  proper_nouns: ConfirmedTerm[];
}

/** Request body of `glossary_upsert` (`commands::glossary::GlossaryUpsert`). */
export interface GlossaryUpsertRequest {
  /** Absent or `null` creates a term; present edits the existing row. */
  id?: string | null;
  project_id: string;
  source: string;
  target?: string | null;
  kind?: string | null;
  note?: string | null;
  /** `approved` | `candidate` | `rejected` | `conflict`; defaults to `approved`. */
  status?: string | null;
  source_lang?: string | null;
  target_lang?: string | null;
  /** Optimistic lock: the revision the edit started from. */
  expected_revision?: number | null;
}

// --- review and QA (PLAN.md §11.4) ---------------------------------------------------------

/** Row of `suggestion` (`db::models::Suggestion`); `original`/`proposed` are raw strings. */
export interface Suggestion {
  id: string;
  chunk_id: string;
  /** `editor` | `proofreader`. */
  pass: string;
  block_id: string | null;
  field: string | null;
  original: string | null;
  proposed: string | null;
  reason: string | null;
  severity: string | null;
  quote: string | null;
  /** `pending` | `accepted` | `rejected` | `superseded`. */
  status: string;
  created_at: string;
}

/** Row of `qa_finding` (`db::models::QaFinding`); `details_json` is the raw column. */
export interface QaFinding {
  id: string;
  project_id: string;
  chunk_id: string | null;
  block_id: string | null;
  kind: string;
  severity: string;
  details_json: string;
  status: string;
  created_at: string;
}

/** Request body of `review_start` (`commands::review::ReviewStartRequest`). */
export interface ReviewStartRequest {
  project_id: string;
  /** Restrict to these chunks; omitted means every eligible chunk. */
  chunk_ids?: string[] | null;
  chapter_id?: string | null;
  /** `editor` | `proofreader` | `both` (default). */
  pass?: string | null;
  /** Also re-run the QA scan on the selected chunks. */
  with_qa?: boolean;
}

/** Result of `review_start` (`commands::review::ReviewStartResult`). */
export interface ReviewStartResult {
  enqueued: number;
}

/** Request body of `suggestion_list` (`commands::review::SuggestionListRequest`). */
export interface SuggestionListRequest {
  project_id: string;
  chunk_id?: string | null;
  pass?: string | null;
  status?: string | null;
}

/** Request body of `qa_report` (`commands::review::QaReportRequest`). */
export interface QaReportRequest {
  project_id: string;
  kind?: string | null;
  severity?: string | null;
  chunk_id?: string | null;
}

// --- jobs ----------------------------------------------------------------------------------

/** Row of `job` (`db::models::Job`); `payload_json` is the raw column. */
export interface Job {
  id: string;
  project_id: string;
  kind: string;
  payload_json: string;
  priority: number;
  state: string;
  attempts: number;
  max_attempts: number;
  lease_owner: string | null;
  lease_expires_at: string | null;
  run_after: string | null;
  last_error: string | null;
  created_at: string;
  started_at: string | null;
  finished_at: string | null;
}

/** Request body of `job_list` (`commands::jobs::JobListRequest`). */
export interface JobListRequest {
  project_id?: string | null;
  state?: string | null;
  limit?: number | null;
}

// --- chunks and blocks ---------------------------------------------------------------------

/** Row of `chunk` (`db::models::Chunk`); the `*_json` columns are raw strings. */
export interface Chunk {
  id: string;
  document_id: string;
  chapter_id: string | null;
  order_index: number;
  block_ids_json: string;
  source_md: string;
  token_estimate: number;
  context_json: string;
  flags_json: string;
  status: string;
  prompt_hash: string | null;
  model_id: string | null;
  params_json: string | null;
  context_manifest_json: string | null;
  target_md: string | null;
  error: string | null;
  created_at: string;
  updated_at: string;
}

/** Request body of `chunk_list` (`commands::chunks::ChunkListRequest`). */
export interface ChunkListRequest {
  project_id: string;
  status?: string | null;
}

/** Row of `block` (`db::models::Block`); `attrs_json` is the raw column. */
export interface Block {
  id: string;
  document_id: string;
  chapter_id: string | null;
  order_index: number;
  kind: string;
  level: number;
  source_md: string;
  source_text: string;
  translatable: boolean;
  attrs_json: string;
  content_hash: string;
}

/** Row of `block_translation` (`db::models::BlockTranslation`). */
export interface BlockTranslation {
  block_id: string;
  chunk_id: string;
  text_md: string;
  placeholders_ok: boolean;
  origin: string;
  edited_by_user: boolean;
  updated_at: string;
}

/** Row of `llm_call` (`db::models::LlmCall`); `params_json` is the raw column. */
export interface LlmCall {
  id: string;
  job_id: string | null;
  chunk_id: string | null;
  role: string;
  endpoint_id: string | null;
  model: string;
  params_json: string;
  seed: number | null;
  prompt_hash: string;
  prompt_text: string | null;
  prompt_compressed: boolean | null;
  response_text: string | null;
  finish_reason: string | null;
  prompt_tokens: number | null;
  completion_tokens: number | null;
  latency_ms: number | null;
  attempt: number;
  error: string | null;
  created_at: string;
}

/** Result of `chunk_get` (`commands::chunks::ChunkDetail`). */
export interface ChunkDetail {
  chunk: Chunk;
  blocks: Block[];
  translations: BlockTranslation[];
  llm_calls: LlmCall[];
}

// --- metrics -------------------------------------------------------------------------------

/** VRAM reading, in bytes (`resources::vram::VramInfo`). */
export interface VramInfo {
  used_bytes: number;
  total_bytes: number;
}

/** One row of the queue-depth histogram (`commands::metrics::JobCount`). */
export interface JobCount {
  state: string;
  count: number;
}

/**
 * Per-role LLM capacity (`resources::endpoints::EndpointUsage`, PLAN.md §10). The UI shows it so
 * a capped sub-agent is explained instead of looking slow.
 */
export interface EndpointUsage {
  role: string;
  endpoint_id: string | null;
  limit: number;
  in_flight: number;
  reason: string;
}

/**
 * Result of `metrics_get` (`commands::metrics::Metrics`): a point-in-time snapshot of
 * machine resources, queue depth and worker state. `metrics://tick` carries a closely related
 * payload (see `MetricsTickEvent`).
 */
export interface Metrics {
  vram: VramInfo | null;
  free_bytes: number | null;
  suggested_parallel: number;
  reason: ParallelReason;
  jobs: JobCount[];
  /** Per-role endpoint capacity and in-flight counts. */
  endpoints: EndpointUsage[];
  sidecar_in_flight: number;
  worker_running: boolean;
  worker_paused: boolean;
}

// --- sidecar -------------------------------------------------------------------------------

/** Result of `sidecar_status` and payload of `sidecar://status` (`sidecar::supervisor::SidecarStatus`). */
export interface SidecarStatus {
  state: SidecarState;
  pid: number | null;
  attempts: number;
  message: string | null;
}

// --- export --------------------------------------------------------------------------------

/** Request body of `export_build` (`pipeline::export::ExportRequest`). */
export interface ExportRequest {  project_id: string;
  /** `pdf` | `epub` | `docx` | `html`. */
  output_format: string;
  /** Absolute destination; `null` lets the backend place the file under the project output directory. */
  output_path?: string | null;
  /** Absolute template override; `null` uses the format's default from `pandoc/`. */
  template?: string | null;
  /** Absolute CSS override; `null` uses the format's default. */
  css?: string | null;
  /** Include the table of contents (default true). */
  toc?: boolean;
  /** Build only this chapter into a standalone file. */
  chapter_id?: string | null;
  /** Bypass the unchanged-build skip. */
  force?: boolean;
}

/** Result of `export_build` (`pipeline::export::ExportOutcome`). */
export interface ExportOutcome {
  output_path: string;
  /** Number of Markdown units that were rendered. */
  units: number;
  log: string;
  duration_ms: number | null;
  /** True when the build was skipped because nothing changed. */
  from_cache: boolean;
  /** Unit keys rebuilt since the previous build. */
  changed_units: string[];
  /** Unit keys reused from the previous build. */
  reused_units: number;
  build_id: string;
}

/** One entry of the build history (`pipeline::export::ExportBuildRecord`). */
export interface ExportBuildRecord {
  id: string;
  output_path: string;
  output_format: string;
  chapter_id: string | null;
  template: string | null;
  css: string | null;
  toc: boolean;
  units: number;
  changed_units: string[];
  reused_units: number;
  from_cache: boolean;
  duration_ms: number | null;
  built_at: string;
}

/** Request body of `export_preview` (`pipeline::export::ExportPreviewRequest`). */
export interface ExportPreviewRequest {
  project_id: string;
  chapter_id?: string | null;
}

/** One composed unit of an `export_preview` (`pipeline::export::PreviewUnit`). */
export interface PreviewUnit {
  key: string;
  title: string;
  markdown: string;
  chunks: number;
  untranslated: number;
}

/** Result of `export_preview` (`pipeline::export::ExportPreview`). */
export interface ExportPreview {
  metadata_yaml: string;
  units: PreviewUnit[];
  total_chunks: number;
  untranslated_chunks: number;
}

// --- event payloads ------------------------------------------------------------------------

/**
 * Payload of `job://progress`.
 *
 * The control plane emits the serialized [`Job`] row — exactly the shape `job_list` returns —
 * at every transition it owns: `pending` on enqueue, `leased`/`running` on claim, then `done`,
 * `failed` or `cancelled`. Views still treat it as an **invalidation trigger** and refetch
 * through commands, because a job event says nothing about the other rows.
 */
export type JobProgressEvent = Job;

/**
 * Payload of `metrics://tick` (`crates/app/src/lib.rs::spawn_metrics_ticker`). It is close to,
 * but not identical to, the `metrics_get` result: it carries the sidecar status inline and no
 * `vram` / `sidecar_in_flight` fields.
 */
export interface MetricsTickEvent {
  free_bytes: number | null;
  suggested_parallel: number;
  reason: ParallelReason;
  jobs: JobCount[];
  endpoints: EndpointUsage[];
  worker_running: boolean;
  worker_paused: boolean;
  sidecar: SidecarStatus;
}

/**
 * Payload of `log://line` (`crates/app/src/events.rs::LogLine`).
 *
 * `source` is the emitter — `sidecar` for the sidecar's stderr, `worker` for job failures. The
 * control plane does not populate `project_id` today, so the view treats a missing value as a
 * global line rather than dropping it.
 */
export interface LogLineEvent {
  ts: string;
  level: LogLevel;
  source: string;
  message: string;
  project_id?: string | null;
}

/** Payload of `export://progress` (`commands::export`, one of a `started` / `done` ack). */
export interface ExportProgressEvent {
  state: string;
  format?: string;
  output_path?: string;
  units?: number;
}
