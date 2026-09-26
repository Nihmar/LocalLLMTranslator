import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge } from "../components/StatusBadge";
import { onExportProgress } from "../lib/events";
import { basename, countLabel, formatDuration, formatNumber } from "../lib/format";
import { exportBuild, openPath, projectGet, toErrorMessage } from "../lib/ipc";
import type { Chapter, ExportFormat, ExportOutcome, ExportProgressEvent, Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Step 5 of the wizard: format, template and CSS selection, build and open the result
 * (`PLAN.md` §11.5).
 *
 * `export_build` takes only a project and the rendering options: the backend splits the document
 * into per-chapter units itself, so there is no `units` argument and no per-chapter selection to
 * send — the chapter list is read-only context from `project_get`. The build is followed through
 * `export://progress` and the artefact is revealed with `open_path`.
 *
 * Two things PLAN.md mentions but the frozen table cannot express are absent by design — a live
 * preview of the rendered page (`export_preview` is not in the table) and the build history (no
 * such command).
 */

export interface ExportViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
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

/** Italian description of the last `export://progress` state. */
function progressLabel(event: ExportProgressEvent): string {
  if (event.state === "started") {
    return `Build avviata${event.format === undefined ? "" : ` (${event.format.toUpperCase()})`}.`;
  }
  if (event.state === "done") {
    return `Build conclusa${event.output_path === undefined ? "" : `: ${event.output_path}`}.`;
  }
  return `Stato: ${event.state}.`;
}

export function ExportView({ project, onNavigate }: ExportViewProps) {
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [format, setFormat] = useState<ExportFormat>("epub");
  const [template, setTemplate] = useState("pandoc/templates/book.html");
  const [css, setCss] = useState("pandoc/styles/book.css");
  const [outputPath, setOutputPath] = useState("");

  const [building, setBuilding] = useState(false);
  const [buildError, setBuildError] = useState<string | null>(null);
  const [result, setResult] = useState<ExportOutcome | null>(null);
  const [progress, setProgress] = useState<ExportProgressEvent | null>(null);
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
      const detail = await projectGet(projectId);
      setChapters(detail.chapters);
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
    setProgress(null);
    setBuildError(null);
    setOpenError(null);
  }, [projectId]);

  useEffect(
    () =>
      onExportProgress((event) => {
        setProgress(event);
      }),
    [],
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
    setProgress(null);
    try {
      const built = await exportBuild({
        project_id: projectId,
        output_format: formatSpec.value,
        template: template.trim().length > 0 ? template.trim() : null,
        css: formatSpec.css.length > 0 && css.trim().length > 0 ? css.trim() : null,
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
          description="L'elenco delle unità esportabili viene ricavato dai capitoli del progetto."
          details={error}
          actionLabel="Riprova"
          onAction={() => {
            void loadChapters();
          }}
        />
      ) : chapters.length === 0 ? (
        <EmptyState
          title="Nessun capitolo da esportare"
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
                <span className="mono-chip">{formatNumber(chapters.length)} capitoli</span>
              </div>

              <div className="table-scroll" style={{ maxHeight: "22rem" }}>
                <table className="data-table">
                  <thead>
                    <tr>
                      <th style={{ width: "4.5rem" }} className="num">
                        Ordine
                      </th>
                      <th style={{ minWidth: "14rem" }}>Capitolo</th>
                      <th style={{ width: "5rem" }}>Livello</th>
                      <th style={{ width: "9rem" }} className="num">
                        Blocchi
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {chapters.map((chapter) => (
                      <tr key={chapter.id}>
                        <td className="num">{formatNumber(chapter.order_index)}</td>
                        <td>
                          <span className="block truncate text-ink-soft" title={chapter.title}>
                            {chapter.title}
                          </span>
                          <span className="font-mono text-[0.68rem] text-faint">{chapter.id}</span>
                        </td>
                        <td>
                          <span className="mono-chip">H{formatNumber(chapter.level)}</span>
                        </td>
                        <td className="num font-mono text-[0.72rem] text-muted">
                          {`${formatNumber(chapter.block_first)} – ${formatNumber(chapter.block_last)}`}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>

              <div className="panel-pad">
                <p className="text-[0.72rem] text-faint">
                  Il backend suddivide il documento in unità per capitolo e le impagina tutte in una
                  sola build: il contratto attuale non espone una selezione parziale, quindi qui
                  l&apos;elenco è di sola lettura.
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
              <div className="panel panel-pad">
                <p className="flex items-center gap-2 text-xs text-muted">
                  <span className="spinner" aria-hidden="true" />
                  {progress === null ? "Avvio della build…" : progressLabel(progress)}
                </p>
              </div>
            ) : progress !== null && result === null ? (
              <div className="panel panel-pad">
                <p className="text-xs text-muted">{progressLabel(progress)}</p>
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
                      File generato ({countLabel(result.units, "unità", "unità")}):{" "}
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
