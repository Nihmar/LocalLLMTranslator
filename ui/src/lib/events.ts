/**
 * Typed `listen()` helpers for the frozen Tauri event surface (`PLAN.md` §12.2).
 *
 * This module is the **only** place in the frontend where an event name appears as a string
 * literal. Every helper returns an unsubscribe function so React effects can clean up:
 *
 * ```ts
 * useEffect(() => onJobProgress(handleProgress), [handleProgress]);
 * ```
 *
 * The returned function is safe to call twice and safe to call before the underlying
 * `listen()` promise has settled — a component that unmounts immediately still detaches the
 * listener instead of leaking it.
 */

import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { apiHeaders, parseBody } from "./http.ts";
import { isTauriRuntime } from "./ipc.ts";
import type {
  ExportProgressEvent,
  JobProgressEvent,
  LogLineEvent,
  MetricsTickEvent,
  SidecarStatus,
} from "./types";

/** Frozen event surface (`PLAN.md` §12.2). */
export const EVENTS = {
  jobProgress: "job://progress",
  logLine: "log://line",
  metricsTick: "metrics://tick",
  sidecarStatus: "sidecar://status",
  exportProgress: "export://progress",
} as const;

export type EventName = (typeof EVENTS)[keyof typeof EVENTS];

/** Idempotent teardown handle returned by every subscription helper. */
export type Unsubscribe = () => void;

/**
 * Subscribes to a Tauri event and returns the teardown handle.
 *
 * Failures to attach (for example when the page is opened in a plain browser instead of the
 * Tauri webview) are reported on the console and degrade to a no-op unsubscribe, so views keep
 * rendering instead of crashing.
 */
export function subscribe<TPayload>(
  event: EventName,
  handler: (payload: TPayload) => void,
): Unsubscribe {
  if (!isTauriRuntime()) {
    // The headless server streams the same events over SSE (`PLAN.md` §12.3).
    return subscribeStream(event, (payload) => handler(payload as TPayload));
  }
  let unlisten: UnlistenFn | null = null;
  let disposed = false;

  void listen<TPayload>(event, (message) => {
    handler(message.payload);
  })
    .then((detach) => {
      if (disposed) {
        detach();
        return;
      }
      unlisten = detach;
    })
    .catch((error: unknown) => {
      console.error(`[events] could not subscribe to ${event}`, error);
    });

  return () => {
    disposed = true;
    if (unlisten !== null) {
      unlisten();
      unlisten = null;
    }
  };
}

// --- SSE transport (no Tauri shell) ---------------------------------------------------------

/** How long to wait before reopening the event stream after a drop. */
const SSE_RECONNECT_MS = 2000;

type SseHandler = (payload: unknown) => void;

/** All subscribers of the one event stream, by event name. */
const sseHandlers = new Map<string, Set<SseHandler>>();
let sseLoop: Promise<void> | null = null;

/** Joins the shared event stream; the stream itself outlives individual subscribers. */
function subscribeStream(event: string, handler: SseHandler): Unsubscribe {
  let handlers = sseHandlers.get(event);
  if (handlers === undefined) {
    handlers = new Set();
    sseHandlers.set(event, handlers);
  }
  handlers.add(handler);
  ensureEventStream();
  return () => {
    const current = sseHandlers.get(event);
    current?.delete(handler);
    if (current !== undefined && current.size === 0) {
      sseHandlers.delete(event);
    }
  };
}

/** Starts the reconnecting reader once; every subscriber shares it. */
function ensureEventStream(): void {
  if (sseLoop !== null) {
    return;
  }
  sseLoop = (async () => {
    for (;;) {
      try {
        await consumeEventStream();
      } catch (error) {
        console.error("[events] event stream interrupted, retrying", error);
      }
      await new Promise((resolve) => setTimeout(resolve, SSE_RECONNECT_MS));
    }
  })();
}

/** Reads `GET /api/events` and dispatches each frame to the subscribers of its event name. */
async function consumeEventStream(): Promise<void> {
  const response = await fetch("/api/events", { headers: apiHeaders() });
  if (!response.ok || response.body === null) {
    throw new Error(`the event stream answered HTTP ${response.status}`);
  }
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) {
      return;
    }
    buffer = (buffer + decoder.decode(value, { stream: true })).replace(/\r\n/g, "\n");
    let boundary = buffer.indexOf("\n\n");
    while (boundary >= 0) {
      dispatchFrame(buffer.slice(0, boundary));
      buffer = buffer.slice(boundary + 2);
      boundary = buffer.indexOf("\n\n");
    }
  }
}

/** One Server-Sent Event frame (`event:` + `data:` lines). */
function dispatchFrame(frame: string): void {
  let name = "message";
  const data: string[] = [];
  for (const line of frame.split("\n")) {
    if (line.startsWith(":")) {
      continue; // a keep-alive comment
    }
    const colon = line.indexOf(":");
    const field = colon >= 0 ? line.slice(0, colon) : line;
    const value = colon >= 0 ? line.slice(colon + 1).replace(/^ /, "") : "";
    if (field === "event") {
      name = value;
    } else if (field === "data") {
      data.push(value);
    }
  }
  const handlers = sseHandlers.get(name);
  if (data.length === 0 || handlers === undefined || handlers.size === 0) {
    return;
  }
  const payload = parseBody(data.join("\n"));
  for (const handler of handlers) {
    handler(payload);
  }
}

/**
 * `job://progress` — per-job progress. The payload is the serialized `Job` row (see
 * `JobProgressEvent`), the same shape `job_list` returns, but it says nothing about the other
 * rows: consumers still treat it as an invalidation trigger and refetch through `job_list` /
 * `chunk_list`.
 */
export function onJobProgress(handler: (payload: JobProgressEvent) => void): Unsubscribe {
  return subscribe<JobProgressEvent>(EVENTS.jobProgress, handler);
}

/** `log://line` — structured log line from the Rust control plane. */
export function onLogLine(handler: (payload: LogLineEvent) => void): Unsubscribe {
  return subscribe<LogLineEvent>(EVENTS.logLine, handler);
}

/** `metrics://tick` — periodic resource and queue snapshot (see `MetricsTickEvent`). */
export function onMetricsTick(handler: (payload: MetricsTickEvent) => void): Unsubscribe {
  return subscribe<MetricsTickEvent>(EVENTS.metricsTick, handler);
}

/** `sidecar://status` — sidecar supervisor state changes. */
export function onSidecarStatus(handler: (payload: SidecarStatus) => void): Unsubscribe {
  return subscribe<SidecarStatus>(EVENTS.sidecarStatus, handler);
}

/** `export://progress` — per-unit progress of a Pandoc build. */
export function onExportProgress(handler: (payload: ExportProgressEvent) => void): Unsubscribe {
  return subscribe<ExportProgressEvent>(EVENTS.exportProgress, handler);
}
