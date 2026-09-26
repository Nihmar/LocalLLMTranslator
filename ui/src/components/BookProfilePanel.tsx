import { useCallback, useEffect, useRef, useState } from "react";
import { FormField } from "./FormField";
import {
  glossaryDelete,
  glossaryUpsert,
  reconConfirm,
  reconGet,
  reconStart,
  toErrorMessage,
} from "../lib/ipc";
import type {
  BookProfile,
  ConfirmedTerm,
  ProfileProvenance,
  ReconConfirmRequest,
  ReconSnapshot,
} from "../lib/types";

/**
 * Book profile panel (PLAN.md §9.4, step 3 of the wizard).
 *
 * The profile is a **candidate** until the user confirms field by field; only then
 * `recon_confirm` writes `style_guide`/`synopsis` where the context builder reads them and the
 * accepted names into the glossary. A field marked `inferred` is never confirmation-checked by
 * default, so it cannot silently become a fact in the prompt.
 *
 * Reloads on `job://progress` (the parent bumps `reloadToken`) but only replaces the local form
 * when the loaded snapshot actually changed, so a long translation run cannot wipe in-progress
 * edits.
 */

export interface BookProfilePanelProps {
  projectId: string;
  /** Bumped by the parent on `job://progress` so the panel refetches. */
  reloadToken: number;
}

type FieldKey =
  | "source_language"
  | "genre"
  | "audience"
  | "era"
  | "narrative_voice"
  | "register"
  | "style_notes"
  | "themes"
  | "synopsis";

interface FieldForm {
  value: string;
  basis: string;
  confirmed: boolean;
}

interface TermForm {
  source: string;
  kind: string;
  note: string;
  target: string;
  accepted: boolean;
}

interface GlossaryRowForm {
  id: string | null;
  source: string;
  target: string;
  kind: string;
  status: string;
  note: string;
}

interface FormState {
  fields: Record<FieldKey, FieldForm>;
  styleGuide: string;
  terms: TermForm[];
  glossary: GlossaryRowForm[];
}

const EMPTY_PROVENANCE: ProfileProvenance = {
  generated_at: "",
  model: "",
  prompt_hash: "",
  excerpt_blocks: 0,
  metadata: false,
  pasted_chars: 0,
};

const FIELD_ROWS: ReadonlyArray<{
  key: FieldKey;
  label: string;
  hint?: string;
  multiline?: boolean;
}> = [
  { key: "source_language", label: "Lingua di partenza" },
  { key: "genre", label: "Genere" },
  { key: "audience", label: "Pubblico" },
  { key: "era", label: "Epoca" },
  { key: "narrative_voice", label: "Voce narrante" },
  { key: "register", label: "Registro" },
  { key: "style_notes", label: "Note di stile", hint: "Una per riga.", multiline: true },
  { key: "themes", label: "Temi", hint: "Uno per riga.", multiline: true },
  {
    key: "synopsis",
    label: "Sinossi",
    hint: "Iniettata in ogni prompt: poche frasi.",
    multiline: true,
  },
];

const TERM_KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

const GLOSSARY_KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "term", label: "Termine" },
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

const GLOSSARY_STATUSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "candidate", label: "Candidato" },
  { value: "approved", label: "Approvato" },
  { value: "rejected", label: "Rifiutato" },
];

function emptyFields(): Record<FieldKey, FieldForm> {
  const rows = FIELD_ROWS.map((row) => [row.key, { value: "", basis: "user", confirmed: false }]);
  return Object.fromEntries(rows) as Record<FieldKey, FieldForm>;
}

function emptyForm(): FormState {
  return { fields: emptyFields(), styleGuide: "", terms: [], glossary: [] };
}

function splitLines(value: string): string[] {
  return value
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

function fieldValue(profile: BookProfile, key: FieldKey): { value: string; basis: string } {
  if (key === "style_notes" || key === "themes") {
    const field = profile[key];
    return { value: field.value.join("\n"), basis: field.basis };
  }
  const field = profile[key];
  return { value: field.value, basis: field.basis };
}

function formFromSnapshot(snapshot: ReconSnapshot): FormState {
  const profile = snapshot.profile;
  const fields = emptyFields();
  if (profile !== null) {
    for (const row of FIELD_ROWS) {
      const { value, basis } = fieldValue(profile, row.key);
      fields[row.key] = { value, basis, confirmed: basis !== "inferred" };
    }
  }
  const terms: TermForm[] = (profile?.proper_nouns ?? []).map((term) => {
    const existing = snapshot.glossary.find(
      (row) => row.source.toLowerCase() === term.source.toLowerCase(),
    );
    return {
      source: term.source,
      kind: term.kind,
      note: term.note,
      target: existing?.target ?? term.source,
      accepted: true,
    };
  });
  const glossary: GlossaryRowForm[] = snapshot.glossary.map((term) => ({
    id: term.id,
    source: term.source,
    target: term.target,
    kind: term.kind,
    status: term.status,
    note: term.note ?? "",
  }));
  return { fields, styleGuide: snapshot.style_guide, terms, glossary };
}

/** The stable part of a snapshot: a change to it means the candidate really changed. */
function snapshotSignature(snapshot: ReconSnapshot): string {
  return JSON.stringify([
    snapshot.profile?.provenance.generated_at ?? "",
    snapshot.profile?.provenance.prompt_hash ?? "",
    snapshot.style_guide,
    snapshot.synopsis,
    snapshot.book_meta,
    snapshot.glossary.map((term) => `${term.id}:${term.revision}:${term.status}`).join(","),
    snapshot.style_notes,
  ]);
}

function basisLabel(basis: string): string {
  switch (basis) {
    case "from_text":
      return "dal testo";
    case "metadata":
      return "dai metadati";
    case "inferred":
      return "inferito";
    default:
      return "manuale";
  }
}

function basisClass(basis: string): string {
  switch (basis) {
    case "from_text":
      return "badge badge-success";
    case "metadata":
      return "badge badge-info";
    case "inferred":
      return "badge badge-warning";
    default:
      return "badge badge-neutral";
  }
}

function composeStyleGuide(fields: Record<FieldKey, FieldForm>): string {
  const pairs: ReadonlyArray<readonly [string, string]> = [
    ["Genere", fields.genre.value],
    ["Pubblico", fields.audience.value],
    ["Epoca", fields.era.value],
    ["Voce narrante", fields.narrative_voice.value],
    ["Registro", fields.register.value],
  ];
  const lines = pairs
    .filter(([, value]) => value.trim().length > 0)
    .map(([label, value]) => `${label}: ${value.trim()}`);
  const notes = splitLines(fields.style_notes.value);
  if (notes.length > 0) {
    lines.push("Note:");
    lines.push(...notes.map((note) => `- ${note}`));
  }
  return lines.join("\n");
}

function formToRequest(
  projectId: string,
  snapshot: ReconSnapshot,
  form: FormState,
): ReconConfirmRequest {
  const fields = form.fields;
  const profile: BookProfile = {
    source_language: { value: fields.source_language.value.trim(), basis: fields.source_language.basis },
    genre: { value: fields.genre.value.trim(), basis: fields.genre.basis },
    audience: { value: fields.audience.value.trim(), basis: fields.audience.basis },
    era: { value: fields.era.value.trim(), basis: fields.era.basis },
    narrative_voice: { value: fields.narrative_voice.value.trim(), basis: fields.narrative_voice.basis },
    register: { value: fields.register.value.trim(), basis: fields.register.basis },
    style_notes: { value: splitLines(fields.style_notes.value), basis: fields.style_notes.basis },
    themes: { value: splitLines(fields.themes.value), basis: fields.themes.basis },
    synopsis: { value: fields.synopsis.value.trim(), basis: fields.synopsis.basis },
    proper_nouns: form.terms.map((term) => ({
      source: term.source,
      kind: term.kind,
      note: term.note,
    })),
    provenance: snapshot.profile?.provenance ?? EMPTY_PROVENANCE,
  };
  const proper_nouns: ConfirmedTerm[] = form.terms
    .filter((term) => term.accepted)
    .map((term) => ({
      source: term.source,
      target: term.target.trim().length === 0 ? null : term.target.trim(),
      kind: term.kind,
      note: term.note.trim().length === 0 ? null : term.note.trim(),
    }));
  return {
    project_id: projectId,
    profile,
    confirmed_fields: FIELD_ROWS.filter((row) => fields[row.key].confirmed).map((row) => row.key),
    style_guide: form.styleGuide,
    proper_nouns,
  };
}

export function BookProfilePanel({ projectId, reloadToken }: BookProfilePanelProps) {
  const [snapshot, setSnapshot] = useState<ReconSnapshot | null>(null);
  const [form, setForm] = useState<FormState>(emptyForm);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [starting, setStarting] = useState(false);
  const [saving, setSaving] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [pasted, setPasted] = useState("");
  const [expanded, setExpanded] = useState(false);
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
    setForm((current) => ({
      ...current,
      fields: { ...current.fields, [key]: { ...current.fields[key], ...patch } },
    }));
  }

  function updateTerm(index: number, patch: Partial<TermForm>) {
    setForm((current) => ({
      ...current,
      terms: current.terms.map((term, position) =>
        position === index ? { ...term, ...patch } : term,
      ),
    }));
  }

  function updateGlossaryRow(index: number, patch: Partial<GlossaryRowForm>) {
    setForm((current) => ({
      ...current,
      glossary: current.glossary.map((row, position) =>
        position === index ? { ...row, ...patch } : row,
      ),
    }));
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
        { id: null, source: "", target: "", kind: "term", status: "candidate", note: "" },
      ],
    }));
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
      setNotice(
        "Ricognizione accodata: il profilo candidato comparirà qui al termine del job.",
      );
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

            <FormField
              label="Materiale da incollare (opzionale)"
              htmlFor="recon-pasted"
              hint="Testo di cui sei già in possesso. L'app non scarica nulla dalla rete."
            >
              <textarea
                id="recon-pasted"
                className="textarea"
                rows={3}
                value={pasted}
                onChange={(event) => {
                  setPasted(event.target.value);
                }}
                placeholder="Incolla qui una pagina rappresentativa…"
              />
            </FormField>

            <div className="flex items-center gap-2">
              <button
                type="button"
                className="btn btn-primary"
                disabled={!canStart}
                onClick={() => void runStart()}
                title={
                  snapshot?.orchestrator_bound === false
                    ? "Assegna un modello al ruolo orchestrator"
                    : "Genera un profilo candidato dai materiali locali"
                }
              >
                {starting || running ? <span className="spinner" aria-hidden="true" /> : null}
                {running ? "Ricognizione in corso…" : "Riconosci il libro"}
              </button>
              <button
                type="button"
                className="btn"
                disabled={loading}
                onClick={() => void load()}
              >
                Ricarica
              </button>
            </div>

            {profile !== null ? (
              <p className="field-hint">
                Generato da <span className="mono-chip">{profile.provenance.model}</span> ·{" "}
                {profile.provenance.excerpt_blocks} estratti · metadati{" "}
                {profile.provenance.metadata ? "sì" : "no"} ·{" "}
                {profile.provenance.pasted_chars} caratteri incollati
              </p>
            ) : null}

            <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
              {FIELD_ROWS.map((row) => (
                <FormField key={row.key} label={row.label} hint={row.hint}>
                  {row.multiline === true ? (
                    <textarea
                      className="textarea"
                      rows={3}
                      value={form.fields[row.key].value}
                      onChange={(event) => {
                        updateField(row.key, { value: event.target.value });
                      }}
                    />
                  ) : (
                    <input
                      className="input"
                      value={form.fields[row.key].value}
                      onChange={(event) => {
                        updateField(row.key, { value: event.target.value });
                      }}
                    />
                  )}
                  <div className="mt-1 flex items-center justify-between gap-2">
                    <label className="flex items-center gap-1 text-xs text-muted">
                      <input
                        type="checkbox"
                        checked={form.fields[row.key].confirmed}
                        onChange={(event) => {
                          updateField(row.key, { confirmed: event.target.checked });
                        }}
                      />
                      Conferma
                    </label>
                    <span className={basisClass(form.fields[row.key].basis)}>
                      {basisLabel(form.fields[row.key].basis)}
                    </span>
                  </div>
                </FormField>
              ))}
            </div>

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

            {form.terms.length > 0 ? (
              <div>
                <div className="field-label">Nomi propri proposti</div>
                <div className="table-scroll">
                  <table className="data-table">
                    <thead>
                      <tr>
                        <th style={{ width: "4rem" }}>Usa</th>
                        <th>Nome</th>
                        <th>Tipo</th>
                        <th>Traduzione</th>
                        <th>Nota</th>
                      </tr>
                    </thead>
                    <tbody>
                      {form.terms.map((term, index) => (
                        <tr key={term.source}>
                          <td>
                            <input
                              type="checkbox"
                              checked={term.accepted}
                              onChange={(event) => {
                                updateTerm(index, { accepted: event.target.checked });
                              }}
                            />
                          </td>
                          <td>
                            <span className="mono-chip">{term.source}</span>
                          </td>
                          <td>
                            <select
                              className="select"
                              value={term.kind}
                              onChange={(event) => {
                                updateTerm(index, { kind: event.target.value });
                              }}
                            >
                              {TERM_KINDS.map((kind) => (
                                <option key={kind.value} value={kind.value}>
                                  {kind.label}
                                </option>
                              ))}
                            </select>
                          </td>
                          <td>
                            <input
                              className="input"
                              value={term.target}
                              onChange={(event) => {
                                updateTerm(index, { target: event.target.value });
                              }}
                            />
                          </td>
                          <td>
                            <input
                              className="input"
                              value={term.note}
                              onChange={(event) => {
                                updateTerm(index, { note: event.target.value });
                              }}
                            />
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : null}

            {snapshot !== null ? (
              <div>
                <div className="field-label">
                  Glossario del progetto ({form.glossary.length})
                </div>
                {form.glossary.length > 0 ? (
                  <div className="table-scroll">
                    <table className="data-table">
                      <thead>
                        <tr>
                          <th>Sorgente</th>
                          <th>Traduzione</th>
                          <th>Tipo</th>
                          <th>Stato</th>
                          <th>Nota</th>
                          <th style={{ width: "3rem" }} />
                        </tr>
                      </thead>
                      <tbody>
                        {form.glossary.map((row, index) => (
                          <tr key={row.id ?? `new-${index}`}>
                            <td>
                              <input
                                className="input"
                                value={row.source}
                                onChange={(event) => {
                                  updateGlossaryRow(index, { source: event.target.value });
                                }}
                              />
                            </td>
                            <td>
                              <input
                                className="input"
                                value={row.target}
                                onChange={(event) => {
                                  updateGlossaryRow(index, { target: event.target.value });
                                }}
                              />
                            </td>
                            <td>
                              <select
                                className="select"
                                value={row.kind}
                                onChange={(event) => {
                                  updateGlossaryRow(index, { kind: event.target.value });
                                }}
                              >
                                {GLOSSARY_KINDS.map((kind) => (
                                  <option key={kind.value} value={kind.value}>
                                    {kind.label}
                                  </option>
                                ))}
                              </select>
                            </td>
                            <td>
                              <select
                                className="select"
                                value={row.status}
                                onChange={(event) => {
                                  updateGlossaryRow(index, { status: event.target.value });
                                }}
                              >
                                {GLOSSARY_STATUSES.map((status) => (
                                  <option key={status.value} value={status.value}>
                                    {status.label}
                                  </option>
                                ))}
                              </select>
                            </td>
                            <td>
                              <input
                                className="input"
                                value={row.note}
                                onChange={(event) => {
                                  updateGlossaryRow(index, { note: event.target.value });
                                }}
                              />
                            </td>
                            <td>
                              <button
                                type="button"
                                className="btn btn-sm btn-ghost"
                                title="Rimuovi il termine"
                                onClick={() => {
                                  removeGlossaryRow(index);
                                }}
                              >
                                ✕
                              </button>
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                ) : (
                  <p className="field-hint">Nessun termine: aggiungine uno o lancia la ricognizione.</p>
                )}
                <div className="mt-2 flex items-center gap-2">
                  <button type="button" className="btn btn-sm" onClick={addGlossaryRow}>
                    Aggiungi termine
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-primary"
                    disabled={glossarySaving}
                    onClick={() => void saveGlossary()}
                  >
                    {glossarySaving ? <span className="spinner" aria-hidden="true" /> : null}
                    Salva glossario
                  </button>
                  <span className="field-hint">
                    I termini rifiutati non entrano nel prompt.
                  </span>
                </div>
              </div>
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
