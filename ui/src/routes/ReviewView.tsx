import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { readableText } from "../lib/chapters";
import { onJobProgress } from "../lib/events";
import { formatDateTime, formatNumber } from "../lib/format";
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
import {
  currentTextByBlock,
  inboxOrder,
  isImportant,
  proposalSegments,
  severityClass,
} from "../lib/review";
import type { ChunkDetail, Project, QaFinding, Suggestion } from "../lib/types";
import type { ViewId } from "../App";

/**
 * "Rivedi": the review as an inbox of decisions (issue #14).
 *
 * Every proposal of the editor and proofreader passes is one item: the serious ones first, in
 * the reading order of the book. The detail shows the source paragraph and the translation with
 * the correction inside it; Accetta / Rifiuta decide and move to the next item, also from the
 * keyboard (J/K to move, A to accept, R to reject). "Decise" is the correction history, "Controlli
 * QA" the advisory findings. Accepting rewrites the block on the control plane and recomposes
 * the chunk, so an export right after sees the decision.
 */

export interface ReviewViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

type Tab = "open" | "decided" | "qa";
type SeverityFilter = "important" | "all" | "minor";
type PassFilter = "all" | "editor" | "proofreader";

const PASSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "both", label: "Editor + proofreader" },
  { value: "editor", label: "Solo editor" },
  { value: "proofreader", label: "Solo proofreader" },
];

const SEVERITY_LABEL: Record<string, string> = {
  critical: "Critica",
  major: "Importante",
  minor: "Minore",
};

const QA_KIND_LABEL: Record<string, string> = {
  untranslated: "Non tradotto",
  glossary_mismatch: "Glossario non rispettato",
  glossary_conflict: "Conflitto di glossario",
  placeholder_broken: "Segnaposto rotti",
  markdown_malformed: "Struttura alterata",
  length_anomaly: "Lunghezza anomala",
  duplicate: "Duplicato",
  empty: "Vuoto",
  latin_leftover: "Testo non tradotto rimasto",
};

function passLabel(pass: string): string {
  return pass === "proofreader" ? "proofreader" : "editor";
}

function isTypingTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  );
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
        setError(toErrorMessage(decideError));
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
  const block = detail?.blocks.find((candidate) => candidate.id === selected?.block_id);
  const current =
    block === undefined || detail === undefined
      ? (selected?.original ?? "")
      : (currentTextByBlock(detail.translations).get(block.id) ?? selected?.original ?? "");

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="font-serif text-2xl font-medium text-ink">Rivedi</h1>
          <p className="text-sm text-muted">
            {formatNumber(pending.length)} proposte da decidere, di cui{" "}
            {formatNumber(importantCount)} importanti.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <select
            className="select"
            style={{ width: "auto" }}
            aria-label="Passaggi da eseguire"
            value={pass}
            onChange={(event) => {
              setPass(event.target.value);
            }}
          >
            {PASSES.map((option) => (
              <option key={option.value} value={option.value}>
                {option.label}
              </option>
            ))}
          </select>
          <label className="flex items-center gap-1.5 text-sm text-ink-soft">
            <input
              type="checkbox"
              checked={withQa}
              onChange={(event) => {
                setWithQa(event.target.checked);
              }}
            />
            Rilancia QA
          </label>
          <button
            type="button"
            className="btn btn-primary"
            disabled={starting}
            onClick={() => {
              void handleStart();
            }}
          >
            {starting ? <span className="spinner" aria-hidden="true" /> : null}
            Avvia revisione
          </button>
        </div>
      </div>

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

      <div className="flex flex-wrap items-center gap-3">
        <div role="tablist" aria-label="Sezione" className="segmented">
          {(
            [
              ["open", `Da decidere · ${formatNumber(pending.length)}`],
              ["decided", `Decise · ${formatNumber(decided.length)}`],
              ["qa", `Controlli QA · ${formatNumber(findings.length)}`],
            ] as const
          ).map(([id, label]) => (
            <button
              key={id}
              type="button"
              role="tab"
              aria-selected={tab === id}
              className="segmented-item"
              onClick={() => {
                setTab(id);
                setSelectedId(null);
              }}
            >
              {label}
            </button>
          ))}
        </div>
        {tab === "qa" ? null : (
          <div className="flex flex-wrap gap-2">
            {tab === "open"
              ? (
                  [
                    ["important", "Importanti"],
                    ["minor", "Minori"],
                    ["all", "Tutte"],
                  ] as const
                ).map(([id, label]) => (
                  <button
                    key={id}
                    type="button"
                    className="chip"
                    aria-pressed={severity === id}
                    onClick={() => {
                      setSeverity(id);
                    }}
                  >
                    {label}
                  </button>
                ))
              : null}
            <select
              className="select"
              style={{ width: "auto" }}
              aria-label="Passaggio"
              value={passFilter}
              onChange={(event) => {
                const value = event.target.value;
                setPassFilter(value === "editor" || value === "proofreader" ? value : "all");
              }}
            >
              <option value="all">Editor e proofreader</option>
              <option value="editor">Solo editor</option>
              <option value="proofreader">Solo proofreader</option>
            </select>
          </div>
        )}
        {tab === "open" ? (
          <span className="ml-auto text-xs text-muted">
            <kbd className="kbd">J</kbd> <kbd className="kbd">K</kbd> scorri ·{" "}
            <kbd className="kbd">A</kbd> accetta · <kbd className="kbd">R</kbd> rifiuta
          </span>
        ) : null}
      </div>

      {loading ? (
        <EmptyState tone="loading" title="Lettura delle proposte…" />
      ) : tab === "qa" ? (
        <QaList findings={findings} chapterOf={chunkChapter} />
      ) : (
        <div className="flex flex-wrap items-start gap-4">
          <section aria-label="Elenco" className="panel inbox-list">
            {visible.length === 0 ? (
              <p className="p-6 text-center text-sm text-muted">
                {tab === "open"
                  ? pending.length === 0
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
                    setSelectedId(item.id);
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
                    {block === undefined ? (detail === undefined ? "…" : "—") : readableText(block.source_md)}
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
                        void decide(true);
                      }}
                    >
                      Accetta <kbd className="kbd">A</kbd>
                    </button>
                    <button
                      type="button"
                      className="btn btn-lg"
                      disabled={deciding}
                      onClick={() => {
                        void decide(false);
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
      )}
    </div>
  );
}

function QaList({
  findings,
  chapterOf,
}: {
  findings: readonly QaFinding[];
  chapterOf: ReadonlyMap<string, string>;
}) {
  if (findings.length === 0) {
    return (
      <EmptyState
        title="Nessun controllo aperto"
        description="I controlli girano dopo ogni capitolo tradotto e con «Rilancia QA»."
      />
    );
  }
  return (
    <ul className="panel divide-y divide-line">
      {findings.map((finding) => (
        <li key={finding.id} className="flex flex-wrap items-start gap-3 px-5 py-3">
          <span className={severityClass(finding.severity)}>
            {SEVERITY_LABEL[finding.severity] ?? finding.severity}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-medium text-ink">
              {QA_KIND_LABEL[finding.kind] ?? finding.kind}
            </span>
            <span className="block text-xs text-muted">
              {finding.chunk_id === null ? "Tutto il libro" : chapterOf.get(finding.chunk_id)}
            </span>
            <span className="mt-1 block font-mono text-[0.7rem] break-all text-faint">
              {finding.details_json}
            </span>
          </span>
        </li>
      ))}
    </ul>
  );
}
