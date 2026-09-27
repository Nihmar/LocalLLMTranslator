import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge } from "../components/StatusBadge";
import { onExportProgress } from "../lib/events";
import { basename, countLabel, formatDuration, formatNumber } from "../lib/format";
import {
  exportBuild,
  exportHistory,
  exportPreview,
  openPath,
  projectGet,
  toErrorMessage,
} from "../lib/ipc";
import type {
  Chapter,
  ExportBuildRecord,
  ExportFormat,
  ExportOutcome,
  ExportPreview,
  ExportProgressEvent,
  Project,
} from "../lib/types";
import type { ViewId } from "../App";

/**
 * Export page (`PLAN.md` §11.5): format, template and CSS selection, build, preview and build
 * history.
 *
 * `export_build` takes only a project and the rendering options: the backend splits the document
 * into per-chapter units itself, resolves the default template/CSS/Lua filters from the `pandoc/`
 * assets, and skips the build when nothing changed. `export_preview` composes the same units for
 * an in-app look at the content, `export_history` returns the recent builds. The build is followed
 * through `export://progress` and the artifact is revealed with `open_path`.
 */

export interface ExportViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const FORMATS: ReadonlyArray<{
  value: ExportFormat | "html";
  label: string;
  extension: string;
  templateHint: string;
  cssHint: string;
  note: string;
}> = [
  {
    value: "pdf",
    label: "PDF",
    extension: "pdf",
    templateHint: "pandoc/templates/book.tex",
    cssHint: "",
    note: "Impaginazione LaTeX con indice, filtro note e suddivisione in capitoli.",
  },
  {
    value: "epub",
    label: "EPUB",
    extension: "epub",
    templateHint: "pandoc/templates/book.html",
    cssHint: "pandoc/styles/book.css",
    note: "Indice, note e immagini generati da Pandoc.",
  },
  {
    value: "html",
    label: "HTML",
    extension: "html",
    templateHint: "pandoc/templates/book.html",
    cssHint: "pandoc/styles/book.css",
    note: "Utile per l'anteprima impaginata nel browser.",
  },
  {
    value: "docx",
    label: "DOCX",
    extension: "docx",
    templateHint: "",
    cssHint: "",
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

function timestampLabel(value: string): string {
  return value.replace("T", " ").slice(0, 19);
}

export function ExportView({ project, onNavigate }: ExportViewProps) {
  const [chapters, setChapters] = useState<Chapter[]>([]);
  const [history, setHistory] = useState<ExportBuildRecord[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [format, setFormat] = useState<ExportFormat | "html">("epub");
  const [template, setTemplate] = useState("");
  const [css, setCss] = useState("");
  const [outputPath, setOutputPath] = useState("");
  const [toc, setToc] = useState(true);
  const [force, setForce] = useState(false);
  const [chapterScope, setChapterScope] = useState("all");

  const [building, setBuilding] = useState(false);
  const [buildError, setBuildError] = useState<string | null>(null);
  const [result, setResult] = useState<ExportOutcome | null>(null);
  const [progress, setProgress] = useState<ExportProgressEvent | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);

  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewUnit, setPreviewUnit] = useState(0);

  const projectId = project?.id ?? null;
  const formatSpec = useMemo(
    () => FORMATS.find((entry) => entry.value === format) ?? FORMATS[0],
    [format],
  );

  const loadContext = useCallback(async () => {
    if (projectId === null) {
      setChapters([]);
      setHistory([]);
      setLoading(false);
      return;
    }
    setLoading(true);
    setError(null);
    try {
      const [detail, builds] = await Promise.all([
        projectGet(projectId),
        exportHistory(projectId),
      ]);
      setChapters(detail.chapters);
      setHistory(builds);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setChapters([]);
      setHistory([]);
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    void loadContext();
  }, [loadContext]);

  useEffect(() => {
    setResult(null);
    setProgress(null);
    setBuildError(null);
    setOpenError(null);
    setPreview(null);
    setPreviewError(null);
    setChapterScope("all");
  }, [projectId]);

  useEffect(
    () =>
      onExportProgress((event) => {
        setProgress(event);
      }),
    [],
  );

  const selectedPreviewUnit = preview?.units[previewUnit] ?? null;

  async function handlePreview() {
    if (projectId === null) {
      return;
    }
    setPreviewLoading(true);
    setPreviewError(null);
    try {
      const composed = await exportPreview({
        project_id: projectId,
        chapter_id: chapterScope === "all" ? null : chapterScope,
      });
      setPreview(composed);
      setPreviewUnit(0);
    } catch (previewFailure) {
      setPreviewError(toErrorMessage(previewFailure));
      setPreview(null);
    } finally {
      setPreviewLoading(false);
    }
  }

  async function handleBuild() {
    if (projectId === null || formatSpec === undefined) {
      return;
    }
    const trimmedOutput = outputPath.trim();
    const trimmedTemplate = template.trim();
    const trimmedCss = css.trim();
    if (trimmedOutput.length > 0 && !looksAbsolute(trimmedOutput)) {
      setBuildError("Il percorso di destinazione deve essere assoluto, oppure lascialo vuoto.");
      return;
    }
    if (trimmedTemplate.length > 0 && !looksAbsolute(trimmedTemplate)) {
      setBuildError("Il percorso del template deve essere assoluto (o vuoto per il default).");
      return;
    }
    if (trimmedCss.length > 0 && !looksAbsolute(trimmedCss)) {
      setBuildError("Il percorso del CSS deve essere assoluto (o vuoto per il default).");
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
        template: trimmedTemplate.length > 0 ? trimmedTemplate : null,
        css: trimmedCss.length > 0 ? trimmedCss : null,
        output_path: trimmedOutput.length > 0 ? trimmedOutput : null,
        toc,
        chapter_id: chapterScope === "all" ? null : chapterScope,
        force,
      });
      setResult(built);
      setHistory(await exportHistory(projectId).catch(() => history));
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
            impagina il Markdown tradotto con i template e i filtri Lua del pacchetto.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="btn"
            disabled={loading || building}
            onClick={() => {
              void loadContext();
            }}
          >
            Aggiorna
          </button>
          <button
            type="button"
            className="btn"
            disabled={previewLoading || loading}
            onClick={() => {
              void handlePreview();
            }}
          >
            {previewLoading ? <span className="spinner" aria-hidden="true" /> : null}
            Anteprima
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
            void loadContext();
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
            {preview !== null ? (
              <div className="panel">
                <div className="panel-head">
                  <span className="panel-title">Anteprima</span>
                  <span className="flex items-center gap-2">
                    <select
                      className="select"
                      style={{ width: "auto", maxWidth: "20rem" }}
                      value={String(previewUnit)}
                      onChange={(event) => {
                        setPreviewUnit(Number(event.target.value));
                      }}
                    >
                      {preview.units.map((unit, index) => (
                        <option key={unit.key} value={String(index)}>
                          {unit.title}
                        </option>
                      ))}
                    </select>
                    <span className="mono-chip">
                      {formatNumber(preview.total_chunks - preview.untranslated_chunks)}/
                      {formatNumber(preview.total_chunks)} tradotti
                    </span>
                    <button
                      type="button"
                      className="btn btn-sm"
                      onClick={() => {
                        setPreview(null);
                      }}
                    >
                      Chiudi
                    </button>
                  </span>
                </div>
                <div className="panel-pad section-stack">
                  {previewError !== null ? (
                    <div className="banner banner-error" role="alert">
                      <span aria-hidden="true">⚠</span>
                      <span>{previewError}</span>
                    </div>
                  ) : null}
                  {selectedPreviewUnit !== null ? (
                    <>
                      <div className="grid grid-cols-3 gap-2">
                        <div className="stat-tile">
                          <div className="stat-label">Unità</div>
                          <div className="truncate text-xs text-ink-soft">
                            {selectedPreviewUnit.title}
                          </div>
                        </div>
                        <div className="stat-tile">
                          <div className="stat-label">Chunk</div>
                          <div className="stat-value">{formatNumber(selectedPreviewUnit.chunks)}</div>
                        </div>
                        <div className="stat-tile">
                          <div className="stat-label">Da tradurre</div>
                          <div className="stat-value">
                            {formatNumber(selectedPreviewUnit.untranslated)}
                          </div>
                        </div>
                      </div>
                      <pre className="max-h-96 overflow-auto rounded-md border border-line bg-canvas p-3 font-mono text-[0.7rem] whitespace-pre-wrap text-ink-soft">
                        {selectedPreviewUnit.markdown}
                      </pre>
                      <details>
                        <summary className="cursor-pointer text-xs text-muted">
                          metadata.yaml
                        </summary>
                        <pre className="mt-2 max-h-48 overflow-auto rounded-md border border-line bg-canvas p-3 font-mono text-[0.7rem] whitespace-pre-wrap text-ink-soft">
                          {preview.metadata_yaml}
                        </pre>
                      </details>
                    </>
                  ) : null}
                </div>
              </div>
            ) : previewError !== null ? (
              <div className="banner banner-error" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>{previewError}</span>
              </div>
            ) : null}

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
                  La build impagina tutte le unità in un solo file; scegli un capitolo per generarne
                  una versione autonoma. I capitoli senza modifiche vengono riutilizzati: il build
                  salta Pandoc quando nulla è cambiato.
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
                    <StatusBadge status={result.from_cache ? "cached" : "done"} />
                    <span className="mono-chip">{formatDuration(result.duration_ms)}</span>
                  </span>
                </div>

                <div className="panel-pad section-stack">
                  <div className={result.from_cache ? "banner" : "banner banner-ok"} role="status">
                    <span aria-hidden="true">{result.from_cache ? "↺" : "✓"}</span>
                    <span>
                      {result.from_cache
                        ? "Nessuna modifica: build saltata, output riutilizzato"
                        : "File generato"}
                      {` (${countLabel(result.units, "unità", "unità")}): `}
                      <span className="font-mono text-ink">{result.output_path}</span>
                    </span>
                  </div>

                  {!result.from_cache ? (
                    <p className="field-hint">
                      Capitoli ricostruiti: {formatNumber(result.changed_units.length)} · riutilizzati:{" "}
                      {formatNumber(result.reused_units)}
                    </p>
                  ) : null}

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

            <div className="panel">
              <div className="panel-head">
                <span className="panel-title">Cronologia build</span>
                <span className="mono-chip">{formatNumber(history.length)}</span>
              </div>
              {history.length === 0 ? (
                <div className="panel-pad">
                  <p className="field-hint">Nessuna build registrata per questo progetto.</p>
                </div>
              ) : (
                <div className="table-scroll" style={{ maxHeight: "18rem" }}>
                  <table className="data-table">
                    <thead>
                      <tr>
                        <th>Quando</th>
                        <th>Formato</th>
                        <th>Ambito</th>
                        <th className="num">Unità</th>
                        <th>Esito</th>
                      </tr>
                    </thead>
                    <tbody>
                      {history.map((record) => (
                        <tr key={record.id}>
                          <td className="font-mono text-[0.7rem] text-muted">
                            {timestampLabel(record.built_at)}
                          </td>
                          <td>
                            <span className="mono-chip">{record.output_format}</span>
                          </td>
                          <td className="text-xs text-ink-soft">
                            {record.chapter_id === null ? "libro" : record.chapter_id}
                          </td>
                          <td className="num font-mono text-[0.72rem] text-muted">
                            {formatNumber(record.units)}
                          </td>
                          <td>
                            {record.from_cache ? (
                              <span className="badge badge-neutral">saltata</span>
                            ) : (
                              <span className="badge badge-success">
                                {formatNumber(record.changed_units.length)} modificate
                              </span>
                            )}
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              )}
            </div>
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

                <FormField label="Ambito" htmlFor="export-scope">
                  <select
                    id="export-scope"
                    className="select"
                    value={chapterScope}
                    onChange={(event) => {
                      setChapterScope(event.target.value);
                    }}
                  >
                    <option value="all">Tutto il libro</option>
                    {chapters.map((chapter) => (
                      <option key={chapter.id} value={chapter.id}>
                        Solo: {chapter.title}
                      </option>
                    ))}
                  </select>
                </FormField>

                <FormField
                  label="Template Pandoc"
                  htmlFor="export-template"
                  hint={
                    formatSpec === undefined || formatSpec.templateHint.length === 0
                      ? "Non applicabile a questo formato: lascia vuoto."
                      : `Vuoto = default dal pacchetto (${formatSpec.templateHint}). Un percorso personalizzato deve essere assoluto.`
                  }
                >
                  <input
                    id="export-template"
                    className="input"
                    value={template}
                    spellCheck={false}
                    onChange={(event) => {
                      setTemplate(event.target.value);
                    }}
                    placeholder="automatico"
                  />
                </FormField>

                <FormField
                  label="Foglio di stile CSS"
                  htmlFor="export-css"
                  hint={
                    formatSpec === undefined || formatSpec.cssHint.length === 0
                      ? "Ignorato dai formati non HTML."
                      : `Vuoto = default dal pacchetto (${formatSpec.cssHint}).`
                  }
                >
                  <input
                    id="export-css"
                    className="input"
                    value={css}
                    spellCheck={false}
                    disabled={formatSpec === undefined || formatSpec.cssHint.length === 0}
                    onChange={(event) => {
                      setCss(event.target.value);
                    }}
                    placeholder="automatico"
                  />
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

                <label className="flex items-center gap-2 text-xs text-muted">
                  <input
                    type="checkbox"
                    checked={toc}
                    onChange={(event) => {
                      setToc(event.target.checked);
                    }}
                  />
                  Indice (table of contents)
                </label>

                <label className="flex items-center gap-2 text-xs text-muted">
                  <input
                    type="checkbox"
                    checked={force}
                    onChange={(event) => {
                      setForce(event.target.checked);
                    }}
                  />
                  Rigenera anche se nulla è cambiato
                </label>

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
              <div className="panel-title mb-2">Cosa fa la build</div>
              <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
                <li>
                  Il documento viene diviso in unità per capitolo e <span className="mono-chip">metadata.yaml</span>{" "}
                  è composto dai metadati del progetto e del documento.
                </li>
                <li>
                  Template, CSS e filtri Lua (<span className="mono-chip">footnotes</span>,{" "}
                  <span className="mono-chip">tables</span> e per EPUB{" "}
                  <span className="mono-chip">epub_cleanup</span>) arrivano dal pacchetto{" "}
                  <span className="mono-chip">pandoc/</span>; un percorso scelto a mano li sostituisce.
                </li>
                <li>
                  Una build senza modifiche viene saltata e registrata come tale: la cronologia dice
                  cosa è stato ricostruito o riutilizzato.
                </li>
              </ul>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
