/**
 * The book overview's "Prossima azione": one recommendation derived from where the book is, so
 * opening a book answers "what do I do now?" before any table. Pure, so it is unit-tested.
 */

export interface BookState {
  chapters: number;
  parts: number;
  translated: number;
  running: number;
  failed: number;
  openDecisions: number;
  importantDecisions: number;
  candidateTerms: number;
}

export type StepId = "ingest" | "glossary" | "translate" | "review" | "export";

export interface NextAction {
  title: string;
  detail: string;
  step: StepId;
  label: string;
}

export function nextAction(book: BookState): NextAction {
  if (book.chapters === 0) {
    return {
      title: "Il libro non è ancora importato.",
      detail: "L'importazione divide il documento in capitoli e paragrafi.",
      step: "ingest",
      label: "Importa il libro",
    };
  }
  if (book.translated === 0 && book.running === 0) {
    return {
      title: "Pronto per tradurre.",
      detail:
        book.candidateTerms > 0
          ? `Prima, se vuoi, approva i ${book.candidateTerms} termini proposti: il traduttore usa solo quelli approvati.`
          : "Profilo e glossario sono facoltativi: puoi partire subito.",
      step: book.candidateTerms > 0 ? "glossary" : "translate",
      label: book.candidateTerms > 0 ? "Prepara il glossario" : "Avvia la traduzione",
    };
  }
  if (book.failed > 0) {
    return {
      title: `${book.failed} parti non sono riuscite.`,
      detail: "Senza traduzione uscirebbero nella lingua originale: rimettile in coda.",
      step: "translate",
      label: "Vai alla traduzione",
    };
  }
  if (book.translated < book.parts) {
    return {
      title: `Tradotte ${book.translated} parti su ${book.parts}.`,
      detail:
        book.importantDecisions > 0
          ? `Puoi già rivedere: ${book.importantDecisions} proposte importanti aspettano una decisione.`
          : "Puoi seguire la traduzione capitolo per capitolo.",
      step: book.importantDecisions > 0 ? "review" : "translate",
      label: book.importantDecisions > 0 ? "Inizia a rivedere" : "Segui la traduzione",
    };
  }
  if (book.openDecisions > 0) {
    return {
      title: "Traduzione completa.",
      detail: `${book.openDecisions} proposte di revisione aspettano una decisione, ${book.importantDecisions} importanti.`,
      step: "review",
      label: "Rivedi",
    };
  }
  return {
    title: "Il libro è pronto.",
    detail: "Tutto tradotto, nessuna proposta in sospeso.",
    step: "export",
    label: "Esporta",
  };
}
