import { readableText } from "../../lib/chapters";
import { formatDateTime } from "../../lib/format";
import { currentTextByBlock, proposalSegments, severityClass } from "../../lib/review";
import type { ChunkDetail, Suggestion } from "../../lib/types";
import { passLabel, SEVERITY_LABEL, type Tab } from "./shared";

/** The inbox: the item list and the detail with the source, the correction and the decision. */
export function SuggestionInbox({
  tab,
  visible,
  selected,
  selectedId,
  pendingCount,
  chunkChapter,
  detail,
  deciding,
  onSelect,
  onDecide,
}: {
  tab: Exclude<Tab, "qa">;
  visible: readonly Suggestion[];
  selected: Suggestion | null;
  selectedId: string | null;
  pendingCount: number;
  chunkChapter: ReadonlyMap<string, string>;
  detail: ChunkDetail | undefined;
  deciding: boolean;
  onSelect: (id: string) => void;
  onDecide: (accept: boolean) => void;
}) {
  const block = detail?.blocks.find((candidate) => candidate.id === selected?.block_id);
  const current =
    block === undefined || detail === undefined
      ? (selected?.original ?? "")
      : (currentTextByBlock(detail.translations).get(block.id) ?? selected?.original ?? "");

  return (
    <div className="flex flex-wrap items-start gap-4">
      <section aria-label="Elenco" className="panel inbox-list">
        {visible.length === 0 ? (
          <p className="p-6 text-center text-sm text-muted">
            {tab === "open"
              ? pendingCount === 0
                ? "Niente da decidere. Avvia la revisione sui capitoli tradotti."
                : "Nessuna proposta con questi filtri."
              : "Nessuna decisione ancora."}
          </p>
        ) : (
          visible.map((item) => (
            <button
              key={item.id}
              type="button"
              className="inbox-item"
              aria-current={item.id === selectedId ? "true" : undefined}
              onClick={() => {
                onSelect(item.id);
              }}
            >
              <span className="flex flex-wrap items-center gap-2">
                <span className={severityClass(item.severity)}>
                  {SEVERITY_LABEL[item.severity ?? ""] ?? "Nota"}
                </span>
                <span className="text-xs text-muted">{chunkChapter.get(item.chunk_id)}</span>
                <span className="ml-auto text-xs text-faint">
                  {tab === "decided"
                    ? item.status === "accepted"
                      ? "accettata"
                      : "rifiutata"
                    : passLabel(item.pass)}
                </span>
              </span>
              <span className="mt-1 line-clamp-2 block text-sm text-ink">
                {item.reason ?? item.quote ?? item.proposed ?? ""}
              </span>
            </button>
          ))
        )}
      </section>

      <section aria-label="Dettaglio" className="panel inbox-detail">
        {selected === null ? (
          <p className="p-6 text-center text-sm text-muted">Seleziona una voce.</p>
        ) : (
          <div className="section-stack p-6">
            <div className="flex flex-wrap items-center gap-2">
              <span className={severityClass(selected.severity)}>
                {SEVERITY_LABEL[selected.severity ?? ""] ?? "Nota"}
              </span>
              <span className="text-sm text-muted">
                {chunkChapter.get(selected.chunk_id)} · {passLabel(selected.pass)}
              </span>
              {selected.decided_at === null ? null : (
                <span className="ml-auto text-xs text-faint">
                  decisa il {formatDateTime(selected.decided_at)}
                </span>
              )}
            </div>

            <div>
              <div className="field-label">Originale</div>
              <p className="reading reading-source">
                {block === undefined
                  ? detail === undefined
                    ? "…"
                    : "—"
                  : readableText(block.source_md)}
              </p>
            </div>

            <div>
              <div className="field-label">
                {selected.status === "pending" ? "Traduzione con la correzione" : "Correzione"}
              </div>
              <p className="reading">
                {proposalSegments(
                  selected.status === "pending" ? current : (selected.original ?? ""),
                  selected,
                ).map((segment, index) => (
                  <span key={index} className={`diff-${segment.kind}`}>
                    {segment.text}
                  </span>
                ))}
              </p>
            </div>

            {selected.reason === null ? null : (
              <div className="banner">
                <span>
                  <strong className="font-semibold text-ink">Perché:</strong> {selected.reason}
                </span>
              </div>
            )}

            {selected.status === "pending" ? (
              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  className="btn btn-primary btn-lg"
                  disabled={deciding}
                  onClick={() => {
                    onDecide(true);
                  }}
                >
                  Accetta <kbd className="kbd">A</kbd>
                </button>
                <button
                  type="button"
                  className="btn btn-lg"
                  disabled={deciding}
                  onClick={() => {
                    onDecide(false);
                  }}
                >
                  Rifiuta <kbd className="kbd">R</kbd>
                </button>
              </div>
            ) : null}
          </div>
        )}
      </section>
    </div>
  );
}
