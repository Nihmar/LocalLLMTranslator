import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { onJobProgress } from "../lib/events";
import { formatNumber } from "../lib/format";
import {
  chunkGet,
  chunkList,
  projectGet,
  qaReport,
  reviewStart,
  suggestionAccept,
  suggestionHistory,
  suggestionList,
  suggestionReject,
  toErrorMessage,
} from "../lib/ipc";
import { inboxOrder, isImportant } from "../lib/review";
import type { ChunkDetail, Project, QaFinding, Suggestion } from "../lib/types";
import type { ViewId } from "../App";
import { QaList } from "./review/QaList";
import { ReviewHeader } from "./review/ReviewHeader";
import { ReviewTabs } from "./review/ReviewTabs";
import { SuggestionInbox } from "./review/SuggestionInbox";
import { isTypingTarget, type PassFilter, type SeverityFilter, type Tab } from "./review/shared";

/**
 * "Rivedi": the review as an inbox of decisions (issue #14).
 *
 * Every proposal of the editor and proofreader passes is one item: the serious ones first, in
 * the reading order of the book. The detail shows the source paragraph and the translation with
 * the correction inside it; Accetta / Rifiuta decide and move to the next item, also from the
 * keyboard (J/K to move, A to accept, R to reject). "Decise" is the correction history, "Controlli
 * QA" the advisory findings. Accepting rewrites the block on the control plane and recomposes
 * the chunk, so an export right after sees the decision.
 *
 * The panels in `./review/` own the presentation; this component keeps the loaded inbox, the
 * filters, the selection and the keyboard.
 */

export interface ReviewViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

export function ReviewView({ project, onNavigate }: ReviewViewProps) {
  const projectId = project?.id ?? null;
  const [tab, setTab] = useState<Tab>("open");
  const [severity, setSeverity] = useState<SeverityFilter>("important");
  const [passFilter, setPassFilter] = useState<PassFilter>("all");
  const [pending, setPending] = useState<Suggestion[]>([]);
  const [decided, setDecided] = useState<Suggestion[]>([]);
  const [findings, setFindings] = useState<QaFinding[]>([]);
  const [chunkOrder, setChunkOrder] = useState<Map<string, number>>(new Map());
  const [chunkChapter, setChunkChapter] = useState<Map<string, string>>(new Map());
  const [details, setDetails] = useState<Map<string, ChunkDetail>>(new Map());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [deciding, setDeciding] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pass, setPass] = useState("both");
  const [withQa, setWithQa] = useState(true);
  const [starting, setStarting] = useState(false);

  const load = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    try {
      const [open, history, qa, chunks, detail] = await Promise.all([
        suggestionList({ project_id: projectId, status: "pending" }),
        suggestionHistory({ project_id: projectId, limit: 500 }),
        qaReport({ project_id: projectId, status: "open" }),
        chunkList({ project_id: projectId }),
        projectGet(projectId),
      ]);
      const titles = new Map(detail.chapters.map((chapter) => [chapter.id, chapter.title]));
      setPending(open);
      setDecided(history);
      setFindings(qa);
      setChunkOrder(new Map(chunks.map((chunk) => [chunk.id, chunk.order_index])));
      setChunkChapter(
        new Map(
          chunks.map((chunk) => [
            chunk.id,
            (chunk.chapter_id === null ? undefined : titles.get(chunk.chapter_id)) ?? "",
          ]),
        ),
      );
      setError(null);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    setLoading(true);
    setSelectedId(null);
    setDetails(new Map());
    void load();
  }, [load]);

  useEffect(() => onJobProgress(() => void load()), [load]);

  const visible = useMemo(() => {
    const source = tab === "decided" ? decided : pending;
    const filtered = source.filter(
      (item) =>
        (passFilter === "all" || item.pass === passFilter) &&
        (tab === "decided" ||
          severity === "all" ||
          (severity === "important" ? isImportant(item) : !isImportant(item))),
    );
    return tab === "decided" ? filtered : inboxOrder(filtered, chunkOrder);
  }, [tab, decided, pending, passFilter, severity, chunkOrder]);

  const importantCount = useMemo(() => pending.filter(isImportant).length, [pending]);

  // Keep a valid selection: the first item when nothing (or a vanished item) is selected.
  useEffect(() => {
    if (tab === "qa") {
      return;
    }
    if (selectedId === null || !visible.some((item) => item.id === selectedId)) {
      setSelectedId(visible[0]?.id ?? null);
    }
  }, [tab, visible, selectedId]);

  const selected = visible.find((item) => item.id === selectedId) ?? null;
  const selectedChunk = selected?.chunk_id ?? null;

  useEffect(() => {
    if (selectedChunk === null || details.has(selectedChunk)) {
      return;
    }
    void chunkGet(selectedChunk)
      .then((detail) => {
        setDetails((current) => new Map(current).set(selectedChunk, detail));
      })
      .catch((detailError: unknown) => {
        setError(toErrorMessage(detailError));
      });
  }, [selectedChunk, details]);

  const move = useCallback(
    (step: number) => {
      const index = visible.findIndex((item) => item.id === selectedId);
      const next = visible[Math.min(Math.max(index + step, 0), visible.length - 1)];
      if (next !== undefined) {
        setSelectedId(next.id);
      }
    },
    [visible, selectedId],
  );

  const decide = useCallback(
    async (accept: boolean) => {
      if (selected === null || selected.status !== "pending" || deciding) {
        return;
      }
      const index = visible.findIndex((item) => item.id === selected.id);
      const following = visible[index + 1] ?? visible[index - 1] ?? null;
      setDeciding(true);
      setNotice(null);
      try {
        await (accept ? suggestionAccept(selected.id) : suggestionReject(selected.id));
        setNotice(accept ? "Correzione applicata al testo." : "Proposta rifiutata.");
        setDetails((current) => {
          const next = new Map(current);
          next.delete(selected.chunk_id);
          return next;
        });
        setSelectedId(following?.id ?? null);
        await load();
      } catch (decideError) {
        const message = toErrorMessage(decideError);
        setError(
          message.includes("would alter the markup")
            ? "Questa correzione toglierebbe un link, del codice o una nota dal paragrafo: non può essere applicata così. Rifiutala o correggi il testo a mano."
            : message,
        );
      } finally {
        setDeciding(false);
      }
    },
    [selected, deciding, visible, load],
  );

  useEffect(() => {
    if (tab === "qa") {
      return;
    }
    function onKey(event: KeyboardEvent) {
      if (event.ctrlKey || event.metaKey || event.altKey || isTypingTarget(event.target)) {
        return;
      }
      const key = event.key.toLowerCase();
      if (key === "j") {
        move(1);
      } else if (key === "k") {
        move(-1);
      } else if (key === "a") {
        void decide(true);
      } else if (key === "r") {
        void decide(false);
      } else {
        return;
      }
      event.preventDefault();
    }
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
    };
  }, [tab, move, decide]);

  async function handleStart() {
    if (projectId === null) {
      return;
    }
    setStarting(true);
    setNotice(null);
    try {
      const result = await reviewStart({ project_id: projectId, pass, with_qa: withQa });
      setNotice(
        result.enqueued === 0
          ? "Niente da accodare: i capitoli tradotti hanno già una revisione in corso."
          : `${formatNumber(result.enqueued)} revisioni accodate: le proposte arrivano capitolo per capitolo.`,
      );
    } catch (startError) {
      setError(toErrorMessage(startError));
    } finally {
      setStarting(false);
    }
  }

  if (project === null) {
    return (
      <EmptyState
        title="Nessun libro aperto"
        description="La revisione lavora sul libro aperto."
        actionLabel="Vai alla libreria"
        onAction={() => {
          onNavigate("projects");
        }}
      />
    );
  }

  const detail = selected === null ? undefined : details.get(selected.chunk_id);

  return (
    <div className="section-stack">
      <ReviewHeader
        pendingCount={pending.length}
        importantCount={importantCount}
        pass={pass}
        onPass={setPass}
        withQa={withQa}
        onWithQa={setWithQa}
        starting={starting}
        onStart={() => void handleStart()}
      />

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
        </div>
      ) : null}
      {notice !== null ? (
        <div className="banner banner-ok" role="status">
          <span aria-hidden="true">✓</span>
          <span>{notice}</span>
        </div>
      ) : null}

      <ReviewTabs
        tab={tab}
        onTab={(next) => {
          setTab(next);
          setSelectedId(null);
        }}
        pendingCount={pending.length}
        decidedCount={decided.length}
        findingsCount={findings.length}
        severity={severity}
        onSeverity={setSeverity}
        passFilter={passFilter}
        onPassFilter={setPassFilter}
      />

      {loading ? (
        <EmptyState tone="loading" title="Lettura delle proposte…" />
      ) : tab === "qa" ? (
        <QaList findings={findings} chapterOf={chunkChapter} />
      ) : (
        <SuggestionInbox
          tab={tab}
          visible={visible}
          selected={selected}
          selectedId={selectedId}
          pendingCount={pending.length}
          chunkChapter={chunkChapter}
          detail={detail}
          deciding={deciding}
          onSelect={setSelectedId}
          onDecide={(accept) => {
            void decide(accept);
          }}
        />
      )}
    </div>
  );
}
