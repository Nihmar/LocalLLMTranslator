import { EmptyState } from "../../components/EmptyState";
import type { ViewId } from "../../App";

/** The empty library: welcome, the three steps, and the two ways to add the first book. */
export function FirstRunPanel({
  onNewBook,
  onImport,
  importing,
  onNavigate,
}: {
  onNewBook: () => void;
  onImport: () => void;
  importing: boolean;
  onNavigate: (view: ViewId) => void;
}) {
  return (
    <div className="section-stack">
      <EmptyState
        title="Benvenuto in LocalLLMTranslator"
        description="Due passi per iniziare: assegna un modello in «Modelli», poi scegli il file del libro: l'importazione parte da sola."
        actionLabel="Aggiungi il primo libro"
        onAction={onNewBook}
      />
      <div className="panel panel-pad">
        <div className="panel-title mb-2">Primo avvio</div>
        <ol className="list-decimal space-y-1 pl-4 text-xs text-muted">
          <li>
            In <strong>Modelli</strong> registra l&apos;endpoint di{" "}
            <span className="mono-chip">llama-server</span> e assegna il ruolo <em>traduttore</em>;
            l&apos;orchestratore serve alla ricognizione e alla memoria del libro.
          </li>
          <li>
            Con <strong>Nuovo libro</strong> scegli il file EPUB, PDF o Markdown: titolo e lingua
            vengono letti dal file e l&apos;importazione parte da sola. Un bundle{" "}
            <span className="mono-chip">.llmtz</span> creato altrove si apre con «Importa».
          </li>
          <li>
            Poi segui i passi del libro: <strong>Prepara</strong> (facoltativo),{" "}
            <strong>Traduci</strong>, <strong>Rivedi</strong>, <strong>Esporta</strong>.
          </li>
        </ol>
        <div className="mt-2 flex items-center gap-2">
          <button
            type="button"
            className="btn btn-sm"
            onClick={() => {
              onNavigate("models");
            }}
          >
            Vai ai modelli
          </button>
          <button type="button" className="btn btn-sm" disabled={importing} onClick={onImport}>
            Importa .llmtz
          </button>
        </div>
      </div>
    </div>
  );
}
