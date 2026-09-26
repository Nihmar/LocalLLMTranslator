/**
 * Presentation helpers for numbers, sizes, durations and timestamps.
 *
 * Everything is implemented with the platform `Intl` data already shipped in the webview: no
 * remote fonts, no locale bundles to download, and the app stays usable offline.
 * Every helper is total: an unusable input renders as an em dash instead of throwing.
 */

const EM_DASH = "—";

const numberFormatter = new Intl.NumberFormat("it-IT");
const decimalFormatter = new Intl.NumberFormat("it-IT", {
  minimumFractionDigits: 1,
  maximumFractionDigits: 1,
});
const decimal2Formatter = new Intl.NumberFormat("it-IT", {
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const dateTimeFormatter = new Intl.DateTimeFormat("it-IT", {
  dateStyle: "short",
  timeStyle: "short",
});

function isUsable(value: number | null | undefined): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

/** Clamps `value` into `[min, max]`. */
export function clamp(value: number, min: number, max: number): number {
  if (Number.isNaN(value)) {
    return min;
  }
  return Math.min(Math.max(value, min), max);
}

/** Raw integer with Italian thousands separators: `1234 -> "1.234"`. */
export function formatNumber(value: number | null | undefined): string {
  if (!isUsable(value)) {
    return EM_DASH;
  }
  return numberFormatter.format(value);
}

/** Fixed-point decimal with Italian separators: `0.5 -> "0,5"`. */
export function formatDecimal(value: number | null | undefined, digits: 1 | 2 = 1): string {
  if (!isUsable(value)) {
    return EM_DASH;
  }
  return digits === 1 ? decimalFormatter.format(value) : decimal2Formatter.format(value);
}

/** `done / total` as a 0-100 number; `total <= 0` yields `0`. */
export function percent(value: number, total: number): number {
  if (!isUsable(value) || !isUsable(total) || total <= 0) {
    return 0;
  }
  return clamp((value / total) * 100, 0, 100);
}

/** Percentage string, e.g. `"42%"`. Accepts an already-computed 0-100 number. */
export function formatPercent(value: number | null | undefined, digits: 0 | 1 = 0): string {
  if (!isUsable(value)) {
    return EM_DASH;
  }
  const rounded = clamp(value, 0, 100);
  return digits === 0
    ? `${numberFormatter.format(Math.round(rounded))}%`
    : `${decimalFormatter.format(rounded)}%`;
}

const BYTE_UNITS = ["B", "KiB", "MiB", "GiB", "TiB"] as const;

/** Byte count in binary units: `1610612736 -> "1,5 GiB"`. */
export function formatBytes(bytes: number | null | undefined): string {
  if (!isUsable(bytes) || bytes < 0) {
    return EM_DASH;
  }
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < BYTE_UNITS.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const unit = BYTE_UNITS[unitIndex] ?? "B";
  const digits = unitIndex === 0 ? 0 : value >= 100 ? 0 : 1;
  return `${digits === 0 ? numberFormatter.format(Math.round(value)) : decimalFormatter.format(value)} ${unit}`;
}

/** Token count with an explicit unit: `12345 -> "12.345 tok"`. */
export function formatTokens(tokens: number | null | undefined): string {
  if (!isUsable(tokens)) {
    return EM_DASH;
  }
  return `${numberFormatter.format(Math.round(tokens))} tok`;
}

function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

/**
 * Human duration, coarse to fine: `"340 ms"`, `"8,4 s"`, `"12 min 30 s"`, `"1 h 05 min"`.
 * `null`, negative and non-finite inputs render as an em dash.
 */
export function formatDuration(ms: number | null | undefined): string {
  if (!isUsable(ms) || ms < 0) {
    return EM_DASH;
  }
  if (ms < 1000) {
    return `${numberFormatter.format(Math.round(ms))} ms`;
  }
  const totalSeconds = ms / 1000;
  if (totalSeconds < 60) {
    return `${decimalFormatter.format(totalSeconds)} s`;
  }
  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) {
    const seconds = Math.floor(totalSeconds % 60);
    return `${formatNumber(totalMinutes)} min ${pad2(seconds)} s`;
  }
  const totalHours = Math.floor(totalMinutes / 60);
  if (totalHours < 24) {
    const minutes = totalMinutes % 60;
    return `${formatNumber(totalHours)} h ${pad2(minutes)} min`;
  }
  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  return `${formatNumber(days)} g ${pad2(hours)} h`;
}

/**
 * Estimated time remaining, prefixed so it cannot be mistaken for a measurement: `"≈ 12 min 30 s"`.
 * An unusable estimate (no completions yet, unknown total, zero elapsed) renders as an em dash.
 */
export function formatEta(ms: number | null | undefined): string {
  if (!isUsable(ms) || ms < 0) {
    return EM_DASH;
  }
  return `≈ ${formatDuration(ms)}`;
}

/**
 * Remaining time derived from *observed* throughput:
 * `elapsed / done * (total - done)`. Returns `null` whenever the extrapolation would be a
 * guess — nothing done yet, no total, or no elapsed time.
 */
export function estimateRemainingMs(
  done: number,
  total: number,
  elapsedMs: number,
): number | null {
  if (!isUsable(done) || !isUsable(total) || !isUsable(elapsedMs)) {
    return null;
  }
  if (done <= 0 || elapsedMs <= 0 || total <= done) {
    return null;
  }
  const remaining = total - done;
  return (elapsedMs / done) * remaining;
}

/** Chunks completed per minute, from observed throughput. `null` when not yet measurable. */
export function chunksPerMinute(done: number, elapsedMs: number): number | null {
  if (!isUsable(done) || !isUsable(elapsedMs) || done <= 0 || elapsedMs <= 0) {
    return null;
  }
  return done / (elapsedMs / 60_000);
}

/** Chunks per minute rendered as `"2,50 chunk/min"`. */
export function formatThroughput(done: number, elapsedMs: number): string {
  const rate = chunksPerMinute(done, elapsedMs);
  if (rate === null) {
    return EM_DASH;
  }
  return `${decimal2Formatter.format(rate)} chunk/min`;
}

/** Parses an ISO timestamp; returns `null` when the string is absent or unparseable. */
export function parseTimestamp(iso: string | null | undefined): Date | null {
  if (typeof iso !== "string" || iso.length === 0) {
    return null;
  }
  const date = new Date(iso);
  return Number.isNaN(date.getTime()) ? null : date;
}

/** Local short date + time: `"26/09/26, 10:45"`. */
export function formatDateTime(iso: string | null | undefined): string {
  const date = parseTimestamp(iso);
  if (date === null) {
    return EM_DASH;
  }
  return dateTimeFormatter.format(date);
}

/** Local wall-clock time with milliseconds, for log lines: `"10:45:03.412"`. */
export function formatClock(iso: string | null | undefined): string {
  const date = parseTimestamp(iso);
  if (date === null) {
    return "--:--:--";
  }
  return `${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}.${String(
    date.getMilliseconds(),
  ).padStart(3, "0")}`;
}

/**
 * Coarse "time ago" in Italian. Hand-rolled instead of `Intl.RelativeTimeFormat` so the wording
 * is exactly what the UI convention calls for.
 */
export function formatRelative(
  iso: string | null | undefined,
  now: number = Date.now(),
): string {
  const date = parseTimestamp(iso);
  if (date === null) {
    return EM_DASH;
  }
  const deltaSeconds = Math.round((now - date.getTime()) / 1000);
  if (deltaSeconds < 0) {
    return "adesso";
  }
  if (deltaSeconds < 45) {
    return "adesso";
  }
  if (deltaSeconds < 3600) {
    return `${formatNumber(Math.round(deltaSeconds / 60))} min fa`;
  }
  if (deltaSeconds < 86_400) {
    return `${formatNumber(Math.round(deltaSeconds / 3600))} h fa`;
  }
  return `${formatNumber(Math.round(deltaSeconds / 86_400))} g fa`;
}

/** Identifier shortened for table cells: `"b000417" -> "b000417"`, long ids get an ellipsis. */
export function shortId(id: string | null | undefined, length = 10): string {
  if (typeof id !== "string" || id.length === 0) {
    return EM_DASH;
  }
  return id.length <= length ? id : `${id.slice(0, length - 1)}…`;
}

/** Truncates on a character boundary and appends an ellipsis. */
export function truncate(text: string | null | undefined, max: number): string {
  if (typeof text !== "string") {
    return EM_DASH;
  }
  if (text.length <= max) {
    return text;
  }
  return `${text.slice(0, Math.max(0, max - 1))}…`;
}

/** Joins non-empty parts with a separator; `["a", null, "b"] -> "a · b"`. */
export function joinParts(parts: ReadonlyArray<string | null | undefined>, separator = " · "): string {
  return parts.filter((part): part is string => typeof part === "string" && part.length > 0).join(separator);
}

/** File extension (lower-case, without the dot) or `null` when there is none. */
export function fileExtension(path: string): string | null {
  const trimmed = path.trim();
  const lastSegmentIndex = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const basename = trimmed.slice(lastSegmentIndex + 1);
  const dotIndex = basename.lastIndexOf(".");
  if (dotIndex <= 0 || dotIndex === basename.length - 1) {
    return null;
  }
  return basename.slice(dotIndex + 1).toLowerCase();
}

/** Last path segment, for compact display: `"/a/b/book.epub" -> "book.epub"`. */
export function basename(path: string | null | undefined): string {
  if (typeof path !== "string" || path.length === 0) {
    return EM_DASH;
  }
  const lastSegmentIndex = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  const name = path.slice(lastSegmentIndex + 1);
  return name.length > 0 ? name : EM_DASH;
}

/** `1` -> `"1 elemento"`, `n` -> `"n elementi"` (invariant noun, as Italian requires here). */
export function countLabel(count: number, singular: string, plural: string): string {
  return `${formatNumber(count)} ${count === 1 ? singular : plural}`;
}
