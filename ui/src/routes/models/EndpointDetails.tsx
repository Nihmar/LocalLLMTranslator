import { formatDateTime, formatNumber, truncate } from "../../lib/format";
import type { Endpoint, EndpointTestResult, ModelInfo } from "../../lib/types";

/** Health, `/props` and model list of one endpoint. */
export function EndpointDetails({
  endpoint,
  test,
  models,
  modelsLoading,
  modelsError,
  onReloadModels,
  onClose,
}: {
  endpoint: Endpoint;
  test: EndpointTestResult | undefined;
  models: ModelInfo[] | undefined;
  /** Id of the endpoint whose models are being fetched. */
  modelsLoading: string | null;
  modelsError: string | null;
  onReloadModels: () => Promise<void>;
  onClose: () => void;
}) {
  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">
          Dettagli · {endpoint.name}
        </span>
        <span className="flex items-center gap-2">
          <button
            type="button"
            className="btn btn-sm"
            disabled={modelsLoading === endpoint.id}
            onClick={() => {
              void onReloadModels();
            }}
          >
            {modelsLoading === endpoint.id ? (
              <span className="spinner" aria-hidden="true" />
            ) : null}
            Ricarica modelli
          </button>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => {
              onClose();
            }}
          >
            Chiudi
          </button>
        </span>
      </div>

      <div className="panel-pad section-stack">
        {test === undefined ? (
          <p className="text-xs text-muted">
            Nessuna verifica recente. Usa «Verifica» per leggere <span className="mono-chip">/health</span>{" "}
            e <span className="mono-chip">/props</span>.
          </p>
        ) : (
          <>
            <div
              className={test.health.ok ? "banner banner-ok" : "banner banner-error"}
              role="status"
            >
              <span aria-hidden="true">{test.health.ok ? "✓" : "⚠"}</span>
              <span>
                {test.health.status ??
                  (test.health.ok ? "Raggiungibile" : "Non raggiungibile")}
              </span>
            </div>
            <div className="grid grid-cols-3 gap-2">
              <div className="stat-tile">
                <div className="stat-label">Slot totali</div>
                <div className="stat-value">
                  {test.props === null || test.props.total_slots === null
                    ? "—"
                    : formatNumber(test.props.total_slots)}
                </div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Contesto (n_ctx)</div>
                <div className="stat-value">
                  {test.props === null || test.props.n_ctx === null
                    ? "—"
                    : formatNumber(test.props.n_ctx)}
                </div>
              </div>
              <div className="stat-tile">
                <div className="stat-label">Codice HTTP</div>
                <div className="stat-value">{formatNumber(test.health.code)}</div>
              </div>
            </div>
            {test.props !== null && test.props.model_path !== null ? (
              <p className="truncate font-mono text-[0.72rem] text-muted" title={test.props.model_path}>
                {test.props.model_path}
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
          {modelsLoading === endpoint.id && models === undefined ? (
            <p className="flex items-center gap-2 text-xs text-muted">
              <span className="spinner" aria-hidden="true" />
              Lettura dei modelli…
            </p>
          ) : models === undefined || models.length === 0 ? (
            <p className="text-xs text-muted">
              Nessun modello esposto. Il campo resta libero: puoi scrivere il nome del modello a
              mano quando associ un ruolo.
            </p>
          ) : (
            <ul className="flex flex-wrap gap-1">
              {models.map((model: ModelInfo) => (
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
          Ultima verifica registrata: {formatDateTime(endpoint.last_health_at)}
        </p>
      </div>
    </div>
  );
}
