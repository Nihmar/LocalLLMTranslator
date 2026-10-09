import { useEffect, useRef } from "react";
import { markdown } from "@codemirror/lang-markdown";
import { MergeView } from "@codemirror/merge";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

/**
 * CodeMirror 6 wrappers for the review screen (`PLAN.md` §1, §11.4).
 *
 * A small theme built from the design tokens is applied on top of the default
 * light theme, so the diff follows the app palette. Both editors are read-only: accepting or
 * rejecting a proposal is a control-plane operation, not a direct edit, so the
 * review never mutates the text locally.
 */

const tokenTheme = EditorView.theme(
  {
    "&": {
      color: "var(--color-ink)",
      backgroundColor: "var(--color-surface)",
      fontSize: "0.72rem",
    },
    ".cm-content": {
      caretColor: "var(--color-accent)",
      fontFamily: "var(--font-mono)",
      lineHeight: "1.6",
    },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--color-accent)" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
      backgroundColor: "rgba(14, 106, 101, 0.18)",
    },
    ".cm-gutters": {
      backgroundColor: "var(--color-surface-2)",
      color: "var(--color-faint)",
      border: "none",
    },
    ".cm-activeLine": { backgroundColor: "var(--color-surface-2)" },
    ".cm-activeLineGutter": {
      backgroundColor: "var(--color-surface-3)",
      color: "var(--color-ink-soft)",
    },
    ".cm-changedLine": { backgroundColor: "rgba(47, 107, 47, 0.06)" },
    ".cm-changedText": {
      backgroundColor: "var(--color-ok-soft)",
      textDecoration: "underline",
      textDecorationColor: "var(--color-ok)",
    },
    ".cm-changedLineGutter": { backgroundColor: "var(--color-ok-soft)" },
    ".cm-deletedChunk": { backgroundColor: "rgba(161, 52, 42, 0.05)" },
    ".cm-deletedText": { backgroundColor: "var(--color-danger-soft)" },
    ".cm-mergeSpacer": { backgroundColor: "var(--color-surface-2)" },
    ".cm-mergeView .cm-changedLine": { borderColor: "var(--color-ok)" },
  },
  { dark: false },
);

function extensions(readOnly: boolean): Extension[] {
  return [
    tokenTheme,
    markdown(),
    EditorView.lineWrapping,
    EditorState.readOnly.of(readOnly),
    EditorView.editable.of(!readOnly),
  ];
}

/** A single read-only markdown editor (the source column). */
export function ReadOnlyCode({ text }: { text: string }) {
  const host = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const parent = host.current;
    if (parent === null) {
      return;
    }
    const view = new EditorView({
      doc: text,
      extensions: extensions(true),
      parent,
    });
    return () => {
      view.destroy();
    };
  }, [text]);

  return <div ref={host} className="codemirror-host" />;
}

/** Side-by-side merge view: current translation (a) against the proposal (b). */
export function MergeDiff({ before, after }: { before: string; after: string }) {
  const host = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const parent = host.current;
    if (parent === null) {
      return;
    }
    const changed = before !== after;
    const view = new MergeView({
      a: { doc: before, extensions: extensions(true) },
      b: { doc: after, extensions: extensions(true) },
      parent,
      orientation: "a-b",
      highlightChanges: true,
      gutter: true,
      // Without a change the whole document would collapse into a placeholder,
      // which would hide the block being browsed.
      ...(changed ? { collapseUnchanged: { margin: 3, minSize: 4 } } : {}),
    });
    return () => {
      view.destroy();
    };
  }, [before, after]);

  return <div ref={host} className="codemirror-host codemirror-merge" />;
}
