import { FormField } from "../FormField";
import type { BookProfile } from "../../lib/types";

/** Start the reconnaissance (with optional pasted material) and reload it. */
export function ReconControls({
  pasted,
  onPasted,
  canStart,
  running,
  starting,
  loading,
  orchestratorBound,
  profile,
  onStart,
  onReload,
}: {
  pasted: string;
  onPasted: (value: string) => void;
  canStart: boolean;
  running: boolean;
  starting: boolean;
  loading: boolean;
  orchestratorBound: boolean | undefined;
  profile: BookProfile | null;
  onStart: () => void;
  onReload: () => void;
}) {
  return (
    <>
      <FormField
        label="Materiale da incollare (opzionale)"
        htmlFor="recon-pasted"
        hint="Testo di cui sei già in possesso. L'app non scarica nulla dalla rete."
      >
        <textarea
          id="recon-pasted"
          className="textarea"
          rows={3}
          value={pasted}
          onChange={(event) => {
            onPasted(event.target.value);
          }}
          placeholder="Incolla qui una pagina rappresentativa…"
        />
      </FormField>

      <div className="flex items-center gap-2">
        <button
          type="button"
          className="btn btn-primary"
          disabled={!canStart}
          onClick={onStart}
          title={
            orchestratorBound === false
              ? "Assegna un modello al ruolo orchestrator"
              : "Genera un profilo candidato dai materiali locali"
          }
        >
          {starting || running ? <span className="spinner" aria-hidden="true" /> : null}
          {running ? "Ricognizione in corso…" : "Riconosci il libro"}
        </button>
        <button type="button" className="btn" disabled={loading} onClick={onReload}>
          Ricarica
        </button>
      </div>

      {profile !== null ? (
        <p className="field-hint">
          Generato da <span className="mono-chip">{profile.provenance.model}</span> ·{" "}
          {profile.provenance.excerpt_blocks} estratti · metadati{" "}
          {profile.provenance.metadata ? "sì" : "no"} · {profile.provenance.pasted_chars} caratteri
          incollati
        </p>
      ) : null}
    </>
  );
}
