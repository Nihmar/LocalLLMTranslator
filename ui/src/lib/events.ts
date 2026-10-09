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
