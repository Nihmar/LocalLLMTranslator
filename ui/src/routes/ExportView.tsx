import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { ProgressBar } from "../components/ProgressBar";
import { StatusBadge } from "../components/StatusBadge";
import { onExportProgress } from "../lib/events";
import { basename, countLabel, formatDuration, formatNumber, truncate } from "../lib/format";
import { chunkList, exportBuild, openPath, toErrorMessage } from "../lib/ipc";
import type {
  ExportBuildResult,
  ExportFormat,
  ExportProgressEvent,
  ExportUnit,
  Project,
} from "../lib/types";
import type { ViewId } from "../App";

/**
 * Step 5 of the wizard: format, template and CSS selection, per-chapter units, build and open
 * the result (`PLAN.md` §11.5).
 *
 * This page is functional for what the frozen contract allows: it triggers `export_build`,
 * follows `export://progress` and reveals the artefact with `open_path`. Two things PLAN.md
 * mentions but the frozen table cannot express are absent by design — a live preview of the
 * rendered page (`export_preview` is not in the table) and the build history (no such command).
 * Chapter units are derived from `chunk_list`, the only command that reports chapter titles.
 */

export interface ExportViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

interface ChapterOption {
  id: string;
  title: string;
  order: number;
  chunks: number;
  tokens: number;
}

const FORMATS: ReadonlyArray<{
  value: ExportFormat;
  label: string;
  extension: string;
  templates: readonly string[];
  css: readonly string[];
  note: string;
}> = [
  {
    value: "pdf",
    label: "PDF",
    extension: "pdf",
    templates: ["pandoc/templates/book.tex"],
    css: [],
    note: "Impaginazione LaTeX; i filtri Lua gestiscono note e tavole.",
  },
  {
    value: "epub",
    label: "EPUB",
    extension: "epub",
    templates: ["pandoc/templates/book.html"],
    css: ["pandoc/styles/book.css"],
    note: "Indice, note e immagini generati da Pandoc.",
  },
  {
    value: "docx",
    label: "DOCX",
    extension: "docx",
    templates: [],
    css: [],
    note: "Nessun template: Pandoc usa il documento di riferimento predefinito.",
  },
];

const PROGRESS_BUFFER = 12;

function parentDirectory(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  if (index <= 0) {
    return trimmed;
  }
  return trimmed.slice(0, index);
}

function looksAbsolute(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}

function phaseLabel(phase: ExportProgressEvent["phase"]): string {
  switch (phase) {
    case "prepare":
      return "Preparazione delle unità";
    case "render":
      return "Composizione del Markdown per capitolo";
    case "pandoc":
      return "Esecuzione di Pandoc";
    case "done":
      return "Completato";
    case "failed":
      return "Fallito";
  }
}

export function ExportView({ project, onNavigate }: ExportViewProps) {
  const [chapters, setChapters] = useState<ChapterOption[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedChapters, setSelectedChapters] = useState<ReadonlySet<string>>(new Set());

  const [format, setFormat] = useState<ExportFormat>("epub");
  const [template, setTemplate] = useState("pandoc/templates/book.html");
  const [css, setCss] = useState("pandoc/styles/book.css");
  const [outputPath, setOutputPath] = useState("");

  const [building, setBuilding] = useState(false);
  const [buildError, setBuildError] = useState<string | null>(null);
  const [result, setResult] = useState<ExportBuildResult | null>(null);
  const [progress, setProgress] = useState<readonly ExportProgressEvent[]>([]);
  const [openError, setOpenError] = useState<string | null>(null);

  const projectId = project?.id ?? null;
  const formatSpec = useMemo(
    () => FORMATS.find((entry) => entry.value === format) ?? FORMATS[0],
    [format],
  );

  const loadChapters = useCallback(async () => {
    if (projectId === null) {
      setChapters([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const chunks = await chunkList({ project_id: projectId, limit: 5000 });
      const byChapter = new Map<string, ChapterOption>();
      for (const chunk of chunks) {
        const id = chunk.chapter_id ?? "__none__";
        const existing = byChapter.get(id);
        if (existing === undefined) {
          byChapter.set(id, {
            id,
            title: chunk.chapter_title ?? "Senza capitolo",
            order: chunk.order_index,
            chunks: 1,
            tokens: chunk.token_estimate,
          });
        } else {
          existing.chunks += 1;
          existing.tokens += chunk.token_estimate;
          existing.order = Math.min(existing.order, chunk.order_index);
        }
      }
      const ordered = [...byChapter.values()].sort((left, right) => left.order - right.order);
      setChapters(ordered);
      setSelectedChapters(new Set(ordered.map((chapter) => chapter.id)));
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setChapters([]);
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void loadChapters();
  }, [loadChapters]);

  useEffect(() => {
    setResult(null);
    setProgress([]);
    setBuildError(null);
    setOpenError(null);
  }, [projectId]);

  useEffect(
    () =>
      onExportProgress((event) => {
        if (projectId === null || event.project_id !== projectId) {
          return;
        }
        setProgress((current) => {
          const next = [event, ...current];
          return next.length > PROGRESS_BUFFER ? next.slice(0, PROGRESS_BUFFER) : next;
        });
      }),
    [projectId],
  );

  // Keep the template/CSS fields aligned with the chosen format.
  useEffect(() => {
    const spec = formatSpec;
    if (spec === undefined) {
      return;
    }
    setTemplate(spec.templates[0] ?? "");
    setCss(spec.css[0] ?? "");
  }, [formatSpec]);

  const latest = progress[0];
  const totalChunks = useMemo(
    () => chapters.reduce((sum, chapter) => sum + chapter.chunks, 0),
    [chapters],
  );
  const selectedUnits = useMemo<ExportUnit[]>(
    () =>
      chapters
        .filter((chapter) => selectedChapters.has(chapter.id))
        .map((chapter) => ({ path: chapter.id, title: chapter.title })),
    [chapters, selectedChapters],
  );

  async function handleBuild() {
    if (projectId === null || formatSpec === undefined) {
      return;
    }
    if (outputPath.trim().length > 0 && !looksAbsolute(outputPath.trim())) {
      setBuildError("Il percorso di destinazione deve essere assoluto, oppure lascialo vuoto.");
      return;
    }

    setBuilding(true);
    setBuildError(null);
    setOpenError(null);
    setResult(null);
    setProgress([]);
    try {
      const built = await exportBuild({
        project_id: projectId,
        output_format: formatSpec.value,
        template: template.trim().length > 0 ? template.trim() : null,
        css: formatSpec.css.length > 0 && css.trim().length > 0 ? css.trim() : null,
        // An empty selection means "every chapter": the driver splits the document itself.
        units: selectedUnits.length === chapters.length ? [] : selectedUnits,
        output_path: outputPath.trim().length > 0 ? outputPath.trim() : null,
      });
      setResult(built);
    } catch (buildFailure) {
      setBuildError(toErrorMessage(buildFailure));
    } finally {
      setBuilding(false);
    }
  }

  async function handleOpen(path: string) {
    setOpenError(null);
    try {
      await openPath(path);
    } catch (openFailure) {
      setOpenError(toErrorMessage(openFailure));
    }
  }

  if (project === null) {
    return (
      <div className="section-stack">
        <h2 className="text-lg font-semibold text-ink">Export</h2>
        <EmptyState
          title="Nessun progetto aperto"
          description="L'export impagina la traduzione di un progetto: aprine uno per scegliere formato e capitoli."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Export</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> — Pandoc
            impagina il Markdown tradotto secondo formato, template e CSS scelti.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="btn"
            disabled={loading || building}
            onClick={() => {
              void loadChapters();
            }}
          >
            Aggiorna capitoli
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={building || formatSpec === undefined}
            onClick={() => {
              void handleBuild();
            }}
          >
            {building ? <span className="spinner" aria-hidden="true" /> : null}
            {building ? "Build in corso…" : "Genera output"}
          </button>
        </div>
      </div>

      {loading ? (
        <EmptyState tone="loading" title="Lettura dei capitoli…" />
      ) : error !== null ? (
        <EmptyState
          tone="error"
          title="Impossibile leggere i capitoli"
          description="L'elenco delle unità esportabili viene ricavato dai chunk del progetto."
          details={error}
          actionLabel="Riprova"
          onAction={() => {
            void loadChapters();
          }}
        />
      ) : chapters.length === 0 ? (
        <EmptyState
          title="Nessun chunk da esportare"
          description="Non c'è ancora nulla da impaginare: importa il documento e avvia la traduzione."
          actionLabel="Vai all'ingestione"
          onAction={() => {
            onNavigate("ingest");
          }}
        />
      ) : (
        <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_24rem]">
          <div className="section-stack min-w-0">
            <div className="panel">
              <div className="panel-head">
                <span className="panel-title">Unità da esportare</span>
                <span className="flex items-center gap-2">
                  <span className="mono-chip">
                    {formatNumber(selectedChapters.size)} / {formatNumber(chapters.length)} capitoli
                  </span>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => {
                      setSelectedChapters(new Set(chapters.map((chapter) => chapter.id)));
                    }}
                  >
                    Tutti
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => {
                      setSelectedChapters(new Set());
                    }}
                  >
                    Nessuno
                  </button>
                </span>
              </div>

              <div className="table-scroll" style={{ maxHeight: "22rem" }}>
                <table className="data-table">
                  <thead>
                    <tr>
                      <th style={{ width: "2.2rem" }} />
                      <th style={{ minWidth: "14rem" }}>Capitolo</th>
                      <th style={{ width: "6rem" }} className="num">
                        Chunk
                      </th>
                      <th style={{ width: "7rem" }} className="num">
                        Token
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {chapters.map((chapter) => {
                      const checked = selectedChapters.has(chapter.id);
                      return (
                        <tr key={chapter.id} data-selected={checked ? "true" : "false"}>
                          <td>
                            <input
                              type="checkbox"
                              checked={checked}
                              onChange={(event) => {
                                setSelectedChapters((current) => {
                                  const next = new Set(current);
                                  if (event.target.checked) {
                                    next.add(chapter.id);
                                  } else {
                                    next.delete(chapter.id);
                                  }
                                  return next;
                                });
                              }}
                              aria-label={`Esporta ${chapter.title}`}
                            />
                          </td>
                          <td>
                            <span className="block truncate text-ink-soft" title={chapter.title}>
                              {chapter.title}
                            </span>
                            <span className="font-mono text-[0.68rem] text-faint">{chapter.id}</span>
                          </td>
                          <td className="num">{formatNumber(chapter.chunks)}</td>
                          <td className="num">{formatNumber(chapter.tokens)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>

              <div className="panel-pad">
                <p className="text-[0.72rem] text-faint">
                  Selezionando tutti i capitoli viene richiesta una build unica dell&apos;intero
                  documento; selezionandone solo alcuni viene richiesta una build selettiva delle
                  sole unità scelte (rebuild incrementale di PLAN.md §11.5).
                </p>
              </div>
            </div>

            {buildError !== null ? (
              <div className="banner banner-error" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>{buildError}</span>
              </div>
            ) : null}

            {building ? (
              <div className="panel panel-pad section-stack">
                <ProgressBar
                  value={latest?.done ?? 0}
                  total={latest?.total === undefined || latest.total === 0 ? 1 : latest.total}
                  label={latest === undefined ? "Avvio della build" : phaseLabel(latest.phase)}
                  indeterminate={latest === undefined}
                  tone="accent"
                  showCounts={latest !== undefined}
                />
                <p className="text-[0.72rem] text-faint">
                  Pandoc lavora su file temporanei e rinomina il risultato solo a fine build.
                </p>
              </div>
            ) : null}

            {progress.length > 0 ? (
              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Avanzamento ricevuto</span>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => {
                      setProgress([]);
                    }}
                  >
                    Svuota
                  </button>
                </div>
                <ul className="panel-pad space-y-1 text-[0.72rem]">
                  {progress.map((event, index) => (
                    <li
                      key={`${event.unit}-${event.phase}-${String(index)}`}
                      className="flex items-baseline gap-2"
                    >
                      <StatusBadge
                        status={event.phase === "failed" ? "failed" : event.phase === "done" ? "done" : "running"}
                        label={phaseLabel(event.phase)}
                      />
                      <span className="font-mono text-muted">
                        {formatNumber(event.done)}/{formatNumber(event.total)}
                      </span>
                      <span className="truncate text-ink-soft" title={event.unit}>
                        {event.unit}
                      </span>
                      {event.message === null ? null : (
                        <span className="truncate text-muted" title={event.message}>
                          {truncate(event.message, 60)}
                        </span>
                      )}
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            {result !== null ? (
              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Risultato</span>
                  <span className="flex items-center gap-2">
                    <StatusBadge status="done" />
                    <span className="mono-chip">{formatDuration(result.duration_ms)}</span>
                  </span>
                </div>

                <div className="panel-pad section-stack">
                  <div className="banner banner-ok" role="status">
                    <span aria-hidden="true">✓</span>
                    <span>
                      File generato:{" "}
                      <span className="font-mono text-ink">{result.output_path}</span>
                    </span>
                  </div>

                  {openError !== null ? (
                    <div className="banner banner-error" role="alert">
                      <span aria-hidden="true">⚠</span>
                      <span>{openError}</span>
                    </div>
                  ) : null}

                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      className="btn btn-primary"
                      onClick={() => {
                        void handleOpen(result.output_path);
                      }}
                    >
                      Apri output
                    </button>
                    <button
                      type="button"
                      className="btn"
                      onClick={() => {
                        void handleOpen(parentDirectory(result.output_path));
                      }}
                    >
                      Apri cartella
                    </button>
                    <span className="mono-chip" title={result.output_path}>
                      {basename(result.output_path)}
                    </span>
                  </div>

                  <div>
                    <div className="stat-label mb-1">Log di Pandoc</div>
                    <pre className="max-h-64 overflow-auto rounded-md border border-line bg-canvas p-3 font-mono text-[0.7rem] whitespace-pre-wrap text-ink-soft">
                      {result.log.length === 0 ? "— nessun output —" : result.log}
                    </pre>
                  </div>
                </div>
              </div>
            ) : null}
          </div>

          <div className="section-stack">
            <div className="panel">
              <div className="panel-head">
                <span className="panel-title">Impostazioni di build</span>
              </div>

              <div className="panel-pad section-stack">
                <FormField
                  label="Formato di output"
                  htmlFor="export-format"
                  hint={formatSpec?.note ?? undefined}
                >
                  <select
                    id="export-format"
                    className="select"
                    value={format}
                    onChange={(event) => {
                      const matched = FORMATS.find((entry) => entry.value === event.target.value);
                      if (matched !== undefined) {
                        setFormat(matched.value);
                      }
                    }}
                  >
                    {FORMATS.map((entry) => (
                      <option key={entry.value} value={entry.value}>
                        {entry.label}
                      </option>
                    ))}
                  </select>
                </FormField>

                <FormField
                  label="Template Pandoc"
                  htmlFor="export-template"
                  hint={
                    formatSpec === undefined || formatSpec.templates.length === 0
                      ? "Non applicabile a questo formato: lascia vuoto."
                      : `Percorso del template; suggerito: ${formatSpec.templates.join(", ")}`
                  }
                >
                  <input
                    id="export-template"
                    className="input"
                    list="export-template-options"
                    value={template}
                    spellCheck={false}
                    onChange={(event) => {
                      setTemplate(event.target.value);
                    }}
                    placeholder="pandoc/templates/book.tex"
                  />
                  <datalist id="export-template-options">
                    {(formatSpec?.templates ?? []).map((entry) => (
                      <option key={entry} value={entry} />
                    ))}
                  </datalist>
                </FormField>

                <FormField
                  label="Foglio di stile CSS"
                  htmlFor="export-css"
                  hint={
                    formatSpec === undefined || formatSpec.css.length === 0
                      ? "Ignorato dai formati non HTML."
                      : `Suggerito: ${formatSpec.css.join(", ")}`
                  }
                >
                  <input
                    id="export-css"
                    className="input"
                    list="export-css-options"
                    value={css}
                    spellCheck={false}
                    disabled={formatSpec === undefined || formatSpec.css.length === 0}
                    onChange={(event) => {
                      setCss(event.target.value);
                    }}
                    placeholder="pandoc/styles/book.css"
                  />
                  <datalist id="export-css-options">
                    {(formatSpec?.css ?? []).map((entry) => (
                      <option key={entry} value={entry} />
                    ))}
                  </datalist>
                </FormField>

                <FormField
                  label="Percorso di destinazione"
                  htmlFor="export-output"
                  hint="Vuoto: il file viene scritto nella cartella di output del progetto."
                >
                  <input
                    id="export-output"
                    className="input"
                    value={outputPath}
                    spellCheck={false}
                    onChange={(event) => {
                      setOutputPath(event.target.value);
                    }}
                    placeholder="/home/utente/output/libro.epub"
                  />
                </FormField>

                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={building}
                  onClick={() => {
                    void handleBuild();
                  }}
                >
                  {building ? <span className="spinner" aria-hidden="true" /> : null}
                  Genera output
                </button>

                <p className="text-[0.72rem] text-faint">
                  Unità selezionate: {countLabel(selectedUnits.length, "capitolo", "capitoli")} su{" "}
                  {formatNumber(chapters.length)} ({formatNumber(totalChunks)} chunk in totale).
                </p>
              </div>
            </div>

            <div className="panel panel-pad">
              <div className="panel-title mb-2">Note sullo scope</div>
              <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
                <li>
                  L&apos;anteprima impaginata richiede <span className="mono-chip">export_preview</span>,
                  previsto da PLAN.md §12.2 ma assente dal contratto UI → Tauri: qui non è offerta.
                </li>
                <li>
                  La cronologia delle build non ha un comando dedicato: l&apos;ultimo risultato resta
                  visibile finché non ricarichi la pagina.
                </li>
                <li>
                  <span className="mono-chip">metadata.yaml</span> viene composto dal driver Pandoc a
                  partire dai metadati del documento.
                </li>
              </ul>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
