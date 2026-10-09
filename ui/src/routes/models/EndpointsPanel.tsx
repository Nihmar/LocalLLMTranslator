import { useState } from "react";
import { EmptyState } from "../../components/EmptyState";
import { StatusBadge } from "../../components/StatusBadge";
import { formatNumber, formatRelative, truncate } from "../../lib/format";
import type { Endpoint, EndpointTestResult } from "../../lib/types";
import { healthStatus } from "./shared";

/** The configured endpoints: health, details, edit, two-step delete. */
export function EndpointsPanel({
  endpoints,
  loading,
  error,
  testing,
  testResults,
  detailsId,
  onReload,
  onTest,
  onDetails,
  onEdit,
  onDelete,
}: {
  endpoints: readonly Endpoint[];
  loading: boolean;
  error: string | null;
  testing: string | null;
  testResults: Readonly<Record<string, EndpointTestResult>>;
  detailsId: string | null;
  onReload: () => Promise<void>;
  onTest: (id: string) => Promise<void>;
  onDetails: (id: string) => Promise<void>;
  /** Opens the endpoint form: `null` for a new endpoint. */
  onEdit: (endpoint: Endpoint | null) => void;
  onDelete: (id: string) => Promise<boolean>;
}) {
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  async function handleDelete(id: string) {
    if (await onDelete(id)) {
      setConfirmDeleteId(null);
    }
  }

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Endpoint configurati</span>
        <button
          type="button"
          className="btn btn-sm btn-ghost"
          disabled={loading}
          onClick={() => {
            void onReload();
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
              void onReload();
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
              onEdit(null);
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
                            void onTest(endpoint.id);
                          }}
                        >
                          Verifica
                        </button>
                        <button
                          type="button"
                          className="btn btn-sm btn-ghost"
                          onClick={() => {
                            void onDetails(endpoint.id);
                          }}
                        >
                          Dettagli
                        </button>
                        <button
                          type="button"
                          className="btn btn-sm btn-ghost"
                          onClick={() => {
                            onEdit(endpoint);
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
                                void handleDelete(endpoint.id);
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
  );
}
