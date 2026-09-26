import type { DiffSegment } from "../lib/diff";

/**
 * Renders one side of a word diff. The `added` segments are hidden on the
 * before side and the `removed` ones on the after side, so the two `<pre>`
 * blocks of the review always read like the text they represent, with the
 * changed spans highlighted in place.
 */
export function DiffText({
  segments,
  side,
  fallback,
}: {
  segments: readonly DiffSegment[];
  side: "before" | "after";
  fallback: string;
}) {
  const hidden = side === "before" ? "added" : "removed";
  const highlighted = side === "before" ? "removed" : "added";
  const parts = segments.filter((segment) => segment.kind !== hidden);
  if (parts.length === 0) {
    return <>{fallback}</>;
  }
  return (
    <>
      {parts.map((segment, index) =>
        segment.kind === highlighted ? (
          <span key={index} className={highlighted === "removed" ? "diff-removed" : "diff-added"}>
            {segment.text}
          </span>
        ) : (
          <span key={index}>{segment.text}</span>
        ),
      )}
    </>
  );
}
