import { EmptyState } from "../../components/EmptyState";
import { StatusBadge } from "../../components/StatusBadge";
import { formatNumber } from "../../lib/format";
import type { ChunkDetail } from "../../lib/types";
import { blockOriginLabel } from "./shared";

/** The read-only drawer for one chunk: its blocks and the translation in effect. */
export function ChunkDetailPanel({
  detailId,
  detail,
  detailLoading,
  detailError,
  chapterTitle,
  onReload,
  onClose,
}: {
  detailId: string;
  detail: ChunkDetail | null;
  detailLoading: boolean;
  detailError: string | null;
  chapterTitle: ReadonlyMap<string, string>;
  onReload: () => void;
  onClose: () => void;
}) {
  const rows =
    detail === null
      ? []
      : (() => {
          const byBlock = new Map(detail.translations.map((entry) => [entry.block_id, entry]));
          return detail.blocks.map((block) => ({
            block,
            translation: byBlock.get(block.id) ?? null,
          }));
        })();

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">
          Dettagli chunk <span className="mono-chip">{detailId}</span>
        </span>
        <span className="flex items-center gap-2">
          <button type="button" className="btn btn-sm" disabled={detailLoading} onClick={onReload}>
            Ricarica
          </button>
          <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>
            Chiudi
          </button>
        </span>
      </div>

      {detailLoading ? (
        <div className="panel-pad">
          <EmptyState tone="loading" compact title="Lettura del chunk…" />
        </div>
      ) : detailError !== null ? (
        <div className="panel-pad">
          <EmptyState
            tone="error"
            compact
            title="Impossibile leggere il chunk"
            details={detailError}
          />
        </div>
      ) : detail === null ? (
        <div className="panel-pad">
          <EmptyState compact title="Nessun dato" />
        </div>
      ) : (
        <div className="panel-pad section-stack">
          <div className="grid grid-cols-4 gap-2">
            <div className="stat-tile">
              <div className="stat-label">Capitolo</div>
              <div className="truncate text-xs text-ink-soft">
                {detail.chunk.chapter_id === null
                  ? "—"
                  : (chapterTitle.get(detail.chunk.chapter_id) ?? detail.chunk.chapter_id)}
              </div>
            </div>
            <div className="stat-tile">
              <div className="stat-label">Token stimati</div>
              <div className="stat-value">{formatNumber(detail.chunk.token_estimate)}</div>
            </div>
            <div className="stat-tile">
              <div className="stat-label">Blocchi</div>
              <div className="stat-value">{formatNumber(detail.blocks.length)}</div>
            </div>
            <div className="stat-tile">
              <div className="stat-label">Hash prompt</div>
              <div className="truncate font-mono text-[0.7rem] text-muted">
                {detail.chunk.prompt_hash ?? "—"}
              </div>
            </div>
          </div>

          {rows.length === 0 ? (
            <EmptyState
              compact
              title="Nessun blocco associato"
              description="Il chunk è stato salvato ma i blocchi non sono leggibili: verifica la migrazione o ripeti l'ingestione."
            />
          ) : (
            <div className="table-scroll" style={{ maxHeight: "26rem" }}>
              <table className="data-table">
                <thead>
                  <tr>
                    <th style={{ width: "9rem" }}>Blocco</th>
                    <th>Sorgente</th>
                    <th>Traduzione</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map(({ block, translation }) => (
                    <tr key={block.id}>
                      <td>
                        <span className="block">
                          <span className="mono-chip">{block.id}</span>
                        </span>
                        <span className="mt-1 block text-[0.68rem] text-faint">
                          {block.kind}
                          {block.level > 0 ? ` · L${formatNumber(block.level)}` : ""}
                          {block.translatable ? "" : " · non traducibile"}
                        </span>
                      </td>
                      <td>
                        <pre className="max-w-prose font-mono text-[0.72rem] whitespace-pre-wrap text-ink-soft">
                          {block.source_md}
                        </pre>
                      </td>
                      <td>
                        {translation === null ? (
                          <span className="text-faint">— nessuna traduzione —</span>
                        ) : (
                          <>
                            <pre className="max-w-prose font-mono text-[0.72rem] whitespace-pre-wrap text-ink">
                              {translation.text_md}
                            </pre>
                            <span className="mt-1 flex items-center gap-1">
                              <span className="mono-chip">
                                {blockOriginLabel(translation.origin)}
                              </span>
                              <StatusBadge
                                status={translation.placeholders_ok ? "ok" : "failed"}
                                tone={translation.placeholders_ok ? "success" : "danger"}
                                label={
                                  translation.placeholders_ok
                                    ? "placeholder integri"
                                    : "placeholder alterati"
                                }
                              />
                              {translation.edited_by_user ? (
                                <span className="badge badge-neutral">modificato a mano</span>
                              ) : null}
                            </span>
                          </>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {detail.chunk.error !== null ? (
            <div className="banner banner-error" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>{detail.chunk.error}</span>
            </div>
          ) : null}
        </div>
      )}
    </div>
  );
}
