import { ChunkTable } from "../../components/ChunkTable";
import type { ChunkRow } from "../../components/ChunkTable";
import { EmptyState } from "../../components/EmptyState";
import { formatNumber } from "../../lib/format";
import type { ViewId } from "../../App";
import { PAGE_SIZE } from "./shared";

/** The paginated chunk table, with the empty/loading states and "Carica altri". */
export function ChunkBrowser({
  loading,
  error,
  rows,
  visibleCount,
  limit,
  statusFilter,
  onReload,
  onLoadMore,
  onOpenDetails,
  onNavigate,
  onResetFilter,
}: {
  loading: boolean;
  error: string | null;
  rows: readonly ChunkRow[];
  visibleCount: number;
  limit: number;
  statusFilter: string;
  onReload: () => void;
  onLoadMore: () => void;
  onOpenDetails: (chunkId: string) => void;
  onNavigate: (view: ViewId) => void;
  onResetFilter: () => void;
}) {
  return (
    <div className="panel flex min-h-0 flex-col" style={{ maxHeight: "34rem" }}>
      <div className="panel-head">
        <span className="panel-title">Chunk</span>
        <span className="flex items-center gap-2">
          <span className="mono-chip">
            {visibleCount > limit
              ? `${formatNumber(limit)} di ${formatNumber(visibleCount)} righe`
              : `${formatNumber(visibleCount)} righe`}
          </span>
          {visibleCount > limit ? (
            <button type="button" className="btn btn-sm" disabled={loading} onClick={onLoadMore}>
              Carica altri {formatNumber(PAGE_SIZE)}
            </button>
          ) : null}
        </span>
      </div>

      {loading ? (
        <div className="panel-pad">
          <EmptyState tone="loading" compact title="Lettura dei chunk…" />
        </div>
      ) : error !== null ? (
        <div className="panel-pad">
          <EmptyState
            tone="error"
            compact
            title="Impossibile leggere i chunk"
            details={error}
            actionLabel="Riprova"
            onAction={onReload}
          />
        </div>
      ) : visibleCount === 0 ? (
        <div className="panel-pad">
          <EmptyState
            compact
            title={
              statusFilter === "all"
                ? "Nessun chunk nel progetto"
                : "Nessun chunk con questo stato"
            }
            description={
              statusFilter === "all"
                ? "Importa il documento dalla pagina Ingestione: i chunk vengono costruiti lì."
                : "Cambia il filtro per vedere gli altri chunk."
            }
            actionLabel={statusFilter === "all" ? "Vai all'ingestione" : "Mostra tutti"}
            onAction={() => {
              if (statusFilter === "all") {
                onNavigate("ingest");
              } else {
                onResetFilter();
              }
            }}
          />
        </div>
      ) : (
        <ChunkTable chunks={rows} onOpenDetails={onOpenDetails} />
      )}
    </div>
  );
}
