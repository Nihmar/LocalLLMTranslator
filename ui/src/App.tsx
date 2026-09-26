import { useCallback, useEffect, useMemo, useState } from "react";
import { StatusBadge } from "./components/StatusBadge";
import { onSidecarStatus } from "./lib/events";
import { projectGet, sidecarStatus, toErrorMessage } from "./lib/ipc";
import type { Project, SidecarStatus } from "./lib/types";
import { ExportView } from "./routes/ExportView";
import { IngestView } from "./routes/IngestView";
import { JobsView } from "./routes/JobsView";
import { ModelsView } from "./routes/ModelsView";
import { ProjectsView } from "./routes/ProjectsView";
import { ReviewView } from "./routes/ReviewView";
import { TranslateView } from "./routes/TranslateView";

/**
 * Application shell: sidebar navigation plus a typed view switcher.
 *
 * There is deliberately no routing library: the wizard has seven fixed destinations, a
 * `useState<ViewId>` is smaller, fully typed and needs no dependency. `PLAN.md` §11 calls each
 * step "a route you can visit freely" — which is exactly what this switcher provides.
 *
 * `ViewId` is exported so views can type their `onNavigate` prop; views import it with
 * `import type`, which `verbatimModuleSyntax` erases, so there is no runtime import cycle.
 */

export type ViewId =
  | "projects"
  | "ingest"
  | "models"
  | "translate"
  | "review"
  | "export"
  | "jobs";

interface NavEntry {
  id: ViewId;
  /** Wizard step number, or a bullet for the dashboard. */
  step: string;
  label: string;
  hint: string;
}

const NAV_ENTRIES: readonly NavEntry[] = [
  { id: "projects", step: "0", label: "Progetti", hint: "Elenco, creazione, apertura" },
  { id: "ingest", step: "1", label: "Ingestione", hint: "File, formato, capitoli" },
  { id: "models", step: "2", label: "Modelli", hint: "Endpoint, salute, ruoli" },
  { id: "translate", step: "3", label: "Traduzione", hint: "Chunk, avvio, risorse" },
  { id: "review", step: "4", label: "Revisione", hint: "Diff bilingue (M4)" },
  { id: "export", step: "5", label: "Export", hint: "PDF, EPUB, DOCX" },
  { id: "jobs", step: "•", label: "Job", hint: "Coda, ETA, log live" },
];

const STORAGE_KEY = "llmtranslator.current_project_id";

function readStoredProjectId(): string | null {
  try {
    return window.localStorage.getItem(STORAGE_KEY);
  } catch {
    return null;
  }
}

function writeStoredProjectId(projectId: string | null): void {
  try {
    if (projectId === null) {
      window.localStorage.removeItem(STORAGE_KEY);
    } else {
      window.localStorage.setItem(STORAGE_KEY, projectId);
    }
  } catch {
    // Storage unavailable: the selection simply does not survive a reload.
  }
}

export default function App() {
  const [view, setView] = useState<ViewId>("projects");
  const [project, setProject] = useState<Project | null>(null);
  const [restoring, setRestoring] = useState(true);
  const [sidecar, setSidecar] = useState<SidecarStatus | null>(null);
  const [sidecarError, setSidecarError] = useState<string | null>(null);

  const refreshSidecar = useCallback(async () => {
    try {
      const status = await sidecarStatus();
      setSidecar(status);
      setSidecarError(null);
    } catch (error) {
      setSidecarError(toErrorMessage(error));
    }
  }, []);

  // Restore the last opened project so a reload does not drop the user's context.
  useEffect(() => {
    let cancelled = false;
    const storedId = readStoredProjectId();

    if (storedId === null) {
      setRestoring(false);
      return;
    }

    void projectGet(storedId)
      .then((restored) => {
        if (cancelled) {
          return;
        }
        setProject(restored.project);
      })
      .catch(() => {
        // The project was deleted or the backend is unreachable: forget the stale id.
        writeStoredProjectId(null);
      })
      .finally(() => {
        if (!cancelled) {
          setRestoring(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    void refreshSidecar();
  }, [refreshSidecar]);

  useEffect(() => onSidecarStatus((status) => {
    setSidecar(status);
    setSidecarError(null);
  }), []);

  const handleOpenProject = useCallback((opened: Project) => {
    setProject(opened);
    writeStoredProjectId(opened.id);
  }, []);

  const currentEntry = useMemo(
    () => NAV_ENTRIES.find((entry) => entry.id === view) ?? NAV_ENTRIES[0],
    [view],
  );

  const sidecarReady = sidecar !== null && sidecar.state === "running";

  return (
    <div className="app-canvas">
      {/* Sidebar */}
      <aside className="flex w-64 shrink-0 flex-col gap-3 border-r border-line bg-surface/60 p-3">
        <div className="flex items-center gap-2 px-1 py-1">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-accent font-mono text-sm font-bold text-canvas">
            iL
          </span>
          <span className="min-w-0">
            <span className="block truncate text-sm font-semibold text-ink">
              LocalLLMTranslator
            </span>
            <span className="block truncate text-[0.68rem] text-faint">
              traduzione locale, offline
            </span>
          </span>
        </div>

        <nav className="flex flex-col gap-1" aria-label="Sezioni dell'applicazione">
          {NAV_ENTRIES.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className="nav-item"
              aria-current={entry.id === view ? "page" : undefined}
              onClick={() => {
                setView(entry.id);
              }}
            >
              <span className="nav-index" aria-hidden="true">
                {entry.step}
              </span>
              <span className="min-w-0">
                <span className="block truncate text-[0.82rem] font-medium">{entry.label}</span>
                <span className="block truncate text-[0.68rem] text-faint">{entry.hint}</span>
              </span>
            </button>
          ))}
        </nav>

        <div className="mt-auto flex flex-col gap-2 rounded-lg border border-line bg-canvas p-2.5">
          <div className="flex items-center justify-between gap-2">
            <span className="stat-label">Sidecar</span>
            <StatusBadge
              status={sidecarError === null ? (sidecar?.state ?? "starting") : "error"}
              pulse={sidecar !== null && (sidecar.state === "starting" || sidecar.state === "restarting")}
              label={
                sidecarError !== null
                  ? "Stato non leggibile"
                  : sidecar === null
                    ? "Interrogazione…"
                    : sidecar.state === "running"
                      ? "In esecuzione"
                      : undefined
              }
            />
          </div>

          <dl className="grid grid-cols-2 gap-x-2 gap-y-0.5 font-mono text-[0.66rem] text-muted">
            <dt className="text-faint">stato</dt>
            <dd className="truncate">{sidecar?.state ?? "—"}</dd>
            <dt className="text-faint">pid</dt>
            <dd className="truncate">{sidecar?.pid ?? "—"}</dd>
            <dt className="text-faint">tentativi</dt>
            <dd className="truncate">{sidecar?.attempts ?? "—"}</dd>
          </dl>

          {sidecar?.message !== null && sidecar?.message !== undefined ? (
            <p className="text-[0.66rem] text-danger" title={sidecar.message}>
              {sidecar.message}
            </p>
          ) : null}

          <button type="button" className="btn btn-sm btn-ghost" onClick={() => void refreshSidecar()}>
            Aggiorna stato
          </button>
        </div>

        <p className="px-1 text-[0.62rem] leading-relaxed text-faint">
          Nessuna telemetria, nessun font remoto, nessuna chiamata di rete oltre agli endpoint
          llama-server che configuri tu.
        </p>
      </aside>

      {/* Main column */}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-line bg-surface/60 px-5 py-3">
          <div className="min-w-0">
            <h1 className="truncate text-sm font-semibold text-ink">
              {currentEntry?.label ?? ""}
              <span className="ml-2 font-normal text-muted">
                {currentEntry?.hint ?? ""}
              </span>
            </h1>
            <p className="truncate text-[0.7rem] text-faint">
              {project === null
                ? restoring
                  ? "Ripristino del progetto…"
                  : "Nessun progetto aperto"
                : `Progetto: ${project.name} · ${project.source_lang ?? "?"} → ${project.target_lang}`}
            </p>
          </div>

          <div className="flex items-center gap-2">
            {project !== null ? (
              <span className="mono-chip" title={project.source_path}>
                {project.source_path.length > 42
                  ? `…${project.source_path.slice(-41)}`
                  : project.source_path}
              </span>
            ) : null}
            <button
              type="button"
              className="btn btn-sm"
              disabled={sidecar === null && sidecarError === null}
              onClick={() => {
                void refreshSidecar();
              }}
            >
              {sidecarReady ? "Sidecar pronto" : "Verifica sidecar"}
            </button>
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              onClick={() => {
                setView("projects");
              }}
            >
              {project === null ? "Scegli progetto" : "Cambia progetto"}
            </button>
          </div>
        </header>

        {sidecarError !== null || (sidecar !== null && !sidecarReady) ? (
          <div className="shrink-0 px-5 pt-3">
            <div className="banner banner-warn" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>
                <strong className="font-semibold">Sidecar non pronto.</strong>{" "}
                {sidecarError ?? sidecar?.message ?? "Il supervisore lo sta avviando o lo sta riavviando."}{" "}
                L&apos;interfaccia resta navigabile, ma ingestione, chunking ed export richiedono il
                sidecar attivo.
              </span>
            </div>
          </div>
        ) : null}

        <main className="min-h-0 flex-1 overflow-y-auto p-5">
          {view === "projects" ? (
            <ProjectsView
              currentProjectId={project?.id ?? null}
              onOpenProject={handleOpenProject}
              onNavigate={setView}
            />
          ) : view === "ingest" ? (
            <IngestView project={project} onNavigate={setView} />
          ) : view === "models" ? (
            <ModelsView project={project} />
          ) : view === "translate" ? (
            <TranslateView project={project} onNavigate={setView} />
          ) : view === "review" ? (
            <ReviewView project={project} onNavigate={setView} />
          ) : view === "export" ? (
            <ExportView project={project} onNavigate={setView} />
          ) : (
            <JobsView project={project} onNavigate={setView} />
          )}
        </main>
      </div>
    </div>
  );
}
