import { DiffPlaceholder } from "../components/DiffPlaceholder";
import { EmptyState } from "../components/EmptyState";
import { StatusBadge } from "../components/StatusBadge";
import type { Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * Step 4 of the wizard, milestone M1: an honest placeholder.
 *
 * Bilingual revision — the JSON editor pass, the proofreader, the three-column diff, per-change
 * accept/reject and the filterable QA report — is milestone **M4** (`PLAN.md` §11.4, §13). The
 * commands it needs (`suggestion_list/accept/reject`, `qa_report`, `review_start`) are listed in
 * `PLAN.md` §12.2 but are **not** part of the frozen `UI -> Tauri` table, so this page must not
 * pretend to offer them: it explains the scope and previews the layout instead.
 */

export interface ReviewViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

const M4_SCOPE: ReadonlyArray<{ title: string; detail: string }> = [
  {
    title: "Editor bilingue",
    detail:
      "Un passaggio LLM confronta sorgente e traduzione e propone solo difetti reali: sviste, omissioni, aggiunte, violazioni di glossario, salti di registro, Markdown rotto, placeholder spostati.",
  },
  {
    title: "Proofreader",
    detail:
      "Un secondo passaggio lavora sulla sola lingua di arrivo: grammatica, concordanze, calchi, collocazioni. Non cambia il significato e non tocca i placeholder.",
  },
  {
    title: "Diff a tre colonne",
    detail:
      "Originale, tradotto e corretto affiancati, con diff a livello di blocco e di carattere e navigazione da un suggerimento all'altro.",
  },
  {
    title: "Accetta / rifiuta per modifica",
    detail:
      "Ogni proposta è accettabile o rifiutabile singolarmente; le modifiche fatte a mano restano tracciate come origin = user.",
  },
  {
    title: "Report QA filtrabile",
    detail:
      "Non tradotti, incoerenze di glossario, placeholder rotti, lunghezze anomale, Markdown malformato, duplicati e vuoti, con gravità e stato.",
  },
  {
    title: "Glossario con conflitti",
    detail:
      "I termini proposti restano candidati finché non vengono confermati; due rese diverse per lo stesso termine producono un conflitto da risolvere qui.",
  },
];

export function ReviewView({ project, onNavigate }: ReviewViewProps) {
  return (
    <div className="section-stack">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold text-ink">Revisione</h2>
          <p className="mt-0.5 text-xs text-muted">
            Editor bilingue, proofreader, diff e report QA.
          </p>
        </div>
        <span className="flex items-center gap-2">
          <StatusBadge status="pending" label="Non ancora disponibile" />
          <span className="badge badge-accent">Arriva con M4</span>
        </span>
      </div>

      {project === null ? (
        <EmptyState
          title="Nessun progetto aperto"
          description="La revisione lavorerà sulle traduzioni di un progetto: aprine uno per vedere la coda."
          actionLabel="Vai ai progetti"
          onAction={() => {
            onNavigate("projects");
          }}
        />
      ) : (
        <>
          <div className="banner" role="note">
            <span aria-hidden="true">🧭</span>
            <span>
              Questa pagina è un segnaposto deliberato della milestone M1. Il progetto{" "}
              <span className="font-semibold text-ink-soft">{project.name}</span> può già essere
              tradotto e completato, ma la revisione bilingue non è implementata: nessun comando del
              contratto attuale espone suggerimenti, diff o report QA. Fino a M4 puoi consultare il
              testo tradotto in sola lettura dalla pagina{" "}
              <button
                type="button"
                className="underline decoration-dotted hover:text-ink"
                onClick={() => {
                  onNavigate("translate");
                }}
              >
                Traduzione
              </button>
              .
            </span>
          </div>

          <DiffPlaceholder milestone="M4" />

          <div className="panel">
            <div className="panel-head">
              <span className="panel-title">Cosa porterà la milestone M4</span>
              <span className="mono-chip">PLAN.md §11.4 · §13</span>
            </div>
            <div className="panel-pad grid grid-cols-1 gap-3 md:grid-cols-2">
              {M4_SCOPE.map((entry) => (
                <div key={entry.title} className="stat-tile">
                  <div className="mb-1 flex items-center gap-2">
                    <span className="badge badge-neutral">M4</span>
                    <span className="text-sm font-semibold text-ink">{entry.title}</span>
                  </div>
                  <p className="text-xs text-muted">{entry.detail}</p>
                </div>
              ))}
            </div>
          </div>

          <div className="panel panel-pad">
            <div className="panel-title mb-2">Perché non è simulata</div>
            <p className="text-xs text-muted">
              Mostrare un diff finto o un report QA vuoto sarebbe peggio di non mostrare nulla: la
              revisione è il passaggio in cui si decide se una traduzione è utilizzabile, e un
              pannello che sembra funzionante ma non scrive nulla produrrebbe una falsa sicurezza.
              Meglio dichiarare lo stato reale e rimandare a M4.
            </p>
          </div>
        </>
      )}
    </div>
  );
}
