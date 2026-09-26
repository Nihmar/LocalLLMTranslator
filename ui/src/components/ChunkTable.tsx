import { useMemo, useState } from "react";
import { formatNumber, truncate } from "../lib/format";
import type { ChunkStatus, ChunkSummary } from "../lib/types";
import { StatusBadge } from "./StatusBadge";

/**
 * Chunk table for the translation page.
 *
 * Columns: order, chapter, kind/flags, token estimate, status, model, attempts, row actions.
 * The chunk entity has no `kind` column (`PLAN.md` §4.3): the structural information lives in
 * `flags`, so that column renders the flags themselves and falls back to "testo".
 */

export interface ChunkTableProps {
  chunks: readonly ChunkSummary[];
  selection: ReadonlySet<string>;
  onSelectionChange: (next: ReadonlySet<string>) => void;
  onRetry: (chunkId: string) => void;
  onSkip: (chunkId: string) => void;
  /** Opens the read-only original/target drawer. */
  onOpenDetails?: ((chunkId: string) => void) | undefined;
  /** Rows with a command in flight: their actions are disabled. */
  busyIds?: ReadonlySet<string> | undefined;
  /** Rows touched by the last `job://progress` event, briefly highlighted. */
  highlightedIds?: ReadonlySet<string> | undefined;
}

type SortKey = "order" | "chapter" | "tokens" | "status" | "model" | "attempts";

interface SortState {
  key: SortKey;
  direction: "asc" | "desc";
}

/** Order in which statuses are worth looking at: what needs attention comes first. */
const STATUS_RANK: Readonly<Record<ChunkStatus, number>> = {
  running: 0,
  pending: 1,
  failed: 2,
  needs_review: 3,
  done: 4,
};

/** Italian rendering of the chunker flags (`PLAN.md` §4.3). Unknown flags pass through. */
export function describeFlag(flag: string): string {
  if (flag === "table") {
    return "tabella";
  }
  if (flag === "continues") {
    return "prosegue";
  }
  if (flag === "oversized") {
    return "sovradimensionato";
  }
  if (flag.startsWith("table_part:")) {
    return `tabella ${flag.slice("table_part:".length)}`;
  }
  return flag;
}

function compareChunks(left: ChunkSummary, right: ChunkSummary, key: SortKey): number {
  switch (key) {
    case "order":
      return left.order_index - right.order_index;
    case "chapter": {
      const byChapter = (left.chapter_title ?? "").localeCompare(right.chapter_title ?? "", "it");
      return byChapter !== 0 ? byChapter : left.order_index - right.order_index;
    }
    case "tokens":
      return left.token_estimate - right.token_estimate;
    case "status": {
      const byStatus = (STATUS_RANK[left.status] ?? 99) - (STATUS_RANK[right.status] ?? 99);
      return byStatus !== 0 ? byStatus : left.order_index - right.order_index;
    }
    case "model": {
      const byModel = (left.model_id ?? "").localeCompare(right.model_id ?? "", "it");
      return byModel !== 0 ? byModel : left.order_index - right.order_index;
    }
    case "attempts": {
      const byAttempts = left.attempts - right.attempts;
      return byAttempts !== 0 ? byAttempts : left.order_index - right.order_index;
    }
  }
}

interface SortButtonProps {
  label: string;
  sortKey: SortKey;
  sort: SortState;
  onChange: (next: SortState) => void;
  align?: "left" | "right" | undefined;
}

function SortButton({ label, sortKey, sort, onChange, align = "left" }: SortButtonProps) {
  const active = sort.key === sortKey;
  const arrow = active ? (sort.direction === "asc" ? "▲" : "▼") : "";
  return (
    <button
      type="button"
      className={`sort-button ${align === "right" ? "ml-auto" : ""}`}
      onClick={() => {
        onChange({
          key: sortKey,
          direction: active && sort.direction === "asc" ? "desc" : "asc",
        });
      }}
      aria-label={`Ordina per ${label}`}
    >
      {label}
      <span aria-hidden="true" className="text-[0.6rem]">
        {arrow}
      </span>
    </button>
  );
}

export function ChunkTable({
  chunks,
  selection,
  onSelectionChange,
  onRetry,
  onSkip,
  onOpenDetails,
  busyIds,
  highlightedIds,
}: ChunkTableProps) {
  const [sort, setSort] = useState<SortState>({ key: "order", direction: "asc" });

  const rows = useMemo(() => {
    const sorted = [...chunks];
    sorted.sort((left, right) => {
      const result = compareChunks(left, right, sort.key);
      return sort.direction === "asc" ? result : -result;
    });
    return sorted;
  }, [chunks, sort]);

  const allSelected = rows.length > 0 && rows.every((row) => selection.has(row.id));

  function toggleAll(checked: boolean) {
    const next = new Set(selection);
    for (const row of rows) {
      if (checked) {
        next.add(row.id);
      } else {
        next.delete(row.id);
      }
    }
    onSelectionChange(next);
  }

  function toggleOne(chunkId: string, checked: boolean) {
    const next = new Set(selection);
    if (checked) {
      next.add(chunkId);
    } else {
      next.delete(chunkId);
    }
    onSelectionChange(next);
  }

  return (
    <div className="table-scroll">
      <table className="data-table">
        <thead>
          <tr>
            <th style={{ width: "2.2rem" }}>
              <input
                type="checkbox"
                checked={allSelected}
                onChange={(event) => {
                  toggleAll(event.target.checked);
                }}
                aria-label="Seleziona tutti i chunk visibili"
              />
            </th>
            <th style={{ width: "5.5rem" }}>
              <SortButton label="#" sortKey="order" sort={sort} onChange={setSort} />
            </th>
            <th style={{ minWidth: "12rem" }}>
              <SortButton label="Capitolo" sortKey="chapter" sort={sort} onChange={setSort} />
            </th>
            <th style={{ minWidth: "9rem" }}>Tipo / flag</th>
            <th style={{ width: "7rem" }} className="num">
              <SortButton
                label="Token"
                sortKey="tokens"
                sort={sort}
                onChange={setSort}
                align="right"
              />
            </th>
            <th style={{ width: "9rem" }}>
              <SortButton label="Stato" sortKey="status" sort={sort} onChange={setSort} />
            </th>
            <th style={{ minWidth: "9rem" }}>
              <SortButton label="Modello" sortKey="model" sort={sort} onChange={setSort} />
            </th>
            <th style={{ width: "5rem" }} className="num">
              <SortButton
                label="Tent."
                sortKey="attempts"
                sort={sort}
                onChange={setSort}
                align="right"
              />
            </th>
            <th style={{ width: "11rem" }}>Azioni</th>
          </tr>
        </thead>

        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td colSpan={9} className="text-center text-xs text-muted">
                Nessun chunk da mostrare.
              </td>
            </tr>
          ) : (
            rows.map((row) => {
              const busy = busyIds?.has(row.id) === true;
              const highlighted = highlightedIds?.has(row.id) === true;
              const selected = selection.has(row.id);

              return (
                <tr
                  key={row.id}
                  data-selected={selected ? "true" : "false"}
                  style={highlighted ? { outline: "1px solid var(--color-accent)" } : undefined}
                >
                  <td>
                    <input
                      type="checkbox"
                      checked={selected}
                      onChange={(event) => {
                        toggleOne(row.id, event.target.checked);
                      }}
                      aria-label={`Seleziona il chunk ${row.id}`}
                    />
                  </td>

                  <td className="num" title={row.id}>
                    {formatNumber(row.order_index)}
                  </td>

                  <td>
                    <span className="block truncate" title={row.chapter_title ?? undefined}>
                      {row.chapter_title ?? "—"}
                    </span>
                    <span className="text-[0.68rem] text-faint">
                      {formatNumber(row.block_count)} blocchi
                    </span>
                  </td>

                  <td>
                    <span className="flex flex-wrap gap-1">
                      {row.flags.length === 0 ? (
                        <span className="mono-chip">testo</span>
                      ) : (
                        row.flags.map((flag) => (
                          <span key={flag} className="mono-chip" title={flag}>
                            {describeFlag(flag)}
                          </span>
                        ))
                      )}
                    </span>
                  </td>

                  <td className="num">{formatNumber(row.token_estimate)}</td>

                  <td>
                    <span className="flex items-center gap-1">
                      <StatusBadge status={row.status} pulse={row.status === "running"} />
                      {row.error !== null && row.error.length > 0 ? (
                        <span className="text-danger" title={truncate(row.error, 400)}>
                          ⚠
                        </span>
                      ) : null}
                    </span>
                  </td>

                  <td>
                    {row.model_id !== null && row.model_id.length > 0 ? (
                      <span className="mono-chip" title={row.model_id}>
                        {truncate(row.model_id, 22)}
                      </span>
                    ) : (
                      <span className="text-faint">—</span>
                    )}
                  </td>

                  <td className="num">{formatNumber(row.attempts)}</td>

                  <td>
                    <span className="flex flex-wrap gap-1">
                      <button
                        type="button"
                        className="btn btn-sm"
                        disabled={busy}
                        onClick={() => {
                          onRetry(row.id);
                        }}
                      >
                        Riprova
                      </button>
                      <button
                        type="button"
                        className="btn btn-sm btn-ghost"
                        disabled={busy}
                        onClick={() => {
                          onSkip(row.id);
                        }}
                      >
                        Salta
                      </button>
                      {onOpenDetails !== undefined ? (
                        <button
                          type="button"
                          className="btn btn-sm btn-ghost"
                          onClick={() => {
                            onOpenDetails(row.id);
                          }}
                        >
                          Dettagli
                        </button>
                      ) : null}
                    </span>
                  </td>
                </tr>
              );
            })
          )}
        </tbody>
      </table>
    </div>
  );
}
