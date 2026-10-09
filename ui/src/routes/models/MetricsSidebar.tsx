import { ResourceGauge } from "../../components/ResourceGauge";
import type { Metrics } from "../../lib/types";

/** VRAM/queue gauge from `metrics_get`, plus how the roles use the endpoints. */
export function MetricsSidebar({
  metrics,
  loading,
  error,
}: {
  metrics: Metrics | null;
  loading: boolean;
  error: string | null;
}) {
  return (
    <div className="section-stack">
      <ResourceGauge metrics={metrics} loading={loading} />

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
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
          <li>Le API key restano nel keyring: qui si salva solo il nome della voce.</li>
        </ul>
      </div>
    </div>
  );
}
