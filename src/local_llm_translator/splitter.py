from __future__ import annotations

import re
from dataclasses import dataclass

_MAX_HEADING_LEVEL = 6
_MIN_PARTS_FOR_SPLIT = 2


@dataclass
class Section:
    heading: str
    level: int
    text: str
    token_estimate: int
    index: int


_HEADING_RE = re.compile(r"^(#{1,6})\s+(.+)$", re.MULTILINE)


def estimate_tokens(text: str) -> int:
    """Estimate token count using character-based heuristic (~4 chars/token)."""
    return max(1, len(text) // 3)


def _detect_chapter_level(markdown: str) -> int:
    """Find the heading level that defines chapters.

    Prefers the deepest level with 2+ occurrences (h2 for multi-part books
    like LotR, h1 for simple books). Falls back to the most common level.
    """
    all_headings = _HEADING_RE.findall(markdown)
    if not all_headings:
        return 1

    counts: dict[int, int] = {}
    for hashes, _ in all_headings:
        level = len(hashes)
        counts[level] = counts.get(level, 0) + 1

    for level in range(_MAX_HEADING_LEVEL, 0, -1):
        if counts.get(level, 0) >= _MIN_PARTS_FOR_SPLIT:
            return level

    return max(counts, key=counts.get)  # type: ignore[arg-type]


def _parse_headings(markdown: str, chapter_level: int) -> list[Section]:  # noqa: C901, PLR0915
    """Split markdown at *chapter_level* headings.

    Text found between a higher-level heading and the first chapter-level
    heading underneath is preserved as its own introductory section.
    """
    lines = markdown.split("\n")
    sections: list[Section] = []
    current_lines: list[str] = []
    current_heading = ""
    current_level = 0

    def flush() -> None:
        nonlocal current_heading, current_level, current_lines
        if current_lines or current_heading:
            sections.append(
                Section(
                    heading=current_heading,
                    level=current_level,
                    text="\n".join(current_lines).strip("\n"),
                    token_estimate=0,
                    index=len(sections),
                )
            )
            current_heading = ""
            current_level = 0
            current_lines = []

    pending_higher_heading: str | None = None
    higher_lines: list[str] = []

    for line in lines:
        m = _HEADING_RE.match(line)
        if not m:
            if pending_higher_heading is not None:
                higher_lines.append(line)
            else:
                current_lines.append(line)
            continue

        level = len(m.group(1))
        heading_text = m.group(2)

        if level == chapter_level:
            if pending_higher_heading is not None and any(higher_lines):
                current_heading = pending_higher_heading
                current_level = level - 1
                current_lines = higher_lines
                flush()
                pending_higher_heading = None
            elif pending_higher_heading is not None:
                pending_higher_heading = None
                higher_lines = []

            flush()
            current_heading = heading_text
            current_level = level
            current_lines = []
        elif level < chapter_level:
            flush()
            pending_higher_heading = heading_text
            higher_lines = []
            current_lines = []
            current_heading = ""
            current_level = 0
        else:
            current_lines.append(line)

    if pending_higher_heading is not None and any(higher_lines):
        current_heading = pending_higher_heading
        current_level = chapter_level - 1
        current_lines = higher_lines
        flush()

    flush()
    return sections


def _chunk_by_chars(
    heading: str, level: int, text: str, max_chars: int, start_index: int
) -> list[Section]:
    """Split text into fixed-size character chunks, each as a Section."""
    chunks: list[Section] = []
    pos = 0
    text_len = len(text)
    while pos < text_len:
        # Try to break at a word boundary near max_chars
        end = min(pos + max_chars, text_len)
        if end < text_len:
            space = text.rfind(" ", pos, end)
            if space > pos:
                end = space + 1
        chunk_text = text[pos:end]
        chunks.append(
            Section(
                heading=f"{heading} (cont.)",
                level=level,
                text=chunk_text,
                token_estimate=estimate_tokens(chunk_text),
                index=start_index + len(chunks),
            )
        )
        pos = end
    return chunks


def _split_oversized(section: Section, max_tokens: int, start_index: int) -> list[Section]:
    """Recursively split a section that exceeds max_tokens by lowering heading level."""
    if estimate_tokens(section.text) <= max_tokens:
        section.token_estimate = estimate_tokens(section.text)
        section.index = start_index
        return [section]

    next_level = section.level + 1
    if next_level > _MAX_HEADING_LEVEL:
        return _chunk_by_chars(
            section.heading,
            section.level,
            section.text,
            max_tokens * 3,
            start_index,
        )

    pattern = re.compile(rf"^{'#' * next_level}\s+(.+)$", re.MULTILINE)
    parts = pattern.split(section.text)
    if len(parts) < _MIN_PARTS_FOR_SPLIT:
        return _chunk_by_chars(
            section.heading,
            section.level,
            section.text,
            max_tokens * 3,
            start_index,
        )

    sub_sections: list[Section] = []
    preamble = parts[0].strip() if not _HEADING_RE.match(parts[0]) else ""
    if preamble:
        sub_sections.append(
            Section(
                heading=section.heading,
                level=section.level,
                text=preamble,
                token_estimate=0,
                index=0,
            )
        )

    for i in range(1, len(parts), 2):
        h = parts[i]
        content = parts[i + 1] if i + 1 < len(parts) else ""
        sub_sections.append(
            Section(
                heading=h,
                level=next_level,
                text=content.strip(),
                token_estimate=0,
                index=0,
            )
        )

    result: list[Section] = []
    idx = start_index
    for ss in sub_sections:
        for s in _split_oversized(ss, max_tokens, idx):
            result.append(s)
            idx += 1
    return result


def split_markdown(markdown: str, context_size: int) -> list[Section]:
    """Split markdown into sections that fit within context_size tokens.

    Uses 70% of context_size as the target to leave room for system prompt,
    previous context, and the LLM response.
    """
    target = int(context_size * 0.7)
    chapter_level = _detect_chapter_level(markdown)
    raw = _parse_headings(markdown, chapter_level)

    sections: list[Section] = []
    for section in raw:
        sections.extend(_split_oversized(section, target, len(sections)))

    for i, s in enumerate(sections):
        s.index = i
        s.token_estimate = estimate_tokens(s.text)

    return sections
