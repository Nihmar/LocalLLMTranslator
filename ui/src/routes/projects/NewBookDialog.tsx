import { useRef, useState } from "react";
import { Dialog } from "../../components/Dialog";
import { FormField } from "../../components/FormField";
import { pickDocumentFile } from "../../lib/dialog";
import { basename } from "../../lib/format";
import { documentInspect, projectCreate, toErrorMessage } from "../../lib/ipc";
import type { CreateProjectRequest, Project } from "../../lib/types";
import {
  detectFormatFromPath,
  EMPTY_FORM,
  FORMAT_OPTIONS,
  isSourceFormat,
  LANGUAGES,
  languageName,
  looksAbsolute,
  suggestedName,
  type FormErrors,
  type FormState,
} from "./shared";

/** "Nuovo libro": pick a document, read its title/language, create the project. */
export function NewBookDialog({
  onCreated,
  onClose,
}: {
  /** The parent closes the dialog, adds the book to the list, opens it and starts ingestion. */
  onCreated: (project: Project) => Promise<void>;
  onClose: () => void;
}) {
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [formErrors, setFormErrors] = useState<FormErrors>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);
  /** What the file said about itself, as one line ("EPUB · francese · Pierre Bottero"). */
  const [inspection, setInspection] = useState<string | null>(null);
  const [inspecting, setInspecting] = useState(false);
  // Only the answer for the latest path may fill the form: a slow read of an earlier file
  // must not overwrite the one the user picked after it.
  const inspectToken = useRef(0);

  function updatePath(path: string) {
    const detected = detectFormatFromPath(path);
    setForm((current) => {
      const next: FormState = { ...current, source_path: path };
      if (detected !== null) {
        next.source_format = detected;
        if (current.name.trim().length === 0) {
          const stem = suggestedName(path);
          if (stem !== null) {
            next.name = stem;
          }
        }
      }
      return next;
    });
  }

  async function inspectPath(path: string) {
    if (!looksAbsolute(path.trim())) {
      return;
    }
    const token = ++inspectToken.current;
    setInspecting(true);
    try {
      const info = await documentInspect(path.trim());
      if (token !== inspectToken.current) {
        return;
      }
      const { title, author, language } = info.metadata;
      const stem = basename(path).replace(/\.[^.]+$/, "");
      setForm((current) => ({
        ...current,
        source_format: isSourceFormat(info.format) ? info.format : current.source_format,
        // The file name is only a placeholder: the book's own title replaces it.
        name:
          title !== null && title !== undefined && (current.name === "" || current.name === stem)
            ? title
            : current.name,
        source_lang:
          current.source_lang === "" && language !== null && language !== undefined
            ? language
            : current.source_lang,
      }));
      setInspection(
        [
          info.format.toUpperCase(),
          language === null || language === undefined ? null : languageName(language),
          author ?? null,
        ]
          .filter((part): part is string => part !== null && part !== "")
          .join(" · "),
      );
    } catch {
      // Inspection only pre-fills the form; the ingestion reports a real problem.
      if (token === inspectToken.current) {
        setInspection(null);
      }
    } finally {
      if (token === inspectToken.current) {
        setInspecting(false);
      }
    }
  }

  async function handlePickSource() {
    const picked = await pickDocumentFile();
    if (picked !== null) {
      updatePath(picked);
      setFormErrors((current) => ({ ...current, source_path: undefined }));
      void inspectPath(picked);
    }
  }

  function validate(state: FormState): FormErrors {
    const errors: FormErrors = {};
    if (state.name.trim().length === 0) {
      errors.name = "Indica un nome per il libro.";
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
      await onCreated(await projectCreate(request));
    } catch (submitError) {
      setFormError(toErrorMessage(submitError));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog title="Nuovo libro" size="compact" onClose={onClose}>
      <form
        className="section-stack"
        onSubmit={(event) => {
          event.preventDefault();
          void handleSubmit();
        }}
      >
        <FormField
          label="Documento"
          htmlFor="project-source"
          required
          hint="EPUB, PDF o Markdown: titolo e lingua vengono letti dal file."
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
              onBlur={(event) => {
                void inspectPath(event.target.value);
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
              Scegli il file…
            </button>
          </div>
        </FormField>

        {inspecting ? (
          <p className="text-sm text-muted">Lettura del file…</p>
        ) : inspection !== null ? (
          <p className="banner banner-ok">
            <span aria-hidden="true">✓</span>
            <span>Rilevato: {inspection}</span>
          </p>
        ) : null}

        <div className="grid grid-cols-2 gap-3">
          <FormField label="Nome del libro" htmlFor="project-name" required error={formErrors.name}>
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

          <FormField label="Traduci in" htmlFor="project-target" required error={formErrors.target_lang}>
            <select
              id="project-target"
              className="select"
              value={form.target_lang}
              onChange={(event) => {
                setForm((current) => ({ ...current, target_lang: event.target.value }));
              }}
            >
              {LANGUAGES.some((language) => language.code === form.target_lang) ? null : (
                <option value={form.target_lang}>{form.target_lang}</option>
              )}
              {LANGUAGES.map((language) => (
                <option key={language.code} value={language.code}>
                  {language.name}
                </option>
              ))}
            </select>
          </FormField>
        </div>

        <details>
          <summary className="cursor-pointer text-sm text-accent">
            Opzioni avanzate: lingua di partenza, formato
          </summary>
          <div className="mt-3 grid grid-cols-2 gap-3">
            <FormField
              label="Lingua di partenza"
              htmlFor="project-source-lang"
              hint="Codice ISO 639-1. Vuoto: la rileva la ricognizione."
            >
              <input
                id="project-source-lang"
                className="input"
                value={form.source_lang}
                onChange={(event) => {
                  setForm((current) => ({ ...current, source_lang: event.target.value }));
                }}
                placeholder="fr"
              />
            </FormField>

            <FormField label="Formato" htmlFor="project-format">
              <select
                id="project-format"
                className="select"
                value={form.source_format}
                onChange={(event) => {
                  const value = event.target.value;
                  if (isSourceFormat(value)) {
                    setForm((current) => ({ ...current, source_format: value }));
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
          </div>
        </details>

        {formError !== null ? (
          <div className="banner banner-error" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>{formError}</span>
          </div>
        ) : null}

        <div className="flex justify-end gap-2">
          <button type="button" className="btn" onClick={onClose} disabled={submitting}>
            Annulla
          </button>
          <button type="submit" className="btn btn-primary" disabled={submitting}>
            {submitting ? <span className="spinner" aria-hidden="true" /> : null}
            Crea e importa
          </button>
        </div>
      </form>
    </Dialog>
  );
}
