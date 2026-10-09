import { useMemo, useState } from "react";
import { formatNumber, truncate } from "../lib/format";
import { StatusBadge } from "./StatusBadge";

/**
 * Chunk table for the translation page.
 *
 * `chunk_list` returns `ChunkView` rows: the JSON columns are already decoded on the Rust side
 * (`block_ids`, `flags`) and there is no chapter title or attempt counter on the row. Those are
 * derived one level up (in `TranslateView`, from `project_get` chapters and `job_list`) and passed
 * here as `ChunkRow`, so the table keeps its original columns without a per-row backend call.
 */

/** Presentation view of one chunk, assembled by `TranslateView` from `chunk_list`. */
export interface ChunkRow {
  id: string;
  chapter_title: string | null;
  order_index: number;
  flags: string[];
  block_count: number;
  token_estimate: number;
  status: string;
  model_id: string | null;
  attempts: number;
  error: string | null;
}

export interface ChunkTableProps {
  chunks: readonly ChunkRow[];
  /** Opens the read-only original/target drawer. */
  onOpenDetails?: ((chunkId: string) => void) | undefined;
}

type SortKey = "order" | "chapter" | "tokens" | "status" | "model" | "attempts";

interface SortState {
  key: SortKey;
  direction: "asc" | "desc";
}

/** Order in which statuses are worth looking at: what needs attention comes first. */
const STATUS_RANK: Readonly<Record<string, number>> = {
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

function compareChunks(left: ChunkRow, right: ChunkRow, key: SortKey): number {
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

export function ChunkTable({ chunks, onOpenDetails }: ChunkTableProps) {
  const [sort, setSort] = useState<SortState>({ key: "order", direction: "asc" });

  const rows = useMemo(() => {
    const sorted = [...chunks];
    sorted.sort((left, right) => {
      const result = compareChunks(left, right, sort.key);
      return sort.direction === "asc" ? result : -result;
    });
    return sorted;
  }, [chunks, sort]);

  return (
    <div className="table-scroll">
      <table className="data-table">
        <thead>
          <tr>
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
            <th style={{ width: "7rem" }}>Azioni</th>
          </tr>
        </thead>

        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td colSpan={8} className="text-center text-xs text-muted">
                Nessun chunk da mostrare.
              </td>
            </tr>
          ) : (
            rows.map((row) => {
              return (
                <tr key={row.id}>
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
                    </span>
                    {row.error !== null && row.error.length > 0 ? (
                      <span className="mt-1 block text-[0.68rem] text-danger" title={row.error}>
                        {truncate(row.error, 60)}
                      </span>
                    ) : null}
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
