import { useMemo, useState } from "react";
import { pickSeriesBundleFile } from "../../lib/dialog";
import { parentDirectory } from "../../lib/format";
import { downloadUrl } from "../../lib/http";
import {
  isTauriRuntime,
  openPath,
  seriesExport,
  seriesImport,
  seriesQaScan,
  seriesReconConfirm,
  seriesReconStart,
} from "../../lib/ipc";
import type { ConfirmedSeriesCharacter, SeriesDetail, SeriesProfile } from "../../lib/types";
import { parseSeriesProfile, type SeriesPanelProps } from "./shared";

/**
 * Bundles, the series-wide QA scan and the series reconnaissance with its candidate profile.
 * The candidate is never injected into a prompt: the user picks what enters the canon.
 */
export function SeriesCanonPanel({
  detail,
  busy,
  run,
  notify,
  onChanged,
  onImported,
}: SeriesPanelProps & {
  detail: SeriesDetail;
  onChanged: () => Promise<void>;
  /** Reloads the list and selects the imported series. */
  onImported: (id: string) => Promise<void>;
}) {
  // Last exported bundle, so the view can reveal it.
  const [exportedPath, setExportedPath] = useState<string | null>(null);
  // Re-synthesize the candidate even when no book profile or canon changed.
  const [forceRecon, setForceRecon] = useState(false);
  // Candidate confirmation drafts: `undefined` means "use the candidate value", so a new
  // candidate is picked up automatically without a reset effect.
  const [synopsisDraft, setSynopsisDraft] = useState<string | undefined>(undefined);
  const [styleDraft, setStyleDraft] = useState<string | undefined>(undefined);
  const [synopsisChecked, setSynopsisChecked] = useState<boolean | undefined>(undefined);
  const [styleChecked, setStyleChecked] = useState<boolean | undefined>(undefined);
  const [characterDrafts, setCharacterDrafts] = useState<
    Record<string, { checked?: boolean; target?: string }>
  >({});

  const candidate = useMemo(() => {
    const raw = detail.memory.find((row) => row.key === "series_profile")?.value;
    return raw === undefined ? null : parseSeriesProfile(raw);
  }, [detail]);
  const candidateSynopsis = candidate?.synopsis ?? "";
  const candidateStyle = candidate?.style_notes.join("\n") ?? "";
  const effectiveSynopsisChecked = synopsisChecked ?? candidateSynopsis.trim().length > 0;
  const effectiveStyleChecked = styleChecked ?? candidateStyle.trim().length > 0;

  function characterChecked(source: string, fallback: boolean): boolean {
    return characterDrafts[source]?.checked ?? fallback;
  }

  function characterTarget(source: string, fallback: string): string {
    return characterDrafts[source]?.target ?? fallback;
  }

  function resetDrafts(): void {
    setSynopsisDraft(undefined);
    setStyleDraft(undefined);
    setSynopsisChecked(undefined);
    setStyleChecked(undefined);
    setCharacterDrafts({});
  }

  async function handleExport(): Promise<void> {
    await run("export", async () => {
      const outcome = await seriesExport({ series_id: detail.series.id, output_path: null });
      setExportedPath(outcome.output_path);
      return (
        `Canone esportato (${outcome.terms} termini, ${outcome.variants} alias, ` +
        `${outcome.books} libri): ${outcome.output_path}`
      );
    });
  }

  async function handleImport(): Promise<void> {
    const path = await pickSeriesBundleFile();
    if (path === null) {
      notify("Importazione annullata o selettore file non disponibile.");
      return;
    }
    await run("import", async () => {
      const outcome = await seriesImport({ archive_path: path });
      await onImported(outcome.series.id);
      return (
        `Serie «${outcome.series.name}» importata: ${outcome.terms_added} termini aggiunti, ` +
        `${outcome.terms_updated} aggiornati, ${outcome.conflicts} conflitti da rivedere, ` +
        `${outcome.books_imported} libri importati, ${outcome.books_skipped} già presenti.`
      );
    });
  }

  async function handleQaScan(): Promise<void> {
    await run("qa-scan", async () => {
      const outcome = await seriesQaScan(detail.series.id);
      return outcome.enqueued === 0
        ? "Nessun chunk da scansionare: i libri non hanno traduzioni complete."
        : `${outcome.enqueued} job di scansione QA accodati su tutti i libri della serie.`;
    });
  }

  async function handleReconStart(): Promise<void> {
    await run("recon", async () => {
      await seriesReconStart(detail.series.id, forceRecon);
      return "Ricognizione di serie accodata: se profili e canone non sono cambiati il candidato resta quello attuale.";
    });
  }

  async function handleConfirmCandidate(profile: SeriesProfile): Promise<void> {
    const accepted: ConfirmedSeriesCharacter[] = [];
    const rejected: string[] = [];
    for (const character of profile.characters) {
      const checked = characterChecked(character.source, true);
      const target = characterTarget(character.source, character.target).trim();
      if (checked && target.length > 0) {
        accepted.push({
          source: character.source,
          target,
          note: character.note.length > 0 ? character.note : null,
        });
      } else if (!checked) {
        rejected.push(character.source);
      }
    }
    if (
      accepted.length === 0 &&
      rejected.length === 0 &&
      !effectiveSynopsisChecked &&
      !effectiveStyleChecked
    ) {
      notify("Nessun campo selezionato da confermare.");
      return;
    }
    await run("confirm", async () => {
      const outcome = await seriesReconConfirm({
        series_id: detail.series.id,
        synopsis: effectiveSynopsisChecked ? (synopsisDraft ?? profile.synopsis) : null,
        style_guide: effectiveStyleChecked
          ? (styleDraft ?? profile.style_notes.join("\n"))
          : null,
        characters: accepted,
        rejected_characters: rejected,
        discard: false,
      });
      resetDrafts();
      await onChanged();
      return (
        `Confermato: ${outcome.characters_accepted} personaggi nel canone, ` +
        `${outcome.characters_rejected} rifiutati` +
        `${outcome.synopsis_updated ? ", sinossi aggiornata" : ""}` +
        `${outcome.style_guide_updated ? ", style guide aggiornata" : ""}.`
      );
    });
  }

  async function handleDiscardCandidate(): Promise<void> {
    if (candidate === null) {
      return;
    }
    await run("discard", async () => {
      const rejected = candidate.characters
        .filter((character) => !characterChecked(character.source, true))
        .map((character) => character.source);
      await seriesReconConfirm({
        series_id: detail.series.id,
        characters: [],
        rejected_characters: rejected,
        discard: true,
      });
      resetDrafts();
      await onChanged();
      return "Candidato scartato; i rifiuti restano registrati.";
    });
  }

  return (
    <div className="panel">
          <div className="panel-head">
            <span className="panel-title">Bundle e coerenza</span>
          </div>
          <div className="panel-pad section-stack">
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                className="btn"
                disabled={busy}
                onClick={() => void handleExport()}
              >
                Esporta canone (.llmtsz)
              </button>
              <button
                type="button"
                className="btn"
                disabled={busy}
                onClick={() => void handleImport()}
              >
                Importa canone…
              </button>
              <button
                type="button"
                className="btn"
                disabled={busy || detail.projects.length === 0}
                onClick={() => void handleQaScan()}
              >
                Scansiona QA tutti i libri
              </button>
              {exportedPath !== null ? (
                isTauriRuntime() ? (
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => void openPath(parentDirectory(exportedPath))}
                  >
                    Apri cartella
                  </button>
                ) : (
                  <a className="btn btn-sm btn-ghost" href={downloadUrl(exportedPath)} download>
                    Scarica bundle
                  </a>
                )
              ) : null}
            </div>
            <p className="field-hint">
              L&apos;export include il canone e i libri della serie (una istantanea del
              database e le loro cartelle di lavoro). L&apos;import fonde per revisione:
              rendering uguali aggiornati, rendering diversi mai sovrascritti (restano come
              conflitto) e libri già presenti saltati. La scansione QA riusa i job
              <span className="mono-chip ml-1">qa_scan</span> sulle traduzioni esistenti di
              tutti i libri della serie.
            </p>
            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                className="btn"
                disabled={busy || detail.projects.length === 0}
                onClick={() => void handleReconStart()}
              >
                Genera profilo di serie
              </button>
              <label className="flex items-center gap-1 text-xs text-muted">
                <input
                  type="checkbox"
                  checked={forceRecon}
                  onChange={(event) => {
                    setForceRecon(event.target.checked);
                  }}
                />
                Forza rigenerazione
              </label>
              <span className="field-hint">
                Usa il ruolo orchestrator sui profili confermati dei libri; solo i libri
                cambiati vengono ritradotti in profilo.
              </span>
            </div>
    
            {candidate !== null ? (
              <div className="section-stack rounded border border-line p-2">
                <span className="stat-label">Profilo candidato</span>
                <p className="field-hint">
                  Nulla è attivo finché non confermi: la sinossi e la style guide scelte
                  entrano nella memoria di serie, i personaggi spuntati diventano termini
                  approvati, quelli deselezionati restano rifiutati e non verranno più
                  proposti.
                </p>
    
                <label className="flex items-start gap-2 text-xs text-ink-soft">
                  <input
                    type="checkbox"
                    checked={effectiveSynopsisChecked}
                    onChange={(event) => {
                      setSynopsisChecked(event.target.checked);
                    }}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="field-label">Sinossi di serie</span>
                    <textarea
                      className="input min-h-16"
                      aria-label="Sinossi candidata"
                      value={synopsisDraft ?? candidate.synopsis}
                      disabled={!effectiveSynopsisChecked}
                      onChange={(event) => {
                        setSynopsisDraft(event.target.value);
                      }}
                    />
                  </span>
                </label>
    
                <label className="flex items-start gap-2 text-xs text-ink-soft">
                  <input
                    type="checkbox"
                    checked={effectiveStyleChecked}
                    onChange={(event) => {
                      setStyleChecked(event.target.checked);
                    }}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="field-label">Style guide di serie</span>
                    <textarea
                      className="input min-h-16"
                      aria-label="Style guide candidata"
                      value={styleDraft ?? candidate.style_notes.join("\n")}
                      disabled={!effectiveStyleChecked}
                      onChange={(event) => {
                        setStyleDraft(event.target.value);
                      }}
                    />
                  </span>
                </label>
    
                {candidate.characters.length > 0 ? (
                  <div className="section-stack">
                    <span className="field-label">Personaggi</span>
                    <ul className="section-stack">
                      {candidate.characters.map((character) => (
                        <li
                          key={character.source}
                          className="flex flex-wrap items-center gap-2 rounded border border-line p-2"
                        >
                          <label className="flex items-center gap-2 text-[0.72rem] text-muted">
                            <input
                              type="checkbox"
                              checked={characterChecked(character.source, true)}
                              onChange={(event) => {
                                setCharacterDrafts((current) => ({
                                  ...current,
                                  [character.source]: {
                                    ...current[character.source],
                                    checked: event.target.checked,
                                  },
                                }));
                              }}
                            />
                            <span className="font-medium text-ink-soft">
                              {character.source}
                            </span>
                          </label>
                          <input
                            className="input"
                            style={{ width: "12rem" }}
                            aria-label={`Rendering di ${character.source}`}
                            value={characterTarget(character.source, character.target)}
                            disabled={!characterChecked(character.source, true)}
                            onChange={(event) => {
                              setCharacterDrafts((current) => ({
                                ...current,
                                [character.source]: {
                                  ...current[character.source],
                                  target: event.target.value,
                                },
                              }));
                            }}
                          />
                          {character.note.length > 0 ? (
                            <span className="text-[0.68rem] text-faint">
                              {character.note}
                            </span>
                          ) : null}
                        </li>
                      ))}
                    </ul>
                  </div>
                ) : null}
    
                {candidate.rejected.length > 0 ? (
                  <p className="field-hint">
                    Già rifiutati: {candidate.rejected.join(", ")}
                  </p>
                ) : null}
    
                <div className="flex flex-wrap items-center gap-2">
                  <button
                    type="button"
                    className="btn btn-primary"
                    disabled={busy}
                    onClick={() => void handleConfirmCandidate(candidate)}
                  >
                    Conferma selezionati
                  </button>
                  <button
                    type="button"
                    className="btn"
                    disabled={busy}
                    onClick={() => void handleDiscardCandidate()}
                  >
                    Scarta candidato
                  </button>
                </div>
              </div>
            ) : null}
          </div>
        </div>
  );
}
