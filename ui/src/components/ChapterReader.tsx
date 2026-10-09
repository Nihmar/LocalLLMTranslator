import { useEffect, useMemo, useState } from "react";
import { readableText, type ChapterProgress } from "../lib/chapters";
import { chunkGet, toErrorMessage } from "../lib/ipc";
import { currentTextByBlock } from "../lib/review";
import type { Chunk, ChunkDetail } from "../lib/types";

/**
 * The translation as something to read (issue #14): the chapters on the left, the selected one
 * paragraph by paragraph on the right, source next to translation. A chunk without a usable
 * translation says so in place — it is what an export would print in the source language — and
 * the chapter being translated fills in as its chunks complete: the parent reloads the chunk rows
 * on every job transition, and a changed row refetches its paragraphs.
 */

export interface ChapterReaderProps {
  chapters: readonly ChapterProgress[];
  chunks: readonly Chunk[];
}

type ChapterState = "done" | "flag" | "running" | "queued";

function chapterState(chapter: ChapterProgress): ChapterState {
  if (chapter.failed > 0 || chapter.needs_review > 0) {
    return "flag";
  }
  if (chapter.running > 0) {
    return "running";
  }
  return chapter.translated === chapter.total ? "done" : "queued";
}

const STATE_LABEL: Record<ChapterState, string> = {
  done: "Tradotto",
  flag: "Da controllare",
  running: "In traduzione",
  queued: "In coda",
};

/** The chapter to open first: the one being translated, else the last translated one. */
function initialChapter(chapters: readonly ChapterProgress[]): string | null {
  const running = chapters.find((chapter) => chapter.running > 0);
  if (running !== undefined) {
    return running.id;
  }
  const translated = chapters.filter((chapter) => chapter.translated > 0);
  return translated[translated.length - 1]?.id ?? chapters[0]?.id ?? null;
}

export function ChapterReader({ chapters, chunks }: ChapterReaderProps) {
  const [chapterId, setChapterId] = useState<string | null>(() => initialChapter(chapters));
  const [sideBySide, setSideBySide] = useState(true);
  const [details, setDetails] = useState<ChunkDetail[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (chapterId === null || !chapters.some((chapter) => chapter.id === chapterId)) {
      setChapterId(initialChapter(chapters));
    }
  }, [chapters, chapterId]);

  const chapterChunks = useMemo(
    () =>
      chunks
        .filter((chunk) => chunk.chapter_id === chapterId)
        .sort((a, b) => a.order_index - b.order_index),
    [chunks, chapterId],
  );
  // Refetch when a chunk of this chapter changes state or text, not on every unrelated event.
  const signature = chapterChunks
    .map((chunk) => `${chunk.id}:${chunk.status}:${chunk.updated_at}`)
    .join("|");

  useEffect(() => {
    let cancelled = false;
    const ids = signature === "" ? [] : signature.split("|").map((part) => part.split(":")[0] ?? "");
    void Promise.all(ids.map((id) => chunkGet(id)))
      .then((loaded) => {
        if (!cancelled) {
          setDetails(loaded);
          setError(null);
        }
      })
      .catch((loadError: unknown) => {
        if (!cancelled) {
          setError(toErrorMessage(loadError));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [signature]);

  const current = chapters.find((chapter) => chapter.id === chapterId) ?? null;

  return (
    <div className="flex flex-wrap items-start gap-4">
      <nav aria-label="Capitoli" className="panel reader-rail">
        {chapters.map((chapter) => {
          const state = chapterState(chapter);
          return (
            <button
              key={chapter.id}
              type="button"
              className="reader-chapter"
              aria-current={chapter.id === chapterId ? "true" : undefined}
              onClick={() => {
                setChapterId(chapter.id);
              }}
            >
              <span className="reader-dot" data-state={state} aria-hidden="true" />
              <span className="min-w-0 flex-1 truncate font-serif">
                {chapter.title === "" ? "Senza titolo" : chapter.title}
              </span>
              <span className="text-xs text-muted">
                {state === "done" ? "" : STATE_LABEL[state].toLowerCase()}
              </span>
            </button>
          );
        })}
      </nav>

      <section aria-label="Lettura" className="panel reader-main">
        {current === null ? (
          <p className="p-6 text-sm text-muted">Nessun capitolo.</p>
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-3 border-b border-line px-6 py-4">
              <h2 className="font-serif text-xl font-medium text-ink">
                {current.title === "" ? "Senza titolo" : current.title}
              </h2>
              <span className="text-sm text-muted">
                {STATE_LABEL[chapterState(current)]} · {current.translated}/{current.total} parti
              </span>
              <div role="group" aria-label="Vista" className="segmented ml-auto">
                <button
                  type="button"
                  className="segmented-item"
                  aria-selected={sideBySide}
                  onClick={() => {
                    setSideBySide(true);
                  }}
                >
                  Affiancato
                </button>
                <button
                  type="button"
                  className="segmented-item"
                  aria-selected={!sideBySide}
                  onClick={() => {
                    setSideBySide(false);
                  }}
                >
                  Solo traduzione
                </button>
              </div>
            </div>

            {error !== null ? (
              <div className="banner banner-error m-6" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>{error}</span>
              </div>
            ) : null}

            {details.map((detail) => {
              const texts = currentTextByBlock(detail.translations);
              const status = detail.chunk.status;
              const note =
                status === "failed"
                  ? "Non tradotto: nell'export comparirebbe nella lingua originale. «Riprova falliti» lo rimette in coda."
                  : status === "needs_review"
                    ? "Da controllare: la traduzione c'è, ma il modello ha restituito qualcosa di irregolare."
                    : status === "running"
                      ? "In traduzione…"
                      : null;
              return (
                <div key={detail.chunk.id}>
                  {note === null ? null : (
                    <div className="reader-note" data-status={status}>
                      <span>{note}</span>
                      {detail.chunk.error === null || status === "running" ? null : (
                        <span className="block text-xs opacity-80">{detail.chunk.error}</span>
                      )}
                    </div>
                  )}
                  {detail.blocks.map((block) => {
                    const translated = texts.get(block.id);
                    return (
                      <div
                        key={block.id}
                        className="reader-row"
                        data-side-by-side={sideBySide ? "true" : "false"}
                        data-kind={block.kind}
                      >
                        {sideBySide ? (
                          <p className="reading reading-source">{readableText(block.source_md)}</p>
                        ) : null}
                        {translated !== undefined ? (
                          <p className="reading">{readableText(translated)}</p>
                        ) : block.translatable ? (
                          <p className="reader-pending">
                            {status === "running" ? "in traduzione…" : "in attesa"}
                          </p>
                        ) : (
                          <p className="reading reading-source">{readableText(block.source_md)}</p>
                        )}
                      </div>
                    );
                  })}
                </div>
              );
            })}
          </>
        )}
      </section>
    </div>
  );
}
