import { useCallback, useEffect, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { StatusBadge } from "../components/StatusBadge";
import { basename, fileExtension, formatBytes, formatDateTime, formatRelative } from "../lib/format";
import { pickBundleFile, pickDocumentFile } from "../lib/dialog";
import {
  openPath,
  ingestStart,
  projectCreate,
  projectDelete,
  projectExport,
  projectGet,
  projectImport,
  projectList,
  toErrorMessage,
} from "../lib/ipc";
import type {
  CreateProjectRequest,
  ExportBundleOutcome,
  Project,
  SourceFormat,
} from "../lib/types";
import type { ViewId } from "../App";

/**
 * Project list / create / open / delete (`PLAN.md` §11, "Progetti").
 *
 * The `.llmtz` export/import of `PLAN.md` §6 is offered here through `project_export` and
 * `project_import`; the note at the bottom of the list spells out what the bundle contains.
 */

export interface ProjectsViewProps {
  currentProjectId: string | null;
  onOpenProject: (project: Project) => void;
  /** Reports a deleted project so the shell can drop it as the open one. */
  onDeleteProject: (projectId: string) => void;
  onNavigate: (view: ViewId) => void;
}

interface FormState {
  name: string;
  source_path: string;
  source_format: SourceFormat;
  source_lang: string;
  target_lang: string;
}

const FORMAT_OPTIONS: ReadonlyArray<{ value: SourceFormat; label: string }> = [
  { value: "epub", label: "EPUB" },
  { value: "pdf", label: "PDF" },
  { value: "markdown", label: "Markdown" },
];

const EMPTY_FORM: FormState = {
  name: "",
  source_path: "",
  source_format: "epub",
  source_lang: "",
  target_lang: "it",
};

/** Maps a file extension to the format the sidecar will report (`PLAN.md` §12.1). */
function detectFormatFromPath(path: string): SourceFormat | null {
  const extension = fileExtension(path);
  if (extension === null) {
    return null;
  }
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

function looksAbsolute(path: string): boolean {
  return path.startsWith("/") || /^[A-Za-z]:[\\/]/.test(path) || path.startsWith("\\\\");
}

function formatLabel(format: string): string {
  return FORMAT_OPTIONS.find((option) => option.value === format)?.label ?? format;
}

interface FormErrors {
  name?: string | undefined;
  source_path?: string | undefined;
  target_lang?: string | undefined;
}

export function ProjectsView({ currentProjectId, onOpenProject, onDeleteProject, onNavigate }: ProjectsViewProps) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [formErrors, setFormErrors] = useState<FormErrors>({});
  const [submitting, setSubmitting] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [detectedFormat, setDetectedFormat] = useState<SourceFormat | null>(null);

  const [busyId, setBusyId] = useState<string | null>(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
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

  function updatePath(path: string) {
    const detected = detectFormatFromPath(path);
    setDetectedFormat(detected);
    setForm((current) => {
      const next: FormState = { ...current, source_path: path };
      if (detected !== null) {
        next.source_format = detected;
        if (current.name.trim().length === 0) {
          const stem = basename(path).replace(/\.[^.]+$/, "");
          if (stem.length > 0 && stem !== "—") {
            next.name = stem;
          }
        }
      }
      return next;
    });
  }

  async function handlePickSource() {
    const picked = await pickDocumentFile();
    if (picked !== null) {
      updatePath(picked);
      setFormErrors((current) => ({ ...current, source_path: undefined }));
    }
  }

  function validate(state: FormState): FormErrors {
    const errors: FormErrors = {};
    if (state.name.trim().length === 0) {
      errors.name = "Indica un nome per il progetto.";
    }
    if (state.source_path.trim().length === 0) {
      errors.source_path = "Indica il percorso del documento sorgente.";
    } else if (!looksAbsolute(state.source_path.trim())) {
      errors.source_path = "Serve un percorso assoluto (es. /home/utente/libro.epub).";
    }
    if (state.target_lang.trim().length === 0) {
      errors.target_lang = "Indica la lingua di destinazione (es. it).";
    }
    return errors;
  }

  async function handleSubmit() {
    const errors = validate(form);
    setFormErrors(errors);
    setFormError(null);
    if (Object.keys(errors).length > 0) {
      return;
    }

    setSubmitting(true);
    try {
      const request: CreateProjectRequest = {
        name: form.name.trim(),
        source_path: form.source_path.trim(),
        source_format: form.source_format,
        source_lang: form.source_lang.trim().length > 0 ? form.source_lang.trim() : null,
        target_lang: form.target_lang.trim(),
      };
      const created = await projectCreate(request);
      setProjects((current) => [created, ...current]);
      setForm(EMPTY_FORM);
      setDetectedFormat(null);
      setFormOpen(false);
      onOpenProject(created);
      // Creating a book and importing it are one step: the file was just chosen, asking for it
      // again on the ingestion page was busywork. A failed start is shown there, on the job.
      await ingestStart({ project_id: created.id, pdf_backend: null }).catch(() => null);
      onNavigate("ingest");
    } catch (submitError) {
      setFormError(toErrorMessage(submitError));
    } finally {
      setSubmitting(false);
    }
  }

  async function handleOpen(projectId: string) {
    setBusyId(projectId);
    setActionError(null);
    try {
      const fresh = await projectGet(projectId);
      onOpenProject(fresh.project);
      onNavigate("ingest");
    } catch (openError) {
      setActionError(toErrorMessage(openError));
    } finally {
      setBusyId(null);
    }
  }

  async function handleDelete(projectId: string) {
    setBusyId(projectId);
    setActionError(null);
    try {
      await projectDelete(projectId);
      setProjects((current) => current.filter((project) => project.id !== projectId));
      setConfirmDeleteId(null);
      onDeleteProject(projectId);
    } catch (deleteError) {
      setActionError(toErrorMessage(deleteError));
    } finally {
      setBusyId(null);
    }
  }

  async function handleExportBundle(projectId: string) {
    setBusyId(projectId);
    setActionError(null);
    setActionNotice(null);
    setBundleResult(null);
    try {
      const outcome = await projectExport({ project_id: projectId, output_path: null });
      setBundleResult(outcome);
      setActionNotice(`Bundle creato: ${outcome.output_path}`);
    } catch (exportError) {
      setActionError(toErrorMessage(exportError));
    } finally {
      setBusyId(null);
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
    } catch (error) {
      setImportError(toErrorMessage(error));
    } finally {
      setBundleBusy(false);
    }
  }

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Progetti</h2>
          <p className="mt-0.5 text-xs text-muted">
            Un progetto è un documento sorgente con la sua lingua di destinazione, la sua copia dei
            prompt e i suoi checkpoint.
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
              setFormOpen((open) => !open);
            }}
          >
            {formOpen ? "Chiudi" : "Nuovo progetto"}
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
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() => {
                    void openPath(bundleResult.output_path);
                  }}
                >
                  Apri cartella
                </button>
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
        <form
          className="panel"
          onSubmit={(event) => {
            event.preventDefault();
            void handleSubmit();
          }}
        >
          <div className="panel-head">
            <span className="panel-title">Nuovo progetto</span>
            {detectedFormat !== null ? (
              <StatusBadge
                status="ok"
                tone="info"
                label={`Formato rilevato dal percorso: ${formatLabel(detectedFormat)}`}
              />
            ) : null}
          </div>

          <div className="panel-pad section-stack">
            <div className="grid grid-cols-2 gap-3">
              <FormField label="Nome progetto" htmlFor="project-name" required error={formErrors.name}>
                <input
                  id="project-name"
                  className="input"
                  value={form.name}
                  onChange={(event) => {
                    setForm((current) => ({ ...current, name: event.target.value }));
                  }}
                  placeholder="Il nome della rosa"
                />
              </FormField>

              <FormField
                label="Lingua di destinazione"
                htmlFor="project-target"
                required
                hint="Codice ISO 639-1, es. it, en, fr."
                error={formErrors.target_lang}
              >
                <input
                  id="project-target"
                  className="input"
                  value={form.target_lang}
                  onChange={(event) => {
                    setForm((current) => ({ ...current, target_lang: event.target.value }));
                  }}
                />
              </FormField>
            </div>

            <FormField
              label="Documento sorgente"
              htmlFor="project-source"
              required
              hint="Percorso assoluto del file EPUB, PDF o Markdown. Usa «Sfoglia» oppure incollalo."
              error={formErrors.source_path}
            >
              <div className="flex items-center gap-2">
                <input
                  id="project-source"
                  className="input"
                  value={form.source_path}
                  onChange={(event) => {
                    updatePath(event.target.value);
                  }}
                  placeholder="/home/utente/libri/il-nome-della-rosa.epub"
                  spellCheck={false}
                />
                <button
                  type="button"
                  className="btn shrink-0"
                  onClick={() => {
                    void handlePickSource();
                  }}
                >
                  Sfoglia…
                </button>
              </div>
            </FormField>

            <div className="grid grid-cols-2 gap-3">
              <FormField
                label="Formato sorgente"
                htmlFor="project-format"
                hint="Sovrascrivibile: il valore autorevole resta quello rilevato dal sidecar."
              >
                <select
                  id="project-format"
                  className="select"
                  value={form.source_format}
                  onChange={(event) => {
                    const value = event.target.value;
                    const matched = FORMAT_OPTIONS.find((option) => option.value === value);
                    if (matched !== undefined) {
                      setForm((current) => ({ ...current, source_format: matched.value }));
                    }
                  }}
                >
                  {FORMAT_OPTIONS.map((option) => (
                    <option key={option.value} value={option.value}>
                      {option.label}
                    </option>
                  ))}
                </select>
              </FormField>

              <FormField
                label="Lingua di partenza"
                htmlFor="project-source-lang"
                hint="Lascia vuoto per farla rilevare al modello."
              >
                <input
                  id="project-source-lang"
                  className="input"
                  value={form.source_lang}
                  onChange={(event) => {
                    setForm((current) => ({ ...current, source_lang: event.target.value }));
                  }}
                  placeholder="en"
                />
              </FormField>
            </div>

            {formError !== null ? (
              <div className="banner banner-error" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>{formError}</span>
              </div>
            ) : null}

            <div className="flex items-center gap-2">
              <button type="submit" className="btn btn-primary" disabled={submitting}>
                {submitting ? <span className="spinner" aria-hidden="true" /> : null}
                Crea e importa
              </button>
              <button
                type="button"
                className="btn btn-ghost"
                onClick={() => {
                  setForm(EMPTY_FORM);
                  setFormErrors({});
                  setFormError(null);
                  setDetectedFormat(null);
                }}
                disabled={submitting}
              >
                Azzera
              </button>
            </div>
          </div>
        </form>
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
        <div className="section-stack">
          <EmptyState
            title="Benvenuto in LocalLLMTranslator"
            description="Tre passi per iniziare: assegna un modello, crea o importa un progetto, poi importa il documento."
            actionLabel="Crea il primo progetto"
            onAction={() => {
              setFormOpen(true);
            }}
          />
          <div className="panel panel-pad">
            <div className="panel-title mb-2">Primo avvio</div>
            <ol className="list-decimal space-y-1 pl-4 text-xs text-muted">
              <li>
                In <strong>Modelli</strong> registra l&apos;endpoint di{" "}
                <span className="mono-chip">llama-server</span> e assegna il ruolo <em>traduttore</em>;
                l&apos;orchestratore serve alla ricognizione e alla memoria del libro.
              </li>
              <li>
                Crea un progetto con il documento EPUB/PDF/Markdown, oppure importa un bundle{" "}
                <span className="mono-chip">.llmtz</span> creato altrove.
              </li>
              <li>
                Dalla pagina <strong>Ingestione</strong> estrai il documento, poi traduci dalla pagina{" "}
                <strong>Traduzione</strong> e rivedi da <strong>Revisione</strong>.
              </li>
            </ol>
            <div className="mt-2 flex items-center gap-2">
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => {
                  onNavigate("models");
                }}
              >
                Vai ai modelli
              </button>
              <button
                type="button"
                className="btn btn-sm"
                disabled={bundleBusy}
                onClick={() => {
                  void handleImportBundle();
                }}
              >
                Importa .llmtz
              </button>
            </div>
          </div>
        </div>
      ) : (
        <div className="grid grid-cols-2 gap-3 xl:grid-cols-3">
          {projects.map((project) => {
            const isCurrent = project.id === currentProjectId;
            const isBusy = busyId === project.id;
            const isConfirming = confirmDeleteId === project.id;

            return (
              <article key={project.id} className="panel flex flex-col">
                <div className="panel-head">
                  <span className="flex min-w-0 items-center gap-2">
                    <span className="truncate text-sm font-semibold text-ink" title={project.name}>
                      {project.name}
                    </span>
                    {isCurrent ? <StatusBadge status="ok" tone="accent" label="Aperto" /> : null}
                  </span>
                  <span className="mono-chip">{formatLabel(project.source_format)}</span>
                </div>

                <div className="panel-pad flex flex-1 flex-col gap-2">
                  <p className="truncate font-mono text-[0.72rem] text-muted" title={project.source_path}>
                    {basename(project.source_path)}
                  </p>

                  <dl className="grid grid-cols-2 gap-1 text-[0.72rem]">
                    <dt className="text-faint">Lingue</dt>
                    <dd className="font-mono text-ink-soft">
                      {project.source_lang ?? "?"} → {project.target_lang}
                    </dd>
                    <dt className="text-faint">Modificato</dt>
                    <dd className="text-ink-soft">{formatRelative(project.updated_at)}</dd>
                    <dt className="text-faint">Creato</dt>
                    <dd className="text-ink-soft">{formatDateTime(project.created_at)}</dd>
                  </dl>

                  {project.doc_title !== null ? (
                    <p className="truncate text-[0.72rem] text-muted" title={project.doc_title}>
                      {project.doc_title}
                      {project.doc_author !== null ? ` — ${project.doc_author}` : ""}
                    </p>
                  ) : null}

                  {isConfirming ? (
                    <div className="banner banner-warn" role="alert">
                      <span aria-hidden="true">⚠</span>
                      <span>
                        Eliminare «{project.name}» e tutti i suoi checkpoint?
                        <span className="mt-2 flex gap-2">
                          <button
                            type="button"
                            className="btn btn-sm btn-danger"
                            disabled={isBusy}
                            onClick={() => {
                              void handleDelete(project.id);
                            }}
                          >
                            Elimina
                          </button>
                          <button
                            type="button"
                            className="btn btn-sm"
                            onClick={() => {
                              setConfirmDeleteId(null);
                            }}
                          >
                            Annulla
                          </button>
                        </span>
                      </span>
                    </div>
                  ) : null}

                  <div className="mt-auto flex flex-wrap items-center gap-2 pt-1">
                    <button
                      type="button"
                      className="btn btn-primary btn-sm"
                      disabled={isBusy}
                      onClick={() => {
                        void handleOpen(project.id);
                      }}
                    >
                      Apri
                    </button>
                    <button
                      type="button"
                      className="btn btn-sm btn-ghost"
                      onClick={() => {
                        onOpenProject(project);
                        onNavigate("ingest");
                      }}
                    >
                      Ingestione
                    </button>
                    <button
                      type="button"
                      className="btn btn-sm btn-ghost"
                      disabled={isBusy}
                      onClick={() => {
                        void handleExportBundle(project.id);
                      }}
                    >
                      Esporta .llmtz
                    </button>
                    <button
                      type="button"
                      className="btn btn-sm btn-ghost"
                      disabled={isBusy}
                      onClick={() => {
                        setConfirmDeleteId(isConfirming ? null : project.id);
                      }}
                    >
                      Elimina
                    </button>
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      )}

      <p className="text-[0.72rem] text-faint">
        Il bundle <span className="mono-chip">.llmtz</span> contiene il database del progetto, il
        Markdown con gli asset, l&apos;output e lo snapshot dei prompt; l&apos;importazione rifiuta un
        progetto già presente invece di sovrascriverlo.
      </p>
    </div>
  );
}
