import { useCallback, useEffect, useState } from "react";
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
import { SeriesView } from "./routes/SeriesView";
import { TranslateView } from "./routes/TranslateView";

/**
 * Application shell: a grouped sidebar plus a typed view switcher.
 *
 * The navigation is split by **scope**, not by step number: the four project steps only make
 * sense while a project is open and only appear then, while "Progetti", "Modelli", "Serie" and
 * "Job" are application-wide destinations that are always reachable. The open project itself
 * lives in the header, so it stays visible on every page (`PLAN.md` §11).
 *
 * There is deliberately no routing library: the wizard has fixed destinations, a
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
  | "jobs"
  | "series";

interface NavEntry {
  id: ViewId;
  /** Wizard step number for the project flow, or a bullet for the application destinations. */
  step: string;
  label: string;
  hint: string;
}

/** Steps of the translation pipeline; scoped to the open project, in reading order. */
const PROJECT_NAV: readonly NavEntry[] = [
  { id: "ingest", step: "1", label: "Ingestione", hint: "File, formato, capitoli" },
  { id: "translate", step: "2", label: "Traduzione", hint: "Capitoli, anteprima, chunk" },
  { id: "review", step: "3", label: "Revisione", hint: "Diff bilingue, suggerimenti, QA" },
  { id: "export", step: "4", label: "Export", hint: "PDF, EPUB, DOCX" },
];

/** Application-wide destinations, independent of any project. */
const APP_NAV: readonly NavEntry[] = [
  { id: "projects", step: "•", label: "Progetti", hint: "Elenco, creazione, apertura" },
  { id: "models", step: "•", label: "Modelli", hint: "Endpoint, salute, ruoli" },
  { id: "series", step: "•", label: "Serie", hint: "Canone condiviso tra i libri" },
  { id: "jobs", step: "•", label: "Job", hint: "Coda, ETA, log live" },
];

const PROJECT_VIEWS: ReadonlySet<ViewId> = new Set(PROJECT_NAV.map((entry) => entry.id));

const STORAGE_KEY = "llmtranslator.current_project_id";

const PRIVACY_NOTE =
  "Nessuna telemetria, nessun font remoto, nessuna chiamata di rete oltre agli endpoint " +
  "llama-server che configuri tu.";

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

function NavGroup({
  title,
  entries,
  current,
  onSelect,
}: {
  title: string;
  entries: readonly NavEntry[];
  current: ViewId;
  onSelect: (view: ViewId) => void;
}) {
  return (
    <div className="flex flex-col gap-1">
      <div className="nav-group-label">{title}</div>
      {entries.map((entry) => (
        <button
          key={entry.id}
          type="button"
          className="nav-item"
          title={entry.hint}
          aria-current={entry.id === current ? "page" : undefined}
          onClick={() => {
            onSelect(entry.id);
          }}
        >
          <span className="nav-index" aria-hidden="true">
            {entry.step}
          </span>
          <span className="truncate text-[0.82rem] font-medium">{entry.label}</span>
        </button>
      ))}
    </div>
  );
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
        // The project was deleted or the backend is unreachable: the persistence effect below
        // forgets the stale id once `restoring` settles.
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

  // A project step with no project open has nothing to show: fall back to the project list.
  useEffect(() => {
    if (project === null && PROJECT_VIEWS.has(view)) {
      setView("projects");
    }
  }, [project, view]);

  useEffect(() => {
    void refreshSidecar();
  }, [refreshSidecar]);

  useEffect(() => onSidecarStatus((status) => {
    setSidecar(status);
    setSidecarError(null);
  }), []);

  const handleOpenProject = useCallback((opened: Project) => {
    setProject(opened);
  }, []);

  // The open project is the single persisted piece of navigation state. Writing it from an
  // effect (after the restore settled) keeps the stored id and the header in step, including
  // when the project is deleted from the list.
  useEffect(() => {
    if (!restoring) {
      writeStoredProjectId(project?.id ?? null);
    }
  }, [project, restoring]);

  const handleDeleteProject = useCallback((deletedId: string) => {
    setProject((current) => (current?.id === deletedId ? null : current));
  }, []);

  const sidecarReady = sidecar !== null && sidecar.state === "running";

  return (
    <div className="app-canvas">
      {/* Sidebar: project steps first, application destinations below. */}
      <aside className="flex w-60 shrink-0 flex-col gap-4 border-r border-line bg-surface/60 p-3">
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

        {project === null ? (
          <div className="flex flex-col gap-1">
            <div className="nav-group-label">Progetto</div>
            <div className="rounded-lg border border-dashed border-line-strong p-3 text-center">
              <p className="text-xs text-muted">
                {restoring ? "Ripristino dell'ultimo progetto…" : "Nessun progetto aperto."}
              </p>
              <button
                type="button"
                className="btn btn-primary btn-sm mt-2 w-full"
                onClick={() => {
                  setView("projects");
                }}
              >
                Scegli un progetto
              </button>
            </div>
          </div>
        ) : (
          <nav aria-label="Sezioni del progetto aperto">
            <NavGroup
              title="Progetto"
              entries={PROJECT_NAV}
              current={view}
              onSelect={setView}
            />
          </nav>
        )}

        <nav aria-label="Sezioni dell'applicazione">
          <NavGroup title="Applicazione" entries={APP_NAV} current={view} onSelect={setView} />
        </nav>

        <p
          className="mt-auto px-1 text-[0.62rem] leading-relaxed text-faint"
          title={PRIVACY_NOTE}
        >
          Offline. Nessuna telemetria, nessuna chiamata di rete verso l&apos;esterno.
        </p>
      </aside>

      {/* Main column */}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex shrink-0 flex-wrap items-center justify-between gap-3 border-b border-line bg-surface/60 px-5 py-3">
          <div className="min-w-0 flex-1">
            {project === null ? (
              <>
                <h1 className="truncate text-sm font-semibold text-ink">Nessun progetto aperto</h1>
                <p className="truncate text-[0.7rem] text-faint">
                  {restoring
                    ? "Ripristino dell'ultimo progetto…"
                    : "Apri o crea un progetto per iniziare la pipeline di traduzione."}
                </p>
              </>
            ) : (
              <>
                <h1 className="flex min-w-0 items-center gap-2 text-base font-semibold text-ink">
                  <span className="truncate" title={project.name}>
                    {project.name}
                  </span>
                  <span className="mono-chip shrink-0">
                    {project.source_lang ?? "?"} → {project.target_lang}
                  </span>
                </h1>
                <p
                  className="truncate font-mono text-[0.68rem] text-faint"
                  title={project.source_path}
                >
                  {project.source_path}
                </p>
              </>
            )}
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <button
              type="button"
              className="btn btn-sm btn-ghost"
              disabled={sidecar === null && sidecarError === null}
              title={
                sidecar === null
                  ? "Verifica lo stato del sidecar"
                  : `Sidecar ${sidecar.state}${
                      sidecar.pid === null ? "" : ` · pid ${String(sidecar.pid)}`
                    }${sidecar.attempts > 0 ? ` · ${String(sidecar.attempts)} tentativi` : ""}`
              }
              onClick={() => {
                void refreshSidecar();
              }}
            >
              <StatusBadge
                status={sidecarError === null ? (sidecar?.state ?? "starting") : "error"}
                pulse={
                  sidecar !== null &&
                  (sidecar.state === "starting" || sidecar.state === "restarting")
                }
                label={
                  sidecarError !== null
                    ? "Stato non leggibile"
                    : sidecar === null
                      ? "Interrogazione…"
                      : sidecar.state === "running"
                        ? "Sidecar pronto"
                        : undefined
                }
              />
            </button>

            <button
              type="button"
              className="btn btn-sm"
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
              onDeleteProject={handleDeleteProject}
              onNavigate={setView}
            />
          ) : view === "series" ? (
            <SeriesView />
          ) : view === "ingest" ? (
            <IngestView project={project} onNavigate={setView} />
          ) : view === "models" ? (
            <ModelsView />
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
