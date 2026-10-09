import { useCallback, useEffect, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { onExportProgress } from "../lib/events";
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
import { BuildProgressPanel } from "./export/BuildProgressPanel";
import { BuildSettingsPanel } from "./export/BuildSettingsPanel";
import { ExportChecklist } from "./export/ExportChecklist";
import { ExportHistoryPanel } from "./export/ExportHistoryPanel";
import { ExportPreviewPanel } from "./export/ExportPreviewPanel";
import { ExportResultPanel } from "./export/ExportResultPanel";
import { ExportUnitsPanel } from "./export/ExportUnitsPanel";
import { FORMATS, looksAbsolute } from "./export/shared";

/**
 * Export page (`PLAN.md` §11.5): format, template and CSS selection, build, preview and build
 * history.
 *
 * `export_build` takes only a project and the rendering options: the backend splits the document
 * into per-chapter units itself, resolves the default template/CSS/Lua filters from the `pandoc/`
 * assets, and skips the build when nothing changed. `export_preview` composes the same units for
 * an in-app look at the content, `export_history` returns the recent builds. The build is followed
 * through `export://progress` and the artifact is revealed with `open_path`.
 *
 * The panels in `./export/` own the presentation; this component keeps the choices, the loaded
 * chapters/history and the build lifecycle.
 */

export interface ExportViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
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
  /** Set when a build would emit untranslated chunks in the source language. */
  const [untranslatedWarning, setUntranslatedWarning] = useState<{
    missing: number;
    total: number;
  } | null>(null);
  const [result, setResult] = useState<ExportOutcome | null>(null);
  const [progress, setProgress] = useState<ExportProgressEvent | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);

  const [preview, setPreview] = useState<ExportPreview | null>(null);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);

  const projectId = project?.id ?? null;
  const formatSpec = FORMATS.find((entry) => entry.value === format) ?? FORMATS[0];

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

  useEffect(() => onExportProgress(setProgress), []);

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
    } catch (previewFailure) {
      setPreviewError(toErrorMessage(previewFailure));
      setPreview(null);
    } finally {
      setPreviewLoading(false);
    }
  }

  async function handleBuild(allowUntranslated = false) {
    if (projectId === null || formatSpec === undefined) {
      return;
    }
    setUntranslatedWarning(null);
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

    const chapterId = chapterScope === "all" ? null : chapterScope;
    if (!allowUntranslated) {
      // The backend refuses a half-translated build anyway; asking first turns that into a
      // choice instead of an error. A failing preview falls through to the build's own check.
      const composed = await exportPreview({ project_id: projectId, chapter_id: chapterId }).catch(
        () => null,
      );
      if (composed !== null && composed.untranslated_chunks > 0) {
        setUntranslatedWarning({
          missing: composed.untranslated_chunks,
          total: composed.total_chunks,
        });
        return;
      }
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
        chapter_id: chapterId,
        force,
        allow_untranslated: allowUntranslated,
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
        <h1 className="font-serif text-2xl font-medium text-ink">Esporta</h1>
        <EmptyState
          title="Nessun libro aperto"
          description="L'export impagina la traduzione di un progetto: aprine uno per scegliere formato e capitoli."
          actionLabel="Vai alla libreria"
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
          <h1 className="font-serif text-2xl font-medium text-ink">Esporta</h1>
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
            <ExportPreviewPanel
              preview={preview}
              error={previewError}
              onClose={() => {
                setPreview(null);
              }}
            />

            <ExportUnitsPanel chapters={chapters} />

            <BuildProgressPanel
              warning={untranslatedWarning}
              buildError={buildError}
              building={building}
              progress={progress}
              hasResult={result !== null}
              onExportAnyway={() => {
                void handleBuild(true);
              }}
              onDismissWarning={() => {
                setUntranslatedWarning(null);
              }}
            />

            {result !== null ? (
              <ExportResultPanel
                result={result}
                openError={openError}
                onOpen={(path) => {
                  void handleOpen(path);
                }}
              />
            ) : null}

            <ExportHistoryPanel history={history} />
          </div>

          <div className="section-stack">
            <ExportChecklist projectId={project.id} onNavigate={onNavigate} />

            <BuildSettingsPanel
              format={format}
              onFormat={setFormat}
              scope={chapterScope}
              onScope={setChapterScope}
              chapters={chapters}
              template={template}
              onTemplate={setTemplate}
              css={css}
              onCss={setCss}
              outputPath={outputPath}
              onOutputPath={setOutputPath}
              toc={toc}
              onToc={setToc}
              force={force}
              onForce={setForce}
              busy={building}
              onBuild={() => {
                void handleBuild();
              }}
            />
          </div>
        </div>
      )}
    </div>
  );
}
