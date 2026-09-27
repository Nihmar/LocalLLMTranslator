/**
 * Queue vocabulary shared by the job dashboard and the translation page: the Italian label of
 * every job kind, the states in which a job occupies a worker, and the chunk a job works on —
 * which lives in `payload_json`, not on the job row.
 */

/** Italian label per job kind. Unknown kinds are shown verbatim, never hidden. */
export const KIND_LABELS: Readonly<Record<string, string>> = {
  ingest: "Ingestione",
  translate_chunk: "Traduzione chunk",
  summarize: "Riassunto",
  book_recon: "Ricognizione",
  edit_chunk: "Revisione editor",
  proofread_chunk: "Proofread",
  qa_scan: "Scansione QA",
  export_unit: "Export",
};

export function jobKindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind;
}

/** States in which a job occupies a worker: it has been claimed and has not finished yet. */
const ACTIVE_STATES: ReadonlySet<string> = new Set(["running", "leased"]);

export function isActiveJobState(state: string): boolean {
  return ACTIVE_STATES.has(state);
}

/** The chunk a job belongs to lives in its payload, not on the job row. */
export function payloadChunkId(payloadJson: string): string | null {
  try {
    const parsed: unknown = JSON.parse(payloadJson);
    if (typeof parsed === "object" && parsed !== null) {
      const value = (parsed as { chunk_id?: unknown }).chunk_id;
      return typeof value === "string" ? value : null;
    }
  } catch {
    // A malformed payload must not break the progress trigger.
  }
  return null;
}
