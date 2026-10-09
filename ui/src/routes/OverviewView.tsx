import { useCallback, useEffect, useState, type ReactNode } from "react";
import { EmptyState } from "../components/EmptyState";
import { ProgressBar } from "../components/ProgressBar";
import { chunkTranslated } from "../lib/chapters";
import { onJobProgress } from "../lib/events";
import { formatNumber } from "../lib/format";
import { chunkList, glossaryList, projectGet, suggestionList, toErrorMessage } from "../lib/ipc";
import { nextAction, type BookState } from "../lib/overview";
import { isImportant } from "../lib/review";
import type { Project } from "../lib/types";
import type { ViewId } from "../App";

/**
 * "Panoramica": where the book stands and what to do next (issue #14). One recommendation on
 * top, then translation, review and glossary at a glance, each linking to its step.
 */

export interface OverviewViewProps {
  project: Project | null;
  onNavigate: (view: ViewId) => void;
}

export function OverviewView({ project, onNavigate }: OverviewViewProps) {
  const projectId = project?.id ?? null;
  const [book, setBook] = useState<BookState | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (projectId === null) {
      return;
    }
    try {
      const [detail, chunks, decisions, terms] = await Promise.all([
        projectGet(projectId),
        chunkList({ project_id: projectId }),
        suggestionList({ project_id: projectId, status: "pending" }),
        glossaryList(projectId),
      ]);
      setBook({
        chapters: detail.chapters.length,
        parts: chunks.length,
        translated: chunks.filter(chunkTranslated).length,
        running: chunks.filter((chunk) => chunk.status === "running").length,
        failed: chunks.filter((chunk) => chunk.status === "failed").length,
        openDecisions: decisions.length,
        importantDecisions: decisions.filter(isImportant).length,
        candidateTerms: terms.filter((term) => term.status === "candidate").length,
      });
      setError(null);
    } catch (loadError) {
      setError(toErrorMessage(loadError));
    }
  }, [projectId]);

  useEffect(() => {
    setBook(null);
    void load();
  }, [load]);

  useEffect(() => onJobProgress(() => void load()), [load]);

  if (project === null) {
    return (
      <EmptyState
        title="Nessun libro aperto"
        actionLabel="Vai alla libreria"
        onAction={() => {
          onNavigate("projects");
        }}
      />
    );
  }
  if (error !== null) {
    return (
      <div className="banner banner-error" role="alert">
        <span aria-hidden="true">⚠</span>
        <span>{error}</span>
      </div>
    );
  }
  if (book === null) {
    return <EmptyState tone="loading" title="Lettura del libro…" />;
  }

  const action = nextAction(book);

  return (
    <div className="section-stack">
      <section className="panel flex flex-wrap items-center justify-between gap-6 p-7">
        <div className="flex min-w-0 flex-[1_1_32rem] flex-col gap-2">
          <span className="text-xs font-semibold tracking-wider text-accent uppercase">
            Prossima azione
          </span>
          <h1 className="font-serif text-3xl font-medium text-ink">{action.title}</h1>
          <p className="max-w-[62ch] text-ink-soft">{action.detail}</p>
        </div>
        <button
          type="button"
          className="btn btn-primary btn-lg"
          onClick={() => {
            onNavigate(action.step);
          }}
        >
          {action.label}
        </button>
      </section>

      <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
        <OverviewCard
          title="Traduzione"
          figure={`${formatNumber(book.translated)} / ${formatNumber(book.parts)} parti`}
          onOpen={() => {
            onNavigate("translate");
          }}
        >
          <ProgressBar value={book.translated} total={Math.max(book.parts, 1)} tone="accent" />
          <p className="text-sm text-muted">
            {book.running > 0 ? "In corso. " : ""}
            {book.failed > 0 ? `${formatNumber(book.failed)} non riuscite.` : ""}
          </p>
        </OverviewCard>
        <OverviewCard
          title="Revisione"
          figure={`${formatNumber(book.openDecisions)} da decidere`}
          onOpen={() => {
            onNavigate("review");
          }}
        >
          <p className="text-sm text-muted">
            {book.openDecisions === 0
              ? "Nessuna proposta in sospeso."
              : `${formatNumber(book.importantDecisions)} importanti, le altre minori.`}
          </p>
        </OverviewCard>
        <OverviewCard
          title="Glossario"
          figure={`${formatNumber(book.candidateTerms)} da approvare`}
          onOpen={() => {
            onNavigate("glossary");
          }}
        >
          <p className="text-sm text-muted">Il traduttore usa solo i termini approvati.</p>
        </OverviewCard>
      </div>
    </div>
  );
}

function OverviewCard({
  title,
  figure,
  onOpen,
  children,
}: {
  title: string;
  figure: string;
  onOpen: () => void;
  children: ReactNode;
}) {
  return (
    <section className="panel flex flex-col gap-3 p-5">
      <div className="flex items-baseline justify-between gap-2">
        <h2 className="font-semibold text-ink">{title}</h2>
        <span className="font-mono text-sm text-ink-soft">{figure}</span>
      </div>
      {children}
      <button type="button" className="btn btn-sm self-start" onClick={onOpen}>
        Apri
      </button>
    </section>
  );
}
