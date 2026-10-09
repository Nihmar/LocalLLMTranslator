import { FormField } from "../../components/FormField";
import { openPath, projectSetSeries } from "../../lib/ipc";
import type { Project, SeriesDetail } from "../../lib/types";
import type { SeriesPanelProps } from "./shared";

/** The books of a series: open, detach, attach another one. */
export function SeriesBooksPanel({
  detail,
  projects,
  busy,
  run,
  onChanged,
}: SeriesPanelProps & {
  detail: SeriesDetail;
  /** Every book of the library, to offer the ones not yet in the series. */
  projects: readonly Project[];
  onChanged: () => Promise<void>;
}) {
  const freeProjects = projects.filter(
    (project) => !detail.projects.some((member) => member.id === project.id),
  );

  async function handleAttach(projectId: string): Promise<void> {
    if (projectId === "") {
      return;
    }
    await run(`attach:${projectId}`, async () => {
      await projectSetSeries({
        project_id: projectId,
        series_id: detail.series.id,
        series_order: detail.projects.length + 1,
      });
      await onChanged();
      return "Libro collegato alla serie.";
    });
  }

  async function handleDetach(projectId: string): Promise<void> {
    await run(`detach:${projectId}`, async () => {
      await projectSetSeries({ project_id: projectId, series_id: null, series_order: null });
      await onChanged();
      return "Libro scollegato: userà solo il proprio glossario.";
    });
  }

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Libri della serie</span>
        <span className="mono-chip">{detail.projects.length}</span>
      </div>
      <div className="panel-pad section-stack">
        {detail.projects.length === 0 ? (
          <p className="field-hint">Nessun libro collegato.</p>
        ) : (
          <ul className="section-stack">
            {detail.projects.map((member) => (
              <li
                key={member.id}
                className="flex flex-wrap items-center justify-between gap-2 rounded border border-line p-2"
              >
                <span className="min-w-0">
                  <span className="block truncate text-xs font-medium text-ink">
                    {member.series_order ?? "—"}. {member.name}
                  </span>
                  <span className="block truncate text-[0.68rem] text-faint">
                    {member.source_lang ?? "?"} → {member.target_lang}
                  </span>
                </span>
                <span className="flex items-center gap-1">
                  <button
                    type="button"
                    className="btn btn-sm btn-ghost"
                    onClick={() => void openPath(member.source_path)}
                  >
                    Apri sorgente
                  </button>
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={busy}
                    onClick={() => void handleDetach(member.id)}
                  >
                    Scollega
                  </button>
                </span>
              </li>
            ))}
          </ul>
        )}

        {freeProjects.length > 0 ? (
          <FormField label="Collega un libro" htmlFor="series-attach">
            <select
              id="series-attach"
              className="select"
              value=""
              disabled={busy}
              onChange={(event) => {
                void handleAttach(event.target.value);
              }}
            >
              <option value="">Scegli un libro…</option>
              {freeProjects.map((project) => (
                <option key={project.id} value={project.id}>
                  {project.name} ({project.source_lang ?? "?"} → {project.target_lang})
                </option>
              ))}
            </select>
          </FormField>
        ) : null}
      </div>
    </div>
  );
}
