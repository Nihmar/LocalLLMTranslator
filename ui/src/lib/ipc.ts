/**
 * Typed wrappers over Tauri `invoke()`.
 *
 * This module is the **only** place in the frontend where a Tauri command name appears as a
 * string literal. Command names and their argument keys are frozen by `AGENTS.md`
 * ("UI -> Tauri"); the argument keys are derived from the Rust command signatures in
 * `crates/app/src/commands/*.rs`, which is the source of truth.
 *
 * Three argument conventions appear in the Rust signatures and are reproduced here exactly:
 *
 * - commands that take a struct declare it as `req` and receive `{ req: {...} }`;
 * - commands that take a primitive id declare it as `id` / `chunk_id` / `path`;
 * - commands with no payload (`role_binding_list`, `metrics_get`, `sidecar_status`,
 *   `translation_pause`, `project_list`, `endpoint_list`) are called with no arguments at all.
 *
 * There is no direct network access anywhere in the UI: every backend interaction goes through
 * `invoke` here or through `listen` in `lib/events.ts` (`PLAN.md` §2, `AGENTS.md` §TypeScript).
 */

import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  Ack,
  Chunk,
  ChunkDetail,
  ChunkListRequest,
  CreateProjectRequest,
  Endpoint,
  EndpointModelsRequest,
  EndpointTestResult,
  EndpointUpsert,
  ExportOutcome,
  ExportPreview,
  ExportPreviewRequest,
  ExportRequest,
  ExportBuildRecord,
  ExportBundleOutcome,
  ExportBundleRequest,
  GlossaryTerm,
  GlossaryUpsertRequest,
  ProjectSetSeriesRequest,
  PromoteOutcome,
  Series,
  SeriesCreateRequest,
  SeriesDetail,
  SeriesGlossaryTerm,
  SeriesGlossaryUpsertRequest,
  SeriesGlossaryVariant,
  SeriesImportOutcome,
  SeriesImportRequest,
  SeriesExportOutcome,
  SeriesExportRequest,
  SeriesPromoteRequest,
  SeriesQaScanResult,
  SeriesUpdateRequest,
  SeriesVariantUpsertRequest,
  IngestStartRequest,
  ImportBundleRequest,
  Job,
  JobListRequest,
  JobStarted,
  Metrics,
  ModelInfo,
  Project,
  ProjectDetail,
  ReconConfirmRequest,
  ReconSnapshot,
  ReconStartRequest,
  QaFinding,
  QaReportRequest,
  ReviewStartRequest,
  ReviewStartResult,
  RoleBinding,
  RoleBindingSet,
  SidecarStatus,
  Suggestion,
  SuggestionListRequest,
  TranslationStartRequest,
  TranslationStartResult,
} from "./types";

/** Frozen command surface (`AGENTS.md` -> "UI -> Tauri"). */
const COMMANDS = {
  projectList: "project_list",
  projectCreate: "project_create",
  projectGet: "project_get",
  projectDelete: "project_delete",
  projectExport: "project_export",
  projectImport: "project_import",
  endpointList: "endpoint_list",
  endpointUpsert: "endpoint_upsert",
  endpointDelete: "endpoint_delete",
  endpointTest: "endpoint_test",
  endpointModels: "endpoint_models",
  roleBindingList: "role_binding_list",
  roleBindingSet: "role_binding_set",
  ingestStart: "ingest_start",
  translationStart: "translation_start",
  translationPause: "translation_pause",
  translationCancel: "translation_cancel",
  reconStart: "recon_start",
  reconGet: "recon_get",
  reconConfirm: "recon_confirm",
  glossaryList: "glossary_list",
  glossaryUpsert: "glossary_upsert",
  glossaryDelete: "glossary_delete",
  seriesList: "series_list",
  seriesCreate: "series_create",
  seriesGet: "series_get",
  seriesUpdate: "series_update",
  seriesDelete: "series_delete",
  projectSetSeries: "project_set_series",
  seriesGlossaryList: "series_glossary_list",
  seriesGlossaryUpsert: "series_glossary_upsert",
  seriesGlossaryDelete: "series_glossary_delete",
  seriesVariantUpsert: "series_variant_upsert",
  seriesVariantDelete: "series_variant_delete",
  seriesPromoteTerm: "series_promote_term",
  seriesExport: "series_export",
  seriesImport: "series_import",
  seriesQaScan: "series_qa_scan",
  reviewStart: "review_start",
  suggestionList: "suggestion_list",
  suggestionAccept: "suggestion_accept",
  suggestionReject: "suggestion_reject",
  qaReport: "qa_report",
  jobList: "job_list",
  chunkList: "chunk_list",
  chunkGet: "chunk_get",
  metricsGet: "metrics_get",
  sidecarStatus: "sidecar_status",
  exportBuild: "export_build",
  exportPreview: "export_preview",
  exportHistory: "export_history",
  openPath: "open_path",
} as const;

/** True when the page runs inside the Tauri webview (false when opened in a plain browser). */
export function isTauriRuntime(): boolean {
  return isTauri();
}

/**
 * Shape Rust's `AppError` serialises to (`crates/app/src/error.rs`). A rejected `invoke()` carries
 * this object, not a string.
 */
interface SerializedAppError {
  code: string;
  message: string;
  retryable: boolean;
}

/** True for the `{ code, message, retryable }` object every `AppError` serialises to. */
function isSerializedAppError(value: unknown): value is SerializedAppError {
  if (value === null || typeof value !== "object") {
    return false;
  }
  const record = value as Record<string, unknown>;
  return typeof record["message"] === "string" && typeof record["code"] === "string";
}

/**
 * Normalises anything a rejected `invoke()` can carry into a displayable message. Rust commands
 * reject with the serialised `AppError` object, so that shape is unwrapped first; otherwise the
 * usual `Error` and string forms are handled and any other value falls back to JSON. Backend text
 * is passed through unchanged.
 */
export function toErrorMessage(error: unknown): string {
  if (isSerializedAppError(error)) {
    return error.code.length > 0 ? `${error.message} (${error.code})` : error.message;
  }
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  if (error === null || error === undefined) {
    return "";
  }
  try {
    const serialised = JSON.stringify(error);
    return serialised === undefined ? String(error) : serialised;
  } catch {
    return String(error);
  }
}

async function call<TResult>(command: string, args?: Record<string, unknown>): Promise<TResult> {
  try {
    return await invoke<TResult>(command, args);
  } catch (error) {
    throw new Error(toErrorMessage(error));
  }
}

// --- projects ------------------------------------------------------------------------------

export function projectList(): Promise<Project[]> {
  return call<Project[]>(COMMANDS.projectList);
}

export function projectCreate(request: CreateProjectRequest): Promise<Project> {
  return call<Project>(COMMANDS.projectCreate, { req: request });
}

export function projectGet(id: string): Promise<ProjectDetail> {
  return call<ProjectDetail>(COMMANDS.projectGet, { id });
}

export function projectDelete(id: string): Promise<Ack> {
  return call<Ack>(COMMANDS.projectDelete, { id });
}

/** Writes the project as a `.llmtz` bundle (PLAN.md §6). */
export function projectExport(request: ExportBundleRequest): Promise<ExportBundleOutcome> {
  return call<ExportBundleOutcome>(COMMANDS.projectExport, { req: request });
}

/** Imports a `.llmtz` bundle; an id that already exists is rejected. */
export function projectImport(request: ImportBundleRequest): Promise<Project> {
  return call<Project>(COMMANDS.projectImport, { req: request });
}

// --- LLM endpoints -------------------------------------------------------------------------

export function endpointList(): Promise<Endpoint[]> {
  return call<Endpoint[]>(COMMANDS.endpointList);
}

export function endpointUpsert(request: EndpointUpsert): Promise<Endpoint> {
  return call<Endpoint>(COMMANDS.endpointUpsert, { req: request });
}

export function endpointDelete(id: string): Promise<Ack> {
  return call<Ack>(COMMANDS.endpointDelete, { id });
}

/** `GET /health` + `GET /props` + `GET /v1/models` against the configured base URL (`PLAN.md` §7.1). */
export function endpointTest(id: string): Promise<EndpointTestResult> {
  return call<EndpointTestResult>(COMMANDS.endpointTest, { id });
}

/** `GET /v1/models` against an endpoint id or an explicit base URL (`PLAN.md` §7.1). */
export function endpointModels(request: EndpointModelsRequest): Promise<ModelInfo[]> {
  return call<ModelInfo[]>(COMMANDS.endpointModels, { req: request });
}

// --- role bindings -------------------------------------------------------------------------

export function roleBindingList(): Promise<RoleBinding[]> {
  return call<RoleBinding[]>(COMMANDS.roleBindingList);
}

export function roleBindingSet(request: RoleBindingSet): Promise<RoleBinding> {
  return call<RoleBinding>(COMMANDS.roleBindingSet, { req: request });
}

// --- ingestion -----------------------------------------------------------------------------

/**
 * Enqueues the `ingest` job and returns its id immediately. The extraction itself runs on the
 * worker pool; follow it through `job_list` / `job://progress`, then read chapters from
 * `project_get` and chunks from `chunk_list`.
 */
export function ingestStart(request: IngestStartRequest): Promise<JobStarted> {
  return call<JobStarted>(COMMANDS.ingestStart, { req: request });
}

// --- translation control -------------------------------------------------------------------

/**
 * Starts (or resumes) the translation queue. Resume is a re-issue of this command rather than a
 * separate `translation_resume`, which is absent from the frozen command table.
 */
export function translationStart(request: TranslationStartRequest): Promise<TranslationStartResult> {
  return call<TranslationStartResult>(COMMANDS.translationStart, { req: request });
}

/** Pauses the worker pool; the queue keeps its state for a later resume. */
export function translationPause(): Promise<Ack> {
  return call<Ack>(COMMANDS.translationPause);
}

/**
 * Cancels the open project's translation run: the pool stops claiming work, in-flight chunks go
 * back to `pending` and unfinished jobs are marked `cancelled`.
 */
export function translationCancel(projectId: string | null): Promise<Ack> {
  return call<Ack>(COMMANDS.translationCancel, { req: { project_id: projectId } });
}

// --- book reconnaissance (PLAN.md §9.4) ------------------------------------------------------

/**
 * Enqueues the `book_recon` job and returns its id immediately. The call runs on the orchestrator
 * role; follow it through `job://progress`, then read the candidate back with `reconGet`.
 */
export function reconStart(request: ReconStartRequest): Promise<JobStarted> {
  return call<JobStarted>(COMMANDS.reconStart, { req: request });
}

/** Candidate profile, already-confirmed values and glossary of a project. */
export function reconGet(projectId: string): Promise<ReconSnapshot> {
  return call<ReconSnapshot>(COMMANDS.reconGet, { project_id: projectId });
}

/** Writes the fields the user confirmed into project memory and the glossary. */
export function reconConfirm(request: ReconConfirmRequest): Promise<ReconSnapshot> {
  return call<ReconSnapshot>(COMMANDS.reconConfirm, { req: request });
}

// --- glossary (PLAN.md §5, §9.2) -------------------------------------------------------------

/** Every term of a project, candidates included. */
export function glossaryList(projectId: string): Promise<GlossaryTerm[]> {
  return call<GlossaryTerm[]>(COMMANDS.glossaryList, { project_id: projectId });
}

/** Create or update a term; the returned row is the persisted one. */
export function glossaryUpsert(request: GlossaryUpsertRequest): Promise<GlossaryTerm> {
  return call<GlossaryTerm>(COMMANDS.glossaryUpsert, { req: request });
}

/** Remove a term. The translator prompt stops seeing it immediately. */
export function glossaryDelete(id: string): Promise<Ack> {
  return call<Ack>(COMMANDS.glossaryDelete, { id });
}

// --- series (PLAN.md §9.5) -----------------------------------------------------------------

/** Every series, ordered by name. */
export function seriesList(): Promise<Series[]> {
  return call<Series[]>(COMMANDS.seriesList);
}

export function seriesCreate(request: SeriesCreateRequest): Promise<Series> {
  return call<Series>(COMMANDS.seriesCreate, { req: request });
}

/** Series row, member books and memory values. */
export function seriesGet(id: string): Promise<SeriesDetail> {
  return call<SeriesDetail>(COMMANDS.seriesGet, { id });
}

/** Updates the row and, when given, the series `style_guide`/`synopsis` memory. */
export function seriesUpdate(request: SeriesUpdateRequest): Promise<Series> {
  return call<Series>(COMMANDS.seriesUpdate, { req: request });
}

/** Deletes the series; its books survive, detached (the glossary cascade goes with it). */
export function seriesDelete(id: string): Promise<Ack> {
  return call<Ack>(COMMANDS.seriesDelete, { id });
}

/** Places a book in a series (or detaches it with `series_id: null`). */
export function projectSetSeries(request: ProjectSetSeriesRequest): Promise<Project> {
  return call<Project>(COMMANDS.projectSetSeries, { req: request });
}

/** The series glossary; the translator prompt sees it merged with the book's own. */
export function seriesGlossaryList(seriesId: string): Promise<SeriesGlossaryTerm[]> {
  return call<SeriesGlossaryTerm[]>(COMMANDS.seriesGlossaryList, { series_id: seriesId });
}

/** Create or update a series term; a change flags every book rendering it differently. */
export function seriesGlossaryUpsert(
  request: SeriesGlossaryUpsertRequest,
): Promise<SeriesGlossaryTerm> {
  return call<SeriesGlossaryTerm>(COMMANDS.seriesGlossaryUpsert, { req: request });
}

export function seriesGlossaryDelete(id: string): Promise<Ack> {
  return call<Ack>(COMMANDS.seriesGlossaryDelete, { id });
}

/** Add a surface form (`the Keeper`) that also triggers the term in a chunk. */
export function seriesVariantUpsert(
  request: SeriesVariantUpsertRequest,
): Promise<SeriesGlossaryVariant> {
  return call<SeriesGlossaryVariant>(COMMANDS.seriesVariantUpsert, { req: request });
}

export function seriesVariantDelete(id: string): Promise<Ack> {
  return call<Ack>(COMMANDS.seriesVariantDelete, { id });
}

/** Copies a book term into its series; a differing canon rendering is flagged, not overwritten. */
export function seriesPromoteTerm(request: SeriesPromoteRequest): Promise<PromoteOutcome> {
  return call<PromoteOutcome>(COMMANDS.seriesPromoteTerm, { req: request });
}

/** Writes the series canon as a `.llmtsz` bundle (mergeable on import). */
export function seriesExport(request: SeriesExportRequest): Promise<SeriesExportOutcome> {
  return call<SeriesExportOutcome>(COMMANDS.seriesExport, { req: request });
}

/** Merges a series bundle: never overwrites a differing rendering. */
export function seriesImport(request: SeriesImportRequest): Promise<SeriesImportOutcome> {
  return call<SeriesImportOutcome>(COMMANDS.seriesImport, { req: request });
}

/** Re-runs the QA scan on every translated chunk of the member books. */
export function seriesQaScan(seriesId: string): Promise<SeriesQaScanResult> {
  return call<SeriesQaScanResult>(COMMANDS.seriesQaScan, { series_id: seriesId });
}

// --- review and QA (PLAN.md §11.4) -----------------------------------------------------------

/**
 * Enqueues the review passes for the eligible chunks of a project. A pending equivalent job is
 * not duplicated; follow the work through `job://progress`.
 */
export function reviewStart(request: ReviewStartRequest): Promise<ReviewStartResult> {
  return call<ReviewStartResult>(COMMANDS.reviewStart, { req: request });
}

/** `suggestion` rows of a project, candidates included unless filtered. */
export function suggestionList(request: SuggestionListRequest): Promise<Suggestion[]> {
  return call<Suggestion[]>(COMMANDS.suggestionList, { req: request });
}

/** Accept a proposal: the block translation is rewritten and the chunk recomposed. */
export function suggestionAccept(id: string): Promise<Suggestion> {
  return call<Suggestion>(COMMANDS.suggestionAccept, { id });
}

/** Reject a proposal without touching the translation. */
export function suggestionReject(id: string): Promise<Suggestion> {
  return call<Suggestion>(COMMANDS.suggestionReject, { id });
}

/** `qa_finding` rows of a project, filterable by kind, severity and chunk. */
export function qaReport(request: QaReportRequest): Promise<QaFinding[]> {
  return call<QaFinding[]>(COMMANDS.qaReport, { req: request });
}

// --- jobs and chunks -----------------------------------------------------------------------

export function jobList(request: JobListRequest = {}): Promise<Job[]> {
  return call<Job[]>(COMMANDS.jobList, { req: request });
}

export function chunkList(request: ChunkListRequest): Promise<Chunk[]> {
  return call<Chunk[]>(COMMANDS.chunkList, { req: request });
}

export function chunkGet(chunkId: string): Promise<ChunkDetail> {
  return call<ChunkDetail>(COMMANDS.chunkGet, { chunk_id: chunkId });
}

// --- metrics and sidecar -------------------------------------------------------------------

export function metricsGet(): Promise<Metrics> {
  return call<Metrics>(COMMANDS.metricsGet);
}

export function sidecarStatus(): Promise<SidecarStatus> {
  return call<SidecarStatus>(COMMANDS.sidecarStatus);
}

// --- export --------------------------------------------------------------------------------

export function exportBuild(request: ExportRequest): Promise<ExportOutcome> {
  return call<ExportOutcome>(COMMANDS.exportBuild, { req: request });
}

/** The composed units and the `metadata.yaml` a build would use, without invoking Pandoc. */
export function exportPreview(request: ExportPreviewRequest): Promise<ExportPreview> {
  return call<ExportPreview>(COMMANDS.exportPreview, { req: request });
}

/** The recent build records, newest first. */
export function exportHistory(projectId: string): Promise<ExportBuildRecord[]> {
  return call<ExportBuildRecord[]>(COMMANDS.exportHistory, { project_id: projectId });
}

/** Opens a file or directory with the OS handler; the only filesystem command the UI needs. */
export function openPath(path: string): Promise<Ack> {
  return call<Ack>(COMMANDS.openPath, { path });
}
