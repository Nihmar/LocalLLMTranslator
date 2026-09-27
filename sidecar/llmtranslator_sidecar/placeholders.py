r"""Inline placeholder substitution.

Asking a model to "preserve the Markdown" does not survive contact with real text: URLs
get translated, inline code gets mangled, footnote markers drift. So inline markup is
lifted out of the string entirely and replaced with opaque tokens, and the literals are
put back afterwards from the map.

Delimiters keep their content translatable, because the words inside are prose::

    **La città**          ->  ⟦1⟧La città⟦2⟧
    [il re](https://x)    ->  ⟦3⟧il re⟦4⟧        (the URL never reaches the model)

Opaque atoms are replaced by a single token whose content never enters the prompt::

    `x = 1`               ->  ⟦5⟧
    $E=mc^2$              ->  ⟦6⟧
    [^3]                  ->  ⟦7⟧

So is a line that opens with an escaped block marker — the dash a book prints for dialogue,
escaped by the extractor so the paragraph does not read as a list. See it and the model answers
``- Bonjour``::

    \- Bonjour            ->  ⟦8⟧ Bonjour

The map is a pure function of the input, so it never needs to be persisted: regenerating
it from the same source always yields the same tokens in the same order.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from collections.abc import Sequence

#: Primary token form. Rare enough that models almost always copy it verbatim.
_OPEN = "\u27e6"
_CLOSE = "\u27e7"

_TOKEN_RE = re.compile(rf"{_OPEN}\s*(\d{{1,6}})\s*{_CLOSE}")

#: Accepted variants, tried only for tokens still missing after the primary pass. Keeping
#: them as a fallback matters: a bare ``[3]`` in the prose is a legitimate citation, so it
#: must not be claimed as a placeholder unless nothing else explains it.
_VARIANT_RES: tuple[re.Pattern[str], ...] = (
    re.compile(r"\u3010\s*(\d{1,6})\s*\u3011"),  # 【3】
    re.compile(r"\[\[\s*(\d{1,6})\s*\]\]"),  # [[3]]
    re.compile(r"\[\s*(\d{1,6})\s*\]"),  # [3]
)

_MASTER_RE = re.compile(
    # A line that opens with an *escaped* block marker: the dash a book prints for dialogue,
    # which the extractor escapes so the paragraph does not read as a list. The model must
    # not see it — asked to translate ``\- Bonjour`` it answers ``- Bonjour``, and a list is
    # not the paragraph the chunk declared — so the escape travels as a token like any other
    # literal the model is not allowed to touch.
    r"(?P<escape>(?m:^(?:\\[-+*#>|]|\d{1,9}\\[.)])))"
    r"|(?P<code>(?P<code_fence>`+)(?P<code_body>.+?)(?P=code_fence))"
    r"|(?P<math_block>\$\$.+?\$\$)"
    r"|(?P<math>\$[^$\n]+?\$)"
    r"|(?P<image>!\[[^\]]*\]\([^)\s]*\))"
    r"|(?P<link>\[(?P<link_text>[^\]]*)\]\((?P<link_url>[^)\s]*)\))"
    r"|(?P<footnote>\[\^[^\]]+\])"
    r"|(?P<autolink><[A-Za-z][A-Za-z0-9+.-]*:[^>]*>)"
    r"|(?P<url>[A-Za-z][A-Za-z0-9+.-]*://[^\s<>\)\]]+)"
    r"|(?P<html></?[A-Za-z][^>\n]*>)"
    r"|(?P<strong>\*\*(?P<strong_body>.+?)\*\*)"
    r"|(?P<strong_underscore>__(?P<strong_underscore_body>.+?)__)"
    r"|(?P<strike>~~(?P<strike_body>.+?)~~)"
    r"|(?P<em>\*(?P<em_body>[^*\n]+?)\*)"
    r"|(?P<em_underscore>_(?P<em_underscore_body>[^_\n]+?)_)",
)

_OPAQUE_KINDS = frozenset(
    {"code", "math_block", "math", "image", "footnote", "autolink", "url", "html", "escape"},
)
_PAIR_KINDS = frozenset({"strong", "strong_underscore", "strike", "em", "em_underscore"})

#: Names in the order the alternatives appear, so the matched one can be identified
#: without relying on ``lastgroup`` (which reports the innermost group).
_ALTERNATIVES: tuple[str, ...] = (
    "escape",
    "code",
    "math_block",
    "math",
    "image",
    "link",
    "footnote",
    "autolink",
    "url",
    "html",
    "strong",
    "strong_underscore",
    "strike",
    "em",
    "em_underscore",
)

_DELIMITERS: dict[str, str] = {
    "strong": "**",
    "strong_underscore": "__",
    "strike": "~~",
    "em": "*",
    "em_underscore": "_",
}


@dataclass(slots=True)
class ReinjectResult:
    """Outcome of putting the literals back into a model response."""

    text: str
    ok: bool
    missing: list[int] = field(default_factory=list[int])
    duplicated: list[int] = field(default_factory=list[int])
    reordered: bool = False
    #: Indices the model invented (never allocated by :func:`substitute`).
    unknown: list[int] = field(default_factory=list[int])


class _Builder:
    """Walks the text once, allocating placeholder numbers in output order."""

    def __init__(self) -> None:
        self.mapping: list[list[Any]] = []

    def token(self, literal: str) -> str:
        """Allocate the next number for ``literal`` and return its token."""
        self.mapping.append([len(self.mapping) + 1, literal])
        return f"{_OPEN}{len(self.mapping)}{_CLOSE}"


def _alternative(match: re.Match[str]) -> str:
    for name in _ALTERNATIVES:
        if match.group(name) is not None:
            return name
    return ""


def _scan(text: str, builder: _Builder) -> str:
    out: list[str] = []
    position = 0
    for match in _MASTER_RE.finditer(text):
        out.append(text[position : match.start()])
        position = match.end()
        kind = _alternative(match)
        if kind in _OPAQUE_KINDS or kind == "link":
            if kind == "link":
                out.append(builder.token("["))
                out.append(_scan(match.group("link_text") or "", builder))
                out.append(builder.token(f"]({match.group('link_url') or ''})"))
            else:
                out.append(builder.token(match.group(0)))
        elif kind in _PAIR_KINDS:
            delimiter = _DELIMITERS[kind]
            out.append(builder.token(delimiter))
            out.append(_scan(match.group(f"{kind}_body") or "", builder))
            out.append(builder.token(delimiter))
        else:
            out.append(match.group(0))
    out.append(text[position:])
    return "".join(out)


def substitute(text: str) -> tuple[str, list[list[Any]]]:
    """Replace inline markup with placeholder tokens.

    Returns the text to send to the model and the ``[[index, literal], ...]`` map needed
    to rebuild it. The map is a pure function of ``text``.
    """
    builder = _Builder()
    return _scan(text, builder), builder.mapping


def _primary_occurrences(text: str) -> list[tuple[int, int, int]]:
    return [(m.start(), m.end(), int(m.group(1))) for m in _TOKEN_RE.finditer(text)]


def _fallback_occurrences(
    text: str,
    wanted: set[int],
    covered: list[tuple[int, int]],
) -> list[tuple[int, int, int]]:
    """Look for variant spellings of the tokens still missing, skipping claimed spans.

    Only tokens that the primary pass did not find are considered, which is what keeps a
    legitimate ``[3]`` citation in the prose from being mistaken for a placeholder.
    """
    found: list[tuple[int, int, int]] = []
    claimed = list(covered)
    for pattern in _VARIANT_RES:
        for match in pattern.finditer(text):
            index = int(match.group(1))
            if index not in wanted:
                continue
            if any(start < match.end() and match.start() < end for start, end in claimed):
                continue
            found.append((match.start(), match.end(), index))
            claimed.append((match.start(), match.end()))
    return found


def reinject(text: str, placeholders: Sequence[Sequence[Any]]) -> ReinjectResult:
    """Put the literals back, tolerating the ways models mangle the tokens."""
    literals: dict[int, str] = {int(pair[0]): str(pair[1]) for pair in placeholders}
    if not literals:
        return ReinjectResult(text=text, ok=True)

    expected = set(literals)
    occurrences = _primary_occurrences(text)
    seen = {index for _, _, index in occurrences}

    wanted = {index for index in expected if index not in seen}
    if wanted:
        occurrences.extend(
            _fallback_occurrences(text, wanted, [(s, e) for s, e, _ in occurrences]),
        )
    occurrences.sort(key=lambda item: item[0])

    # A model can invent an index that was never allocated (⟦99⟧, ⟦0⟧). That is a mangled
    # token, not a literal: the placement loop below drops it from the text instead of
    # raising, and it is reported so the caller can retry.
    unknown = sorted({index for _, _, index in occurrences if index not in literals})
    known = [item for item in occurrences if item[2] in literals]

    counts: dict[int, int] = {}
    for _, _, index in known:
        counts[index] = counts.get(index, 0) + 1

    missing = sorted(index for index in expected if counts.get(index, 0) == 0)
    duplicated = sorted(index for index, count in counts.items() if count > 1)

    # Complete but out of order: the model has moved a token, so the positions no longer
    # mean anything. Re-place the literals in the order they should appear.
    ordered_indices = [index for _, _, index in known]
    reordered = (
        not missing
        and not duplicated
        and not unknown
        and ordered_indices != sorted(ordered_indices)
    )

    out: list[str] = []
    position = 0
    placed: set[int] = set()
    for order, (start, end, index) in enumerate(occurrences):
        out.append(text[position:start])
        position = end
        if index not in literals:
            continue  # an invented token leaves no trace in the output
        if reordered:
            replacement = literals[sorted(expected)[order]]
        elif index in placed:
            replacement = ""  # a duplicate token is dropped; the first one already stood in
        else:
            replacement = literals[index]
            placed.add(index)
        out.append(replacement)
    out.append(text[position:])

    return ReinjectResult(
        text="".join(out),
        ok=not missing and not duplicated and not unknown,
        missing=missing,
        duplicated=duplicated,
        reordered=reordered,
        unknown=unknown,
    )
