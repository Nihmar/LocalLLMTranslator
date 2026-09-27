import { ProgressBar } from "./ProgressBar";
import { StatusBadge } from "./StatusBadge";
import type { ChapterProgress } from "../lib/chapters";
import { countLabel, formatNumber, formatTokens } from "../lib/format";

/**
 * Chapter outline for the translation page: one row per chapter with the translated-chunk
 * count, so a run can be followed book-by-book instead of chunk-by-chunk.
 *
 * Every row encodes its state the same way: a chip per condition in the "Stato" column (running,
 * failures, chunks to review, completed) and the mini progress bar as the only amount indicator.
 * Prose under the title would move with the state and read differently from row to row; the
 * chips keep their column, so a chapter with failures is not the one row that looks different.
 *
 * The whole row is a double-click target (`PLAN.md` §11.3): the preview is a navigation
 * action, and Enter or the explicit button do the same from the keyboard or for anyone who
 * does not guess the gesture. Chapters without chunks never reach this component.
 */

export interface ChapterListProps {
  chapters: readonly ChapterProgress[];
  /** Chapter whose preview is open, highlighted in the list. */
  activeId: string | null;
  /** Opens the live preview for a chapter. */
  onOpen: (chapterId: string) => void;
}

export function ChapterList({ chapters, activeId, onOpen }: ChapterListProps) {
  return (
    <div className="table-scroll" style={{ maxHeight: "17rem" }}>
      <table className="data-table">
        <thead>
          <tr>
            <th style={{ width: "3.5rem" }} className="num">
              #
            </th>
            <th>Capitolo</th>
            <th style={{ width: "9rem" }}>Stato</th>
            <th style={{ width: "11rem" }}>Avanzamento</th>
            <th style={{ width: "6.5rem" }} className="num">
              Token
            </th>
            <th style={{ width: "6.5rem" }} />
          </tr>
        </thead>

        <tbody>
          {chapters.map((chapter) => {
            const running = chapter.running > 0;
            const active = chapter.id === activeId;
            const clean = !running && chapter.failed === 0 && chapter.needs_review === 0;
            const complete = chapter.total > 0 && chapter.translated === chapter.total;

            return (
              <tr
                key={chapter.id}
                data-selected={active}
                className="cursor-pointer"
                tabIndex={0}
                title="Doppio clic per l'anteprima della traduzione"
                onDoubleClick={() => {
                  onOpen(chapter.id);
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    onOpen(chapter.id);
                  }
                }}
              >
                <td className="num text-muted">{formatNumber(chapter.order_index)}</td>

                <td>
                  <span className="flex min-w-0 items-center gap-2">
                    {chapter.level > 1 ? (
                      <span className="mono-chip">H{formatNumber(chapter.level)}</span>
                    ) : null}
                    <span className="truncate text-ink-soft" title={chapter.title}>
                      {chapter.title}
                    </span>
                  </span>
                  <span className="text-[0.68rem] text-faint">
                    {countLabel(chapter.total, "chunk", "chunk")}
                  </span>
                </td>

                <td>
                  <span className="flex flex-wrap items-center gap-1">
                    {running ? <StatusBadge status="running" pulse /> : null}
                    {chapter.failed > 0 ? (
                      <StatusBadge
                        status="failed"
                        tone="danger"
                        label={countLabel(chapter.failed, "fallito", "falliti")}
                      />
                    ) : null}
                    {chapter.needs_review > 0 ? (
                      <StatusBadge
                        status="needs_review"
                        tone="warning"
                        label={countLabel(chapter.needs_review, "da rivedere", "da rivedere")}
                      />
                    ) : null}
                    {clean && complete ? (
                      <StatusBadge status="done" tone="success" label="Completato" />
                    ) : null}
                  </span>
                </td>

                <td>
                  <span className="flex items-center gap-2">
                    <span className="min-w-16 flex-1">
                      <ProgressBar
                        value={chapter.translated}
                        total={chapter.total}
                        tone={chapter.translated === chapter.total ? "success" : "accent"}
                      />
                    </span>
                    <span className="font-mono text-[0.7rem] whitespace-nowrap text-muted tabular-nums">
                      {formatNumber(chapter.translated)}/{formatNumber(chapter.total)}
                    </span>
                  </span>
                </td>

                <td className="num text-muted">{formatTokens(chapter.tokens)}</td>

                <td className="text-right">
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => {
                      onOpen(chapter.id);
                    }}
                  >
                    Anteprima
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
