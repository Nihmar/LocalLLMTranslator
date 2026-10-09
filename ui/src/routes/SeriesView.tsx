import { useCallback, useEffect, useState } from "react";
import { EmptyState } from "../components/EmptyState";
import { onJobProgress } from "../lib/events";
import { projectList, qaReport, seriesGet, seriesList, toErrorMessage } from "../lib/ipc";
import type { Project, QaFinding, Series, SeriesDetail } from "../lib/types";
import { SeriesBooksPanel } from "./series/SeriesBooksPanel";
import { SeriesCanonPanel } from "./series/SeriesCanonPanel";
import { SeriesConflictsPanel } from "./series/SeriesConflictsPanel";
import { SeriesGlossaryPanel } from "./series/SeriesGlossaryPanel";
import { SeriesProfilePanel } from "./series/SeriesProfilePanel";
import { SeriesPromotePanel } from "./series/SeriesPromotePanel";
import { SeriesSidebar } from "./series/SeriesSidebar";
import type { Run } from "./series/shared";

/**
 * Series: the shared canon across books (`PLAN.md` §9.5).
 *
 * A series owns the pinned language pair, the glossary its books inherit (a book term
 * overrides the series rendering for the same source) and memory values. This page selects a
 * series and loads it; every panel in `./series/` owns its own form state and acts through
 * `run`, which keeps the busy flag and the page banners in one place. The panels are keyed by
 * the series id, so switching series starts them fresh.
 */
export function SeriesView() {
  const [series, setSeries] = useState<Series[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SeriesDetail | null>(null);
  const [conflicts, setConflicts] = useState<ReadonlyArray<{ project: Project; finding: QaFinding }>>(
    [],
  );
  // Conflicts panel: closed findings stay hidden unless asked for.
  const [showClosedConflicts, setShowClosedConflicts] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const loadSeries = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [seriesRows, projectRows] = await Promise.all([seriesList(), projectList()]);
      setSeries(seriesRows);
      setProjects(projectRows);
      setSelectedId((current) =>
        current !== null && seriesRows.some((row) => row.id === current)
          ? current
          : (seriesRows[0]?.id ?? null),
      );
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    } finally {
      setLoading(false);
    }
  }, []);

  const loadDetail = useCallback(async () => {
    if (selectedId === null) {
      setDetail(null);
      setConflicts([]);
      return;
    }
    try {
      const loaded = await seriesGet(selectedId);
      // Conflicts are project-scoped findings: gather them from every book.
      const perProject = await Promise.all(
        loaded.projects.map(async (project) => {
          const findings = await qaReport({
            project_id: project.id,
            kind: "glossary_conflict",
            status: showClosedConflicts ? null : "open",
          });
          return findings.map((finding) => ({ project, finding }));
        }),
      );
      setDetail(loaded);
      setConflicts(perProject.flat());
    } catch (loadError) {
      setError(toErrorMessage(loadError));
      setDetail(null);
    }
  }, [selectedId, showClosedConflicts]);

  useEffect(() => {
    void loadSeries();
  }, [loadSeries]);

  useEffect(() => {
    void loadDetail();
  }, [loadDetail]);

  useEffect(
    () =>
      onJobProgress((job) => {
        // The series reconnaissance is the only job this view waits for; reloading on its
        // transition surfaces the candidate without a manual refresh.
        if (job.kind === "series_recon") {
          void loadDetail();
        }
      }),
    [loadDetail],
  );

  const run: Run = useCallback(async (action, work) => {
    setBusy(action);
    setError(null);
    setNotice(null);
    try {
      const message = await work();
      if (message !== null) {
        setNotice(message);
      }
    } catch (actionError) {
      setError(toErrorMessage(actionError));
    } finally {
      setBusy(null);
    }
  }, []);

  const reloadAll = useCallback(async () => {
    await loadSeries();
    await loadDetail();
  }, [loadSeries, loadDetail]);

  const selectAfterLoad = useCallback(
    async (id: string) => {
      await loadSeries();
      setSelectedId(id);
    },
    [loadSeries],
  );

  const panel = { busy: busy !== null, run, notify: setNotice };

  return (
    <div className="section-stack">
      <div>
        <h1 className="font-serif text-2xl font-medium text-ink">Serie</h1>
        <p className="mt-0.5 text-xs text-muted">
          Il canone condiviso: glossario e memoria che i libri della saga ereditano. Un termine
          del libro vince su quello di serie; una modifica al canone segnala i libri incoerenti.
        </p>
      </div>

      {error !== null ? (
        <div className="banner banner-error" role="alert">
          <span aria-hidden="true">⚠</span>
          <span>{error}</span>
        </div>
      ) : null}
      {notice !== null ? (
        <div className="banner banner-ok" role="status">
          <span aria-hidden="true">✓</span>
          <span>{notice}</span>
        </div>
      ) : null}

      <div className="grid grid-cols-1 gap-3 xl:grid-cols-[20rem_minmax(0,1fr)]">
        <SeriesSidebar
          series={series}
          loading={loading}
          selectedId={selectedId}
          onSelect={setSelectedId}
          busy={busy !== null}
          run={run}
          onCreated={selectAfterLoad}
        />

        <div className="section-stack min-w-0">
          {selectedId === null ? (
            <div className="panel">
              <div className="panel-pad">
                <EmptyState
                  title="Nessuna serie selezionata"
                  description="Crea una serie per condividere glossario e memoria tra i libri di una saga."
                />
              </div>
            </div>
          ) : detail === null || detail.series.id !== selectedId ? (
            <div className="panel">
              <div className="panel-pad">
                <EmptyState tone="loading" compact title="Lettura della serie…" />
              </div>
            </div>
          ) : (
            <div key={detail.series.id} className="section-stack">
              <SeriesProfilePanel
                {...panel}
                detail={detail}
                onChanged={reloadAll}
                onDeleted={async () => {
                  setSelectedId(null);
                  await loadSeries();
                }}
              />
              <SeriesBooksPanel
                {...panel}
                detail={detail}
                projects={projects}
                onChanged={reloadAll}
              />
              <SeriesConflictsPanel
                {...panel}
                detail={detail}
                conflicts={conflicts}
                showClosed={showClosedConflicts}
                onShowClosed={setShowClosedConflicts}
                onChanged={loadDetail}
              />
              <SeriesPromotePanel {...panel} detail={detail} onChanged={loadDetail} />
              <SeriesGlossaryPanel {...panel} detail={detail} onChanged={loadDetail} />
              <SeriesCanonPanel
                {...panel}
                detail={detail}
                onChanged={loadDetail}
                onImported={selectAfterLoad}
              />
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
