import type { ReactNode } from "react";

/**
 * Non-functional mock of the bilingual diff editor.
 *
 * It exists so the M1 review page can be *honest*: it shows the layout the real editor will
 * have (originale / tradotto / corretto, per-block and per-character diff, accept/reject per
 * change) with every control disabled, instead of pretending revision is already available.
 * The real editor lands in milestone M4 (`PLAN.md` §11.4, §13).
 */

export interface DiffPlaceholderProps {
  /** Milestone that will deliver the real editor. */
  milestone?: string | undefined;
  /** Controls rendered next to the disabled accept/reject pair. */
  footer?: ReactNode | undefined;
}

function DiffColumn({
  title,
  hint,
  children,
}: {
  title: string;
  hint: string;
  children: ReactNode;
}) {
  return (
    <div className="diff-column">
      <div className="diff-column-head">
        <span className="block">{title}</span>
        <span className="mt-0.5 block text-[0.65rem] font-normal normal-case tracking-normal text-faint">
          {hint}
        </span>
      </div>
      <div className="diff-body">{children}</div>
    </div>
  );
}

export function DiffPlaceholder({ milestone = "M4", footer }: DiffPlaceholderProps) {
  return (
    <div className="flex flex-col gap-3">
      <div className="banner" role="note">
        <span aria-hidden="true">🧭</span>
        <span>
          Questa è un&apos;anteprima non interattiva del revisore bilingue. Il diff a livello di
          blocco e di carattere, la navigazione fra i suggerimenti e l&apos;accetta/rifiuta per
          singola modifica arrivano con la milestone <strong>{milestone}</strong>. Fino ad allora
          la traduzione prodotta è consultabile in sola lettura dalla pagina Traduzione.
        </span>
      </div>

      <div className="grid grid-cols-3 gap-3" aria-disabled="true">
        <DiffColumn title="Originale" hint="en — sorgente, invariato">
          <p>
            The <span className="mono-chip">siege</span> of the city lasted forty days, and
            &nbsp;the garrison held.
          </p>
          <p className="mt-2 text-faint">…blocco b000103 · paragrafo · 480 token</p>
        </DiffColumn>

        <DiffColumn title="Tradotto" hint="it — uscita del traduttore">
          <p>
            L&apos;{" "}
            <span className="diff-removed">assedio</span> della città durò quaranta giorni, e la
            guarnigione resistette.
          </p>
          <p className="mt-2 text-faint">…placeholder ⟦1⟧⟦2⟧ integri · 0 note</p>
        </DiffColumn>

        <DiffColumn title="Corretto" hint="it — editor + proofreader">
          <p>
            L&apos;<span className="diff-added">assedio</span> della città durò quaranta giorni e
            la guarnigione tenne.
          </p>
          <p className="mt-2 text-faint">
            …1 modifica proposta · gravità: minore · tipo: register
          </p>
        </DiffColumn>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className="btn btn-primary" disabled>
          Accetta modifica
        </button>
        <button type="button" className="btn" disabled>
          Rifiuta modifica
        </button>
        <button type="button" className="btn" disabled>
          Modifica successiva
        </button>
        <span className="badge badge-neutral">Disponibile in {milestone}</span>
        {footer}
      </div>
    </div>
  );
}
