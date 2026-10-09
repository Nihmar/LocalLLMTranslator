/** Pieces shared by the models page components (`PLAN.md` §11.2). */

import { toErrorMessage } from "../../lib/ipc";
import type { Endpoint, JsonObject, JsonValue, Role } from "../../lib/types";

export const ROLES: ReadonlyArray<{ value: Role; label: string; description: string }> = [
  { value: "translator", label: "Traduttore", description: "Traduce i chunk di prosa e tabelle." },
  { value: "editor", label: "Editor bilingue", description: "Confronta sorgente e traduzione e propone correzioni." },
  { value: "proofreader", label: "Proofreader", description: "Rifinisce il testo nella sola lingua di arrivo." },
  { value: "orchestrator", label: "Orchestratore", description: "Ricognizione del libro, riassunti, memoria di progetto, termini candidati." },
];

export const DEFAULT_PARAMS = '{\n  "temperature": 0.2,\n  "top_p": 0.95\n}';

export interface EndpointFormState {
  id: string | null;
  name: string;
  base_url: string;
  api_key_ref: string;
  max_concurrency: string;
  notes: string;
}

export interface EndpointFormErrors {
  name?: string | undefined;
  base_url?: string | undefined;
  max_concurrency?: string | undefined;
}

export interface BindingFormState {
  endpoint_id: string;
  role: Role;
  model: string;
  params: string;
  priority: string;
}

export const EMPTY_ENDPOINT_FORM: EndpointFormState = {
  id: null,
  name: "",
  base_url: "http://127.0.0.1:8080",
  api_key_ref: "",
  max_concurrency: "",
  notes: "",
};

/** Structural validation of an untrusted JSON document, so nothing is asserted blindly. */
function isJsonValue(value: unknown): value is JsonValue {
  if (value === null) {
    return true;
  }
  const kind = typeof value;
  if (kind === "string" || kind === "number" || kind === "boolean") {
    return true;
  }
  if (Array.isArray(value)) {
    return value.every((item: unknown) => isJsonValue(item));
  }
  if (kind === "object") {
    const record = value as Record<string, unknown>;
    return Object.values(record).every((item) => isJsonValue(item));
  }
  return false;
}

function isJsonObject(value: unknown): value is JsonObject {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return Object.values(record).every((item) => isJsonValue(item));
}

export function parseJsonObject(text: string): { value: JsonObject | null; error: string | null } {
  const trimmed = text.trim();
  if (trimmed.length === 0) {
    return { value: {}, error: null };
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (parseError) {
    return { value: null, error: `JSON non valido: ${toErrorMessage(parseError)}` };
  }
  if (!isJsonObject(parsed)) {
    return { value: null, error: "Le proprietà devono essere un oggetto JSON di valori serializzabili." };
  }
  return { value: parsed, error: null };
}

export function roleLabel(role: string): string {
  return ROLES.find((entry) => entry.value === role)?.label ?? role;
}

export function healthStatus(endpoint: Endpoint): string {
  if (endpoint.last_health_ok === null) {
    return "untested";
  }
  return endpoint.last_health_ok ? "ok" : "unreachable";
}
