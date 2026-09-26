import { useEffect, useRef } from "react";
import { markdown } from "@codemirror/lang-markdown";
import { MergeView } from "@codemirror/merge";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";

/**
 * CodeMirror 6 wrappers for the review screen (`PLAN.md` §1, §11.4).
 *
 * The app is dark, so a small theme built from the design tokens is applied on
 * top of the default light theme. Both editors are read-only: accepting or
 * rejecting a proposal is a control-plane operation, not a direct edit, so the
 * review never mutates the text locally.
 */

const darkTheme = EditorView.theme(
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
      backgroundColor: "rgba(91, 140, 255, 0.28)",
    },
    ".cm-gutters": {
      backgroundColor: "var(--color-surface-2)",
      color: "var(--color-faint)",
      border: "none",
    },
    ".cm-activeLine": { backgroundColor: "rgba(30, 41, 65, 0.45)" },
    ".cm-activeLineGutter": {
      backgroundColor: "var(--color-surface-3)",
      color: "var(--color-ink-soft)",
    },
    ".cm-changedLine": { backgroundColor: "rgba(52, 211, 153, 0.08)" },
    ".cm-changedText": {
      backgroundColor: "rgba(52, 211, 153, 0.28)",
      textDecoration: "underline",
      textDecorationColor: "var(--color-ok)",
    },
    ".cm-changedLineGutter": { backgroundColor: "rgba(52, 211, 153, 0.14)" },
    ".cm-deletedChunk": { backgroundColor: "rgba(248, 113, 113, 0.08)" },
    ".cm-deletedText": { backgroundColor: "rgba(248, 113, 113, 0.3)" },
    ".cm-mergeSpacer": { backgroundColor: "var(--color-surface-2)" },
    ".cm-mergeView .cm-changedLine": { borderColor: "var(--color-ok)" },
  },
  { dark: true },
);

function extensions(readOnly: boolean): Extension[] {
  return [
    darkTheme,
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
