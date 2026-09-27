import { useEffect, useRef } from "react";
import type { ReactNode } from "react";

/**
 * Reusable modal shell: overlay, titled panel, Escape to close.
 *
 * It exists because more than one view needs to show a wide, scrollable surface without leaving
 * the page (the job monitor first, other LLM activity later), and a `position: fixed` panel per
 * view would drift. The caller owns the content and the actions; the shell owns the layering, the
 * keyboard affordance and the size.
 *
 * `full` is the "ingrandisci" state: the same dialog on almost the whole window, so a long queue
 * does not have to be read through a keyhole.
 */

export type DialogSize = "compact" | "wide" | "full";

export interface DialogProps {
  title: string;
  /** Rendered on the right of the title, before the close button: refresh, expand, ... */
  headerActions?: ReactNode | undefined;
  size?: DialogSize | undefined;
  onClose: () => void;
  children: ReactNode;
}

const SIZE_CLASS: Readonly<Record<DialogSize, string>> = {
  compact: "dialog-compact",
  wide: "dialog-wide",
  full: "dialog-full",
};

export function Dialog({
  title,
  headerActions,
  size = "wide",
  onClose,
  children,
}: DialogProps) {
  const panel = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Focus the panel so Escape reaches the dialog and the tab order starts inside it, instead of
    // leaving the focus on the page behind the overlay.
    panel.current?.focus();
  }, []);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [onClose]);

  return (
    <div className="dialog-overlay">
      {/* Clicking the backdrop closes, like every other dialog in the app; the panel stops the
          propagation so a click inside never dismisses. */}
      <div
        className="dialog-backdrop"
        role="presentation"
        onClick={onClose}
        aria-hidden="true"
      />
      <div
        ref={panel}
        className={`dialog ${SIZE_CLASS[size]}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        tabIndex={-1}
      >
        <div className="dialog-head">
          <span className="panel-title">{title}</span>
          <span className="flex flex-wrap items-center justify-end gap-2">
            {headerActions}
            <button type="button" className="btn btn-sm btn-ghost" onClick={onClose}>
              Chiudi
            </button>
          </span>
        </div>
        <div className="dialog-body">{children}</div>
      </div>
    </div>
  );
}
