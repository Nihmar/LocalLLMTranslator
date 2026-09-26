/**
 * Domain types for the `UI -> Tauri` boundary.
 *
 * The *names* of the commands and of the events consumed here are frozen by `AGENTS.md`
 * ("UI -> Tauri") and must not drift. The *payload shapes* are not spelled out there, so every
 * type below is our best-effort projection of the SQLite schema in `PLAN.md` §5 and of the
 * sidecar payloads in `PLAN.md` §12.1. Field names are intentionally `snake_case` so they match
 * the database columns and the sidecar JSON-RPC output one-to-one.
 *
 * `lib/ipc.ts` is the only module allowed to mention a command name; `lib/events.ts` the only
 * one allowed to mention an event name.
 */

/** JSON value that survived deserialisation but is not modelled further by the UI. */
export type JsonPrimitive = string | number | boolean | null;
export type JsonValue = JsonPrimitive | JsonValue[] | { readonly [key: string]: JsonValue };
export type JsonObject = { readonly [key: string]: JsonValue };

// --- enums shared with the backend ---------------------------------------------------------

/** `PLAN.md` §12.1 `detect_format`. */
export type SourceFormat = "epub" | "pdf" | "markdown";

/** PDF extraction backend; `auto` lets the sidecar pick `pymupdf4llm` (`PLAN.md` §1). */
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

/** `qa_finding.kind` (`PLAN.md` §5). */
export type QaFindingKind =
  | "untranslated"
  | "glossary_mismatch"
  | "placeholder_broken"
  | "markdown_malformed"
  | "length_anomaly"
  | "duplicate"
  | "empty"
  | "latin_leftover"
  | "glossary_conflict";

export type QaSeverity = "critical" | "major" | "minor" | "info";
export type QaFindingStatus = "open" | "resolved" | "ignored";
export type Severity = QaSeverity;

/** Log levels emitted on `log://line`, matching the Rust `tracing` levels. */
export type LogLevel = "trace" | "debug" | "info" | "warn" | "error";

/** Sidecar supervisor state (`PLAN.md` §2, event `sidecar://status`). */
export type SidecarState = "starting" | "ready" | "restarting" | "failed" | "stopped";

/**
 * Where the VRAM figure came from (`PLAN.md` §10: sysfs -> rocm-smi -> nvidia-smi -> unknown).
 * The UI only displays it; the probe order is the backend's business.
 */
export type VramSource = "sysfs" | "rocm-smi" | "nvidia-smi" | "unknown";

/** Output formats offered by the Pandoc driver. */
export type ExportFormat = "pdf" | "epub" | "docx";

/** Phases of a Pandoc build, reported on `export://progress`. */
export type ExportPhase = "prepare" | "render" | "pandoc" | "done" | "failed";

// --- projects ------------------------------------------------------------------------------

/** Row of `project` (`PLAN.md` §5). `settings` is the parsed `settings_json` column. */
export interface Project {
  id: string;
  name: string;
  source_path: string;
  source_hash: string;
  source_format: SourceFormat;
  source_lang: string | null;
  target_lang: string;
  doc_title: string | null;
  doc_author: string | null;
  prompts_snapshot_dir: string | null;
  settings: JsonObject;
  created_at: string;
  updated_at: string;
}

/** Arguments of `project_create`. The backend fills id/hashes/timestamps. */
export interface ProjectCreateRequest {
  name: string;
  source_path: string;
  source_format: SourceFormat;
  source_lang: string | null;
  target_lang: string;
}

// --- LLM endpoints -------------------------------------------------------------------------

/** Row of `llm_endpoint` (`PLAN.md` §5). */
export interface Endpoint {
  id: string;
  name: string;
  base_url: string;
  /** Name of the secret in the OS keyring — never the secret itself (`PLAN.md` §5). */
  api_key_ref: string | null;
  max_concurrency: number | null;
  notes: string | null;
  last_health_at: string | null;
  /** `null` when the endpoint has never been probed. */
  last_health_ok: boolean | null;
  props: JsonObject;
}

/** Arguments of `endpoint_upsert`; `id` absent means "create". */
export interface EndpointUpsertRequest {
  id: string | null;
  name: string;
  base_url: string;
  api_key_ref: string | null;
  max_concurrency: number | null;
  notes: string | null;
}

/** Subset of llama-server `/props` that the models page surfaces (`PLAN.md` §7.1). */
export interface EndpointProps {
  total_slots: number | null;
  n_ctx: number | null;
  model_path: string | null;
  /** Raw `/props` body, kept for the "show details" disclosure. */
  raw: JsonObject | null;
}

/** Result of `endpoint_test` (`GET /health` + `GET /props`). */
export interface EndpointTestResult {
  ok: boolean;
  latency_ms: number | null;
  message: string;
  props: EndpointProps | null;
}

/** One entry of `GET /v1/models`. */
export interface EndpointModel {
  id: string;
  label: string | null;
  context_length: number | null;
}

/** Row of `role_binding` (`PLAN.md` §5). */
export interface RoleBinding {
  id: string;
  endpoint_id: string;
  role: Role;
  model: string;
  params: JsonObject;
  priority: number;
}

/** Arguments of `role_binding_set`; upsert on `(endpoint_id, role)`. */
export interface RoleBindingSetRequest {
  endpoint_id: string;
  role: Role;
  model: string;
  params: JsonObject;
  priority: number;
}

/** Arguments of `role_binding_list`. */
export interface RoleBindingListRequest {
  endpoint_id?: string | undefined;
  role?: Role | undefined;
}

// --- document model ------------------------------------------------------------------------

/** `Block` from `PLAN.md` §4.1 / AGENTS.md. */
export interface Block {
  id: string;
  chapter_id: string | null;
  order: number;
  kind: BlockKind;
  level: number;
  source_md: string;
  source_text: string;
  translatable: boolean;
  attrs: JsonObject;
  content_hash: string;
}

/**
 * `Chapter` from AGENTS.md. `block_first`/`block_last` are `null` right after ingestion, when
 * the block list has not been materialised yet.
 */
export interface Chapter {
  id: string;
  order: number;
  title: string;
  level: number;
  block_first: number | null;
  block_last: number | null;
}

/** Full `Chunk` row (`PLAN.md` §4.3). Returned by `chunk_get`. */
export interface Chunk {
  id: string;
  document_id: string;
  chapter_id: string | null;
  order_index: number;
  block_ids: string[];
  source_md: string;
  token_estimate: number;
  context_carrier: JsonObject;
  flags: string[];
  status: ChunkStatus;
  prompt_hash: string | null;
  model_id: string | null;
  params_json: string | null;
  context_manifest_json: string | null;
  target_md: string | null;
  error: string | null;
  created_at: string;
  updated_at: string;
}

/**
 * Row of the chunk table.
 *
 * The table needs the chapter title and the attempt counter, which live in other tables
 * (`chapter`, `job`). Rather than issuing N+1 queries from the UI, `chunk_list` is assumed to
 * return this denormalised projection.
 */
export interface ChunkSummary {
  id: string;
  chapter_id: string | null;
  chapter_title: string | null;
  order_index: number;
  block_count: number;
  flags: string[];
  token_estimate: number;
  status: ChunkStatus;
  model_id: string | null;
  /** Attempts of the `translate_chunk` job that owns this chunk. */
  attempts: number;
  error: string | null;
  updated_at: string;
}

/** Arguments of `chunk_list`. */
export interface ChunkListRequest {
  project_id: string;
  chapter_id?: string | undefined;
  status?: ChunkStatus | undefined;
  limit?: number | undefined;
  offset?: number | undefined;
}

/** Row of `block_translation` (`PLAN.md` §5): the original <-> translated alignment. */
export interface BlockTranslation {
  block_id: string;
  chunk_id: string;
  text_md: string;
  placeholders_ok: boolean;
  origin: "translator" | "editor" | "proofreader" | "user";
  edited_by_user: boolean;
  updated_at: string;
}

/**
 * Result of `chunk_get`: the chunk plus the blocks it covers and the block-level translations.
 * This is what lets the translation page show the aligned original/target text without a
 * block-listing command (there is none in the frozen `UI -> Tauri` table).
 */
export interface ChunkDetail {
  chunk: Chunk;
  blocks: Block[];
  translations: BlockTranslation[];
}

// --- jobs ----------------------------------------------------------------------------------

/** Row of `job` (`PLAN.md` §5). */
export interface Job {
  id: string;
  project_id: string;
  kind: JobKind;
  payload: JsonObject;
  priority: number;
  state: JobState;
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

/** Arguments of `job_list`. */
export interface JobListRequest {
  project_id?: string | undefined;
  state?: JobState | undefined;
  kind?: JobKind | undefined;
  limit?: number | undefined;
}

/** Common acknowledgement for the fire-and-forget queue commands. */
export interface QueueAck {
  accepted: number;
  job_ids: string[];
  message: string | null;
}

// --- ingestion -----------------------------------------------------------------------------

/** Arguments of `ingest_start`. */
export interface IngestStartRequest {
  project_id: string;
  path: string;
  pdf_backend?: PdfBackend | undefined;
}

/**
 * Result of `ingest_start`.
 *
 * `AGENTS.md` freezes only `ingest_start`; format detection, extraction and chapter preview are
 * therefore assumed to happen inside that call (sidecar `detect_format` + `ingest` +
 * `parse_document`, orchestrated by Rust) and to be reported back in one payload.
 */
export interface IngestResult {
  project_id: string;
  document_id: string;
  markdown_path: string;
  format: SourceFormat;
  extractor: string;
  extractor_version: string | null;
  metadata: JsonObject;
  chapters: Chapter[];
  warnings: string[];
  block_count: number;
  chunk_count: number;
  /** Jobs enqueued by the ingestion (chunk building, summaries). */
  job_ids: string[];
}

// --- translation control -------------------------------------------------------------------

/** Arguments of `translation_start`. Re-issuing it resumes a paused/cancelled run (`PLAN.md` §6). */
export interface TranslationStartRequest {
  project_id: string;
  /** Restrict to specific chunks; omitted means "every chunk not yet done". */
  chunk_ids?: string[] | undefined;
  /** Override the translator model for this run ("ri-traduci con un altro modello"). */
  model?: string | null | undefined;
}

/** Arguments of `translation_pause` / `translation_cancel`. */
export interface TranslationControlRequest {
  project_id: string;
  /**
   * Restricts the cancellation to these chunks. Used by the per-row "Salta" action: there is no
   * dedicated skip command in the frozen table, so skipping is expressed as "cancel the pending
   * translation jobs of this chunk".
   */
  chunk_ids?: string[] | undefined;
}

// --- metrics ------------------------------------------------------------------------------

/** VRAM reading; all values are `null` when the source is `unknown`. */
export interface VramInfo {
  used_bytes: number | null;
  total_bytes: number | null;
  source: VramSource;
}

/** Slot occupancy as reported by llama-server `/props.total_slots` (`PLAN.md` §10). */
export interface SlotInfo {
  endpoint_id: string | null;
  total_slots: number | null;
  free_slots: number | null;
  in_flight: number;
}

/** Resource governor view (`PLAN.md` §10). */
export interface ResourceMetrics {
  vram: VramInfo;
  slots: SlotInfo;
  /** `min(free slots, VRAM headroom / per-slot cost, user limit)`. */
  max_parallel: number;
  /** True when the governor fell back to serial execution. */
  degraded: boolean;
  /** Explicit, user-facing reason for the degradation; shown verbatim in the gauge. */
  degraded_reason: string | null;
}

/** Real throughput observed by the scheduler — the only basis for the ETA in `JobsView`. */
export interface ThroughputMetrics {
  chunks_done: number;
  chunks_total: number;
  tokens_prompt: number;
  tokens_completion: number;
  /** Wall-clock elapsed since the run started. */
  elapsed_ms: number;
  avg_latency_ms: number | null;
}

/** Result of `metrics_get` and payload of `metrics://tick`. */
export interface Metrics {
  project_id: string | null;
  resources: ResourceMetrics;
  throughput: ThroughputMetrics;
  updated_at: string;
}

// --- sidecar ------------------------------------------------------------------------------

/** Result of `sidecar_status` and payload of `sidecar://status`. */
export interface SidecarStatus {
  state: SidecarState;
  version: string | null;
  python: string | null;
  platform: string | null;
  pid: number | null;
  restarts: number;
  last_error: string | null;
}

// --- QA ------------------------------------------------------------------------------------

/** Row of `qa_finding` (`PLAN.md` §5). `details` is the parsed `details_json` column. */
export interface QaFinding {
  id: string;
  project_id: string;
  chunk_id: string | null;
  block_id: string | null;
  kind: QaFindingKind;
  severity: QaSeverity;
  details: JsonObject;
  status: QaFindingStatus;
  created_at: string;
}

// --- export --------------------------------------------------------------------------------

/** A buildable unit: one chapter, or the whole document split by chapter (`PLAN.md` §11.5). */
export interface ExportUnit {
  path: string;
  title: string;
}

/** Arguments of `export_build`. */
export interface ExportBuildRequest {
  project_id: string;
  output_format: ExportFormat;
  /** Pandoc template path; `null` lets the driver use its bundled default. */
  template: string | null;
  /** CSS path, meaningful for HTML/EPUB only. */
  css: string | null;
  /** Empty array means "every chapter". */
  units: ExportUnit[];
  /** `null` lets the backend place the file under the project output directory. */
  output_path: string | null;
}

/** Result of `export_build` (sidecar `pandoc_build`, `PLAN.md` §12.1). */
export interface ExportBuildResult {
  output_path: string;
  log: string;
  duration_ms: number;
}

/** Arguments of `open_path`: reveals a file or directory with the OS handler. */
export interface OpenPathRequest {
  path: string;
}

// --- event payloads ------------------------------------------------------------------------

/**
 * Payload of `job://progress`.
 *
 * Deliberately carries the affected `chunk_id` with its new status/model/attempts so
 * `TranslateView` can patch a single row in place instead of refetching the whole table.
 */
export interface JobProgressEvent {
  job_id: string;
  project_id: string;
  kind: JobKind;
  state: JobState;
  done: number;
  total: number;
  chunk_id: string | null;
  chunk_status: ChunkStatus | null;
  attempts: number;
  model: string | null;
  error: string | null;
  updated_at: string;
}

/** Payload of `log://line`. */
export interface LogLineEvent {
  ts: string;
  level: LogLevel;
  target: string;
  message: string;
  job_id: string | null;
  project_id: string | null;
}

/** Payload of `export://progress`. */
export interface ExportProgressEvent {
  project_id: string;
  unit: string;
  done: number;
  total: number;
  phase: ExportPhase;
  message: string | null;
}
