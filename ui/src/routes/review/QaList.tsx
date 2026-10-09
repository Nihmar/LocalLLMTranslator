import { EmptyState } from "../../components/EmptyState";
import { severityClass } from "../../lib/review";
import type { QaFindingView } from "../../lib/types";
import { QA_KIND_LABEL, SEVERITY_LABEL } from "./shared";

/** The advisory QA findings, one row each; not decidable from here. */
export function QaList({
  findings,
  chapterOf,
}: {
  findings: readonly QaFindingView[];
  chapterOf: ReadonlyMap<string, string>;
}) {
  if (findings.length === 0) {
    return (
      <EmptyState
        title="Nessun controllo aperto"
        description="I controlli girano dopo ogni capitolo tradotto e con «Rilancia QA»."
      />
    );
  }
  return (
    <ul className="panel divide-y divide-line">
      {findings.map((finding) => (
        <li key={finding.id} className="flex flex-wrap items-start gap-3 px-5 py-3">
          <span className={severityClass(finding.severity)}>
            {SEVERITY_LABEL[finding.severity] ?? finding.severity}
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-medium text-ink">
              {QA_KIND_LABEL[finding.kind] ?? finding.kind}
            </span>
            <span className="block text-xs text-muted">
              {finding.chunk_id === null ? "Tutto il libro" : chapterOf.get(finding.chunk_id)}
            </span>
            <span className="mt-1 block font-mono text-[0.7rem] break-all text-faint">
              {JSON.stringify(finding.details)}
            </span>
          </span>
        </li>
      ))}
    </ul>
  );
}
