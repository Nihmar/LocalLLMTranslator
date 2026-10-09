import { useCallback, useEffect, useRef, useState } from "react";
import { FormField } from "./FormField";
import {
  glossaryDelete,
  glossaryUpsert,
  projectSetDialogueStyle,
  reconConfirm,
  reconGet,
  reconStart,
  toErrorMessage,
} from "../lib/ipc";
import type { DialogueStyle, ReconSnapshot } from "../lib/types";
import { GlossaryEditor } from "./bookProfile/GlossaryEditor";
import { ProfileFieldsGrid } from "./bookProfile/ProfileFieldsGrid";
import { ProposedNounsTable } from "./bookProfile/ProposedNounsTable";
import { ReconControls } from "./bookProfile/ReconControls";
import {
  composeStyleGuide,
  emptyForm,
  fieldFormPatch,
  formFromSnapshot,
  formToRequest,
  glossaryRowPatch,
  snapshotSignature,
  termPatch,
  type FieldForm,
  type FieldKey,
  type FormState,
  type GlossaryRowForm,
  type TermForm,
} from "./bookProfile/shared";

/**
 * Book profile panel (PLAN.md §9.4, shown on the translation page).
 *
 * The profile is a **candidate** until the user confirms field by field; only then
 * `recon_confirm` writes `style_guide`/`synopsis` where the context builder reads them and the
 * accepted names into the glossary. A field marked `inferred` is never confirmation-checked by
 * default, so it cannot silently become a fact in the prompt.
 *
 * Reloads on `job://progress` (the parent bumps `reloadToken`) but only replaces the local form
 * when the loaded snapshot actually changed, so a long translation run cannot wipe in-progress
 * edits. The tables live in `./bookProfile/`; this component owns the form and the actions.
 */

export interface BookProfilePanelProps {
  projectId: string;
  /** Bumped by the parent on `job://progress` so the panel refetches. */
  reloadToken: number;
  /** Start expanded: on the "Prepara" page the profile is the content, not a side note. */
  initiallyExpanded?: boolean;
}

export function BookProfilePanel({
  projectId,
  reloadToken,
  initiallyExpanded = false,
}: BookProfilePanelProps) {
  const [snapshot, setSnapshot] = useState<ReconSnapshot | null>(null);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pasted, setPasted] = useState("");
  const [expanded, setExpanded] = useState(initiallyExpanded);
  const signatureRef = useRef("");
  const [deletedTerms, setDeletedTerms] = useState<string[]>([]);
  const [glossarySaving, setGlossarySaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const next = await reconGet(projectId);
      const signature = snapshotSignature(next);
      setSnapshot(next);
      if (signature !== signatureRef.current) {
        signatureRef.current = signature;
        setForm(formFromSnapshot(next));
      }
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, [projectId]);

  useEffect(() => {
    signatureRef.current = "";
    void load();
  }, [load]);

  useEffect(() => {
    if (reloadToken > 0) {
      void load();
    }
  }, [reloadToken, load]);

  function updateField(key: FieldKey, patch: Partial<FieldForm>) {
    setForm((current) => fieldFormPatch(current, key, patch));
  }

  function updateTerm(index: number, patch: Partial<TermForm>) {
    setForm((current) => termPatch(current, index, patch));
  }

  function updateGlossaryRow(index: number, patch: Partial<GlossaryRowForm>) {
    setForm((current) => glossaryRowPatch(current, index, patch));
  }

  function removeGlossaryRow(index: number) {
    setForm((current) => {
      const removedId = current.glossary[index]?.id ?? null;
      if (removedId !== null) {
        setDeletedTerms((deleted) => [...deleted, removedId]);
      }
      return {
        ...current,
        glossary: current.glossary.filter((_, position) => position !== index),
      };
    });
  }

  function addGlossaryRow() {
    setForm((current) => ({
      ...current,
      glossary: [
        ...current.glossary,
        {
          id: null,
          source: "",
          target: "",
          kind: "term",
          status: "candidate",
          note: "",
          revision: 0,
        },
      ],
    }));
  }

  async function changeDialogueStyle(style: DialogueStyle) {
    setActionError(null);
    setNotice(null);
    try {
      setSnapshot(await projectSetDialogueStyle({ project_id: projectId, dialogue_style: style }));
      setNotice(
        "Convenzione dei dialoghi salvata: vale per i chunk tradotti da ora in poi (per i precedenti usa «Riprova»).",
      );
    } catch (styleError) {
      setActionError(toErrorMessage(styleError));
    }
  }

  function appendStyleNote(note: string) {
    setForm((current) => {
      const existing = current.styleGuide.trim();
      if (existing.includes(note)) {
        return current;
      }
      return {
        ...current,
        styleGuide: existing === "" ? note : `${existing}\n- ${note}`,
      };
    });
  }

  async function saveGlossary() {
    for (const row of form.glossary) {
      if (
        row.source.trim() === "" ||
        (row.target.trim() === "" && row.kind !== "do_not_translate")
      ) {
        setActionError(
          "Ogni termine richiede un testo di partenza e una traduzione (oppure il tipo «Non tradurre»).",
        );
        return;
      }
    }
    setGlossarySaving(true);
    setActionError(null);
    setNotice(null);
    try {
      for (const id of deletedTerms) {
        await glossaryDelete(id);
      }
      for (const row of form.glossary) {
        await glossaryUpsert({
          id: row.id,
          project_id: projectId,
          source: row.source.trim(),
          target: row.target.trim(),
          kind: row.kind,
          note: row.note,
          status: row.status,
          expected_revision: row.id === null ? null : row.revision,
        });
      }
      setDeletedTerms([]);
      // Force a refresh from the persisted rows (revisions, ids, statuses).
      signatureRef.current = "";
      await load();
      setNotice("Glossario aggiornato.");
    } catch (saveError) {
      setActionError(toErrorMessage(saveError));
    } finally {
      setGlossarySaving(false);
    }
  }

  async function runStart() {
    setStarting(true);
    setActionError(null);
    setNotice(null);
    try {
      const trimmed = pasted.trim();
      await reconStart({ project_id: projectId, pasted_text: trimmed === "" ? null : trimmed });
      setExpanded(true);
      setNotice("Ricognizione accodata: il profilo candidato comparirà qui al termine del job.");
      await load();
    } catch (startError) {
      setActionError(toErrorMessage(startError));
    } finally {
      setStarting(false);
    }
  }

  async function runConfirm() {
    if (snapshot === null) {
      return;
    }
    setSaving(true);
    setActionError(null);
    setNotice(null);
    try {
      const next = await reconConfirm(formToRequest(projectId, snapshot, form));
      signatureRef.current = snapshotSignature(next);
      setSnapshot(next);
      setForm(formFromSnapshot(next));
      setNotice(
        "Profilo confermato: stile e sinossi entrano nel prompt del traduttore, i nomi accettati nel glossario.",
      );
    } catch (saveError) {
      setActionError(toErrorMessage(saveError));
    } finally {
      setSaving(false);
    }
  }

  const profile = snapshot?.profile ?? null;
  const running = snapshot?.running_job !== null && snapshot?.running_job !== undefined;
  const canStart = snapshot !== null && snapshot.orchestrator_bound && !running && !starting;
  const summary =
    snapshot === null
      ? "Caricamento…"
      : snapshot.style_guide.trim().length > 0
        ? snapshot.style_guide
        : profile !== null
          ? "Profilo candidato pronto: rivedi i campi e conferma."
          : "Nessun profilo: genera la ricognizione o compila i campi a mano.";

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Profilo del libro</span>
        <span className="flex flex-wrap items-center gap-2">
          {running ? <span className="badge badge-info">ricognizione in corso</span> : null}
          {snapshot?.book_meta !== null && snapshot?.book_meta !== undefined ? (
            <span className="badge badge-success">confermato</span>
          ) : null}
          {profile !== null && !running ? (
            <span className="badge badge-warning">candidato</span>
          ) : null}
          <button
            type="button"
            className="btn btn-sm"
            aria-expanded={expanded}
            onClick={() => {
              setExpanded((current) => !current);
            }}
          >
            {expanded ? "Comprimi" : "Espandi"}
          </button>
        </span>
      </div>
      <div className="panel-pad section-stack">
        <p className="text-xs text-muted">{summary}</p>

        {expanded ? (
          <>
            {actionError !== null ? (
              <div className="banner banner-error" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>{actionError}</span>
              </div>
            ) : null}
            {notice !== null ? (
              <div className="banner banner-ok" role="status">
                <span aria-hidden="true">✓</span>
                <span>{notice}</span>
              </div>
            ) : null}
            {snapshot?.last_error !== null && snapshot?.last_error !== undefined ? (
              <div className="banner banner-error" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>Ultima ricognizione fallita: {snapshot.last_error}</span>
              </div>
            ) : null}

            {!snapshot?.orchestrator_bound ? (
              <div className="banner" role="status">
                <span aria-hidden="true">ℹ</span>
                <span>
                  Nessun modello <span className="mono-chip">orchestrator</span> assegnato: puoi
                  compilare i campi a mano oppure assegnarlo in Modelli per generare il profilo
                  automaticamente.
                </span>
              </div>
            ) : null}

            <ReconControls
              pasted={pasted}
              onPasted={setPasted}
              canStart={canStart}
              running={running}
              starting={starting}
              loading={loading}
              orchestratorBound={snapshot?.orchestrator_bound}
              profile={profile}
              onStart={() => void runStart()}
              onReload={() => void load()}
            />

            <ProfileFieldsGrid fields={form.fields} onFieldChange={updateField} />

            <FormField
              label="Guida di stile"
              htmlFor="recon-style-guide"
              hint="Testo libero che il traduttore riceve a ogni chunk."
            >
              <textarea
                id="recon-style-guide"
                className="textarea"
                rows={5}
                value={form.styleGuide}
                onChange={(event) => {
                  setForm((current) => ({ ...current, styleGuide: event.target.value }));
                }}
              />
              <div className="mt-1">
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() => {
                    setForm((current) => ({
                      ...current,
                      styleGuide: composeStyleGuide(current.fields),
                    }));
                  }}
                >
                  Componi dai campi confermati
                </button>
              </div>
            </FormField>

            <FormField
              label="Battute di dialogo"
              htmlFor="recon-dialogue-style"
              hint="Una scelta per tutto il libro: editor e proofreader non la correggono paragrafo per paragrafo."
            >
              <select
                id="recon-dialogue-style"
                className="select"
                value={snapshot?.dialogue_style ?? "keep"}
                disabled={snapshot === null}
                onChange={(event) => {
                  void changeDialogueStyle(event.target.value === "quotes" ? "quotes" : "keep");
                }}
              >
                <option value="keep">Trattino, come l&apos;originale</option>
                <option value="quotes">Virgolette della lingua di arrivo</option>
              </select>
            </FormField>

            <ProposedNounsTable terms={form.terms} onTermChange={updateTerm} />

            {snapshot !== null ? (
              <GlossaryEditor
                rows={form.glossary}
                saving={glossarySaving}
                onRowChange={updateGlossaryRow}
                onRemove={removeGlossaryRow}
                onAdd={addGlossaryRow}
                onSave={() => void saveGlossary()}
              />
            ) : null}

            {snapshot !== null && snapshot.style_notes.length > 0 ? (
              <div>
                <div className="field-label">Osservazioni del riassuntore</div>
                <ul className="space-y-1">
                  {snapshot.style_notes.map((note) => (
                    <li key={note} className="flex items-start justify-between gap-2 text-xs">
                      <span className="text-ink-soft">{note}</span>
                      <button
                        type="button"
                        className="btn btn-sm"
                        onClick={() => {
                          appendStyleNote(note);
                        }}
                      >
                        Aggiungi alla guida
                      </button>
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            <div className="flex items-center gap-2">
              <button
                type="button"
                className="btn btn-primary"
                disabled={saving || snapshot === null}
                onClick={() => void runConfirm()}
              >
                {saving ? <span className="spinner" aria-hidden="true" /> : null}
                Salva profilo confermato
              </button>
              <span className="field-hint">
                Solo i campi spuntati vengono scritti; i nomi partono già approvati.
              </span>
            </div>

            {error !== null ? (
              <div className="banner banner-error" role="alert">
                <span aria-hidden="true">⚠</span>
                <span>{error}</span>
              </div>
            ) : null}
          </>
        ) : null}
      </div>
    </div>
  );
}
