import { useCallback, useEffect, useState } from "react";
import { chunkTranslated } from "../../lib/chapters";
import { onJobProgress } from "../../lib/events";
import { formatNumber } from "../../lib/format";
import { chunkList, qaReport, suggestionList } from "../../lib/ipc";
import type { ViewId } from "../../App";

interface ChecklistCounts {
  total: number;
  untranslated: number;
  failed: number;
  openDecisions: number;
  openFindings: number;
}

/**
 * "Prima di esportare": what the book would carry if it were built now. Untranslated parts would
 * come out in the source language (the build asks for confirmation), open decisions and findings
 * only mean the text is not reviewed yet.
 */
export function ExportChecklist({
  projectId,
  onNavigate,
}: {
  projectId: string;
  onNavigate: (view: ViewId) => void;
}) {
  const [counts, setCounts] = useState<ChecklistCounts | null>(null);

  const load = useCallback(async () => {
    try {
      const [chunks, decisions, findings] = await Promise.all([
        chunkList({ project_id: projectId }),
        suggestionList({ project_id: projectId, status: "pending" }),
        qaReport({ project_id: projectId, status: "open" }),
      ]);
      setCounts({
        total: chunks.length,
        untranslated: chunks.filter((chunk) => !chunkTranslated(chunk)).length,
        failed: chunks.filter((chunk) => chunk.status === "failed").length,
        openDecisions: decisions.length,
        openFindings: findings.length,
      });
    } catch {
      // The checklist is advisory: without it the build still guards untranslated parts.
      setCounts(null);
    }
  }, [projectId]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => onJobProgress(() => void load()), [load]);

  if (counts === null) {
    return null;
  }

  const items: Array<{ tone: "warn" | "info" | "ok"; text: string; view?: ViewId; action?: string }> =
    [
      counts.untranslated > 0
        ? {
            tone: "warn",
            text: `${formatNumber(counts.untranslated)} parti su ${formatNumber(counts.total)} non sono tradotte: uscirebbero nella lingua originale.`,
            view: "translate",
            action: "Vai alla traduzione",
          }
        : { tone: "ok", text: "Tutte le parti sono tradotte." },
      ...(counts.failed > 0
        ? [
            {
              tone: "warn" as const,
              text: `${formatNumber(counts.failed)} parti non sono riuscite: «Riprova falliti» le rimette in coda.`,
              view: "translate" as const,
              action: "Riprova",
            },
          ]
        : []),
      counts.openDecisions > 0
        ? {
            tone: "info",
            text: `${formatNumber(counts.openDecisions)} proposte di revisione aperte: non bloccano, il testo resta com'è.`,
            view: "review",
            action: "Rivedi",
          }
        : { tone: "ok", text: "Nessuna proposta di revisione aperta." },
      ...(counts.openFindings > 0
        ? [
            {
              tone: "info" as const,
              text: `${formatNumber(counts.openFindings)} controlli QA aperti.`,
              view: "review" as const,
              action: "Vedi",
            },
          ]
        : []),
    ];

  return (
    <div className="panel">
      <div className="panel-head">
        <span className="panel-title">Prima di esportare</span>
      </div>
      <ul className="panel-pad flex flex-col gap-2">
        {items.map((item) => (
          <li
            key={item.text}
            className={`banner ${item.tone === "warn" ? "banner-warn" : item.tone === "ok" ? "banner-ok" : ""}`}
          >
            <span aria-hidden="true">
              {item.tone === "warn" ? "!" : item.tone === "ok" ? "✓" : "i"}
            </span>
            <span className="flex-1">{item.text}</span>
            {item.view === undefined ? null : (
              <button
                type="button"
                className="btn btn-sm"
                onClick={() => {
                  if (item.view !== undefined) {
                    onNavigate(item.view);
                  }
                }}
              >
                {item.action}
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
