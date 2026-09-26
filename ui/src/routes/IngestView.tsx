import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge } from "../components/StatusBadge";
import { basename, fileExtension, formatNumber, joinParts } from "../lib/format";
import { chunkList, ingestStart, isTauriRuntime, toErrorMessage } from "../lib/ipc";
import type {
  Chapter,
  IngestResult,
  JsonObject,
  JsonValue,
  PdfBackend,
  Project,
  SourceFormat,
} from "../lib/types";
import type { ViewId } from "../App";

/**
 * Step 1 of the wizard: drop a file, see what was recognised, run the ingestion, inspect the
 * chapters and the warnings (`PLAN.md` §11.1).
 *
 * Format detection is shown from the file extension as soon as a path is known, and the
 * authoritative value is the one `ingest_start` reports back: the sidecar owns
 * `detect_format` (`PLAN.md` §12.1) and there is no separate command for it.
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

function jsonText(value: JsonValue): string {
  if (typeof value === "string") {
    return value;
  }
  if (value === null) {
    return "—";
  }
  if (typeof value === "boolean" || typeof value === "number") {
    return String(value);
  }
  const serialised = JSON.stringify(value);
  return serialised === undefined ? String(value) : serialised;
}

function metadataRows(metadata: JsonObject): ReadonlyArray<{ key: string; value: string }> {
  return Object.entries(metadata).map(([key, value]) => ({ key, value: jsonText(value) }));
}

export function IngestView({ project, onNavigate }: IngestViewProps) {
  const [path, setPath] = useState("");
  const [dragging, setDragging] = useState(false);
  const [pdfBackend, setPdfBackend] = useState<PdfBackend>("auto");
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<IngestResult | null>(null);
  const [liveChunkCount, setLiveChunkCount] = useState<number | null>(null);
  const [refreshingCount, setRefreshingCount] = useState(false);

  const detectedFormat = useMemo(() => detectFormatFromExtension(path), [path]);
  const isPdf = detectedFormat === "pdf";

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

  useEffect(() => {
    setResult(null);
    setLiveChunkCount(null);
    setError(null);
  }, [project?.id]);

  const refreshChunkCount = useCallback(
    async (projectId: string) => {
      setRefreshingCount(true);
      try {
        const chunks = await chunkList({ project_id: projectId, limit: 5000 });
        setLiveChunkCount(chunks.length);
      } catch (countError) {
        setError(toErrorMessage(countError));
      } finally {
        setRefreshingCount(false);
      }
    },
    [],
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
    try {
      const ingested = await ingestStart({
        project_id: project.id,
        path: path.trim(),
        pdf_backend: isPdf ? pdfBackend : undefined,
      });
      setResult(ingested);
      setLiveChunkCount(ingested.chunk_count);
    } catch (startError) {
      setError(toErrorMessage(startError));
      setResult(null);
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
                setResult(null);
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
          title="Estrazione in corso"
          description="Conversione in Markdown, segmentazione in blocchi e costruzione dei chunk."
        />
      ) : null}

      {result !== null ? (
        <>
          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Esito dell&apos;estrazione</span>
              <span className="flex items-center gap-2">
                <StatusBadge status="done" />
                <span className="mono-chip">{result.format.toUpperCase()}</span>
              </span>
            </div>

            <div className="panel-pad section-stack">
              <div className="grid grid-cols-4 gap-2">
                <div className="stat-tile">
                  <div className="stat-label">Blocchi</div>
                  <div className="stat-value">{formatNumber(result.block_count)}</div>
                </div>
                <div className="stat-tile">
                  <div className="stat-label">Chunk</div>
                  <div className="stat-value">{formatNumber(result.chunk_count)}</div>
                </div>
                <div className="stat-tile">
                  <div className="stat-label">Capitoli</div>
                  <div className="stat-value">{formatNumber(result.chapters.length)}</div>
                </div>
                <div className="stat-tile">
                  <div className="stat-label">Job accodati</div>
                  <div className="stat-value">{formatNumber(result.job_ids.length)}</div>
                </div>
              </div>

              <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-[0.75rem]">
                <dt className="text-faint">Estrattore</dt>
                <dd className="font-mono text-ink-soft">
                  {joinParts([result.extractor, result.extractor_version], " ")}
                </dd>
                <dt className="text-faint">Markdown canonico</dt>
                <dd className="truncate font-mono text-ink-soft" title={result.markdown_path}>
                  {result.markdown_path}
                </dd>
              </dl>

              {metadataRows(result.metadata).length > 0 ? (
                <div>
                  <div className="stat-label mb-1">Metadati del documento</div>
                  <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-[0.75rem]">
                    {metadataRows(result.metadata).map((row) => (
                      <div key={row.key} className="contents">
                        <dt className="truncate text-faint" title={row.key}>
                          {row.key}
                        </dt>
                        <dd className="truncate text-ink-soft" title={row.value}>
                          {row.value}
                        </dd>
                      </div>
                    ))}
                  </dl>
                </div>
              ) : null}

              {result.warnings.length > 0 ? (
                <div className="banner banner-warn" role="status">
                  <span aria-hidden="true">⚠</span>
                  <span>
                    <strong className="font-semibold">
                      {formatNumber(result.warnings.length)} avvisi dall&apos;estrazione.
                    </strong>
                    <ul className="mt-1 list-disc space-y-0.5 pl-4">
                      {result.warnings.map((warning, index) => (
                        <li key={`${String(index)}-${warning}`}>{warning}</li>
                      ))}
                    </ul>
                  </span>
                </div>
              ) : (
                <div className="banner banner-ok" role="status">
                  <span aria-hidden="true">✓</span>
                  <span>Nessun avviso: l&apos;estrazione non ha segnalato problemi.</span>
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
                  disabled={refreshingCount}
                  onClick={() => {
                    void refreshChunkCount(result.project_id);
                  }}
                >
                  {refreshingCount ? <span className="spinner" aria-hidden="true" /> : null}
                  Conta i chunk salvati
                </button>
                {liveChunkCount !== null ? (
                  <span className="mono-chip">{formatNumber(liveChunkCount)} chunk nel database</span>
                ) : null}
              </div>
            </div>
          </div>

          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Anteprima capitoli</span>
              <span className="mono-chip">{formatNumber(result.chapters.length)} capitoli</span>
            </div>

            {result.chapters.length === 0 ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title="Nessun capitolo riconosciuto"
                  description="Il documento non contiene heading utilizzabili come capitoli: i chunk verranno comunque costruiti sull'intero flusso di blocchi."
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
                    {result.chapters.map((chapter: Chapter) => (
                      <tr key={chapter.id}>
                        <td className="num">{formatNumber(chapter.order)}</td>
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
                          {chapter.block_first === null || chapter.block_last === null
                            ? "—"
                            : `${formatNumber(chapter.block_first)} – ${formatNumber(chapter.block_last)}`}
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

      {!starting && result === null ? (
        <p className="text-[0.72rem] text-faint">
          Riferimento: gli stessi dati sono mostrati da{" "}
          <span className="mono-chip">{basename(project.source_path)}</span> — l&apos;ingestione non
          riscrive mai il file originale.
        </p>
      ) : null}
    </div>
  );
}
