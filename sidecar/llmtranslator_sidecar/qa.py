"""Heuristic quality checks on a translated chunk.

The checks are deliberately conservative: the goal is to surface the handful of failures a
human reviewer must see, not to second-guess every stylistic choice of the model. Each kind
is emitted at most once per chunk, except where a controlled input can legitimately produce
several instances (duplicates, leftover Latin runs, glossary problems).

Severities (single source of truth in :data:`_SEVERITY`):

- ``critical`` — the translation is structurally broken: ``placeholder_broken``, ``empty``;
- ``major`` — the translation is present but wrong: ``markdown_malformed``,
  ``untranslated``, ``glossary_mismatch``;
- ``minor`` — worth a look, not necessarily a defect: ``length_anomaly``, ``duplicate``,
  ``latin_leftover``, ``glossary_conflict``.

Length band. A translation lengthens or shortens its source by a fair margin, so
``length_anomaly`` only fires when the target/source character ratio leaves the wide band
``[0.2, 8.0]`` (``_LENGTH_RATIO_MIN``/``_LENGTH_RATIO_MAX``), and only when the source is at
least ``_LENGTH_MIN_SOURCE_CHARS`` long: below that the ratio is pure noise. This is meant
to catch a dropped section or a runaway repetition, not an ordinary expansion.
"""

from __future__ import annotations

import re
from itertools import pairwise
from typing import TYPE_CHECKING, Any

from .parse import split_blocks
from .placeholders import reinject

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

    from .blocks import Block

#: Kind -> severity. Every finding kind must appear here.
_SEVERITY: dict[str, str] = {
    "placeholder_broken": "critical",
    "empty": "critical",
    "markdown_malformed": "major",
    "untranslated": "major",
    "glossary_mismatch": "major",
    "length_anomaly": "minor",
    "duplicate": "minor",
    "latin_leftover": "minor",
    "glossary_conflict": "minor",
}

#: Target/source character ratio band; outside it a length_anomaly is reported.
_LENGTH_RATIO_MIN = 0.2
_LENGTH_RATIO_MAX = 8.0

#: Below this source length the ratio is noise and is not evaluated.
_LENGTH_MIN_SOURCE_CHARS = 80

#: An identical run of at least this many words marks the target as untranslated.
_UNTRANSLATED_MIN_WORDS = 8

#: Latin words per run before a leftover is reported, and before it is "long" (major).
_LATIN_MIN_WORDS = 3
_LATIN_LONG_WORDS = 10

#: Above this share of Latin letters the target is Latin-script, where every run matches
#: the source by coincidence; latin_leftover is then suppressed as noise.
_LATIN_SCRIPT_MAX_RATIO = 0.5

_WORD_RE = re.compile(r"\w+", re.UNICODE)
_SENTENCE_RE = re.compile(r"(?<=[.!?\u2026\u3002\uff01\uff1f])\s+")
_LIST_ITEM_RE = re.compile(r"^(\s*)([-*+]|\d{1,9}[.)])(\s+)(.*)$")
_LATIN_RUN_RE = re.compile(
    r"[A-Za-z]+(?:['\u2019-][A-Za-z]+)*(?:[ \t]+[A-Za-z]+(?:['\u2019-][A-Za-z]+)*)*",
)
_LATIN_WORD_RE = re.compile(r"[A-Za-z]+(?:['\u2019-][A-Za-z]+)*")


def _finding(
    kind: str,
    details: dict[str, Any],
    *,
    severity: str | None = None,
) -> dict[str, Any]:
    return {
        "kind": kind,
        "severity": severity or _SEVERITY[kind],
        "block_id": None,
        "details": details,
    }


def _contains(haystack: str, needle: str) -> bool:
    """Case-insensitive, word-boundary search, so ``cat`` does not match ``category``."""
    if not needle:
        return False
    pattern = re.compile(r"(?<!\w)" + re.escape(needle) + r"(?!\w)", re.IGNORECASE | re.UNICODE)
    return pattern.search(haystack) is not None


def _placeholder_findings(
    placeholders: Sequence[Sequence[Any]],
    target_text: str,
) -> list[dict[str, Any]]:
    if not placeholders:
        return []
    result = reinject(target_text, placeholders)
    if result.ok:
        return []
    return [
        _finding(
            "placeholder_broken",
            {
                "missing": result.missing,
                "duplicated": result.duplicated,
                "unknown": result.unknown,
            },
        ),
    ]


def _list_markers(markdown: str) -> list[str]:
    markers: list[str] = []
    for line in markdown.split("\n"):
        match = _LIST_ITEM_RE.match(line)
        if match is not None:
            marker = match.group(2)
            markers.append("ordered" if marker[0].isdigit() else marker)
    return markers


def _table_pipes(markdown: str) -> list[int]:
    return [line.count("|") for line in markdown.split("\n")]


def _fence(markdown: str) -> dict[str, Any] | None:
    lines = markdown.split("\n")
    opening = lines[0].lstrip(" ") if lines else ""
    if not opening.startswith(("```", "~~~")):
        return None
    char = opening[0]
    width = len(opening) - len(opening.lstrip(char))
    closing = re.compile(rf"^ {{0,3}}{re.escape(char)}{{{width},}}[ \t]*$")
    balanced = len(lines) > 1 and closing.match(lines[-1]) is not None
    # Only the fence's shape is compared: the info string is free-form and a translation
    # that merely normalises its whitespace must not be flagged as malformed Markdown.
    return {"char": char, "width": width, "balanced": balanced}


def _block_structure(block: Block) -> dict[str, Any]:
    structure: dict[str, Any] = {"kind": block.kind}
    if block.kind == "heading":
        structure["level"] = block.level
    elif block.kind == "list":
        structure["markers"] = _list_markers(block.source_md)
    elif block.kind == "table":
        structure["pipes"] = _table_pipes(block.source_md)
    elif block.kind == "code":
        structure["fence"] = _fence(block.source_md)
    return structure


def _markdown_issues(source_text: str, target_text: str) -> list[dict[str, Any]]:
    source_blocks = split_blocks(source_text)
    target_blocks = split_blocks(target_text)
    source_kinds = [block.kind for block in source_blocks]
    target_kinds = [block.kind for block in target_blocks]
    if source_kinds != target_kinds:
        return [{"reason": "block_sequence", "source": source_kinds, "target": target_kinds}]

    issues: list[dict[str, Any]] = []
    for source_block, target_block in zip(source_blocks, target_blocks, strict=True):
        source_structure = _block_structure(source_block)
        target_structure = _block_structure(target_block)
        if source_structure != target_structure:
            issues.append(
                {
                    "reason": "block_structure",
                    "source": source_structure,
                    "target": target_structure,
                },
            )
    return issues


def _markdown_findings(source_text: str, target_text: str) -> list[dict[str, Any]]:
    issues = _markdown_issues(source_text, target_text)
    if not issues:
        return []
    return [_finding("markdown_malformed", {"issues": issues})]


def _words(text: str) -> list[str]:
    return [word.casefold() for word in _WORD_RE.findall(text)]


def _longest_common_run(source_words: list[str], target_words: list[str], min_words: int) -> int:
    """Length of the longest run of identical words shared by both sides.

    Returns 0 when either side is shorter than ``min_words`` or no run reaches that length.
    """
    if len(source_words) < min_words or len(target_words) < min_words:
        return 0
    index: dict[tuple[str, ...], list[int]] = {}
    for start in range(len(source_words) - min_words + 1):
        index.setdefault(tuple(source_words[start : start + min_words]), []).append(start)

    best = 0
    for position in range(len(target_words) - min_words + 1):
        for start in index.get(tuple(target_words[position : position + min_words]), []):
            length = min_words
            while (
                start + length < len(source_words)
                and position + length < len(target_words)
                and source_words[start + length] == target_words[position + length]
            ):
                length += 1
            best = max(best, length)
    return best


def _untranslated_findings(source_text: str, target_text: str) -> list[dict[str, Any]]:
    run = _longest_common_run(_words(source_text), _words(target_text), _UNTRANSLATED_MIN_WORDS)
    if run < _UNTRANSLATED_MIN_WORDS:
        return []
    return [_finding("untranslated", {"run_words": run, "min_words": _UNTRANSLATED_MIN_WORDS})]


def _sentences(text: str) -> list[str]:
    return [sentence.strip() for sentence in _SENTENCE_RE.split(text) if sentence.strip()]


def _duplicate_findings(source_text: str, target_text: str) -> list[dict[str, Any]]:
    target_sentences = _sentences(target_text)
    source_sentences = _sentences(source_text)
    source_pairs = {
        (first.casefold(), second.casefold()) for first, second in pairwise(source_sentences)
    }

    findings: list[dict[str, Any]] = []
    seen: set[str] = set()
    for first, second in pairwise(target_sentences):
        if first.casefold() != second.casefold():
            continue
        if (first.casefold(), second.casefold()) in source_pairs or first in seen:
            continue
        seen.add(first)
        findings.append(_finding("duplicate", {"sentence": first}))
    return findings


def _length_findings(source_text: str, target_text: str) -> list[dict[str, Any]]:
    if len(source_text) < _LENGTH_MIN_SOURCE_CHARS:
        return []
    ratio = len(target_text) / len(source_text)
    if _LENGTH_RATIO_MIN <= ratio <= _LENGTH_RATIO_MAX:
        return []
    return [
        _finding(
            "length_anomaly",
            {
                "source_chars": len(source_text),
                "target_chars": len(target_text),
                "ratio": round(ratio, 3),
            },
        ),
    ]


def _latin_ratio(text: str) -> float:
    letters = [char for char in text if char.isalpha()]
    if not letters:
        return 0.0
    latin = sum(1 for char in letters if char.isascii())
    return latin / len(letters)


def _latin_findings(source_text: str, target_text: str) -> list[dict[str, Any]]:
    if _latin_ratio(target_text) > _LATIN_SCRIPT_MAX_RATIO:
        return []

    findings: list[dict[str, Any]] = []
    seen: set[str] = set()
    for match in _LATIN_RUN_RE.finditer(target_text):
        run = " ".join(match.group().split())
        words = len(_LATIN_WORD_RE.findall(run))
        if words < _LATIN_MIN_WORDS or run not in source_text or run in seen:
            continue
        seen.add(run)
        severity = "major" if words >= _LATIN_LONG_WORDS else "minor"
        findings.append(_finding("latin_leftover", {"run": run, "words": words}, severity=severity))
    return findings


def _glossary_mismatch(
    source_text: str,
    target_text: str,
    glossary: Mapping[str, str],
) -> list[dict[str, Any]]:
    findings: list[dict[str, Any]] = []
    for source_term, target_term in glossary.items():
        if not source_term or not target_term:
            continue
        if _contains(source_text, source_term) and not _contains(target_text, target_term):
            findings.append(
                _finding(
                    "glossary_mismatch",
                    {"source_term": source_term, "target_term": target_term},
                ),
            )
    return findings


def _glossary_conflict(
    source_text: str,
    glossary: Mapping[str, str],
) -> list[dict[str, Any]]:
    reverse: dict[str, list[str]] = {}
    for source_term, target_term in glossary.items():
        if target_term and source_term not in reverse.setdefault(target_term, []):
            reverse[target_term].append(source_term)

    findings: list[dict[str, Any]] = []
    for target_term, source_terms in reverse.items():
        present = sorted(term for term in source_terms if _contains(source_text, term))
        if len(present) > 1:
            findings.append(
                _finding(
                    "glossary_conflict",
                    {"target_term": target_term, "source_terms": present},
                ),
            )
    return findings


def check(
    *,
    source_text: str,
    target_text: str,
    glossary: Mapping[str, str],
    placeholders: Sequence[Sequence[Any]],
) -> dict[str, Any]:
    """Run every heuristic on one chunk and return the findings.

    ``placeholders`` is the ``[[index, literal], ...]`` map from :mod:`placeholders`. An
    empty target subsumes every other check, so only ``empty`` is reported for it.
    """
    findings: list[dict[str, Any]] = []

    if not target_text.strip():
        if source_text.strip():
            findings.append(_finding("empty", {"source_chars": len(source_text)}))
        return {"findings": findings}

    findings.extend(_markdown_findings(source_text, target_text))
    findings.extend(_placeholder_findings(placeholders, target_text))
    findings.extend(_untranslated_findings(source_text, target_text))
    findings.extend(_duplicate_findings(source_text, target_text))
    findings.extend(_length_findings(source_text, target_text))
    findings.extend(_latin_findings(source_text, target_text))
    findings.extend(_glossary_mismatch(source_text, target_text, glossary))
    findings.extend(_glossary_conflict(source_text, glossary))
    return {"findings": findings}
