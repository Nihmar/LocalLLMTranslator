/**
 * Status badge with the Italian wording for every state the backend can report.
 *
 * Unknown values fall back to the raw string instead of being hidden, so a new backend state
 * shows up (untranslated) rather than silently disappearing.
 */

export type BadgeTone = "neutral" | "accent" | "info" | "success" | "warning" | "danger";

interface BadgeDescriptor {
  label: string;
  tone: BadgeTone;
}

const STATUS_MAP: Readonly<Record<string, BadgeDescriptor>> = {
  // chunk.status
  pending: { label: "In attesa", tone: "neutral" },
  running: { label: "In corso", tone: "accent" },
  done: { label: "Completato", tone: "success" },
  failed: { label: "Fallito", tone: "danger" },
  needs_review: { label: "Da rivedere", tone: "warning" },
  // job.state
  leased: { label: "Assegnato", tone: "info" },
  cancelled: { label: "Annullato", tone: "neutral" },
  // sidecar://status
  starting: { label: "Avvio", tone: "info" },
  ready: { label: "Pronto", tone: "success" },
  restarting: { label: "Riavvio", tone: "warning" },
  stopped: { label: "Arrestato", tone: "neutral" },
  error: { label: "Errore", tone: "danger" },
  // health / resources
  ok: { label: "Raggiungibile", tone: "success" },
  unreachable: { label: "Non raggiungibile", tone: "danger" },
  untested: { label: "Non verificato", tone: "neutral" },
  degraded: { label: "Modalità seriale", tone: "warning" },
  parallel: { label: "Parallelo", tone: "success" },
  // export build outcome
  cached: { label: "Riutilizzato", tone: "neutral" },
  // qa findings / severity
  critical: { label: "Critico", tone: "danger" },
  major: { label: "Grave", tone: "warning" },
  minor: { label: "Minore", tone: "info" },
  info: { label: "Informativo", tone: "neutral" },
  open: { label: "Aperto", tone: "danger" },
  resolved: { label: "Risolto", tone: "success" },
  ignored: { label: "Ignorato", tone: "neutral" },
  // glossary / series glossary term status
  approved: { label: "Approvato", tone: "success" },
  candidate: { label: "Candidato", tone: "info" },
  conflict: { label: "Conflitto", tone: "warning" },
  rejected: { label: "Rifiutato", tone: "neutral" },
};

const TONE_CLASS: Readonly<Record<BadgeTone, string>> = {
  neutral: "badge badge-neutral",
  accent: "badge badge-accent",
  info: "badge badge-info",
  success: "badge badge-success",
  warning: "badge badge-warning",
  danger: "badge badge-danger",
};

/** Italian label for a backend status token. */
export function statusLabel(status: string): string {
  return STATUS_MAP[status]?.label ?? status;
}

/** Visual tone for a backend status token. */
export function statusTone(status: string): BadgeTone {
  return STATUS_MAP[status]?.tone ?? "neutral";
}

export interface StatusBadgeProps {
  /** Raw status token from the backend (`pending`, `done`, `degraded`, ...). */
  status: string;
  /** Overrides the Italian label derived from `status`. */
  label?: string | undefined;
  /** Overrides the tone derived from `status`. */
  tone?: BadgeTone | undefined;
  /** Adds a pulsing dot, for states that are still moving. */
  pulse?: boolean | undefined;
}

export function StatusBadge({ status, label, tone, pulse = false }: StatusBadgeProps) {
  const resolvedTone = tone ?? statusTone(status);
  const resolvedLabel = label ?? statusLabel(status);

  return (
    <span className={TONE_CLASS[resolvedTone]} title={status}>
      {pulse ? (
        <span className="inline-block size-1.5 shrink-0 animate-pulse rounded-full bg-current" />
      ) : null}
      {resolvedLabel}
    </span>
  );
}
