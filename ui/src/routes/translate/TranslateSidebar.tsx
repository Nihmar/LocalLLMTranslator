import { LogView } from "../../components/LogView";
import { ResourceGauge } from "../../components/ResourceGauge";
import { countLabel } from "../../lib/format";
import type { Metrics } from "../../lib/types";

/** Resource gauge, in-flight summary and the log tail of the translation page. */
export function TranslateSidebar({
  metrics,
  metricsLoading,
  activeJobs,
  pendingJobs,
  onOpenJobs,
}: {
  metrics: Metrics | null;
  metricsLoading: boolean;
  activeJobs: number;
  pendingJobs: number;
  onOpenJobs: () => void;
}) {
  return (
    <div className="section-stack">
      <ResourceGauge metrics={metrics} loading={metricsLoading} compact />

      <div className="panel panel-pad">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="min-w-0">
            <div className="panel-title">Lavori in corso</div>
            <p className="mt-1 text-xs text-muted">
              {countLabel(activeJobs, "job in esecuzione", "job in esecuzione")}
              {pendingJobs > 0
                ? ` · ${countLabel(pendingJobs, "job in attesa", "job in attesa")}`
                : ""}
            </p>
          </div>
          <button
            type="button"
            className="btn btn-sm"
            onClick={onOpenJobs}
            title="Elenco dei lavori di questo e degli altri progetti, con la possibilità di interromperne uno"
          >
            Apri elenco
          </button>
        </div>
        <p className="field-hint">
          Il monitor dice quale chunk sta traducendo ogni worker e permette di interrompere un
          lavoro senza fermare la coda.
        </p>
      </div>

      <div className="panel panel-pad">
        <div className="panel-title mb-2">Come si comporta la coda</div>
        <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
          <li>Un chunk è l&apos;unità di lavoro e di checkpoint: ogni chunk completato è salvato.</li>
          <li>
            «Pausa» ferma l&apos;intero esecutore e non riceve argomenti (
            <span className="mono-chip">translation_pause</span>); «Annulla» agisce sul progetto
            aperto, il cui id viene passato a{" "}
            <span className="mono-chip">translation_cancel</span>.
          </li>
          <li>
            «Avvia / Riprendi» rimette in coda i chunk non completati; «Riprova falliti» solo quelli
            falliti o da rivedere.
          </li>
          <li>I retry automatici rispettano il limite di tentativi del job.</li>
        </ul>
      </div>

      <LogView limit={300} heightClass="h-64" />
    </div>
  );
}
