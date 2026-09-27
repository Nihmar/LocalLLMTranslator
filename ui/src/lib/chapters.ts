/**
 * Aggregations behind the translation page's chapter outline.
 *
 * The chunk rows already carry everything a chapter needs — the chapter id, the status and
 * `target_md` — so the outline and the live preview are derived from the same snapshot the
 * chunk table renders: no extra command, no per-chapter query and no second source of truth
 * that could drift from the table.
 */

import type { Chapter, Chunk } from "./types";

/** Per-chapter aggregates for the translation outline. */
export interface ChapterProgress {
  id: string;
  order_index: number;
  title: string;
  level: number;
  /** Chunks assigned to the chapter. */
  total: number;
  /** Chunks with a usable translation — the predicate the export renderer uses. */
  translated: number;
  running: number;
  failed: number;
  needs_review: number;
  /** Sum of the chunks' `token_estimate`, an estimate of the chapter's size. */
  tokens: number;
}

/**
 * Whether a chunk carries a usable translation.
 *
 * Mirrors `pipeline::export::translated_text`: a missing or whitespace-only `target_md` counts
 * as untranslated, so the outline, the preview and the export cannot disagree.
 */
export function chunkTranslated(chunk: Chunk): boolean {
  return chunk.target_md !== null && chunk.target_md.trim().length > 0;
}

/**
 * One chunk of the live preview, in reading order.
 *
 * A chunk without a usable translation falls back to its source markdown: that is exactly what
 * the export renderer emits, and it keeps the preview readable while the run is still going.
 */
export interface PreviewSegment {
  chunk_id: string;
  order_index: number;
  status: string;
  translated: boolean;
  markdown: string;
}

/**
 * Chunk counts per chapter, in document order. Chapters without chunks are dropped: the
 * preview and the export build skip them, so listing them would offer an action that cannot
 * produce anything.
 */
export function chapterProgress(
  chapters: readonly Chapter[],
  chunks: readonly Chunk[],
): ChapterProgress[] {
  const byChapter = new Map<string, ChapterProgress>();
  for (const chapter of chapters) {
    byChapter.set(chapter.id, {
      id: chapter.id,
      order_index: chapter.order_index,
      title: chapter.title,
      level: chapter.level,
      total: 0,
      translated: 0,
      running: 0,
      failed: 0,
      needs_review: 0,
      tokens: 0,
    });
  }

  for (const chunk of chunks) {
    if (chunk.chapter_id === null) {
      continue;
    }
    const row = byChapter.get(chunk.chapter_id);
    if (row === undefined) {
      continue;
    }
    row.total += 1;
    row.tokens += chunk.token_estimate;
    if (chunkTranslated(chunk)) {
      row.translated += 1;
    }
    if (chunk.status === "running") {
      row.running += 1;
    } else if (chunk.status === "failed") {
      row.failed += 1;
    } else if (chunk.status === "needs_review") {
      row.needs_review += 1;
    }
  }

  return [...byChapter.values()]
    .filter((row) => row.total > 0)
    .sort((left, right) => left.order_index - right.order_index);
}

/** The chunks of one chapter as preview segments, in reading order. */
export function composePreview(
  chunks: readonly Chunk[],
  chapterId: string,
): PreviewSegment[] {
  return chunks
    .filter((chunk) => chunk.chapter_id === chapterId)
    .sort((left, right) => left.order_index - right.order_index)
    .map((chunk) => {
      const target = chunk.target_md;
      const translated = chunkTranslated(chunk);
      return {
        chunk_id: chunk.id,
        order_index: chunk.order_index,
        status: chunk.status,
        translated,
        markdown: translated && target !== null ? target : chunk.source_md,
      };
    });
}
