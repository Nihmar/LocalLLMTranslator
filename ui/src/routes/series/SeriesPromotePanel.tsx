import { useEffect, useState } from "react";
import { FormField } from "../../components/FormField";
import { glossaryList, seriesPromoteTerm, toErrorMessage } from "../../lib/ipc";
import type { GlossaryTerm, SeriesDetail } from "../../lib/types";
import { kindLabel, type SeriesPanelProps } from "./shared";

/** Copy a member book's glossary term into the series canon. */
export function SeriesPromotePanel({
  detail,
  busy,
  run,
  onChanged,
}: SeriesPanelProps & { detail: SeriesDetail; onChanged: () => Promise<void> }) {
  const [projectId, setProjectId] = useState("");
  const [terms, setTerms] = useState<GlossaryTerm[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    setLoadError(null);
    if (projectId === "") {
      setTerms([]);
      return;
    }
    let cancelled = false;
    void glossaryList(projectId)
      .then((rows) => {
        if (!cancelled) {
          setTerms(rows.filter((term) => term.status !== "rejected"));
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setLoadError(toErrorMessage(error));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [projectId]);

  async function handlePromote(term: GlossaryTerm): Promise<void> {
    await run(`promote:${term.id}`, async () => {
      const result = await seriesPromoteTerm({ project_id: projectId, term_id: term.id });
      await onChanged();
      return result.outcome === "added"
        ? `«${term.source}» promosso nel canone di serie.`
        : result.outcome === "conflict"
          ? `«${term.source}»: il canone ha già un rendering diverso; il conflitto è stato registrato.`
          : `«${term.source}» era già nel canone con lo stesso rendering.`;
    });
  }

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Promuovi dal libro al canone</span>
      </div>
      <div className="panel-pad section-stack">
        <FormField label="Libro" htmlFor="series-promote-book">
          <select
            id="series-promote-book"
            className="select"
            value={projectId}
            onChange={(event) => {
              setProjectId(event.target.value);
            }}
          >
            <option value="">Scegli un libro membro…</option>
            {detail.projects.map((member) => (
              <option key={member.id} value={member.id}>
                {member.name}
              </option>
            ))}
          </select>
        </FormField>
        {loadError !== null ? <p className="field-error">{loadError}</p> : null}
        {projectId !== "" ? (
          terms.length === 0 ? (
            <p className="field-hint">Il libro non ha termini di glossario.</p>
          ) : (
            <ul className="section-stack" style={{ maxHeight: "16rem", overflowY: "auto" }}>
              {terms.map((term) => (
                <li
                  key={term.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded border border-line p-2"
                >
                  <span className="min-w-0 text-xs">
                    <span className="font-medium text-ink">{term.source}</span> → {term.target}{" "}
                    <span className="badge badge-neutral">{kindLabel(term.kind)}</span>
                  </span>
                  <button
                    type="button"
                    className="btn btn-sm"
                    disabled={busy}
                    onClick={() => void handlePromote(term)}
                  >
                    Promuovi
                  </button>
                </li>
              ))}
            </ul>
          )
        ) : null}
      </div>
    </div>
  );
}
