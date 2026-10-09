import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { onLogLine } from "../lib/events";
import { countLabel, formatClock } from "../lib/format";
import type { LogLevel, LogLineEvent } from "../lib/types";

/**
 * Live log pane fed by `log://line`.
 *
 * Two properties matter here:
 *
 * 1. **Bounded buffer.** The pane keeps at most `limit` lines and drops the oldest ones, so a
 *    long translation run cannot grow the renderer's memory without bound.
 * 2. **Batched rendering.** Lines are queued and flushed on a short interval instead of causing
 *    one React commit per event: a chatty backend does not starve the UI.
 *
 * The component owns its subscription so every consumer gets the same bounded behaviour; in
 * exchange it never renders more than `limit` lines.
 */

export interface LogViewProps {
  /** Maximum retained lines. Older lines are discarded. */
  limit?: number | undefined;
  title?: string | undefined;
  /** Grows to fill the parent instead of using `heightClass`. */
  heightClass?: string | undefined;
  /** Rendered inside the header, e.g. a pause button. */
  headerExtra?: ReactNode | undefined;
}

const LEVEL_ORDER: Readonly<Record<string, number>> = {
  trace: 0,
  debug: 1,
  info: 2,
  warn: 3,
  error: 4,
};

/** Unknown levels (the control plane sends `String`) count as `info`. */
function levelOrder(level: string): number {
  return LEVEL_ORDER[level] ?? LEVEL_ORDER["info"] ?? 0;
}

const FLUSH_INTERVAL_MS = 220;

function pushBounded(
  current: readonly LogLineEvent[],
  incoming: readonly LogLineEvent[],
  limit: number,
): LogLineEvent[] {
  if (incoming.length === 0) {
    return current as LogLineEvent[];
  }
  const merged = [...current, ...incoming];
  return merged.length > limit ? merged.slice(merged.length - limit) : merged;
}

export function LogView({
  limit = 400,
  title = "Log live",
  heightClass = "h-72",
  headerExtra,
}: LogViewProps) {
  const [lines, setLines] = useState<LogLineEvent[]>([]);
  const [minLevel, setMinLevel] = useState<LogLevel>("info");
  const [follow, setFollow] = useState(true);
  const [paused, setPaused] = useState(false);

  const pendingRef = useRef<LogLineEvent[]>([]);
  const pausedRef = useRef(paused);
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    pausedRef.current = paused;
  }, [paused]);

  useEffect(() => {
    const unsubscribe = onLogLine((line) => {
      if (pausedRef.current) {
        return;
      }
      pendingRef.current.push(line);
    });

    const timer = window.setInterval(() => {
      if (pendingRef.current.length === 0) {
        return;
      }
      const batch = pendingRef.current;
      pendingRef.current = [];
      setLines((current) => pushBounded(current, batch, limit));
    }, FLUSH_INTERVAL_MS);

    return () => {
      unsubscribe();
      window.clearInterval(timer);
      pendingRef.current = [];
    };
  }, [limit]);

  useEffect(() => {
    if (!follow) {
      return;
    }
    const node = scrollRef.current;
    if (node !== null) {
      node.scrollTop = node.scrollHeight;
    }
  }, [lines, follow]);

  const visible = useMemo(() => {
    const threshold = levelOrder(minLevel);
    return lines.filter((line) => levelOrder(line.level) >= threshold);
  }, [lines, minLevel]);

  const handleJumpToEnd = useCallback(() => {
    setFollow(true);
    const node = scrollRef.current;
    if (node !== null) {
      node.scrollTop = node.scrollHeight;
    }
  }, []);

  const handleClear = useCallback(() => {
    pendingRef.current = [];
    setLines([]);
  }, []);

  return (
    <div className="panel flex min-h-0 flex-col overflow-hidden">
      <div className="panel-head">
        <span className="flex items-center gap-2">
          <span className="panel-title">{title}</span>
          <span className="mono-chip">
            {countLabel(visible.length, "riga", "righe")}
            {visible.length !== lines.length ? ` / ${lines.length}` : ""}
          </span>
          {paused ? <span className="badge badge-warning">In pausa</span> : null}
        </span>

        <span className="flex flex-wrap items-center gap-1.5">
          {headerExtra}
          <label className="flex items-center gap-1 text-xs text-muted">
            Livello
            <select
              className="select"
              style={{ width: "auto" }}
              value={minLevel}
              onChange={(event) => {
                setMinLevel(event.target.value as LogLevel);
              }}
              aria-label="Livello minimo dei log"
            >
              <option value="trace">tutto</option>
              <option value="debug">debug</option>
              <option value="info">info</option>
              <option value="warn">avvisi</option>
              <option value="error">errori</option>
            </select>
          </label>

          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => {
              setPaused((value) => !value);
            }}
          >
            {paused ? "Riprendi" : "Pausa"}
          </button>
          <button type="button" className="btn btn-sm btn-ghost" onClick={handleJumpToEnd}>
            In fondo
          </button>
          <button type="button" className="btn btn-sm btn-ghost" onClick={handleClear}>
            Svuota
          </button>
        </span>
      </div>

      <div
        ref={scrollRef}
        className={`log-scroll ${heightClass}`}
        onScroll={(event) => {
          const node = event.currentTarget;
          const atBottom = node.scrollHeight - node.scrollTop - node.clientHeight < 24;
          if (atBottom !== follow) {
            setFollow(atBottom);
          }
        }}
      >
        {visible.length === 0 ? (
          <p className="px-3 py-4 text-center text-[0.72rem] text-faint">
            Nessuna riga di log da mostrare. Compariranno qui durante l&apos;esecuzione.
          </p>
        ) : (
          visible.map((line, index) => (
            <div
              key={`${line.ts}-${line.source}-${String(index)}`}
              className="log-line"
              data-level={line.level}
            >
              <span className="log-ts">{formatClock(line.ts)}</span>
              <span className={`log-level-${line.level}`}>{line.level}</span>
              <span className="log-source" title={line.source}>
                {line.source}
              </span>
              <span className="log-message">{line.message}</span>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
