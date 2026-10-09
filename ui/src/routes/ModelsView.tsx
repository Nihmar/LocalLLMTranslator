import { useCallback, useEffect, useMemo, useState } from "react";
import { QuickModelSetup } from "../components/QuickModelSetup";
import { onMetricsTick } from "../lib/events";
import {
  endpointDelete,
  endpointList,
  endpointModels,
  endpointTest,
  metricsGet,
  roleBindingList,
  toErrorMessage,
} from "../lib/ipc";
import type {
  Endpoint,
  EndpointTestResult,
  Metrics,
  ModelInfo,
  RoleBinding,
} from "../lib/types";
import { EndpointDetails } from "./models/EndpointDetails";
import { EndpointForm } from "./models/EndpointForm";
import { EndpointsPanel } from "./models/EndpointsPanel";
import { MetricsSidebar } from "./models/MetricsSidebar";
import { RoleBindingsPanel } from "./models/RoleBindingsPanel";

/**
 * Models page (`PLAN.md` §11.2): endpoint CRUD, health test, model list and role assignment,
 * plus the VRAM/queue indicator.
 *
 * The secret never reaches this page: `api_key_ref` is the *name* of the entry in the OS keyring
 * (`PLAN.md` §5, "Nessun segreto nel database").
 *
 * The view is endpoint/role-scoped, not project-scoped: it takes no props. Each panel in
 * `./models/` owns its form state and acts through the callbacks it is given; this component
 * keeps the loaded data, the selection and the page-level banners.
 */
export function ModelsView() {
  const [endpoints, setEndpoints] = useState<Endpoint[]>([]);
  const [loading, setLoading] = useState(true);
  // A failed action (delete) is a page banner; a failed list fetch replaces the table.
  const [error, setError] = useState<string | null>(null);
  const [listError, setListError] = useState<string | null>(null);

  const [formOpen, setFormOpen] = useState(false);
  const [formInitial, setFormInitial] = useState<Endpoint | null>(null);

  const [detailsId, setDetailsId] = useState<string | null>(null);
  const [testing, setTesting] = useState<string | null>(null);
  const [testResults, setTestResults] = useState<Record<string, EndpointTestResult>>({});
  const [models, setModels] = useState<Record<string, ModelInfo[]>>({});
  const [modelsLoading, setModelsLoading] = useState<string | null>(null);
  const [modelsError, setModelsError] = useState<string | null>(null);

  const [bindings, setBindings] = useState<RoleBinding[]>([]);
  const [bindingsError, setBindingsError] = useState<string | null>(null);

  const [metrics, setMetrics] = useState<Metrics | null>(null);
  const [metricsLoading, setMetricsLoading] = useState(true);
  const [metricsError, setMetricsError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setListError(null);
    try {
      const rows = await endpointList();
      setEndpoints(rows);
    } catch (loadError) {
      setListError(toErrorMessage(loadError));
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
  useEffect(
    () =>
      onMetricsTick(() => {
        void refreshMetrics();
      }),
    [refreshMetrics],
  );

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
                props_json:
                  result.props === null ? endpoint.props_json : JSON.stringify(result.props),
              }
            : endpoint,
        ),
      );
    } catch (testError) {
      setTestResults((current) => ({
        ...current,
        [endpointId]: {
          health: {
            ok: false,
            status: toErrorMessage(testError),
            code: 0,
            checked_at: new Date().toISOString(),
          },
          props: null,
          models: [],
        },
      }));
    } finally {
      setTesting(null);
    }
  }

  async function handleDeleteEndpoint(endpointId: string): Promise<boolean> {
    setError(null);
    try {
      await endpointDelete(endpointId);
      setEndpoints((current) => current.filter((endpoint) => endpoint.id !== endpointId));
      setBindings((current) => current.filter((binding) => binding.endpoint_id !== endpointId));
      if (detailsId === endpointId) {
        setDetailsId(null);
      }
      return true;
    } catch (deleteError) {
      setError(toErrorMessage(deleteError));
      return false;
    }
  }

  /** An assignment replaces whatever the same role had on the same endpoint. */
  function mergeBindings(current: RoleBinding[], saved: RoleBinding[]): RoleBinding[] {
    const rest = current.filter(
      (binding) =>
        !saved.some(
          (row) => row.endpoint_id === binding.endpoint_id && row.role === binding.role,
        ),
    );
    return [...rest, ...saved].sort((left, right) => right.priority - left.priority);
  }

  function openEndpointForm(endpoint: Endpoint | null) {
    setFormInitial(endpoint);
    setFormOpen(true);
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
            openEndpointForm(null);
          }}
        >
          Nuovo endpoint
        </button>
      </div>

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[minmax(0,1fr)_20rem]">
        <div className="section-stack min-w-0">
          {formOpen ? (
            <EndpointForm
              initial={formInitial}
              onSaved={(saved) => {
                setEndpoints((current) => {
                  const exists = current.some((endpoint) => endpoint.id === saved.id);
                  return exists
                    ? current.map((endpoint) => (endpoint.id === saved.id ? saved : endpoint))
                    : [...current, saved];
                });
                setFormOpen(false);
              }}
              onClose={() => {
                setFormOpen(false);
              }}
            />
          ) : null}

          <EndpointsPanel
            endpoints={endpoints}
            loading={loading}
            error={listError}
            testing={testing}
            testResults={testResults}
            detailsId={detailsId}
            onReload={load}
            onTest={handleTest}
            onDetails={handleOpenDetails}
            onEdit={openEndpointForm}
            onDelete={handleDeleteEndpoint}
          />

          {detailsEndpoint !== null ? (
            <EndpointDetails
              endpoint={detailsEndpoint}
              test={detailsTest}
              models={detailsModels}
              modelsLoading={modelsLoading}
              modelsError={modelsError}
              onReloadModels={async () => {
                await loadModelsFor(detailsEndpoint.id);
              }}
              onClose={() => {
                setDetailsId(null);
              }}
            />
          ) : null}

          <QuickModelSetup
            endpoints={endpoints}
            bindings={bindings}
            onApplied={(saved) => {
              setBindings((current) => mergeBindings(current, saved));
            }}
          />

          <RoleBindingsPanel
            endpoints={endpoints}
            bindings={bindings}
            bindingsError={bindingsError}
            models={models}
            presetEndpointId={detailsId}
            onLoadModels={loadModelsFor}
            onReload={loadBindings}
            onSaved={(saved) => {
              setBindings((current) => mergeBindings(current, saved));
            }}
            onRemoved={(bindingId) => {
              setBindings((current) => current.filter((binding) => binding.id !== bindingId));
            }}
          />
        </div>

        <MetricsSidebar metrics={metrics} loading={metricsLoading} error={metricsError} />
      </div>
    </div>
  );
}
