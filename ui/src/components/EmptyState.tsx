import type { ReactNode } from "react";

/**
 * Single component for the three non-content states a view can be in: nothing to show, a failed
 * load, or work in progress. Keeping them in one place guarantees no view renders a blank page.
 */

export type EmptyStateTone = "empty" | "error" | "loading";

export interface EmptyStateProps {
  tone?: EmptyStateTone | undefined;
  title: string;
  description?: string | undefined;
  /** Raw backend message, rendered in monospace under the description. */
  details?: string | null | undefined;
  actionLabel?: string | undefined;
  onAction?: (() => void) | undefined;
  secondaryActionLabel?: string | undefined;
  onSecondaryAction?: (() => void) | undefined;
  compact?: boolean | undefined;
  children?: ReactNode | undefined;
}

function EmptyGlyph() {
  return (
    <svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M14 3v5h5" />
      <path d="M19 8v11a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h7z" />
      <path d="M9 13h6M9 17h4" />
    </svg>
  );
}

function ErrorGlyph() {
  return (
    <svg viewBox="0 0 24 24" width="26" height="26" fill="none" stroke="currentColor" strokeWidth="1.6">
      <path d="M12 4 3 19h18z" />
      <path d="M12 10v4M12 17h.01" />
    </svg>
  );
}

export function EmptyState({
  tone = "empty",
  title,
  description,
  details,
  actionLabel,
  onAction,
  secondaryActionLabel,
  onSecondaryAction,
  compact = false,
  children,
}: EmptyStateProps) {
  const containerTone =
    tone === "error" ? "border-danger bg-danger-soft" : "border-line-strong bg-surface";

  const iconTone =
    tone === "error" ? "text-danger" : tone === "loading" ? "text-accent" : "text-faint";

  const hasActions =
    (actionLabel !== undefined && onAction !== undefined) ||
    (secondaryActionLabel !== undefined && onSecondaryAction !== undefined);

  return (
    <div
      role={tone === "error" ? "alert" : undefined}
      className={`flex flex-col items-center justify-center gap-2 rounded-xl border border-dashed text-center ${
        compact ? "px-4 py-6" : "px-6 py-12"
      } ${containerTone}`}
    >
      <span className={`flex items-center justify-center ${iconTone}`}>
        {tone === "loading" ? (
          <span className="spinner" aria-hidden="true" />
        ) : tone === "error" ? (
          <ErrorGlyph />
        ) : (
          <EmptyGlyph />
        )}
      </span>

      <p className="text-sm font-semibold text-ink">{title}</p>

      {description !== undefined ? <p className="max-w-prose text-xs text-muted">{description}</p> : null}

      {details !== undefined && details !== null && details.length > 0 ? (
        <pre className="mt-1 max-h-40 max-w-full overflow-auto rounded-md border border-line bg-canvas px-3 py-2 text-left font-mono text-[0.7rem] leading-relaxed whitespace-pre-wrap text-danger">
          {details}
        </pre>
      ) : null}

      {children !== undefined ? <div className="mt-2 w-full">{children}</div> : null}

      {hasActions ? (
        <div className="mt-2 flex flex-wrap items-center justify-center gap-2">
          {actionLabel !== undefined && onAction !== undefined ? (
            <button type="button" className="btn btn-primary" onClick={onAction}>
              {actionLabel}
            </button>
          ) : null}
          {secondaryActionLabel !== undefined && onSecondaryAction !== undefined ? (
            <button type="button" className="btn" onClick={onSecondaryAction}>
              {secondaryActionLabel}
            </button>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
