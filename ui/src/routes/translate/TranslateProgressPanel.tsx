import { ProgressBar } from "../../components/ProgressBar";
import { formatNumber } from "../../lib/format";
import type { Counts } from "./shared";

/** Overall progress plus the status histogram of the whole project. */
export function TranslateProgressPanel({
  translatedCount,
  chunksTotal,
  counts,
  totalTokens,
}: {
  translatedCount: number;
  chunksTotal: number;
  counts: Counts;
  totalTokens: number;
}) {
  return (
    <div className="panel panel-pad section-stack">
      <ProgressBar
        value={translatedCount}
        total={chunksTotal}
        label="Avanzamento complessivo"
        tone="accent"
        showCounts
      />

      <div className="grid grid-cols-6 gap-2">
        <div className="stat-tile">
          <div className="stat-label">In attesa</div>
          <div className="stat-value">{formatNumber(counts.pending)}</div>
        </div>
        <div className="stat-tile">
          <div className="stat-label">In corso</div>
          <div className="stat-value">{formatNumber(counts.running)}</div>
        </div>
        <div className="stat-tile">
          <div className="stat-label">Completati</div>
          <div className="stat-value">{formatNumber(counts.done)}</div>
        </div>
        <div className="stat-tile">
          <div className="stat-label">Falliti</div>
          <div className="stat-value">{formatNumber(counts.failed)}</div>
        </div>
        <div className="stat-tile">
          <div className="stat-label">Da rivedere</div>
          <div className="stat-value">{formatNumber(counts.needs_review)}</div>
        </div>
        <div className="stat-tile">
          <div className="stat-label">Token stimati</div>
          <div className="stat-value">{formatNumber(totalTokens)}</div>
        </div>
      </div>
    </div>
  );
}
