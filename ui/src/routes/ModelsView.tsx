import { useCallback, useEffect, useMemo, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { FormField } from "../components/FormField";
import { ResourceGauge } from "../components/ResourceGauge";
import { QuickModelSetup } from "../components/QuickModelSetup";
import { StatusBadge } from "../components/StatusBadge";
import { onMetricsTick } from "../lib/events";
import { formatDateTime, formatNumber, formatRelative, truncate } from "../lib/format";
import {
  endpointDelete,
  endpointList,
  endpointModels,
  endpointTest,
  endpointUpsert,
  metricsGet,
  roleBindingList,
  roleBindingSet,
  roleBindingDelete,
  toErrorMessage,
} from "../lib/ipc";
import type {
  Endpoint,
  EndpointTestResult,
  JsonObject,
  JsonValue,
  Metrics,
  ModelInfo,
  Role,
  RoleBinding,
} from "../lib/types";

/**
 * Models page (`PLAN.md` §11.2): endpoint CRUD, health test, model list and role assignment,
 * plus the VRAM/queue indicator.
 *
 * The secret never reaches this page: `api_key_ref` is the *name* of the entry in the OS keyring
 * (`PLAN.md` §5, "Nessun segreto nel database").
 *
 * The view is endpoint/role-scoped, not project-scoped: it takes no props.
 */

const ROLES: ReadonlyArray<{ value: Role; label: string; description: string }> = [
  { value: "translator", label: "Traduttore", description: "Traduce i chunk di prosa e tabelle." },
  { value: "editor", label: "Editor bilingue", description: "Confronta sorgente e traduzione e propone correzioni." },
  { value: "proofreader", label: "Proofreader", description: "Rifinisce il testo nella sola lingua di arrivo." },
  { value: "orchestrator", label: "Orchestratore", description: "Ricognizione del libro, riassunti, memoria di progetto, termini candidati." },
];

const DEFAULT_PARAMS = '{\n  "temperature": 0.2,\n  "top_p": 0.95\n}';

interface EndpointFormState {
  id: string | null;
  name: string;
  base_url: string;
  api_key_ref: string;
  max_concurrency: string;
  notes: string;
}

interface EndpointFormErrors {
  name?: string | undefined;
  base_url?: string | undefined;
  max_concurrency?: string | undefined;
}

interface BindingFormState {
  endpoint_id: string;
  role: Role;
  model: string;
  params: string;
  priority: string;
}

const EMPTY_ENDPOINT_FORM: EndpointFormState = {
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

function parseJsonObject(text: string): { value: JsonObject | null; error: string | null } {
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

function roleLabel(role: string): string {
  return ROLES.find((entry) => entry.value === role)?.label ?? role;
}

function healthStatus(endpoint: Endpoint): string {
  if (endpoint.last_health_ok === null) {
    return "untested";
  }
  return endpoint.last_health_ok ? "ok" : "unreachable";
}

export function ModelsView() {
  const [endpoints, setEndpoints] = useState<Endpoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<EndpointFormState>(EMPTY_ENDPOINT_FORM);
  const [formErrors, setFormErrors] = useState<EndpointFormErrors>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const [detailsId, setDetailsId] = useState<string | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, EndpointTestResult>>({});
  const [models, setModels] = useState<Record<string, ModelInfo[]>>({});
  const [modelsLoading, setModelsLoading] = useState<string | null>(null);
  const [modelsError, setModelsError] = useState<string | null>(null);

  const [bindings, setBindings] = useState<RoleBinding[]>([]);
  const [bindingsError, setBindingsError] = useState<string | null>(null);
  const [bindingForm, setBindingForm] = useState<BindingFormState>({
    endpoint_id: "",
    role: "translator",
    model: "",
    params: DEFAULT_PARAMS,
    priority: "0",
  });
  const [bindingError, setBindingError] = useState<string | null>(null);
  const [savingBinding, setSavingBinding] = useState(false);
  // Two-step removal, like the endpoint table: losing an assignment by mistake means re-picking
  // endpoint and model, so the row asks once.
  const [confirmRemoveBindingId, setConfirmRemoveBindingId] = useState<string | null>(null);

  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [metricsLoading, setMetricsLoading] = useState(true);
  const [metricsError, setMetricsError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const rows = await endpointList();
      setEndpoints(rows);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setEndpoints([]);
    } finally {
      setLoading(false);
    }
  }, []);

  const loadBindings = useCallback(async () => {
    setBindingsError(null);
    try {
      const rows = await roleBindingList();
      setBindings(rows);
    } catch (loadError) {
      setBindingsError(toErrorMessage(loadError));
      setBindings([]);
    }
  }, []);

  const refreshMetrics = useCallback(async () => {
    try {
      const snapshot = await metricsGet();
      setMetrics(snapshot);
      setMetricsError(null);
    } catch (loadError) {
      setMetricsError(toErrorMessage(loadError));
    } finally {
      setMetricsLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
    void loadBindings();
  }, [load, loadBindings]);

  useEffect(() => {
    setMetricsLoading(true);
    void refreshMetrics();
  }, [refreshMetrics]);

  // `metrics://tick` is a trigger, not the full snapshot: refetch through `metrics_get`.
  useEffect(() => onMetricsTick(() => {
    void refreshMetrics();
  }), [refreshMetrics]);

  const detailsEndpoint = useMemo(
    () => endpoints.find((endpoint) => endpoint.id === detailsId) ?? null,
    [endpoints, detailsId],
  );

  const detailsModels = detailsId === null ? undefined : models[detailsId];
  const detailsTest = detailsId === null ? undefined : testResults[detailsId];

  async function loadModelsFor(endpointId: string) {
    setModelsLoading(endpointId);
    setModelsError(null);
    try {
      const rows = await endpointModels({ endpoint_id: endpointId });
      setModels((current) => ({ ...current, [endpointId]: rows }));
    } catch (loadError) {
      setModelsError(toErrorMessage(loadError));
    } finally {
      setModelsLoading(null);
    }
  }

  async function handleOpenDetails(endpointId: string) {
    setDetailsId(endpointId);
    setModelsError(null);
    setBindingForm((current) => ({ ...current, endpoint_id: endpointId }));
    if (models[endpointId] === undefined) {
      await loadModelsFor(endpointId);
    }
  }

  async function handleTest(endpointId: string) {
    setTesting(endpointId);
    setError(null);
    try {
      const result = await endpointTest(endpointId);
      setTestResults((current) => ({ ...current, [endpointId]: result }));
      setEndpoints((current) =>
        current.map((endpoint) =>
          endpoint.id === endpointId
            ? {
                ...endpoint,
                last_health_at: result.health.checked_at,
                last_health_ok: result.health.ok,
                props_json: result.props === null ? endpoint.props_json : JSON.stringify(result.props),
              }
            : endpoint,
        ),
      );
    } catch (testError) {
      setTestResults((current) => ({
        ...current,
        [endpointId]: {
          health: { ok: false, status: toErrorMessage(testError), code: 0, checked_at: new Date().toISOString() },
          props: null,
          models: [],
        },
      }));
    } finally {
      setTesting(null);
    }
  }

  function openEditForm(endpoint: Endpoint) {
    setForm({
      id: endpoint.id,
      name: endpoint.name,
      base_url: endpoint.base_url,
      api_key_ref: endpoint.api_key_ref ?? "",
      max_concurrency: endpoint.max_concurrency === null ? "" : String(endpoint.max_concurrency),
      notes: endpoint.notes ?? "",
    });
    setFormErrors({});
    setFormError(null);
    setFormOpen(true);
  }

  function validateEndpoint(state: EndpointFormState): EndpointFormErrors {
    const errors: EndpointFormErrors = {};
    if (state.name.trim().length === 0) {
      errors.name = "Indica un nome per l'endpoint.";
    }
    if (state.base_url.trim().length === 0) {
      errors.base_url = "Indica l'URL di base di llama-server.";
    } else if (!/^https?:\/\//.test(state.base_url.trim())) {
      errors.base_url = "L'URL deve iniziare con http:// o https://.";
    }
    if (state.max_concurrency.trim().length > 0) {
      const parsed = Number.parseInt(state.max_concurrency.trim(), 10);
      if (Number.isNaN(parsed) || parsed < 1) {
        errors.max_concurrency = "Deve essere un intero maggiore di zero.";
      }
    }
    return errors;
  }

  async function handleSaveEndpoint() {
    const errors = validateEndpoint(form);
    setFormErrors(errors);
    setFormError(null);
    if (Object.keys(errors).length > 0) {
      return;
    }

    setSaving(true);
    try {
      const maxConcurrency = form.max_concurrency.trim();
      const saved = await endpointUpsert({
        id: form.id,
        name: form.name.trim(),
        base_url: form.base_url.trim().replace(/\/+$/, ""),
        api_key_ref: form.api_key_ref.trim().length > 0 ? form.api_key_ref.trim() : null,
        max_concurrency: maxConcurrency.length > 0 ? Number.parseInt(maxConcurrency, 10) : null,
        notes: form.notes.trim().length > 0 ? form.notes.trim() : null,
      });
      setEndpoints((current) => {
        const exists = current.some((endpoint) => endpoint.id === saved.id);
        return exists
          ? current.map((endpoint) => (endpoint.id === saved.id ? saved : endpoint))
          : [...current, saved];
      });
      setForm(EMPTY_ENDPOINT_FORM);
      setFormOpen(false);
    } catch (saveError) {
      setFormError(toErrorMessage(saveError));
    } finally {
      setSaving(false);
    }
  }

  async function handleDeleteEndpoint(endpointId: string) {
    setError(null);
    try {
      await endpointDelete(endpointId);
      setEndpoints((current) => current.filter((endpoint) => endpoint.id !== endpointId));
      setBindings((current) => current.filter((binding) => binding.endpoint_id !== endpointId));
      setConfirmDeleteId(null);
      if (detailsId === endpointId) {
        setDetailsId(null);
      }
    } catch (deleteError) {
      setError(toErrorMessage(deleteError));
    }
  }

  async function handleRemoveBinding(bindingId: string) {
    setBindingsError(null);
    try {
      await roleBindingDelete(bindingId);
      setBindings((current) => current.filter((binding) => binding.id !== bindingId));
      setConfirmRemoveBindingId(null);
    } catch (removeError) {
      setBindingsError(toErrorMessage(removeError));
    }
  }

  async function handleSaveBinding() {
    const parsed = parseJsonObject(bindingForm.params);
    if (parsed.error !== null || parsed.value === null) {
      setBindingError(parsed.error ?? "Proprietà non valide.");
      return;
    }
    if (bindingForm.endpoint_id.length === 0) {
      setBindingError("Seleziona un endpoint da associare al ruolo.");
      return;
    }
    if (bindingForm.model.trim().length === 0) {
      setBindingError("Indica il modello da usare per questo ruolo.");
      return;
    }
    const priority = Number.parseInt(bindingForm.priority.trim(), 10);

    setSavingBinding(true);
    setBindingError(null);
    try {
      const saved = await roleBindingSet({
        endpoint_id: bindingForm.endpoint_id,
        role: bindingForm.role,
        model: bindingForm.model.trim(),
        params: parsed.value,
        priority: Number.isNaN(priority) ? 0 : priority,
      });
      setBindings((current) => {
        const rest = current.filter(
          (binding) =>
            !(binding.endpoint_id === saved.endpoint_id && binding.role === saved.role),
        );
        return [...rest, saved].sort((left, right) => right.priority - left.priority);
      });
    } catch (saveError) {
      setBindingError(toErrorMessage(saveError));
    } finally {
      setSavingBinding(false);
    }
  }

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h1 className="font-serif text-2xl font-medium text-ink">Modelli</h1>
          <p className="mt-0.5 text-xs text-muted">
            L&apos;app non avvia llama-server: rileva gli endpoint che hai già in esecuzione, ne
            verifica la salute e legge slot, contesto e modelli disponibili.
          </p>
        </div>
        <button
          type="button"
          className="btn btn-primary"
          onClick={() => {
            setForm(EMPTY_ENDPOINT_FORM);
            setFormErrors({});
            setFormError(null);
            setFormOpen(true);
          }}
        >
          Nuovo endpoint
        </button>
      </div>

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="section-stack min-w-0">
          {formOpen ? (
            <form
              className="panel"
              onSubmit={(event) => {
                event.preventDefault();
                void handleSaveEndpoint();
              }}
            >
              <div className="panel-head">
                <span className="panel-title">
                  {form.id === null ? "Nuovo endpoint" : "Modifica endpoint"}
                </span>
                <button
                  type="button"
                  className="btn btn-sm btn-ghost"
                  onClick={() => {
                    setFormOpen(false);
                    setForm(EMPTY_ENDPOINT_FORM);
                  }}
                >
                  Chiudi
                </button>
              </div>

              <div className="panel-pad section-stack">
                <div className="grid grid-cols-2 gap-3">
                  <FormField label="Nome" htmlFor="endpoint-name" required error={formErrors.name}>
                    <input
                      id="endpoint-name"
                      className="input"
                      value={form.name}
                      onChange={(event) => {
                        setForm((current) => ({ ...current, name: event.target.value }));
                      }}
                      placeholder="traduttore-principale"
                    />
                  </FormField>

                  <FormField
                    label="Concorrenza massima"
                    htmlFor="endpoint-concurrency"
                    hint="Lasciare vuoto per usare gli slot dichiarati dal server."
                    error={formErrors.max_concurrency}
                  >
                    <input
                      id="endpoint-concurrency"
                      className="input"
                      inputMode="numeric"
                      value={form.max_concurrency}
                      onChange={(event) => {
                        setForm((current) => ({ ...current, max_concurrency: event.target.value }));
                      }}
                      placeholder="4"
                    />
                  </FormField>
                </div>

                <FormField
                  label="URL di base"
                  htmlFor="endpoint-url"
                  required
                  hint="Es. http://127.0.0.1:8080 — su questa base vengono chiamati /health, /props e /v1/models."
                  error={formErrors.base_url}
                >
                  <input
                    id="endpoint-url"
                    className="input"
                    value={form.base_url}
                    spellCheck={false}
                    onChange={(event) => {
                      setForm((current) => ({ ...current, base_url: event.target.value }));
                    }}
                  />
                </FormField>

                <div className="grid grid-cols-2 gap-3">
                  <FormField
                    label="Riferimento API key"
                    htmlFor="endpoint-keyref"
                    hint="Nome della voce nel keyring di sistema. Il segreto non viene mai salvato nel database."
                  >
                    <input
                      id="endpoint-keyref"
                      className="input"
                      value={form.api_key_ref}
                      spellCheck={false}
                      onChange={(event) => {
                        setForm((current) => ({ ...current, api_key_ref: event.target.value }));
                      }}
                      placeholder="llmtranslator/translator"
                    />
                  </FormField>

                  <FormField label="Note" htmlFor="endpoint-notes">
                    <input
                      id="endpoint-notes"
                      className="input"
                      value={form.notes}
                      onChange={(event) => {
                        setForm((current) => ({ ...current, notes: event.target.value }));
                      }}
                      placeholder="Qwen2.5 32B, -c 32768 --parallel 4"
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
                  <button type="submit" className="btn btn-primary" disabled={saving}>
                    {saving ? <span className="spinner" aria-hidden="true" /> : null}
                    {form.id === null ? "Crea endpoint" : "Salva modifiche"}
                  </button>
                </div>
              </div>
            </form>
          ) : null}

          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Endpoint configurati</span>
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                disabled={loading}
                onClick={() => {
                  void load();
                }}
              >
                Aggiorna
              </button>
            </div>

            {loading ? (
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Lettura degli endpoint…" />
              </div>
            ) : error !== null ? (
              <div className="panel-pad">
                <EmptyState
                  tone="error"
                  compact
                  title="Impossibile leggere gli endpoint"
                  details={error}
                  actionLabel="Riprova"
                  onAction={() => {
                    void load();
                  }}
                />
              </div>
            ) : endpoints.length === 0 ? (
              <div className="panel-pad">
                <EmptyState
                  compact
                  title="Nessun endpoint"
                  description="Avvia un llama-server e registralo qui: senza un endpoint il ruolo traduttore non ha nulla da chiamare."
                  actionLabel="Aggiungi endpoint"
                  onAction={() => {
                    setForm(EMPTY_ENDPOINT_FORM);
                    setFormOpen(true);
                  }}
                />
              </div>
            ) : (
              <div className="table-scroll">
                <table className="data-table">
                  <thead>
                    <tr>
                      <th style={{ minWidth: "10rem" }}>Nome</th>
                      <th style={{ minWidth: "14rem" }}>URL di base</th>
                      <th style={{ width: "11rem" }}>Salute</th>
                      <th style={{ width: "7rem" }} className="num">
                        Slot
                      </th>
                      <th style={{ width: "12rem" }}>Azioni</th>
                    </tr>
                  </thead>
                  <tbody>
                    {endpoints.map((endpoint) => {
                      const result = testResults[endpoint.id];
                      const isTesting = testing === endpoint.id;
                      const slots =
                        result === undefined || result.props === null ? null : result.props.total_slots;

                      return (
                        <tr key={endpoint.id} data-selected={detailsId === endpoint.id ? "true" : "false"}>
                          <td>
                            <span className="block truncate font-medium text-ink" title={endpoint.name}>
                              {endpoint.name}
                            </span>
                            {endpoint.notes !== null ? (
                              <span className="block truncate text-[0.68rem] text-faint" title={endpoint.notes}>
                                {truncate(endpoint.notes, 40)}
                              </span>
                            ) : null}
                          </td>
                          <td className="font-mono text-[0.72rem] text-muted">{endpoint.base_url}</td>
                          <td>
                            <span className="flex items-center gap-1">
                              <StatusBadge
                                status={healthStatus(endpoint)}
                                pulse={isTesting}
                                label={
                                  isTesting
                                    ? "Verifica…"
                                    : endpoint.last_health_ok === null
                                      ? "Non verificato"
                                      : endpoint.last_health_ok
                                        ? "Raggiungibile"
                                        : "Non raggiungibile"
                                }
                              />
                            </span>
                            <span className="text-[0.68rem] text-faint">
                              {endpoint.last_health_at === null
                                ? "mai verificato"
                                : formatRelative(endpoint.last_health_at)}
                            </span>
                          </td>
                          <td className="num">
                            {slots !== null
                              ? formatNumber(slots)
                              : endpoint.max_concurrency === null
                                ? "auto"
                                : formatNumber(endpoint.max_concurrency)}
                          </td>
                          <td>
                            <span className="flex flex-wrap gap-1">
                              <button
                                type="button"
                                className="btn btn-sm"
                                disabled={isTesting}
                                onClick={() => {
                                  void handleTest(endpoint.id);
                                }}
                              >
                                Verifica
                              </button>
                              <button
                                type="button"
                                className="btn btn-sm btn-ghost"
                                onClick={() => {
                                  void handleOpenDetails(endpoint.id);
                                }}
                              >
                                Dettagli
                              </button>
                              <button
                                type="button"
                                className="btn btn-sm btn-ghost"
                                onClick={() => {
                                  openEditForm(endpoint);
                                }}
                              >
                                Modifica
                              </button>
                              {confirmDeleteId === endpoint.id ? (
                                <>
                                  <button
                                    type="button"
                                    className="btn btn-sm btn-danger"
                                    onClick={() => {
                                      void handleDeleteEndpoint(endpoint.id);
                                    }}
                                  >
                                    Conferma
                                  </button>
                                  <button
                                    type="button"
                                    className="btn btn-sm btn-ghost"
                                    onClick={() => {
                                      setConfirmDeleteId(null);
                                    }}
                                  >
                                    No
                                  </button>
                                </>
                              ) : (
                                <button
                                  type="button"
                                  className="btn btn-sm btn-ghost"
                                  onClick={() => {
                                    setConfirmDeleteId(endpoint.id);
                                  }}
                                >
                                  Elimina
                                </button>
                              )}
                            </span>
                          </td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            )}
          </div>

          {detailsEndpoint !== null ? (
            <div className="panel">
              <div className="panel-head">
                <span className="panel-title">
                  Dettagli · {detailsEndpoint.name}
                </span>
                <span className="flex items-center gap-2">
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={modelsLoading === detailsEndpoint.id}
                    onClick={() => {
                      void loadModelsFor(detailsEndpoint.id);
                    }}
                  >
                    {modelsLoading === detailsEndpoint.id ? (
                      <span className="spinner" aria-hidden="true" />
                    ) : null}
                    Ricarica modelli
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => {
                      setDetailsId(null);
                    }}
                  >
                    Chiudi
                  </button>
                </span>
              </div>

              <div className="panel-pad section-stack">
                {detailsTest === undefined ? (
                  <p className="text-xs text-muted">
                    Nessuna verifica recente. Usa «Verifica» per leggere <span className="mono-chip">/health</span>{" "}
                    e <span className="mono-chip">/props</span>.
                  </p>
                ) : (
                  <>
                    <div
                      className={detailsTest.health.ok ? "banner banner-ok" : "banner banner-error"}
                      role="status"
                    >
                      <span aria-hidden="true">{detailsTest.health.ok ? "✓" : "⚠"}</span>
                      <span>
                        {detailsTest.health.status ??
                          (detailsTest.health.ok ? "Raggiungibile" : "Non raggiungibile")}
                      </span>
                    </div>
                    <div className="grid grid-cols-3 gap-2">
                      <div className="stat-tile">
                        <div className="stat-label">Slot totali</div>
                        <div className="stat-value">
                          {detailsTest.props === null || detailsTest.props.total_slots === null
                            ? "—"
                            : formatNumber(detailsTest.props.total_slots)}
                        </div>
                      </div>
                      <div className="stat-tile">
                        <div className="stat-label">Contesto (n_ctx)</div>
                        <div className="stat-value">
                          {detailsTest.props === null || detailsTest.props.n_ctx === null
                            ? "—"
                            : formatNumber(detailsTest.props.n_ctx)}
                        </div>
                      </div>
                      <div className="stat-tile">
                        <div className="stat-label">Codice HTTP</div>
                        <div className="stat-value">{formatNumber(detailsTest.health.code)}</div>
                      </div>
                    </div>
                    {detailsTest.props !== null && detailsTest.props.model_path !== null ? (
                      <p className="truncate font-mono text-[0.72rem] text-muted" title={detailsTest.props.model_path}>
                        {detailsTest.props.model_path}
                      </p>
                    ) : null}
                  </>
                )}

                {modelsError !== null ? (
                  <div className="banner banner-error" role="alert">
                    <span aria-hidden="true">⚠</span>
                    <span>{modelsError}</span>
                  </div>
                ) : null}

                <div>
                  <div className="stat-label mb-1">
                    Modelli da /v1/models
                  </div>
                  {modelsLoading === detailsEndpoint.id && detailsModels === undefined ? (
                    <p className="flex items-center gap-2 text-xs text-muted">
                      <span className="spinner" aria-hidden="true" />
                      Lettura dei modelli…
                    </p>
                  ) : detailsModels === undefined || detailsModels.length === 0 ? (
                    <p className="text-xs text-muted">
                      Nessun modello esposto. Il campo resta libero: puoi scrivere il nome del modello a
                      mano quando associ un ruolo.
                    </p>
                  ) : (
                    <ul className="flex flex-wrap gap-1">
                      {detailsModels.map((model: ModelInfo) => (
                        <li key={model.id}>
                          <span className="mono-chip" title={model.owned_by ?? model.id}>
                            {truncate(model.id, 42)}
                          </span>
                        </li>
                      ))}
                    </ul>
                  )}
                </div>

                <p className="text-[0.72rem] text-faint">
                  Ultima verifica registrata: {formatDateTime(detailsEndpoint.last_health_at)}
                </p>
              </div>
            </div>
          ) : null}

          <QuickModelSetup
            endpoints={endpoints}
            bindings={bindings}
            onApplied={(saved) => {
              setBindings((current) => {
                const rest = current.filter(
                  (binding) =>
                    !saved.some(
                      (row) => row.endpoint_id === binding.endpoint_id && row.role === binding.role,
                    ),
                );
                return [...rest, ...saved].sort((left, right) => right.priority - left.priority);
              });
            }}
          />

          <details className="panel">
            <summary className="panel-head cursor-pointer">
              <span className="panel-title">Avanzate: un modello per ruolo e parametri</span>
              <button
                type="button"
                className="btn btn-sm btn-ghost"
                onClick={() => {
                  void loadBindings();
                }}
              >
                Aggiorna
              </button>
            </summary>

            <div className="panel-pad section-stack">
              {bindingsError !== null ? (
                <div className="banner banner-error" role="alert">
                  <span aria-hidden="true">⚠</span>
                  <span>{bindingsError}</span>
                </div>
              ) : null}

              {bindings.length === 0 ? (
                <EmptyState
                  compact
                  title="Nessun ruolo assegnato"
                  description="Il ruolo traduttore è obbligatorio per avviare una traduzione; l'orchestratore serve alla ricognizione del libro, editor e proofreader dalla milestone M4."
                />
              ) : (
                <div className="table-scroll">
                  <table className="data-table">
                    <thead>
                      <tr>
                        <th style={{ width: "11rem" }}>Ruolo</th>
                        <th>Modello</th>
                        <th style={{ minWidth: "10rem" }}>Endpoint</th>
                        <th style={{ width: "5rem" }} className="num">
                          Priorità
                        </th>
                        <th style={{ width: "8rem" }} />
                      </tr>
                    </thead>
                    <tbody>
                      {bindings.map((binding) => {
                        const endpoint = endpoints.find((item) => item.id === binding.endpoint_id);
                        return (
                          <tr key={binding.id}>
                            <td className="text-ink-soft">{roleLabel(binding.role)}</td>
                            <td>
                              <span className="mono-chip" title={binding.model}>
                                {truncate(binding.model, 34)}
                              </span>
                            </td>
                            <td className="text-muted">
                              {endpoint === undefined ? binding.endpoint_id : endpoint.name}
                            </td>
                            <td className="num">{formatNumber(binding.priority)}</td>
                            <td className="text-right">
                              {confirmRemoveBindingId === binding.id ? (
                                <span className="flex flex-wrap justify-end gap-1">
                                  <button
                                    type="button"
                                    className="btn btn-sm btn-danger"
                                    onClick={() => {
                                      void handleRemoveBinding(binding.id);
                                    }}
                                  >
                                    Conferma
                                  </button>
                                  <button
                                    type="button"
                                    className="btn btn-sm btn-ghost"
                                    onClick={() => {
                                      setConfirmRemoveBindingId(null);
                                    }}
                                  >
                                    No
                                  </button>
                                </span>
                              ) : (
                                <button
                                  type="button"
                                  className="btn btn-sm btn-ghost"
                                  title="Toglie questo ruolo dal modello scelto"
                                  onClick={() => {
                                    setConfirmRemoveBindingId(binding.id);
                                  }}
                                >
                                  Rimuovi
                                </button>
                              )}
                            </td>
                          </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              )}

              <div className="grid grid-cols-2 gap-3">
                <FormField label="Ruolo" htmlFor="binding-role">
                  <select
                    id="binding-role"
                    className="select"
                    value={bindingForm.role}
                    onChange={(event) => {
                      const matched = ROLES.find((role) => role.value === event.target.value);
                      if (matched !== undefined) {
                        setBindingForm((current) => ({ ...current, role: matched.value }));
                      }
                    }}
                  >
                    {ROLES.map((role) => (
                      <option key={role.value} value={role.value}>
                        {role.label} — {role.description}
                      </option>
                    ))}
                  </select>
                </FormField>

                <FormField label="Endpoint" htmlFor="binding-endpoint">
                  <select
                    id="binding-endpoint"
                    className="select"
                    value={bindingForm.endpoint_id}
                    onChange={(event) => {
                      const value = event.target.value;
                      setBindingForm((current) => ({ ...current, endpoint_id: value }));
                      if (value.length > 0 && models[value] === undefined) {
                        void loadModelsFor(value);
                      }
                    }}
                  >
                    <option value="">— seleziona —</option>
                    {endpoints.map((endpoint) => (
                      <option key={endpoint.id} value={endpoint.id}>
                        {endpoint.name}
                      </option>
                    ))}
                  </select>
                </FormField>

                <FormField
                  label="Modello"
                  htmlFor="binding-model"
                  hint="Nome del modello come lo espone l'endpoint."
                >
                  <input
                    id="binding-model"
                    className="input"
                    list="binding-model-options"
                    value={bindingForm.model}
                    spellCheck={false}
                    onChange={(event) => {
                      setBindingForm((current) => ({ ...current, model: event.target.value }));
                    }}
                    placeholder="qwen2.5-32b-instruct-q5_k_m"
                  />
                  <datalist id="binding-model-options">
                    {(bindingForm.endpoint_id.length === 0
                      ? []
                      : (models[bindingForm.endpoint_id] ?? [])
                    ).map((model) => (
                      <option key={model.id} value={model.id} />
                    ))}
                  </datalist>
                </FormField>

                <FormField
                  label="Priorità"
                  htmlFor="binding-priority"
                  hint="A parità di ruolo vince la priorità più alta."
                >
                  <input
                    id="binding-priority"
                    className="input"
                    inputMode="numeric"
                    value={bindingForm.priority}
                    onChange={(event) => {
                      setBindingForm((current) => ({ ...current, priority: event.target.value }));
                    }}
                  />
                </FormField>
              </div>

              <FormField
                label="Parametri di generazione"
                htmlFor="binding-params"
                hint="Oggetto JSON passato a llama-server (temperature, top_p, seed, max_tokens, grammar, chat_template_kwargs…)."
                error={bindingError}
              >
                <textarea
                  id="binding-params"
                  className="textarea"
                  value={bindingForm.params}
                  spellCheck={false}
                  onChange={(event) => {
                    setBindingForm((current) => ({ ...current, params: event.target.value }));
                  }}
                />
              </FormField>

              <div className="flex items-center gap-2">
                <button
                  type="button"
                  className="btn btn-primary"
                  disabled={savingBinding}
                  onClick={() => {
                    void handleSaveBinding();
                  }}
                >
                  {savingBinding ? <span className="spinner" aria-hidden="true" /> : null}
                  Assegna ruolo
                </button>
              </div>
            </div>
          </details>
        </div>

        <div className="section-stack">
          <ResourceGauge metrics={metrics} loading={metricsLoading} />

          {metricsError !== null ? (
            <div className="banner banner-error" role="alert">
              <span aria-hidden="true">⚠</span>
              <span>{metricsError}</span>
            </div>
          ) : null}

          <div className="panel panel-pad">
            <div className="panel-title mb-2">Come sono usati i ruoli</div>
            <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
              <li>
                Il limite di concorrenza per endpoint è{" "}
                <span className="mono-chip">min(concorrenza, slot totali)</span>.
              </li>
              <li>
                «Rimuovi» toglie un&apos;assegnazione: il ruolo resta non assegnato finché non ne
                scegli un&apos;altra, e l&apos;endpoint torna libero per altri ruoli.
              </li>
              <li>
                Il budget di contesto viene letto da <span className="mono-chip">/props</span>, non
                dalla configurazione salvata qui.
              </li>
              <li>
                Le API key restano nel keyring: qui si salva solo il nome della voce.
              </li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  );
}
