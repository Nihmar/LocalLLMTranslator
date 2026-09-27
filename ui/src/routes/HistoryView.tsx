import { useCallback, useEffect, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { formatDateTime, formatNumber, formatRelative, truncate } from "../lib/format";
import { suggestionHistory, toErrorMessage } from "../lib/ipc";
import { severityClass, suggestionSnippet } from "../lib/review";
import type { Project, Suggestion } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Correction history (`PLAN.md` §11.4): the decisions a project has taken on its
 * proposals, newest first. Accepting one rewrites the block and stamps `decided_at`; the
 * row keeps the before/after and the explanation the pass gave, so this page is the
 * read-back of the review work.
 */

export interface HistoryViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const OUTCOME_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "accepted", label: "Accettate" },
  { value: "rejected", label: "Rifiutate" },
  { value: "all", label: "Tutte" },
];

const PASS_FILTERS: ReadonlyArray<{ value: string; label: string }> = [
  { value: "all", label: "Tutti i passaggi" },
  { value: "editor", label: "Editor" },
  { value: "proofreader", label: "Proofreader" },
];

/** Mirrors `commands::review::DEFAULT_HISTORY_LIMIT`. */
const HISTORY_LIMIT = 1000;

export function HistoryView({ project, onNavigate }: HistoryViewProps) {
  const [rows, setRows] = useState<Suggestion[]>([]);
  const [outcome, setOutcome] = useState("accepted");
  const [pass, setPass] = useState("all");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [reloadToken, setReloadToken] = useState(0);

  const projectId = project?.id ?? null;

  const load = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    setLoading(true);
    setError(null);
    try {
      setRows(
        await suggestionHistory({
          project_id: projectId,
          pass: pass === "all" ? null : pass,
          status: outcome === "all" ? null : outcome,
          limit: HISTORY_LIMIT,
        }),
      );
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setRows([]);
    } finally {
      setLoading(false);
    }
  }, [projectId, outcome, pass]);

  useEffect(() => {
    void load();
  }, [load, reloadToken]);

  if (project === null) {
    return (
      <div className="section-stack">
        <h2 className="text-lg font-semibold text-ink">Storico</h2>
        <EmptyState
          title="Nessun progetto aperto"
          description="Lo storico raccoglie le decisioni di revisione di un progetto: aprine uno e revisiona una traduzione."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      </div>
    );
  }

  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Storico</h2>
          <p className="mt-0.5 text-xs text-muted">
            Progetto <span className="font-semibold text-ink-soft">{project.name}</span> —{" "}
            {formatNumber(rows.length)} decisioni visibili, più recenti in alto.
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <label className="flex items-center gap-1 text-xs text-muted">
            Esito
            <select
              className="select"
              style={{ width: "auto" }}
              value={outcome}
              onChange={(event) => {
                setOutcome(event.target.value);
              }}
            >
              {OUTCOME_FILTERS.map((entry) => (
                <option key={entry.value} value={entry.value}>
                  {entry.label}
                </option>
              ))}
            </select>
          </label>
          <label className="flex items-center gap-1 text-xs text-muted">
            Passaggio
            <select
              className="select"
              style={{ width: "auto" }}
              value={pass}
              onChange={(event) => {
                setPass(event.target.value);
              }}
            >
              {PASS_FILTERS.map((entry) => (
                <option key={entry.value} value={entry.value}>
                  {entry.label}
                </option>
              ))}
            </select>
          </label>
          <button
            type="button"
            className="btn"
            disabled={loading}
            onClick={() => {
              setReloadToken((current) => current + 1);
            }}
          >
            Aggiorna
          </button>
        </div>
      </div>

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
        </div>
      ) : null}

      <div className="panel">
        <div className="panel-head">
          <span className="panel-title">Correzioni</span>
          <span className="mono-chip">{formatNumber(rows.length)}</span>
        </div>
        {loading && rows.length === 0 ? (
          <div className="panel-pad">
            <EmptyState tone="loading" compact title="Lettura dello storico…" />
          </div>
        ) : rows.length === 0 ? (
          <div className="panel-pad">
            <EmptyState
              compact
              title="Nessuna decisione registrata"
              description="Accetta o rifiuta un suggerimento nella revisione: ogni decisione finisce qui con la sua spiegazione."
              actionLabel="Vai alla revisione"
              onAction={() => {
                onNavigate("review");
              }}
            />
          </div>
        ) : (
          <div className="table-scroll" style={{ maxHeight: "34rem" }}>
            <table className="data-table">
              <thead>
                <tr>
                  <th>Quando</th>
                  <th>Esito</th>
                  <th>Passaggio</th>
                  <th>Gravità</th>
                  <th>Correzione</th>
                  <th>Spiegazione</th>
                  <th>Riferimenti</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => {
                  const snippet = suggestionSnippet(row);
                  const reason = row.reason !== null && row.reason.length > 0 ? row.reason : "—";
                  return (
                    <tr key={row.id}>
                      <td
                        className="font-mono text-[0.7rem] text-muted"
                        title={formatRelative(row.decided_at)}
                      >
                        {formatDateTime(row.decided_at)}
                      </td>
                      <td>
                        {row.status === "accepted" ? (
                          <span className="badge badge-success">accettata</span>
                        ) : (
                          <span className="badge badge-danger">rifiutata</span>
                        )}
                      </td>
                      <td>
                        <span className="badge badge-neutral">{row.pass}</span>
                      </td>
                      <td>
                        <span className={severityClass(row.severity)}>
                          {row.severity ?? "nota"}
                        </span>
                      </td>
                      <td className="text-xs text-ink-soft" title={snippet}>
                        {truncate(snippet, 140)}
                      </td>
                      <td className="text-xs text-ink-soft" title={reason}>
                        {reason === "—" ? reason : truncate(reason, 220)}
                      </td>
                      <td className="text-[0.68rem] text-faint">
                        <span className="mono-chip">{row.chunk_id}</span>
                        {row.block_id === null ? null : (
                          <span className="ml-1 font-mono">{row.block_id}</span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </div>

      {rows.length >= HISTORY_LIMIT ? (
        <p className="field-hint">
          Mostrate le {formatNumber(HISTORY_LIMIT)} decisioni più recenti.
        </p>
      ) : null}
    </div>
  );
}
