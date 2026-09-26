/**
 * Word-level diff for the review screen.
 *
 * The review only needs to *show* what a suggestion changes: the app-owned diff keeps the UI
 * dependency-free (see the deviation recorded in `PLAN.md` §1/§11.4) and is unit-tested with
 * `node:test`. The algorithm is a classic longest-common-subsequence over words and whitespace
 * runs; inputs larger than `MAX_TOKENS` fall back to a coarse replacement so a pathological
 * block cannot freeze the page.
 */

export type DiffKind = "equal" | "removed" | "added";

export interface DiffSegment {
  kind: DiffKind;
  text: string;
}

/** Above this many tokens per side the fine-grained diff is skipped. */
export const MAX_TOKENS = 2000;

function tokenize(text: string): string[] {
  return text.split(/(\s+)/).filter((token) => token.length > 0);
}

function coarse(before: string, after: string): DiffSegment[] {
  const segments: DiffSegment[] = [];
  if (before.length > 0) {
    segments.push({ kind: "removed", text: before });
  }
  if (after.length > 0) {
    segments.push({ kind: "added", text: after });
  }
  return segments;
}

/** Longest common subsequence table, walk-back included. */
function lcsSegments(beforeTokens: string[], afterTokens: string[]): DiffSegment[] {
  const rows = beforeTokens.length;
  const columns = afterTokens.length;
  const table = new Uint32Array((rows + 1) * (columns + 1));
  const at = (row: number, column: number): number => table[row * (columns + 1) + column] as number;

  for (let row = rows - 1; row >= 0; row -= 1) {
    for (let column = columns - 1; column >= 0; column -= 1) {
      const value =
        beforeTokens[row] === afterTokens[column]
          ? at(row + 1, column + 1) + 1
          : Math.max(at(row + 1, column), at(row, column + 1));
      table[row * (columns + 1) + column] = value;
    }
  }

  const raw: DiffSegment[] = [];
  const push = (kind: DiffKind, text: string) => {
    const last = raw[raw.length - 1];
    if (last !== undefined && last.kind === kind) {
      last.text += text;
    } else {
      raw.push({ kind, text });
    }
  };

  let row = 0;
  let column = 0;
  while (row < rows && column < columns) {
    if (beforeTokens[row] === afterTokens[column]) {
      push("equal", beforeTokens[row] as string);
      row += 1;
      column += 1;
    } else if (at(row + 1, column) >= at(row, column + 1)) {
      push("removed", beforeTokens[row] as string);
      row += 1;
    } else {
      push("added", afterTokens[column] as string);
      column += 1;
    }
  }
  while (row < rows) {
    push("removed", beforeTokens[row] as string);
    row += 1;
  }
  while (column < columns) {
    push("added", afterTokens[column] as string);
    column += 1;
  }
  return raw;
}

/**
 * Diff `before` and `after` into segments. Concatenating the `equal` and `removed` segments
 * reproduces `before`; `equal` plus `added` reproduces `after`.
 */
export function diffWords(before: string, after: string): DiffSegment[] {
  if (before === after) {
    return before.length === 0 ? [] : [{ kind: "equal", text: before }];
  }
  const beforeTokens = tokenize(before);
  const afterTokens = tokenize(after);
  if (beforeTokens.length > MAX_TOKENS || afterTokens.length > MAX_TOKENS) {
    return coarse(before, after);
  }
  return lcsSegments(beforeTokens, afterTokens);
}
