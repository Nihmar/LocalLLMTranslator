import { useCallback, useEffect, useState } from "react";
import { JobMonitor } from "./components/JobMonitor";
import { onMetricsTick, onSidecarStatus } from "./lib/events";
import { formatNumber } from "./lib/format";
import { projectGet, sidecarStatus, toErrorMessage } from "./lib/ipc";
import type { JobCount, Project, SidecarStatus } from "./lib/types";
import { ExportView } from "./routes/ExportView";
import { GlossaryView } from "./routes/GlossaryView";
import { HistoryView } from "./routes/HistoryView";
import { IngestView } from "./routes/IngestView";
import { JobsView } from "./routes/JobsView";
import { ModelsView } from "./routes/ModelsView";
import { ProjectsView } from "./routes/ProjectsView";
import { ReviewView } from "./routes/ReviewView";
import { SeriesView } from "./routes/SeriesView";
import { TranslateView } from "./routes/TranslateView";

/**
 * Application shell: a header with the open book, the book's five steps and an activity bar.
 *
 * The book steps follow the order of the work — import, prepare, translate, review, export —
 * and only appear while a book is open; "Libreria", "Serie" and "Modelli" are application-wide
 * and always reachable from the header (issue #14, mockups linked there). "Prepara" is the
 * glossary page, which also carries the book profile and the dialogue convention.
 *
 * There is deliberately no routing library: the shell has fixed destinations, a
 * `useState<ViewId>` is smaller, fully typed and needs no dependency.
 *
 * `ViewId` is exported so views can type their `onNavigate` prop; views import it with
 * `import type`, which `verbatimModuleSyntax` erases, so there is no runtime import cycle.
 *
 * The activity bar answers "what is running, and can I stop it?" from every page: it reads the
 * `metrics://tick` queue snapshot and opens the job monitor, mounted once here.
 */

export type ViewId =
  | "projects"
  | "ingest"
  | "models"
  | "translate"
  | "review"
  | "glossary"
  | "history"
  | "export"
  | "jobs"
  | "series";

interface NavEntry {
  id: ViewId;
  label: string;
  hint: string;
}

/** The book's steps, in the order of the work. */
const BOOK_STEPS: readonly NavEntry[] = [
  { id: "ingest", label: "Importa", hint: "File, formato, capitoli" },
  { id: "glossary", label: "Prepara", hint: "Profilo, convenzioni, glossario" },
  { id: "translate", label: "Traduci", hint: "Capitoli, anteprima, avanzamento" },
  { id: "review", label: "Rivedi", hint: "Proposte di editor e proofreader, QA" },
  { id: "export", label: "Esporta", hint: "EPUB, PDF, DOCX" },
];

/** Book pages that are not steps: reachable from the step bar, after the steps. */
const BOOK_EXTRA: readonly NavEntry[] = [
  { id: "history", label: "Storico", hint: "Correzioni accettate e rifiutate" },
];

/** Application-wide destinations, independent of any book. */
const APP_LINKS: readonly NavEntry[] = [
  { id: "projects", label: "Libreria", hint: "I tuoi libri: apri, crea, importa" },
  { id: "series", label: "Serie", hint: "Canone condiviso tra i libri" },
  { id: "models", label: "Modelli", hint: "Endpoint, salute, ruoli" },
];

const PROJECT_VIEWS: ReadonlySet<ViewId> = new Set(
  [...BOOK_STEPS, ...BOOK_EXTRA].map((entry) => entry.id),
);

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
  const [jobsOpen, setJobsOpen] = useState(false);
  const [queue, setQueue] = useState<readonly JobCount[]>([]);

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

  useEffect(() => onMetricsTick((tick) => {
    setQueue(tick.jobs);
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

  // Queue depth from the last metrics tick: the trigger has to say something even before the
  // monitor is opened, and a subscription is cheaper than polling `job_list` here.
  const queueCount = (state: string): number =>
    queue.find((entry) => entry.state === state)?.count ?? 0;
  const runningJobs = queueCount("running") + queueCount("leased");
  const pendingJobs = queueCount("pending");

  return (
    <div className="app-canvas flex-col">
      <header className="shrink-0 border-b border-line bg-surface">
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 px-6 py-3">
          <button
            type="button"
            className="font-serif text-lg font-semibold text-ink"
            onClick={() => {
              setView(project === null ? "projects" : "translate");
            }}
          >
            LLM Translator
          </button>
          <span aria-hidden="true" className="text-faint">
            /
          </span>
          {project === null ? (
            <span className="text-sm text-muted">
              {restoring ? "Ripristino dell'ultimo libro…" : "Nessun libro aperto"}
            </span>
          ) : (
            <button
              type="button"
              className="btn"
              title={`${project.source_path} — cambia libro`}
              onClick={() => {
                setView("projects");
              }}
            >
              <span className="max-w-[18rem] truncate font-semibold">{project.name}</span>
              <span className="text-xs font-normal text-muted">
                {project.source_lang ?? "?"} → {project.target_lang}
              </span>
            </button>
          )}
          <nav aria-label="Applicazione" className="ml-auto flex flex-wrap gap-1">
            {APP_LINKS.map((entry) => (
              <button
                key={entry.id}
                type="button"
                className="app-link"
                title={entry.hint}
                aria-current={entry.id === view ? "page" : undefined}
                onClick={() => {
                  setView(entry.id);
                }}
              >
                {entry.label}
              </button>
            ))}
          </nav>
        </div>

        {project === null ? null : (
          <nav aria-label="Passi del libro" className="flex flex-wrap items-end gap-x-2 px-6">
            {BOOK_STEPS.map((entry, index) => (
              <button
                key={entry.id}
                type="button"
                className="step-tab"
                title={entry.hint}
                aria-current={entry.id === view ? "page" : undefined}
                onClick={() => {
                  setView(entry.id);
                }}
              >
                <span className="step-number" aria-hidden="true">
                  {index + 1}
                </span>
                {entry.label}
              </button>
            ))}
            <span className="ml-auto flex gap-x-2">
              {BOOK_EXTRA.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  className="step-tab step-tab-extra"
                  title={entry.hint}
                  aria-current={entry.id === view ? "page" : undefined}
                  onClick={() => {
                    setView(entry.id);
                  }}
                >
                  {entry.label}
                </button>
              ))}
            </span>
          </nav>
        )}
      </header>

      {sidecarError !== null || (sidecar !== null && !sidecarReady) ? (
        <div className="shrink-0 px-6 pt-3">
          <div className="banner banner-warn" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>
              <strong className="font-semibold">Sidecar non pronto.</strong>{" "}
              {sidecarError ?? sidecar?.message ?? "Il supervisore lo sta avviando o lo sta riavviando."}{" "}
              L&apos;interfaccia resta navigabile, ma importazione, suddivisione ed export richiedono
              il sidecar attivo.
            </span>
          </div>
        </div>
      ) : null}

      <main className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[1320px] p-6">
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
            <TranslateView
              project={project}
              onNavigate={setView}
              onOpenJobs={() => {
                setJobsOpen(true);
              }}
            />
          ) : view === "review" ? (
            <ReviewView project={project} onNavigate={setView} />
          ) : view === "glossary" ? (
            <GlossaryView project={project} onNavigate={setView} />
          ) : view === "history" ? (
            <HistoryView project={project} onNavigate={setView} />
          ) : view === "export" ? (
            <ExportView project={project} onNavigate={setView} />
          ) : (
            <JobsView project={project} onNavigate={setView} />
          )}
        </div>
      </main>

      <footer className="activity-bar" aria-label="Attività">
        <button
          type="button"
          className="activity-item"
          disabled={sidecar === null && sidecarError === null}
          title="Verifica lo stato del sidecar"
          onClick={() => {
            void refreshSidecar();
          }}
        >
          <span
            className="activity-dot"
            data-state={sidecarError !== null ? "error" : sidecarReady ? "ok" : "busy"}
            aria-hidden="true"
          />
          {sidecarError !== null
            ? "Sidecar non raggiungibile"
            : sidecar === null
              ? "Sidecar: verifica…"
              : sidecarReady
                ? "Sidecar pronto"
                : `Sidecar: ${sidecar.state}`}
        </button>
        <span className="activity-item">
          {runningJobs === 0 && pendingJobs === 0
            ? "Nessun lavoro in corso"
            : `${formatNumber(runningJobs)} in corso · ${formatNumber(pendingJobs)} in coda`}
        </span>
        <span className="ml-auto flex gap-2">
          <button
            type="button"
            className="activity-button"
            onClick={() => {
              setView("jobs");
            }}
          >
            Log e coda
          </button>
          <button
            type="button"
            className="activity-button"
            onClick={() => {
              setJobsOpen(true);
            }}
          >
            Attività
          </button>
        </span>
      </footer>

      {jobsOpen ? (
        <JobMonitor
          projectId={project?.id ?? null}
          onClose={() => {
            setJobsOpen(false);
          }}
        />
      ) : null}
    </div>
  );
}
