/**
 * Typed wrappers over Tauri `invoke()`.
 *
 * This module is the **only** place in the frontend where a Tauri command name appears as a
 * string literal. Command names and their argument keys are frozen by `AGENTS.md`
 * ("UI -> Tauri"); argument keys use `snake_case` because that is what the Rust command
 * signatures declare.
 *
 * There is no direct network access anywhere in the UI: every backend interaction goes through
 * `invoke` here or through `listen` in `lib/events.ts` (`PLAN.md` §2, `AGENTS.md` §TypeScript).
 */

import { invoke, isTauri } from "@tauri-apps/api/core";
import type {
  ChunkDetail,
  ChunkListRequest,
  ChunkSummary,
  Endpoint,
  EndpointModel,
  EndpointTestResult,
  EndpointUpsertRequest,
  ExportBuildRequest,
  ExportBuildResult,
  IngestResult,
  IngestStartRequest,
  Job,
  JobListRequest,
  Metrics,
  Project,
  ProjectCreateRequest,
  QueueAck,
  RoleBinding,
  RoleBindingListRequest,
  RoleBindingSetRequest,
  SidecarStatus,
  TranslationControlRequest,
  TranslationStartRequest,
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
 * Normalises anything a rejected `invoke()` can carry (Rust serialises command errors as plain
 * strings) into a displayable message. Backend text is passed through unchanged.
 */
export function toErrorMessage(error: unknown): string {
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

async function callVoid(command: string, args?: Record<string, unknown>): Promise<void> {
  await call<null>(command, args);
}

// --- projects ------------------------------------------------------------------------------

export function projectList(): Promise<Project[]> {
  return call<Project[]>(COMMANDS.projectList);
}

export function projectCreate(request: ProjectCreateRequest): Promise<Project> {
  return call<Project>(COMMANDS.projectCreate, {
    name: request.name,
    source_path: request.source_path,
    source_format: request.source_format,
    source_lang: request.source_lang,
    target_lang: request.target_lang,
  });
}

export function projectGet(projectId: string): Promise<Project> {
  return call<Project>(COMMANDS.projectGet, { project_id: projectId });
}

export function projectDelete(projectId: string): Promise<void> {
  return callVoid(COMMANDS.projectDelete, { project_id: projectId });
}

// --- LLM endpoints -------------------------------------------------------------------------

export function endpointList(): Promise<Endpoint[]> {
  return call<Endpoint[]>(COMMANDS.endpointList);
}

export function endpointUpsert(request: EndpointUpsertRequest): Promise<Endpoint> {
  return call<Endpoint>(COMMANDS.endpointUpsert, {
    id: request.id,
    name: request.name,
    base_url: request.base_url,
    api_key_ref: request.api_key_ref,
    max_concurrency: request.max_concurrency,
    notes: request.notes,
  });
}

export function endpointDelete(endpointId: string): Promise<void> {
  return callVoid(COMMANDS.endpointDelete, { endpoint_id: endpointId });
}

/** `GET /health` + `GET /props` against the configured base URL (`PLAN.md` §7.1). */
export function endpointTest(endpointId: string): Promise<EndpointTestResult> {
  return call<EndpointTestResult>(COMMANDS.endpointTest, { endpoint_id: endpointId });
}

/** `GET /v1/models` against the configured base URL (`PLAN.md` §7.1). */
export function endpointModels(endpointId: string): Promise<EndpointModel[]> {
  return call<EndpointModel[]>(COMMANDS.endpointModels, { endpoint_id: endpointId });
}

// --- role bindings -------------------------------------------------------------------------

export function roleBindingList(filter: RoleBindingListRequest = {}): Promise<RoleBinding[]> {
  return call<RoleBinding[]>(COMMANDS.roleBindingList, {
    endpoint_id: filter.endpoint_id ?? null,
    role: filter.role ?? null,
  });
}

export function roleBindingSet(request: RoleBindingSetRequest): Promise<RoleBinding> {
  return call<RoleBinding>(COMMANDS.roleBindingSet, {
    endpoint_id: request.endpoint_id,
    role: request.role,
    model: request.model,
    params: request.params,
    priority: request.priority,
  });
}

// --- ingestion -----------------------------------------------------------------------------

/** Runs `detect_format` + `ingest` + `parse_document` and enqueues the chunk-building jobs. */
export function ingestStart(request: IngestStartRequest): Promise<IngestResult> {
  return call<IngestResult>(COMMANDS.ingestStart, {
    project_id: request.project_id,
    path: request.path,
    pdf_backend: request.pdf_backend ?? null,
  });
}

// --- translation control -------------------------------------------------------------------

/**
 * Starts (or resumes) the translation queue. Resume is a re-issue of this command rather than a
 * separate `translation_resume`, which is absent from the frozen command table.
 */
export function translationStart(request: TranslationStartRequest): Promise<QueueAck> {
  return call<QueueAck>(COMMANDS.translationStart, {
    project_id: request.project_id,
    chunk_ids: request.chunk_ids ?? null,
    model: request.model ?? null,
  });
}

/** Stops the scheduler and releases the leases; the queue keeps its state for a later resume. */
export function translationPause(request: TranslationControlRequest): Promise<QueueAck> {
  return call<QueueAck>(COMMANDS.translationPause, {
    project_id: request.project_id,
    chunk_ids: request.chunk_ids ?? null,
  });
}

/** Cancels the pending jobs of the project, or of the given chunks only. */
export function translationCancel(request: TranslationControlRequest): Promise<QueueAck> {
  return call<QueueAck>(COMMANDS.translationCancel, {
    project_id: request.project_id,
    chunk_ids: request.chunk_ids ?? null,
  });
}

// --- jobs and chunks -----------------------------------------------------------------------

export function jobList(filter: JobListRequest = {}): Promise<Job[]> {
  return call<Job[]>(COMMANDS.jobList, {
    project_id: filter.project_id ?? null,
    state: filter.state ?? null,
    kind: filter.kind ?? null,
    limit: filter.limit ?? null,
  });
}

export function chunkList(filter: ChunkListRequest): Promise<ChunkSummary[]> {
  return call<ChunkSummary[]>(COMMANDS.chunkList, {
    project_id: filter.project_id,
    chapter_id: filter.chapter_id ?? null,
    status: filter.status ?? null,
    limit: filter.limit ?? null,
    offset: filter.offset ?? null,
  });
}

export function chunkGet(chunkId: string): Promise<ChunkDetail> {
  return call<ChunkDetail>(COMMANDS.chunkGet, { chunk_id: chunkId });
}

// --- metrics and sidecar -------------------------------------------------------------------

export function metricsGet(projectId: string | null = null): Promise<Metrics> {
  return call<Metrics>(COMMANDS.metricsGet, { project_id: projectId });
}

export function sidecarStatus(): Promise<SidecarStatus> {
  return call<SidecarStatus>(COMMANDS.sidecarStatus);
}

// --- export --------------------------------------------------------------------------------

export function exportBuild(request: ExportBuildRequest): Promise<ExportBuildResult> {
  return call<ExportBuildResult>(COMMANDS.exportBuild, {
    project_id: request.project_id,
    output_format: request.output_format,
    template: request.template,
    css: request.css,
    units: request.units,
    output_path: request.output_path,
  });
}

/** Opens a file or directory with the OS handler; the only filesystem command the UI needs. */
export function openPath(path: string): Promise<void> {
  return callVoid(COMMANDS.openPath, { path });
}
