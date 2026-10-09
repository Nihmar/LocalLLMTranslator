import { useState } from "react";
import { StatusBadge } from "../../components/StatusBadge";
import { basename, formatDateTime, formatRelative } from "../../lib/format";
import type { Project } from "../../lib/types";
import { formatLabel } from "./shared";

/**
 * One book in the library. The busy flag and the two-step deletion are colocated here; the
 * parent owns the list and the failure banners the callbacks report.
 */
export function ProjectCard({
  project,
  isCurrent,
  onOpen,
  onIngest,
  onExport,
  onDelete,
}: {
  project: Project;
  isCurrent: boolean;
  onOpen: () => Promise<void>;
  onIngest: () => void;
  onExport: () => Promise<void>;
  /** Resolves `true` when the book was deleted; the confirmation closes only then. */
  onDelete: () => Promise<boolean>;
}) {
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);

  async function run(action: () => Promise<void>) {
    setBusy(true);
    try {
      await action();
    } finally {
      setBusy(false);
    }
  }

  async function confirmDelete() {
    setBusy(true);
    try {
      if (await onDelete()) {
        setConfirming(false);
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <article className="panel flex flex-col">
      <div className="panel-head">
        <span className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm font-semibold text-ink" title={project.name}>
            {project.name}
          </span>
          {isCurrent ? <StatusBadge status="ok" tone="accent" label="Aperto" /> : null}
        </span>
        <span className="mono-chip">{formatLabel(project.source_format)}</span>
      </div>

      <div className="panel-pad flex flex-1 flex-col gap-2">
        <p className="truncate font-mono text-[0.72rem] text-muted" title={project.source_path}>
          {basename(project.source_path)}
        </p>

        <dl className="grid grid-cols-2 gap-1 text-[0.72rem]">
          <dt className="text-faint">Lingue</dt>
          <dd className="font-mono text-ink-soft">
            {project.source_lang ?? "?"} → {project.target_lang}
          </dd>
          <dt className="text-faint">Modificato</dt>
          <dd className="text-ink-soft">{formatRelative(project.updated_at)}</dd>
          <dt className="text-faint">Creato</dt>
          <dd className="text-ink-soft">{formatDateTime(project.created_at)}</dd>
        </dl>

        {project.doc_title !== null ? (
          <p className="truncate text-[0.72rem] text-muted" title={project.doc_title}>
            {project.doc_title}
            {project.doc_author !== null ? ` — ${project.doc_author}` : ""}
          </p>
        ) : null}

        {confirming ? (
          <div className="banner banner-warn" role="alert">
            <span aria-hidden="true">⚠</span>
            <span>
              Eliminare «{project.name}» e tutti i suoi checkpoint?
              <span className="mt-2 flex gap-2">
                <button
                  type="button"
                  className="btn btn-sm btn-danger"
                  disabled={busy}
                  onClick={() => {
                    void confirmDelete();
                  }}
                >
                  Elimina
                </button>
                <button
                  type="button"
                  className="btn btn-sm"
                  onClick={() => {
                    setConfirming(false);
                  }}
                >
                  Annulla
                </button>
              </span>
            </span>
          </div>
        ) : null}

        <div className="mt-auto flex flex-wrap items-center gap-2 pt-1">
          <button
            type="button"
            className="btn btn-primary btn-sm"
            disabled={busy}
            onClick={() => {
              void run(onOpen);
            }}
          >
            Apri
          </button>
          <button type="button" className="btn btn-sm btn-ghost" onClick={onIngest}>
            Ingestione
          </button>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            disabled={busy}
            onClick={() => {
              void run(onExport);
            }}
          >
            Esporta .llmtz
          </button>
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            disabled={busy}
            onClick={() => {
              setConfirming((current) => !current);
            }}
          >
            Elimina
          </button>
        </div>
      </div>
    </article>
  );
}
