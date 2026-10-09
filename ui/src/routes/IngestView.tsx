import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge } from "../components/StatusBadge";
import { basename, fileExtension, formatNumber } from "../lib/format";
import { pickDocumentFile } from "../lib/dialog";
import { onJobProgress } from "../lib/events";
import { ingestStart, isTauriRuntime, jobList, projectGet, toErrorMessage } from "../lib/ipc";
import type { Chapter, Job, PdfBackend, Project, ProjectDetail, SourceFormat } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Ingestion page (`PLAN.md` §11.1): drop or pick a file, see what was recognised, run the
 * ingestion, inspect the chapters that were produced.
 *
 * `ingest_start` only enqueues the `ingest` job and returns its id: format detection, extraction
 * and chunk building run on the worker pool. This page therefore follows the job through
 * `job_list` / `job://progress` and reads the result back from `project_get` (chapters and chunk
 * counters), instead of expecting an ingestion payload inline.
 */

export interface IngestViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const PDF_BACKENDS: ReadonlyArray<{ value: PdfBackend; label: string; hint: string }> = [
  { value: "auto", label: "Automatico", hint: "Il sidecar sceglie il backend migliore disponibile." },
  { value: "pymupdf4llm", label: "pymupdf4llm", hint: "Default: nessuna dipendenza pesante." },
  {
    value: "marker",
    label: "marker",
    hint: "Qualità di impaginazione superiore, richiede l'installazione opzionale.",
  },
];

function detectFormatFromExtension(path: string): SourceFormat | null {
  const extension = fileExtension(path);
  if (extension === "epub") {
    return "epub";
  }
  if (extension === "pdf") {
    return "pdf";
  }
  if (extension === "md" || extension === "markdown" || extension === "mkd") {
    return "markdown";
  }
  return null;
}

export function IngestView({ project, onNavigate }: IngestViewProps) {
  const [path, setPath] = useState("");
  const [dragging, setDragging] = useState(false);
  const [pdfBackend, setPdfBackend] = useState<PdfBackend>("auto");
  const [starting, setStarting] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [job, setJob] = useState<Job | null>(null);
  const [detail, setDetail] = useState<ProjectDetail | null>(null);

  const detectedFormat = useMemo(() => detectFormatFromExtension(path), [path]);
  const isPdf = detectedFormat === "pdf";

  const refreshOutcome = useCallback(async (projectId: string, watchedJobId: string | null) => {
    setRefreshing(true);
    try {
      const loaded = await projectGet(projectId);
      setDetail(loaded);
      if (watchedJobId !== null) {
        const jobs = await jobList({ project_id: projectId, limit: 200 });
        setJob(jobs.find((entry) => entry.id === watchedJobId) ?? null);
      }
    } catch (refreshError) {
      setError(toErrorMessage(refreshError));
    } finally {
      setRefreshing(false);
    }
  }, []);

  // Native file drop. Tauri intercepts the OS drag-and-drop, so this is the only way to learn
  // the absolute path of a dropped file; the browser dataTransfer fallback below cannot.
  useEffect(() => {
    if (!isTauriRuntime()) {
      return;
    }

    let unlisten: (() => void) | null = null;
    let disposed = false;

    void getCurrentWebview()
      .onDragDropEvent((event) => {
        const payload = event.payload;
        if (payload.type === "over" || payload.type === "enter") {
          setDragging(true);
          return;
        }
        if (payload.type === "leave") {
          setDragging(false);
          return;
        }
        setDragging(false);
        const dropped = payload.paths[0];
        if (dropped !== undefined) {
          setPath(dropped);
          setError(null);
        }
      })
      .then((detach) => {
        if (disposed) {
          detach();
          return;
        }
        unlisten = detach;
      })
      .catch((listenError: unknown) => {
        console.error("[ingest] drag-and-drop listener unavailable", listenError);
      });

    return () => {
      disposed = true;
      if (unlisten !== null) {
        unlisten();
        unlisten = null;
      }
    };
  }, []);

  // Reload the stored result whenever the project changes, and drop the previous run's state.
  // The project already knows its document: the field starts from it, so re-importing is one
  // click and only a different file needs typing.
  useEffect(() => {
    setJobId(null);
    setJob(null);
    setDetail(null);
    setError(null);
    setPath(project?.source_path ?? "");
    if (project !== null) {
      void refreshOutcome(project.id, null);
    }
  }, [project, refreshOutcome]);

  // The ingest job is a trigger to refetch: its ack carries no chapters or counters, so the page
  // reads them back through `project_get`.
  useEffect(
    () =>
      onJobProgress(() => {
        if (project !== null) {
          void refreshOutcome(project.id, jobId);
        }
      }),
    [project, jobId, refreshOutcome],
  );

  async function handleStart() {
    if (project === null) {
      return;
    }
    if (path.trim().length === 0) {
      setError("Indica il percorso del documento da importare.");
      return;
    }

    setStarting(true);
    setError(null);
    setJob(null);
    try {
      const started = await ingestStart({
        project_id: project.id,
        source_path: path.trim(),
        pdf_backend: isPdf ? pdfBackend : null,
      });
      setJobId(started.job_id);
      await refreshOutcome(project.id, started.job_id);
    } catch (startError) {
      setError(toErrorMessage(startError));
      setJobId(null);
    } finally {
      setStarting(false);
    }
  }

  if (project === null) {
    return (
      <div className="section-stack">
        <h2 className="text-lg font-semibold text-ink">Ingestione</h2>
        <EmptyState
          title="Nessun progetto aperto"
          description="L'ingestione lavora su un progetto: il documento estratto, i suoi blocchi e i suoi chunk vengono salvati nel progetto scelto."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  const chapters: Chapter[] = detail?.chapters ?? [];
  const jobState = job?.state ?? "pending";

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Ingestione</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> — il
            documento viene convertito in Markdown canonico, segmentato in blocchi con ID stabili e
            raggruppato in chunk.
          </p>
        </div>
        <StatusBadge status="ok" tone="info" label={`Lingue ${project.source_lang ?? "?"} → ${project.target_lang}`} />
      </div>

      <div className="panel">
        <div className="panel-head">
          <span className="panel-title">Documento sorgente</span>
          {detectedFormat !== null ? (
            <StatusBadge
              status="ok"
              tone="accent"
              label={`Formato stimato: ${detectedFormat.toUpperCase()}`}
            />
          ) : (
            <StatusBadge status="untested" label="Formato non riconosciuto dall'estensione" />
          )}
        </div>

        <div className="panel-pad section-stack">
          <div
            className="dropzone"
            data-active={dragging ? "true" : "false"}
            onDragOver={(event) => {
              event.preventDefault();
              setDragging(true);
            }}
            onDragLeave={() => {
              setDragging(false);
            }}
            onDrop={(event) => {
              event.preventDefault();
              setDragging(false);
              if (isTauriRuntime()) {
                // The native listener above already delivered the absolute path.
                return;
              }
              const dropped = event.dataTransfer.files[0];
              setError(
                dropped === undefined
                  ? "Trascinamento non riconosciuto: indica il percorso a mano."
                  : "Nel browser non è possibile leggere il percorso assoluto di un file trascinato: avvia l'app desktop oppure incolla il percorso nel campo qui sotto.",
              );
            }}
          >
            <span aria-hidden="true" className="text-2xl">
              ⤓
            </span>
            <p className="text-sm font-medium text-ink">Trascina qui il documento</p>
            <p className="text-xs text-muted">EPUB, PDF o Markdown</p>
            <button
              type="button"
              className="btn btn-sm"
              disabled={starting}
              onClick={() => {
                void pickDocumentFile().then((picked) => {
                  if (picked !== null) {
                    setPath(picked);
                    setError(null);
                  }
                });
              }}
            >
              Sfoglia…
            </button>
            {!isTauriRuntime() ? (
              <p className="text-[0.72rem] text-warn">
                Fuori dall&apos;app desktop il trascinamento non può fornire un percorso: usa il
                campo manuale.
              </p>
            ) : null}
          </div>

          <FormField
            label="Percorso del documento"
            htmlFor="ingest-path"
            hint="Percorso assoluto; il file non viene modificato né spostato."
          >
            <input
              id="ingest-path"
              className="input"
              value={path}
              spellCheck={false}
              placeholder="/home/utente/libri/il-nome-della-rosa.epub"
              onChange={(event) => {
                setPath(event.target.value);
              }}
            />
          </FormField>

          {isPdf ? (
            <FormField
              label="Backend di estrazione PDF"
              htmlFor="ingest-pdf-backend"
              hint={
                PDF_BACKENDS.find((backend) => backend.value === pdfBackend)?.hint ??
                "Backend di estrazione PDF."
              }
            >
              <select
                id="ingest-pdf-backend"
                className="select"
                value={pdfBackend}
                onChange={(event) => {
                  const matched = PDF_BACKENDS.find((backend) => backend.value === event.target.value);
                  if (matched !== undefined) {
                    setPdfBackend(matched.value);
                  }
                }}
              >
                {PDF_BACKENDS.map((backend) => (
                  <option key={backend.value} value={backend.value}>
                    {backend.label}
                  </option>
                ))}
              </select>
            </FormField>
          ) : null}

          {error !== null ? (
            <div className="banner banner-error" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>{error}</span>
            </div>
          ) : null}

          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              className="btn btn-primary"
              disabled={starting || path.trim().length === 0}
              onClick={() => {
                void handleStart();
              }}
            >
              {starting ? <span className="spinner" aria-hidden="true" /> : null}
              {starting ? "Ingestione in corso…" : "Avvia ingestione"}
            </button>
            <button
              type="button"
              className="btn btn-ghost"
              disabled={starting}
              onClick={() => {
                setPath("");
                setJobId(null);
                setJob(null);
                setError(null);
              }}
            >
              Reimposta
            </button>
          </div>
        </div>
      </div>

      {starting ? (
        <EmptyState
          tone="loading"
          compact
          title="Estrazione accodata"
          description="Conversione in Markdown, segmentazione in blocchi e costruzione dei chunk sulla coda di lavoro."
        />
      ) : null}

      {jobId !== null ? (
        <>
          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Esito dell&apos;estrazione</span>
              <span className="flex items-center gap-2">
                <StatusBadge status={job?.state ?? "pending"} pulse={job?.state === "running" || job?.state === "leased"} />
                <span className="mono-chip">{detectedFormat?.toUpperCase() ?? "—"}</span>
              </span>
            </div>

            <div className="panel-pad section-stack">
              <div className="grid grid-cols-4 gap-2">
                <div className="stat-tile">
                  <div className="stat-label">Capitoli</div>
                  <div className="stat-value">{formatNumber(chapters.length)}</div>
                </div>
                <div className="stat-tile">
                  <div className="stat-label">Chunk</div>
                  <div className="stat-value">{formatNumber(detail?.chunks_total ?? 0)}</div>
                </div>
                <div className="stat-tile">
                  <div className="stat-label">Chunk completati</div>
                  <div className="stat-value">{formatNumber(detail?.chunks_done ?? 0)}</div>
                </div>
                <div className="stat-tile">
                  <div className="stat-label">Job di ingestione</div>
                  <div className="stat-value" style={{ fontSize: "0.72rem" }}>
                    <span className="mono-chip" title={jobId}>
                      {jobId.slice(0, 10)}…
                    </span>
                  </div>
                </div>
              </div>

              <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-[0.75rem]">
                <dt className="text-faint">Stato del job</dt>
                <dd className="font-mono text-ink-soft">{job?.state ?? "sconosciuto"}</dd>
                <dt className="text-faint">Tentativi</dt>
                <dd className="font-mono text-ink-soft">
                  {job === null ? "—" : `${job.attempts} / ${job.max_attempts}`}
                </dd>
              </dl>

              {job !== null && job.last_error !== null ? (
                <div className="banner banner-error" role="alert">
                  <span aria-hidden="true">⚠</span>
                  <span>{job.last_error}</span>
                </div>
              ) : jobState === "failed" ? (
                <div className="banner banner-error" role="alert">
                  <span aria-hidden="true">⚠</span>
                  <span>
                    L&apos;estrazione è fallita: controlla i log e lo stato del sidecar, poi riprova.
                  </span>
                </div>
              ) : jobState === "done" ? (
                <div className="banner banner-ok" role="status">
                  <span aria-hidden="true">✓</span>
                  <span>Estrazione completata: capitoli e chunk sono pronti.</span>
                </div>
              ) : jobState === "cancelled" ? (
                <div className="banner" role="status">
                  <span aria-hidden="true">⏹</span>
                  <span>Estrazione annullata prima del completamento.</span>
                </div>
              ) : (
                <div className="banner" role="status">
                  <span aria-hidden="true">⏳</span>
                  <span>
                    Estrazione in corso: i capitoli e i chunk compaiono qui man mano che il job
                    procede.
                  </span>
                </div>
              )}

              <div className="flex flex-wrap items-center gap-2">
                <button
                  type="button"
                  className="btn btn-primary"
                  onClick={() => {
                    onNavigate("translate");
                  }}
                >
                  Vai alla traduzione
                </button>
                <button
                  type="button"
                  className="btn"
                  disabled={refreshing}
                  onClick={() => {
                    void refreshOutcome(project.id, jobId);
                  }}
                >
                  {refreshing ? <span className="spinner" aria-hidden="true" /> : null}
                  Aggiorna esito
                </button>
              </div>
            </div>
          </div>

          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Anteprima capitoli</span>
              <span className="mono-chip">{formatNumber(chapters.length)} capitoli</span>
            </div>

            {chapters.length === 0 ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title="Nessun capitolo ancora"
                  description="Il documento non è ancora stato segmentato, oppure non contiene heading utilizzabili come capitoli: i chunk vengono comunque costruiti sull'intero flusso di blocchi."
                />
              </div>
            ) : (
              <div className="table-scroll" style={{ maxHeight: "20rem" }}>
                <table className="data-table">
                  <thead>
                    <tr>
                      <th style={{ width: "4rem" }} className="num">
                        Ordine
                      </th>
                      <th style={{ width: "4.5rem" }}>Livello</th>
                      <th>Titolo</th>
                      <th style={{ width: "9rem" }}>Blocchi</th>
                    </tr>
                  </thead>
                  <tbody>
                    {chapters.map((chapter) => (
                      <tr key={chapter.id}>
                        <td className="num">{formatNumber(chapter.order_index)}</td>
                        <td>
                          <span className="mono-chip">H{formatNumber(chapter.level)}</span>
                        </td>
                        <td>
                          <span
                            className="block truncate"
                            style={{ paddingLeft: `${String(Math.max(0, chapter.level - 1) * 12)}px` }}
                            title={chapter.title}
                          >
                            {chapter.title}
                          </span>
                        </td>
                        <td className="font-mono text-[0.72rem] text-muted">
                          {`${formatNumber(chapter.block_first)} – ${formatNumber(chapter.block_last)}`}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </>
      ) : null}

      {!starting && jobId === null ? (
        <p className="text-[0.72rem] text-faint">
          Riferimento: gli stessi dati sono mostrati da{" "}
          <span className="mono-chip">{basename(project.source_path)}</span> — l&apos;ingestione non
          riscrive mai il file originale.
        </p>
      ) : null}
    </div>
  );
}
