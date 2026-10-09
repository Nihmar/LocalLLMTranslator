import { useCallback, useEffect, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { pickBundleFile } from "../lib/dialog";
import { formatBytes } from "../lib/format";
import { downloadUrl } from "../lib/http";
import {
  ingestStart,
  openPath,
  projectDelete,
  projectExport,
  projectGet,
  projectImport,
  projectList,
  isTauriRuntime,
  toErrorMessage,
} from "../lib/ipc";
import type { ExportBundleOutcome, Project } from "../lib/types";
import type { ViewId } from "../App";
import { FirstRunPanel } from "./projects/FirstRunPanel";
import { NewBookDialog } from "./projects/NewBookDialog";
import { ProjectCard } from "./projects/ProjectCard";

/**
 * Project list / create / open / delete (`PLAN.md` §11, "Progetti").
 *
 * The `.llmtz` export/import of `PLAN.md` §6 is offered here through `project_export` and
 * `project_import`; the note at the bottom of the list spells out what the bundle contains.
 *
 * The panels in `./projects/` own their dialogs, forms and per-card busy state; this component
 * keeps the list and the page-level banners.
 */

export interface ProjectsViewProps {
  currentProjectId: string | null;
  onOpenProject: (project: Project) => void;
  /** Reports a deleted project so the shell can drop it as the open one. */
  onDeleteProject: (projectId: string) => void;
  onNavigate: (view: ViewId) => void;
}

export function ProjectsView({
  currentProjectId,
  onOpenProject,
  onDeleteProject,
  onNavigate,
}: ProjectsViewProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [formOpen, setFormOpen] = useState(false);

  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [bundleResult, setBundleResult] = useState<ExportBundleOutcome | null>(null);
  const [bundleBusy, setBundleBusy] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const rows = await projectList();
      setProjects(rows);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setProjects([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /** Creating a book and importing it are one step: the file was just chosen. */
  async function handleCreated(created: Project) {
    setProjects((current) => [created, ...current]);
    setFormOpen(false);
    onOpenProject(created);
    await ingestStart({ project_id: created.id, pdf_backend: null }).catch(() => null);
    onNavigate("ingest");
  }

  async function handleOpen(projectId: string) {
    setActionError(null);
    try {
      const fresh = await projectGet(projectId);
      onOpenProject(fresh.project);
      // A book that was never imported starts at the import; otherwise at its overview.
      onNavigate(fresh.chapters.length === 0 ? "ingest" : "overview");
    } catch (openError) {
      setActionError(toErrorMessage(openError));
    }
  }

  async function handleDelete(projectId: string): Promise<boolean> {
    setActionError(null);
    try {
      await projectDelete(projectId);
      setProjects((current) => current.filter((project) => project.id !== projectId));
      onDeleteProject(projectId);
      return true;
    } catch (deleteError) {
      setActionError(toErrorMessage(deleteError));
      return false;
    }
  }

  async function handleExportBundle(projectId: string) {
    setActionError(null);
    setActionNotice(null);
    setBundleResult(null);
    try {
      const outcome = await projectExport({ project_id: projectId, output_path: null });
      setBundleResult(outcome);
      setActionNotice(`Bundle creato: ${outcome.output_path}`);
    } catch (exportError) {
      setActionError(toErrorMessage(exportError));
    }
  }

  async function handleImportBundle() {
    setImportError(null);
    setActionNotice(null);
    const archive = await pickBundleFile();
    if (archive === null) {
      return;
    }
    setBundleBusy(true);
    try {
      const imported = await projectImport({ archive_path: archive });
      setActionNotice(`Progetto «${imported.name}» importato.`);
      await load();
    } catch (importFailure) {
      setImportError(toErrorMessage(importFailure));
    } finally {
      setBundleBusy(false);
    }
  }

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="font-serif text-2xl font-medium text-ink">I tuoi libri</h1>
          <p className="mt-0.5 text-sm text-muted">
            Ogni libro ha il suo documento, la lingua di arrivo, i suoi prompt e i suoi progressi.
          </p>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            className="btn"
            onClick={() => {
              void load();
            }}
            disabled={loading}
          >
            Aggiorna
          </button>
          <button
            type="button"
            className="btn"
            disabled={bundleBusy}
            onClick={() => {
              void handleImportBundle();
            }}
          >
            {bundleBusy ? <span className="spinner" aria-hidden="true" /> : null}
            Importa .llmtz
          </button>
          <button
            type="button"
            className="btn btn-primary"
            onClick={() => {
              setFormOpen(true);
            }}
          >
            Nuovo libro
          </button>
        </div>
      </div>

      {actionError !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{actionError}</span>
        </div>
      ) : null}

      {actionNotice !== null ? (
        <div className="banner banner-ok" role="status">
          <span aria-hidden="true">✓</span>
          <span>
            {actionNotice}
            {bundleResult !== null ? (
              <span className="mt-2 flex items-center gap-2">
                {isTauriRuntime() ? (
                  <button
                    type="button"
                    className="btn btn-sm"
                    onClick={() => {
                      void openPath(bundleResult.output_path);
                    }}
                  >
                    Apri cartella
                  </button>
                ) : (
                  <a className="btn btn-sm" href={downloadUrl(bundleResult.output_path)} download>
                    Scarica bundle
                  </a>
                )}
                <span className="mono-chip">
                  {formatBytes(bundleResult.bytes)} · {bundleResult.files} file
                </span>
              </span>
            ) : null}
          </span>
        </div>
      ) : null}

      {importError !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>Importazione non riuscita: {importError}</span>
        </div>
      ) : null}

      {formOpen ? (
        <NewBookDialog
          onCreated={handleCreated}
          onClose={() => {
            setFormOpen(false);
          }}
        />
      ) : null}

      {loading ? (
        <EmptyState tone="loading" title="Caricamento dei progetti…" />
      ) : error !== null ? (
        <EmptyState
          tone="error"
          title="Impossibile leggere i progetti"
          description="Il comando project_list non ha risposto. Verifica che il sidecar sia attivo."
          details={error}
          actionLabel="Riprova"
          onAction={() => {
            void load();
          }}
        />
      ) : projects.length === 0 ? (
        <FirstRunPanel
          onNewBook={() => {
            setFormOpen(true);
          }}
          onImport={() => {
            void handleImportBundle();
          }}
          importing={bundleBusy}
          onNavigate={onNavigate}
        />
      ) : (
        <div className="grid grid-cols-2 gap-3 xl:grid-cols-3">
          {projects.map((project) => (
            <ProjectCard
              key={project.id}
              project={project}
              isCurrent={project.id === currentProjectId}
              onOpen={() => handleOpen(project.id)}
              onIngest={() => {
                onOpenProject(project);
                onNavigate("ingest");
              }}
              onExport={() => handleExportBundle(project.id)}
              onDelete={() => handleDelete(project.id)}
            />
          ))}
        </div>
      )}

      <p className="text-[0.72rem] text-faint">
        Il bundle <span className="mono-chip">.llmtz</span> contiene il database del progetto, il
        Markdown con gli asset, l&apos;output e lo snapshot dei prompt; l&apos;importazione rifiuta
        un progetto già presente invece di sovrascriverlo.
      </p>
    </div>
  );
}
