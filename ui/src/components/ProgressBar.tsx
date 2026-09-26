import { formatNumber, formatPercent, percent } from "../lib/format";

export type ProgressTone = "accent" | "success" | "warning" | "danger";

export interface ProgressBarProps {
  value: number;
  total: number;
  /** Caption rendered above the track. */
  label?: string | undefined;
  tone?: ProgressTone | undefined;
  /** Renders `12 / 340` next to the percentage. */
  showCounts?: boolean | undefined;
  /** Ignores `value`/`total` and shows a moving stripe. */
  indeterminate?: boolean | undefined;
  /** Overrides the percentage shown on the right. */
  hint?: string | undefined;
}

const TONE_COLOR: Readonly<Record<ProgressTone, string>> = {
  accent: "var(--color-accent)",
  success: "var(--color-ok)",
  warning: "var(--color-warn)",
  danger: "var(--color-danger)",
};

export function ProgressBar({
  value,
  total,
  label,
  tone = "accent",
  showCounts = false,
  indeterminate = false,
  hint,
}: ProgressBarProps) {
  const ratio = percent(value, total);
  const hasLabel = label !== undefined || showCounts || hint !== undefined;

  return (
    <div className="w-full">
      {hasLabel ? (
        <div className="mb-1 flex items-baseline justify-between gap-3">
          <span className="text-xs font-medium text-ink-soft">{label ?? ""}</span>
          <span className="font-mono text-xs text-muted tabular-nums">
            {showCounts ? `${formatNumber(value)} / ${formatNumber(total)} · ` : null}
            {hint ?? formatPercent(ratio)}
          </span>
        </div>
      ) : null}

      <div
        className="progress-track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={indeterminate ? undefined : Math.round(ratio)}
        aria-label={label ?? "Avanzamento"}
      >
        <div
          className={`progress-fill ${indeterminate ? "animate-pulse" : ""}`}
          style={{
            width: indeterminate ? "100%" : `${ratio}%`,
            backgroundColor: TONE_COLOR[tone],
            opacity: indeterminate ? 0.45 : 1,
          }}
        />
      </div>
    </div>
  );
}
