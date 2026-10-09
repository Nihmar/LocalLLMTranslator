import { useEffect, useState } from "react";
import { endpointModels, roleBindingSet, toErrorMessage } from "../lib/ipc";
import type { Endpoint, ModelInfo, Role, RoleBinding } from "../lib/types";

/**
 * "Configurazione rapida" of the models page (issue #14): one endpoint and one model for every
 * role, the setup most users want. The new bindings outrank the existing ones of each role
 * (priority one above the highest), so they take effect without deleting anything; the per-role
 * form below stays for a different model per role or custom parameters. Thinking is turned off
 * for the JSON roles by the control plane anyway (`chat_call::template_kwargs`).
 */

const ROLE_LABELS: ReadonlyArray<{ role: Role; label: string }> = [
  { role: "translator", label: "Traduttore" },
  { role: "editor", label: "Editor" },
  { role: "proofreader", label: "Proofreader" },
  { role: "orchestrator", label: "Orchestratore" },
];

const QUICK_PARAMS = { temperature: 0.2, top_p: 0.95 };

export interface QuickModelSetupProps {
  endpoints: readonly Endpoint[];
  bindings: readonly RoleBinding[];
  onApplied: (saved: RoleBinding[]) => void;
}

/** The binding a role uses today: the highest priority wins (`repo::role_binding_for`). */
function activeBinding(bindings: readonly RoleBinding[], role: Role): RoleBinding | undefined {
  return bindings
    .filter((binding) => binding.role === role)
    .sort((left, right) => right.priority - left.priority)[0];
}

export function QuickModelSetup({ endpoints, bindings, onApplied }: QuickModelSetupProps) {
  const [endpointId, setEndpointId] = useState(endpoints[0]?.id ?? "");
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [model, setModel] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    if (endpointId === "" && endpoints[0] !== undefined) {
      setEndpointId(endpoints[0].id);
    }
  }, [endpoints, endpointId]);

  useEffect(() => {
    if (endpointId === "") {
      return;
    }
    let cancelled = false;
    setModels([]);
    void endpointModels({ endpoint_id: endpointId })
      .then((rows) => {
        if (!cancelled) {
          setModels(rows);
          setModel((current) => (current === "" ? (rows[0]?.id ?? "") : current));
        }
      })
      .catch(() => {
        // An unreachable endpoint still accepts a typed model name.
      });
    return () => {
      cancelled = true;
    };
  }, [endpointId]);

  async function apply() {
    if (endpointId === "" || model.trim() === "") {
      setError("Scegli un endpoint e un modello.");
      return;
    }
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      const saved: RoleBinding[] = [];
      for (const { role } of ROLE_LABELS) {
        const top = activeBinding(bindings, role);
        const sameEndpoint = top !== undefined && top.endpoint_id === endpointId;
        saved.push(
          await roleBindingSet({
            role,
            endpoint_id: endpointId,
            model: model.trim(),
            params: QUICK_PARAMS,
            // Same endpoint: the existing row is updated in place. Another endpoint: outrank it.
            priority: top === undefined || sameEndpoint ? (top?.priority ?? 0) : top.priority + 1,
          }),
        );
      }
      onApplied(saved);
      setNotice(`Tutti i ruoli usano ora ${model.trim()}.`);
    } catch (applyError) {
      setError(toErrorMessage(applyError));
    } finally {
      setBusy(false);
    }
  }

  const endpointName = (id: string) => endpoints.find((endpoint) => endpoint.id === id)?.name ?? id;

  return (
    <section className="panel">
      <div className="panel-head">
        <span className="panel-title">Configurazione rapida</span>
      </div>
      <div className="panel-pad section-stack">
        <p className="text-sm text-muted">
          Un solo modello per tutti i ruoli: è la scelta giusta se hai un solo llama-server. Per
          modelli diversi per ruolo apri «Avanzate» qui sotto.
        </p>
        {endpoints.length === 0 ? (
          <p className="text-sm text-muted">Aggiungi prima un endpoint.</p>
        ) : (
          <div className="flex flex-wrap items-end gap-3">
            <label className="flex flex-col gap-1 text-sm font-medium text-ink-soft">
              Endpoint
              <select
                className="select"
                value={endpointId}
                onChange={(event) => {
                  setEndpointId(event.target.value);
                  setModel("");
                }}
              >
                {endpoints.map((endpoint) => (
                  <option key={endpoint.id} value={endpoint.id}>
                    {endpoint.name}
                  </option>
                ))}
              </select>
            </label>
            <label className="flex min-w-[16rem] flex-1 flex-col gap-1 text-sm font-medium text-ink-soft">
              Modello
              {models.length > 0 ? (
                <select
                  className="select"
                  value={model}
                  onChange={(event) => {
                    setModel(event.target.value);
                  }}
                >
                  {models.map((info) => (
                    <option key={info.id} value={info.id}>
                      {info.id}
                    </option>
                  ))}
                </select>
              ) : (
                <input
                  className="input"
                  value={model}
                  placeholder="nome del modello"
                  onChange={(event) => {
                    setModel(event.target.value);
                  }}
                />
              )}
            </label>
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy}
              onClick={() => {
                void apply();
              }}
            >
              {busy ? <span className="spinner" aria-hidden="true" /> : null}
              Usa per tutti i ruoli
            </button>
          </div>
        )}
        {error !== null ? (
          <div className="banner banner-error" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>{error}</span>
          </div>
        ) : null}
        {notice !== null ? (
          <div className="banner banner-ok" role="status">
            <span aria-hidden="true">✓</span>
            <span>{notice}</span>
          </div>
        ) : null}
        <dl className="grid grid-cols-1 gap-x-6 gap-y-1 text-sm sm:grid-cols-2">
          {ROLE_LABELS.map(({ role, label }) => {
            const binding = activeBinding(bindings, role);
            return (
              <div key={role} className="flex gap-2">
                <dt className="w-32 shrink-0 text-muted">{label}</dt>
                <dd className="min-w-0 truncate text-ink">
                  {binding === undefined
                    ? "nessun modello"
                    : `${binding.model} · ${endpointName(binding.endpoint_id)}`}
                </dd>
              </div>
            );
          })}
        </dl>
      </div>
    </section>
  );
}
