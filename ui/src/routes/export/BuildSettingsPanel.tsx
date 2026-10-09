import { FormField } from "../../components/FormField";
import type { Chapter, ExportFormat } from "../../lib/types";
import { FORMATS } from "./shared";

/**
 * The build form. It is controlled by the page because the header's "Anteprima" and "Genera
 * output" act on the same choices.
 */
export function BuildSettingsPanel({
  format,
  onFormat,
  scope,
  onScope,
  chapters,
  template,
  onTemplate,
  css,
  onCss,
  outputPath,
  onOutputPath,
  toc,
  onToc,
  force,
  onForce,
  busy,
  onBuild,
}: {
  format: ExportFormat | "html";
  onFormat: (value: ExportFormat | "html") => void;
  scope: string;
  onScope: (value: string) => void;
  chapters: readonly Chapter[];
  template: string;
  onTemplate: (value: string) => void;
  css: string;
  onCss: (value: string) => void;
  outputPath: string;
  onOutputPath: (value: string) => void;
  toc: boolean;
  onToc: (value: boolean) => void;
  force: boolean;
  onForce: (value: boolean) => void;
  busy: boolean;
  onBuild: () => void;
}) {
  const formatSpec = FORMATS.find((entry) => entry.value === format) ?? FORMATS[0];

  return (
    <>
      <div className="panel">
        <div className="panel-head">
          <span className="panel-title">Impostazioni di build</span>
        </div>

        <div className="panel-pad section-stack">
          <FormField
            label="Formato di output"
            htmlFor="export-format"
            hint={formatSpec?.note ?? undefined}
          >
            <select
              id="export-format"
              className="select"
              value={format}
              onChange={(event) => {
                const matched = FORMATS.find((entry) => entry.value === event.target.value);
                if (matched !== undefined) {
                  onFormat(matched.value);
                }
              }}
            >
              {FORMATS.map((entry) => (
                <option key={entry.value} value={entry.value}>
                  {entry.label}
                </option>
              ))}
            </select>
          </FormField>

          <FormField label="Ambito" htmlFor="export-scope">
            <select
              id="export-scope"
              className="select"
              value={scope}
              onChange={(event) => {
                onScope(event.target.value);
              }}
            >
              <option value="all">Tutto il libro</option>
              {chapters.map((chapter) => (
                <option key={chapter.id} value={chapter.id}>
                  Solo: {chapter.title}
                </option>
              ))}
            </select>
          </FormField>

          <FormField
            label="Template Pandoc"
            htmlFor="export-template"
            hint={
              formatSpec === undefined || formatSpec.templateHint.length === 0
                ? "Non applicabile a questo formato: lascia vuoto."
                : `Vuoto = default dal pacchetto (${formatSpec.templateHint}). Un percorso personalizzato deve essere assoluto.`
            }
          >
            <input
              id="export-template"
              className="input"
              value={template}
              spellCheck={false}
              onChange={(event) => {
                onTemplate(event.target.value);
              }}
              placeholder="automatico"
            />
          </FormField>

          <FormField
            label="Foglio di stile CSS"
            htmlFor="export-css"
            hint={
              formatSpec === undefined || formatSpec.cssHint.length === 0
                ? "Ignorato dai formati non HTML."
                : `Vuoto = default dal pacchetto (${formatSpec.cssHint}).`
            }
          >
            <input
              id="export-css"
              className="input"
              value={css}
              spellCheck={false}
              disabled={formatSpec === undefined || formatSpec.cssHint.length === 0}
              onChange={(event) => {
                onCss(event.target.value);
              }}
              placeholder="automatico"
            />
          </FormField>

          <FormField
            label="Percorso di destinazione"
            htmlFor="export-output"
            hint="Vuoto: il file viene scritto nella cartella di output del progetto."
          >
            <input
              id="export-output"
              className="input"
              value={outputPath}
              spellCheck={false}
              onChange={(event) => {
                onOutputPath(event.target.value);
              }}
              placeholder="/home/utente/output/libro.epub"
            />
          </FormField>

          <label className="flex items-center gap-2 text-xs text-muted">
            <input
              type="checkbox"
              checked={toc}
              onChange={(event) => {
                onToc(event.target.checked);
              }}
            />
            Indice (table of contents)
          </label>

          <label className="flex items-center gap-2 text-xs text-muted">
            <input
              type="checkbox"
              checked={force}
              onChange={(event) => {
                onForce(event.target.checked);
              }}
            />
            Rigenera anche se nulla è cambiato
          </label>

          <button type="button" className="btn btn-primary" disabled={busy} onClick={onBuild}>
            {busy ? <span className="spinner" aria-hidden="true" /> : null}
            Genera output
          </button>
        </div>
      </div>

      <div className="panel panel-pad">
        <div className="panel-title mb-2">Cosa fa la build</div>
        <ul className="list-disc space-y-1 pl-4 text-xs text-muted">
          <li>
            Il documento viene diviso in unità per capitolo e{" "}
            <span className="mono-chip">metadata.yaml</span> è composto dai metadati del progetto e
            del documento.
          </li>
          <li>
            Template, CSS e filtri Lua (<span className="mono-chip">footnotes</span>,{" "}
            <span className="mono-chip">tables</span> e per EPUB{" "}
            <span className="mono-chip">epub_cleanup</span>) arrivano dal pacchetto{" "}
            <span className="mono-chip">pandoc/</span>; un percorso scelto a mano li sostituisce.
          </li>
          <li>
            Una build senza modifiche viene saltata e registrata come tale: la cronologia dice cosa
            è stato ricostruito o riutilizzato.
          </li>
        </ul>
      </div>
    </>
  );
}
