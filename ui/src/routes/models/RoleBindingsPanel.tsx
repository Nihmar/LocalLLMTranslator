import { useEffect, useState } from "react";
import { EmptyState } from "../../components/EmptyState";
import { FormField } from "../../components/FormField";
import { formatNumber, truncate } from "../../lib/format";
import { roleBindingDelete, roleBindingSet, toErrorMessage } from "../../lib/ipc";
import type { Endpoint, ModelInfo, RoleBinding } from "../../lib/types";
import {
  DEFAULT_PARAMS,
  parseJsonObject,
  roleLabel,
  ROLES,
  type BindingFormState,
} from "./shared";

/** "Avanzate": one binding per role and endpoint, with priority and generation parameters. */
export function RoleBindingsPanel({
  endpoints,
  bindings,
  bindingsError,
  models,
  presetEndpointId,
  onLoadModels,
  onReload,
  onSaved,
  onRemoved,
}: {
  endpoints: readonly Endpoint[];
  bindings: readonly RoleBinding[];
  bindingsError: string | null;
  models: Readonly<Record<string, ModelInfo[]>>;
  /** The endpoint whose details are open: the form starts from it. */
  presetEndpointId: string | null;
  onLoadModels: (endpointId: string) => Promise<void>;
  onReload: () => Promise<void>;
  onSaved: (saved: RoleBinding[]) => void;
  onRemoved: (bindingId: string) => void;
}) {
  const [bindingForm, setBindingForm] = useState<BindingFormState>({
    endpoint_id: presetEndpointId ?? "",
    role: "translator",
    model: "",
    params: DEFAULT_PARAMS,
    priority: "0",
  });
  const [bindingError, setBindingError] = useState<string | null>(null);
  const [savingBinding, setSavingBinding] = useState(false);
  const [removeError, setRemoveError] = useState<string | null>(null);
  // Two-step removal, like the endpoint table: losing an assignment by mistake means re-picking
  // endpoint and model, so the row asks once.
  const [confirmRemoveBindingId, setConfirmRemoveBindingId] = useState<string | null>(null);
  const shownBindingsError = removeError ?? bindingsError;

  useEffect(() => {
    if (presetEndpointId !== null) {
      setBindingForm((current) => ({ ...current, endpoint_id: presetEndpointId }));
    }
  }, [presetEndpointId]);

  async function handleRemoveBinding(bindingId: string) {
    setRemoveError(null);
    try {
      await roleBindingDelete(bindingId);
      onRemoved(bindingId);
      setConfirmRemoveBindingId(null);
    } catch (removeError) {
      setRemoveError(toErrorMessage(removeError));
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
      onSaved([saved]);
    } catch (saveError) {
      setBindingError(toErrorMessage(saveError));
    } finally {
      setSavingBinding(false);
    }
  }

  return (
    <details className="panel">
      <summary className="panel-head cursor-pointer">
        <span className="panel-title">Avanzate: un modello per ruolo e parametri</span>
        <button
          type="button"
          className="btn btn-sm btn-ghost"
          onClick={() => {
            void onReload();
          }}
        >
          Aggiorna
        </button>
      </summary>

      <div className="panel-pad section-stack">
        {shownBindingsError !== null ? (
          <div className="banner banner-error" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>{shownBindingsError}</span>
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
                  void onLoadModels(value);
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
  );
}
