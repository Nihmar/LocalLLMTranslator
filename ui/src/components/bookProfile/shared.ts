/**
 * Pieces of the book profile panel (`PLAN.md` §9.4): the field keys and their labels, the
 * editable form shape, and the pure conversions between the form and the `recon_*` payloads.
 */

import type {
  BookProfile,
  ConfirmedTerm,
  ProfileProvenance,
  ReconConfirmRequest,
  ReconSnapshot,
} from "../../lib/types";

export type FieldKey =
  | "source_language"
  | "genre"
  | "audience"
  | "era"
  | "narrative_voice"
  | "register"
  | "style_notes"
  | "themes"
  | "synopsis";

export interface FieldForm {
  value: string;
  basis: string;
  confirmed: boolean;
}

export interface TermForm {
  source: string;
  kind: string;
  note: string;
  target: string;
  accepted: boolean;
}

export interface GlossaryRowForm {
  id: string | null;
  source: string;
  target: string;
  kind: string;
  status: string;
  note: string;
  /** Optimistic-lock revision of the persisted row; `0` for a new term. */
  revision: number;
}

export interface FormState {
  fields: Record<FieldKey, FieldForm>;
  styleGuide: string;
  terms: TermForm[];
  glossary: GlossaryRowForm[];
}

export const EMPTY_PROVENANCE: ProfileProvenance = {
  generated_at: "",
  model: "",
  prompt_hash: "",
  excerpt_blocks: 0,
  metadata: false,
  pasted_chars: 0,
};

export const FIELD_ROWS: ReadonlyArray<{
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

export const TERM_KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

export const GLOSSARY_KINDS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "term", label: "Termine" },
  { value: "proper_noun", label: "Nome proprio" },
  { value: "do_not_translate", label: "Non tradurre" },
];

export const GLOSSARY_STATUSES: ReadonlyArray<{ value: string; label: string }> = [
  { value: "candidate", label: "Candidato" },
  { value: "approved", label: "Approvato" },
  { value: "rejected", label: "Rifiutato" },
  { value: "conflict", label: "Conflitto" },
];

export function emptyFields(): Record<FieldKey, FieldForm> {
  const rows = FIELD_ROWS.map((row) => [row.key, { value: "", basis: "user", confirmed: false }]);
  return Object.fromEntries(rows) as Record<FieldKey, FieldForm>;
}

export function emptyForm(): FormState {
  return { fields: emptyFields(), styleGuide: "", terms: [], glossary: [] };
}

export function splitLines(value: string): string[] {
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

export function formFromSnapshot(snapshot: ReconSnapshot): FormState {
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
    revision: term.revision,
  }));
  return { fields, styleGuide: snapshot.style_guide, terms, glossary };
}

/** The stable part of a snapshot: a change to it means the candidate really changed. */
export function snapshotSignature(snapshot: ReconSnapshot): string {
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

export function basisLabel(basis: string): string {
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

export function basisClass(basis: string): string {
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

export function composeStyleGuide(fields: Record<FieldKey, FieldForm>): string {
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

export function formToRequest(
  projectId: string,
  snapshot: ReconSnapshot,
  form: FormState,
): ReconConfirmRequest {
  const fields = form.fields;
  const profile: BookProfile = {
    source_language: {
      value: fields.source_language.value.trim(),
      basis: fields.source_language.basis,
    },
    genre: { value: fields.genre.value.trim(), basis: fields.genre.basis },
    audience: { value: fields.audience.value.trim(), basis: fields.audience.basis },
    era: { value: fields.era.value.trim(), basis: fields.era.basis },
    narrative_voice: {
      value: fields.narrative_voice.value.trim(),
      basis: fields.narrative_voice.basis,
    },
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

/** Immutable updates for the editable form, kept here so the panel reads as actions only. */
export function fieldFormPatch(
  state: FormState,
  key: FieldKey,
  patch: Partial<FieldForm>,
): FormState {
  return {
    ...state,
    fields: { ...state.fields, [key]: { ...state.fields[key], ...patch } },
  };
}

export function termPatch(state: FormState, index: number, patch: Partial<TermForm>): FormState {
  return {
    ...state,
    terms: state.terms.map((term, position) =>
      position === index ? { ...term, ...patch } : term,
    ),
  };
}

export function glossaryRowPatch(
  state: FormState,
  index: number,
  patch: Partial<GlossaryRowForm>,
): FormState {
  return {
    ...state,
    glossary: state.glossary.map((row, position) =>
      position === index ? { ...row, ...patch } : row,
    ),
  };
}
