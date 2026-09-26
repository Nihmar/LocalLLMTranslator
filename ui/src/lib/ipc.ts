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
  ExportRequest,
  IngestStartRequest,
  Job,
  JobListRequest,
  JobStarted,
  Metrics,
  ModelInfo,
  Project,
  ProjectDetail,
  RoleBinding,
  RoleBindingSet,
  SidecarStatus,
  TranslationStartRequest,
  TranslationStartResult,
} from "./types";

/** Frozen command surface (`AGENTS.md` -> "UI -> Tauri"). */
const COMMANDS = {
  projectList: "project_list",
  projectCreate: "project_create",
  projectGet: "project_get",
  projectDelete: "project_delete",
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
  jobList: "job_list",
  chunkList: "chunk_list",
  chunkGet: "chunk_get",
  metricsGet: "metrics_get",
  sidecarStatus: "sidecar_status",
  exportBuild: "export_build",
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

/** Opens a file or directory with the OS handler; the only filesystem command the UI needs. */
export function openPath(path: string): Promise<Ack> {
  return call<Ack>(COMMANDS.openPath, { path });
}
