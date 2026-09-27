import { useCallback, useEffect, useMemo, useState } from "react";
import { MergeDiff, ReadOnlyCode } from "../components/CodeEditor";
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
  suggestionList,
  suggestionReject,
  toErrorMessage,
} from "../lib/ipc";
import { severityClass, suggestionSnippet } from "../lib/review";
import type {
  BlockTranslation,
  Chapter,
  Chunk,
  ChunkDetail,
  Project,
  QaFinding,
  Suggestion,
} from "../lib/types";
import type { ViewId } from "../App";

/**
 * Review page (`PLAN.md` §11.4): the bilingual editor.
 *
 * Three columns per block — original, translated, corrected — with a diff rendered by the
 * CodeMirror 6 merge view (`components/CodeEditor.tsx`). Suggestions come from the editor
 * and proofreader passes; accepting one rewrites the block translation and recomposes the
 * chunk on the control plane, rejecting one only changes its status. The QA report below is
 * advisory and filterable.
 */

export interface ReviewViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const PASSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "both", label: "Editor + proofreader" },
  { value: "editor", label: "Solo editor" },
  { value: "proofreader", label: "Solo proofreader" },
];

const PASS_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "all", label: "Tutti i passaggi" },
  { value: "editor", label: "Editor" },
  { value: "proofreader", label: "Proofreader" },
];

const STATUS_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "pending", label: "Da decidere" },
  { value: "accepted", label: "Accettate" },
  { value: "rejected", label: "Rifiutate" },
  { value: "superseded", label: "Superate" },
  { value: "all", label: "Tutte" },
];

const QA_KINDS: ReadonlyArray<string> = [
  "untranslated",
  "glossary_mismatch",
  "placeholder_broken",
  "markdown_malformed",
  "length_anomaly",
  "duplicate",
  "empty",
  "latin_leftover",
  "glossary_conflict",
];

function originRank(origin: string): number {
  switch (origin) {
    case "user":
      return 3;
    case "proofreader":
      return 2;
    case "editor":
      return 1;
    default:
      return 0;
  }
}

/** The text currently in effect per block: newest wins, pass order breaks ties. */
function preferredByBlock(translations: readonly BlockTranslation[]): Map<string, string> {
  const best = new Map<string, { text: string; updatedAt: string; rank: number }>();
  for (const row of translations) {
    if (row.text_md.trim().length === 0) {
      continue;
    }
    const candidate = {
      text: row.text_md,
      updatedAt: row.updated_at,
      rank: originRank(row.origin),
    };
    const current = best.get(row.block_id);
    if (
      current === undefined ||
      candidate.updatedAt > current.updatedAt ||
      (candidate.updatedAt === current.updatedAt && candidate.rank > current.rank)
    ) {
      best.set(row.block_id, candidate);
    }
  }
  const texts = new Map<string, string>();
  for (const [blockId, value] of best) {
    texts.set(blockId, value.text);
  }
  return texts;
}

/** Mirrors `pipeline::review::accept_suggestion`: quote replacement, fallback to the whole proposal. */
function applyProposal(current: string, suggestion: Suggestion): string {
  const proposed = suggestion.proposed ?? "";
  if (suggestion.pass === "proofreader") {
    return proposed.trim().length > 0 ? proposed : current;
  }
  const quote = suggestion.quote ?? "";
  if (quote.length > 0 && current.includes(quote)) {
    return current.replace(quote, proposed);
  }
  return proposed.trim().length > 0 ? proposed : current;
}

function findingSeverityClass(severity: string): string {
  switch (severity) {
    case "critical":
      return "badge badge-danger";
    case "major":
      return "badge badge-warning";
    default:
      return "badge badge-neutral";
  }
}

export function ReviewView({ project, onNavigate }: ReviewViewProps) {
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [chunks, setChunks] = useState<Chunk[]>([]);
  const [chunkId, setChunkId] = useState<string | null>(null);
  const [detail, setDetail] = useState<ChunkDetail | null>(null);
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [findings, setFindings] = useState<QaFinding[]>([]);
  const [selectedSuggestionId, setSelectedSuggestionId] = useState<string | null>(null);
  const [selectedBlockId, setSelectedBlockId] = useState<string | null>(null);
  const [passFilter, setPassFilter] = useState("all");
  const [statusFilter, setStatusFilter] = useState("pending");
  const [qaKindFilter, setQaKindFilter] = useState("all");
  const [qaSeverityFilter, setQaSeverityFilter] = useState("all");
  const [pass, setPass] = useState("both");
  const [withQa, setWithQa] = useState(true);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);

  const projectId = project?.id ?? null;

  const loadWorld = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const [detailRow, chunkRows, suggestionRows, findingRows] = await Promise.all([
        projectGet(projectId),
        chunkList({ project_id: projectId }),
        suggestionList({ project_id: projectId }),
        qaReport({ project_id: projectId }),
      ]);
      setChapters(detailRow.chapters);
      const reviewable = chunkRows.filter(
        (chunk) =>
          (chunk.status === "done" || chunk.status === "needs_review") &&
          chunk.target_md !== null &&
          chunk.target_md.trim().length > 0,
      );
      setChunks(reviewable);
      setSuggestions(suggestionRows);
      setFindings(findingRows);
      setChunkId((current) =>
        current !== null && reviewable.some((chunk) => chunk.id === current)
          ? current
          : (reviewable[0]?.id ?? null),
      );
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  const loadDetail = useCallback(async () => {
    if (chunkId === null) {
      setDetail(null);
      return;
    }
    try {
      setDetail(await chunkGet(chunkId));
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setDetail(null);
    }
  }, [chunkId]);

  useEffect(() => {
    void loadWorld();
  }, [loadWorld, reloadToken]);

  useEffect(() => {
    void loadDetail();
  }, [loadDetail, reloadToken]);

  useEffect(
    () =>
      onJobProgress(() => {
        setReloadToken((current) => current + 1);
      }),
    [],
  );

  const chapterTitles = useMemo(() => {
    const map = new Map<string, string>();
    for (const chapter of chapters) {
      map.set(chapter.id, chapter.title);
    }
    return map;
  }, [chapters]);

  const visibleSuggestions = useMemo(
    () =>
      suggestions.filter(
        (suggestion) =>
          (passFilter === "all" || suggestion.pass === passFilter) &&
          (statusFilter === "all" || suggestion.status === statusFilter),
      ),
    [suggestions, passFilter, statusFilter],
  );

  useEffect(() => {
    if (visibleSuggestions.length === 0) {
      setSelectedSuggestionId(null);
      return;
    }
    if (!visibleSuggestions.some((suggestion) => suggestion.id === selectedSuggestionId)) {
      setSelectedSuggestionId(visibleSuggestions[0]?.id ?? null);
    }
  }, [visibleSuggestions, selectedSuggestionId]);

  const visibleFindings = useMemo(
    () =>
      findings.filter(
        (finding) =>
          (qaKindFilter === "all" || finding.kind === qaKindFilter) &&
          (qaSeverityFilter === "all" || finding.severity === qaSeverityFilter),
      ),
    [findings, qaKindFilter, qaSeverityFilter],
  );

  const selectedSuggestion = useMemo(
    () => suggestions.find((suggestion) => suggestion.id === selectedSuggestionId) ?? null,
    [suggestions, selectedSuggestionId],
  );

  const blockTexts = useMemo(() => preferredByBlock(detail?.translations ?? []), [detail]);

  const blocks = useMemo(() => detail?.blocks ?? [], [detail]);
  useEffect(() => {
    if (blocks.length === 0) {
      setSelectedBlockId(null);
      return;
    }
    if (selectedBlockId === null || !blocks.some((block) => block.id === selectedBlockId)) {
      setSelectedBlockId(blocks[0]?.id ?? null);
    }
  }, [blocks, selectedBlockId]);

  if (project === null) {
    return (
      <div className="section-stack">
        <h2 className="text-lg font-semibold text-ink">Revisione</h2>
        <EmptyState
          title="Nessun progetto aperto"
          description="La revisione lavora sulle traduzioni di un progetto: aprine uno e traducilo."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  async function runStart() {
    if (projectId === null) {
      return;
    }
    setBusy("starting");
    setError(null);
    setNotice(null);
    try {
      const result = await reviewStart({ project_id: projectId, pass, with_qa: withQa });
      setNotice(
        result.enqueued === 0
          ? "Nessun chunk da revisionare: traduzione già revisionata o ancora da fare."
          : `${result.enqueued} job di revisione accodati. I suggerimenti compariranno qui.`,
      );
    } catch (startError) {
      setError(toErrorMessage(startError));
    } finally {
      setBusy(null);
    }
  }

  async function runDecision(action: "accept" | "reject", id: string) {
    setBusy(`${action}:${id}`);
    setError(null);
    setNotice(null);
    try {
      if (action === "accept") {
        await suggestionAccept(id);
        setNotice("Modifica accettata: il blocco è stato riscritto.");
      } else {
        await suggestionReject(id);
        setNotice("Modifica rifiutata.");
      }
      setReloadToken((current) => current + 1);
    } catch (decisionError) {
      setError(toErrorMessage(decisionError));
    } finally {
      setBusy(null);
    }
  }

  const selected = selectedSuggestion;
  const selectedBlock = blocks.find((block) => block.id === selectedBlockId) ?? null;
  const selectedBlockIndex = blocks.findIndex((block) => block.id === selectedBlockId);
  const currentText =
    selectedBlock === null ? "" : (blockTexts.get(selectedBlock.id) ?? selectedBlock.source_md);
  const proposalActive = selected !== null && selectedBlock !== null && selected.block_id === selectedBlock.id;
  const proposedText = proposalActive && selected !== null ? applyProposal(currentText, selected) : currentText;

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Revisione</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> —{" "}
            {formatNumber(chunks.length)} chunk revisionabili,{" "}
            {formatNumber(visibleSuggestions.length)} suggerimenti visibili.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-1 text-xs text-muted">
            Passaggio
            <select
              className="select"
              style={{ width: "auto" }}
              value={pass}
              onChange={(event) => {
                setPass(event.target.value);
              }}
            >
              {PASSES.map((entry) => (
                <option key={entry.value} value={entry.value}>
                  {entry.label}
                </option>
              ))}
            </select>
          </label>
          <label className="flex items-center gap-1 text-xs text-muted">
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
            disabled={busy !== null}
            onClick={() => void runStart()}
          >
            {busy === "starting" ? <span className="spinner" aria-hidden="true" /> : null}
            Avvia revisione
          </button>
          <button
            type="button"
            className="btn"
            disabled={loading}
            onClick={() => {
              setReloadToken((current) => current + 1);
            }}
          >
            Aggiorna
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

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_22rem]">
        <div className="section-stack min-w-0">
          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Editor bilingue</span>
              <label className="flex items-center gap-1 text-xs text-muted">
                Chunk
                <select
                  className="select"
                  style={{ width: "auto", maxWidth: "22rem" }}
                  value={chunkId ?? ""}
                  disabled={chunks.length === 0}
                  onChange={(event) => {
                    setChunkId(event.target.value === "" ? null : event.target.value);
                    setSelectedSuggestionId(null);
                  }}
                >
                  {chunks.length === 0 ? <option value="">nessun chunk</option> : null}
                  {chunks.map((chunk) => (
                    <option key={chunk.id} value={chunk.id}>
                      {chunk.chapter_id === null
                        ? chunk.id
                        : `${chapterTitles.get(chunk.chapter_id) ?? chunk.chapter_id} · ${chunk.id}`}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            {chunks.length === 0 ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title="Nessun chunk da revisionare"
                  description="La revisione lavora sui chunk tradotti: avvia la traduzione, poi torna qui."
                  actionLabel="Vai alla traduzione"
                  onAction={() => {
                    onNavigate("translate");
                  }}
                />
              </div>
            ) : detail === null ? (
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Lettura del chunk…" />
              </div>
            ) : blocks.length === 0 ? (
              <div className="panel-pad">
                <EmptyState compact title="Il chunk non ha blocchi leggibili" />
              </div>
            ) : selectedBlock === null ? (
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Selezione del blocco…" />
              </div>
            ) : (
              <div className="panel-pad section-stack">
                <div className="flex flex-wrap items-center gap-1">
                  {blocks.map((block, index) => (
                    <button
                      key={block.id}
                      type="button"
                      className="btn btn-sm"
                      data-selected={block.id === selectedBlockId}
                      style={{ fontWeight: block.id === selectedBlockId ? 700 : 400 }}
                      onClick={() => {
                        setSelectedBlockId(block.id);
                      }}
                    >
                      {index + 1}. {block.kind}
                      {block.translatable ? "" : " · fisso"}
                    </button>
                  ))}
                  <span className="ml-auto flex items-center gap-1">
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={selectedBlockIndex <= 0}
                      onClick={() => {
                        setSelectedBlockId(blocks[selectedBlockIndex - 1]?.id ?? selectedBlockId);
                      }}
                    >
                      ◀
                    </button>
                    <button
                      type="button"
                      className="btn btn-sm"
                      disabled={selectedBlockIndex < 0 || selectedBlockIndex >= blocks.length - 1}
                      onClick={() => {
                        setSelectedBlockId(blocks[selectedBlockIndex + 1]?.id ?? selectedBlockId);
                      }}
                    >
                      ▶
                    </button>
                  </span>
                </div>

                <div className="flex flex-wrap items-center gap-2 text-[0.68rem] text-faint">
                  <span className="mono-chip">{selectedBlock.id}</span>
                  <span>{selectedBlock.kind}</span>
                  {proposalActive && selected !== null ? (
                    <>
                      <span className={severityClass(selected.severity)}>
                        {selected.severity ?? "nota"}
                      </span>
                      <span className="badge badge-warning">modifica selezionata</span>
                    </>
                  ) : (
                    <span className="badge badge-neutral">nessun suggerimento su questo blocco</span>
                  )}
                </div>

                <div className="grid grid-cols-3 gap-3">
                  <div className="field-label">Originale</div>
                  <div className="field-label col-span-2">
                    Tradotto (sinistra) → Corretto (destra), diff a caratteri
                  </div>
                </div>
                <div className="grid grid-cols-3 gap-3">
                  <ReadOnlyCode text={selectedBlock.source_md} />
                  <div className="col-span-2">
                    <MergeDiff before={currentText} after={proposedText} />
                  </div>
                </div>

                {proposalActive && selected !== null ? (
                  <div className="rounded border border-line p-2">
                    <div className="flex flex-wrap items-center gap-1">
                      <span className="field-label">Perché questa modifica</span>
                      <span className={severityClass(selected.severity)}>
                        {selected.severity ?? "nota"}
                      </span>
                      <span className="badge badge-neutral">{selected.pass}</span>
                    </div>
                    <p className="field-hint">
                      {selected.reason ??
                        (selected.pass === "proofreader"
                          ? "Il proofreader riscrive il blocco: nessuna spiegazione allegata."
                          : "Nessuna spiegazione per questa proposta.")}
                    </p>
                  </div>
                ) : null}
              </div>
            )}
          </div>
        </div>

        <div className="section-stack">
          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Suggerimenti</span>
              <span className="mono-chip">{formatNumber(visibleSuggestions.length)}</span>
            </div>
            <div className="panel-pad section-stack">
              <div className="flex flex-wrap items-center gap-2">
                <select
                  className="select"
                  style={{ width: "auto" }}
                  value={passFilter}
                  onChange={(event) => {
                    setPassFilter(event.target.value);
                  }}
                >
                  {PASS_FILTERS.map((entry) => (
                    <option key={entry.value} value={entry.value}>
                      {entry.label}
                    </option>
                  ))}
                </select>
                <select
                  className="select"
                  style={{ width: "auto" }}
                  value={statusFilter}
                  onChange={(event) => {
                    setStatusFilter(event.target.value);
                  }}
                >
                  {STATUS_FILTERS.map((entry) => (
                    <option key={entry.value} value={entry.value}>
                      {entry.label}
                    </option>
                  ))}
                </select>
              </div>

              {visibleSuggestions.length === 0 ? (
                <p className="field-hint">
                  Nessun suggerimento con questi filtri. Avvia la revisione per generarne.
                </p>
              ) : (
                <ul className="section-stack" style={{ maxHeight: "28rem", overflowY: "auto" }}>
                  {visibleSuggestions.map((suggestion) => {
                    const isSelected = suggestion.id === selectedSuggestionId;
                    return (
                      <li
                        key={suggestion.id}
                        className="rounded border border-line p-2"
                        data-selected={isSelected}
                        style={{ borderColor: isSelected ? "var(--accent)" : undefined }}
                      >
                        <button
                          type="button"
                          className="w-full text-left"
                          onClick={() => {
                            setSelectedSuggestionId(suggestion.id);
                            if (suggestion.chunk_id !== chunkId) {
                              setChunkId(suggestion.chunk_id);
                            }
                            if (suggestion.block_id !== null) {
                              setSelectedBlockId(suggestion.block_id);
                            }
                          }}
                        >
                          <span className="flex flex-wrap items-center gap-1">
                            <span className={severityClass(suggestion.severity)}>
                              {suggestion.severity ?? "nota"}
                            </span>
                            <span className="badge badge-neutral">{suggestion.pass}</span>
                            <span className="badge badge-neutral">{suggestion.status}</span>
                          </span>
                          <p className="mt-1 text-xs text-ink-soft">
                            {suggestionSnippet(suggestion)}
                          </p>
                          {suggestion.reason !== null ? (
                            <p className="field-hint">{suggestion.reason}</p>
                          ) : null}
                          <p className="mt-1 text-[0.68rem] text-faint">
                            chunk <span className="mono-chip">{suggestion.chunk_id}</span>
                            {suggestion.block_id === null ? "" : ` · blocco ${suggestion.block_id}`}
                          </p>
                        </button>
                        {suggestion.status === "pending" ? (
                          <span className="mt-2 flex items-center gap-2">
                            <button
                              type="button"
                              className="btn btn-sm btn-primary"
                              disabled={busy !== null}
                              onClick={() => void runDecision("accept", suggestion.id)}
                            >
                              Accetta
                            </button>
                            <button
                              type="button"
                              className="btn btn-sm"
                              disabled={busy !== null}
                              onClick={() => void runDecision("reject", suggestion.id)}
                            >
                              Rifiuta
                            </button>
                          </span>
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          </div>

          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Report QA</span>
              <span className="mono-chip">{formatNumber(visibleFindings.length)}</span>
            </div>
            <div className="panel-pad section-stack">
              <div className="flex flex-wrap items-center gap-2">
                <select
                  className="select"
                  style={{ width: "auto" }}
                  value={qaKindFilter}
                  onChange={(event) => {
                    setQaKindFilter(event.target.value);
                  }}
                >
                  <option value="all">Tutti i tipi</option>
                  {QA_KINDS.map((kind) => (
                    <option key={kind} value={kind}>
                      {kind}
                    </option>
                  ))}
                </select>
                <select
                  className="select"
                  style={{ width: "auto" }}
                  value={qaSeverityFilter}
                  onChange={(event) => {
                    setQaSeverityFilter(event.target.value);
                  }}
                >
                  <option value="all">Tutte le gravità</option>
                  <option value="critical">Critica</option>
                  <option value="major">Grave</option>
                  <option value="minor">Minore</option>
                </select>
              </div>

              {visibleFindings.length === 0 ? (
                <p className="field-hint">Nessun rilievo con questi filtri.</p>
              ) : (
                <ul className="section-stack" style={{ maxHeight: "20rem", overflowY: "auto" }}>
                  {visibleFindings.map((finding) => (
                    <li key={finding.id} className="rounded border border-line p-2 text-xs">
                      <span className="flex flex-wrap items-center gap-1">
                        <span className={findingSeverityClass(finding.severity)}>
                          {finding.severity}
                        </span>
                        <span className="badge badge-neutral">{finding.kind}</span>
                      </span>
                      <p className="mt-1 text-[0.68rem] text-faint">
                        chunk <span className="mono-chip">{finding.chunk_id ?? "—"}</span>
                      </p>
                      <p className="mt-1 font-mono text-[0.65rem] break-all text-muted">
                        {finding.details_json}
                      </p>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
