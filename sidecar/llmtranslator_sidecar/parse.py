"""Source-preserving Markdown segmentation.

The governing invariant of the whole project is::

    serialize(split_blocks(md)) == md      # byte for byte, for every input

That is why this is a line scanner and not an AST: a parser that builds a tree and
re-serialises it normalises the Markdown, and any normalisation silently rewrites the
blocks that were never translated.

How the exact round-trip works. ``md.split("\\n")`` always reconstructs the input with
``"\\n".join(...)``, so trailing newlines are not lost information: they are *empty
trailing elements* of that list. Each block owns a contiguous slice of that list
excluding its trailing blank lines, and records how many blank lines separated it from
the next block (``gap_after``). The last block additionally records how many blank
elements remained before end of document (``trailing_newlines``). ``serialize`` then
replays separators and tail verbatim.
"""

from __future__ import annotations

import hashlib
import re
from collections.abc import Callable, Sequence
from typing import Any

from .blocks import NON_TRANSLATABLE_KINDS, Block, Chapter

_ATX_RE = re.compile(r"^(#{1,6})(\s+)(.*)$")
_FENCE_RE = re.compile(r"^ {0,3}(`{3,}|~{3,})[ \t]*([^\n]*)$")
_LIST_RE = re.compile(r"^(\s*)([-*+]|\d{1,9}[.)])(\s+)(.*)$")
_QUOTE_RE = re.compile(r"^ {0,3}>")
_FOOTNOTE_RE = re.compile(r"^\[\^([^\]\s]+)\]:[ \t]?(.*)$")
_HR_RE = re.compile(
    r"^ {0,3}(?:(?:-[ \t]*){3,}|(?:\*[ \t]*){3,}|(?:_[ \t]*){3,})$",
)
_IMAGE_ONLY_RE = re.compile(r"^ {0,3}!\[[^\]]*\]\([^)]*\)[ \t]*$")
_TABLE_DELIM_RE = re.compile(r"^ {0,3}\|?[ \t]*:?-{1,}:?[ \t]*\|[ \t:|-]*$")
_HTML_COMMENT_START_RE = re.compile(r"^ {0,3}<!--")
_HTML_TAG_RE = re.compile(r"^ {0,3}<([A-Za-z][A-Za-z0-9-]*)")

_MIN_TABLE_DELIM_DASHES = 1
_INDENT = ("  ", "\t")

#: A heading level has to occur at least this many times to define chapters; below it the
#: document is treated as flat and the shallowest level wins.
_MIN_HEADINGS_FOR_CHAPTER_LEVEL = 2

#: A block segmenter: given the line list and a start index, either consume a block or
#: return ``None`` to let the next matcher try. The returned index is exclusive and
#: always points past a non-blank line.
_Matcher = Callable[[Sequence[str], int], "tuple[str, int, dict[str, Any]] | None"]


def canonicalize(text: str) -> str:
    """Canonical form used for content hashing: trailing whitespace stripped per line."""
    return "\n".join(line.rstrip() for line in text.split("\n"))


def content_hash(text: str) -> str:
    return hashlib.sha256(canonicalize(text).encode("utf-8")).hexdigest()


def _is_indented(line: str) -> bool:
    return line.startswith(_INDENT)


def _cells(row: str) -> list[str]:
    stripped = row.strip().removeprefix("|").removesuffix("|")
    return [cell.strip() for cell in stripped.split("|")]


def _match_fence(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    opening = _FENCE_RE.match(lines[start])
    if opening is None:
        return None
    fence, info = opening.group(1), opening.group(2).strip()
    char, width = fence[0], len(fence)
    closing = re.compile(rf"^ {{0,3}}{re.escape(char)}{{{width},}}[ \t]*$")
    end = start + 1
    while end < len(lines):
        if closing.match(lines[end]):
            end += 1
            break
        end += 1
    return "code", end, {"lang": info.split()[0] if info else "", "info": info}


def _match_hr(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    if _HR_RE.match(lines[start]):
        return "hr", start + 1, {}
    return None


def _match_heading(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    heading = _ATX_RE.match(lines[start])
    if heading is None:
        return None
    hashes, spacing = heading.group(1), heading.group(2)
    return "heading", start + 1, {"text_prefix": hashes + spacing, "level": len(hashes)}


def _match_footnote(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    footnote = _FOOTNOTE_RE.match(lines[start])
    if footnote is None:
        return None
    end = start + 1
    while end < len(lines):
        candidate = lines[end]
        if candidate.strip() == "":
            following = lines[end + 1] if end + 1 < len(lines) else None
            if following is not None and _is_indented(following):
                end += 1
                continue
            break
        if _is_indented(candidate):
            end += 1
            continue
        break
    return "footnote_def", end, {"ref": f"^{footnote.group(1)}"}


def _match_table(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    if start + 1 >= len(lines) or "|" not in lines[start]:
        return None
    delimiter = _TABLE_DELIM_RE.match(lines[start + 1])
    if delimiter is None or lines[start + 1].count("-") < _MIN_TABLE_DELIM_DASHES:
        return None
    if "|" not in lines[start + 1]:
        return None
    end = start + 2
    while end < len(lines) and lines[end].strip() != "" and "|" in lines[end]:
        end += 1
    align: list[str] = []
    for cell in _cells(lines[start + 1]):
        left, right = cell.startswith(":"), cell.endswith(":")
        if left and right:
            align.append("c")
        elif right:
            align.append("r")
        else:
            align.append("l")
    return "table", end, {"align": align}


def _match_html(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    if _HTML_COMMENT_START_RE.match(lines[start]):
        end = start
        while end < len(lines):
            if "-->" in lines[end]:
                return "html", end + 1, {}
            end += 1
        return "html", len(lines), {}
    tag = _HTML_TAG_RE.match(lines[start])
    if tag is None:
        return None
    name = tag.group(1)
    if f"</{name}>" in lines[start] or lines[start].rstrip().endswith("/>"):
        return "html", start + 1, {}
    end = start + 1
    while end < len(lines) and lines[end].strip() != "":
        if f"</{name}>" in lines[end]:
            return "html", end + 1, {}
        end += 1
    return "html", end, {}


def _match_image(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    if _IMAGE_ONLY_RE.match(lines[start]):
        return "figure", start + 1, {}
    return None


def _match_quote(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    if not _QUOTE_RE.match(lines[start]):
        return None
    end = start
    while end < len(lines):
        if _QUOTE_RE.match(lines[end]):
            end += 1
            continue
        if lines[end].strip() == "" and end + 1 < len(lines) and _QUOTE_RE.match(lines[end + 1]):
            end += 1
            continue
        break
    return "blockquote", end, {}


def _match_list(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]] | None:
    marker = _LIST_RE.match(lines[start])
    if marker is None:
        return None
    end = start
    while end < len(lines):
        current = lines[end]
        if _LIST_RE.match(current):
            end += 1
            continue
        if current.strip() == "":
            following = lines[end + 1] if end + 1 < len(lines) else None
            if following is not None and (_LIST_RE.match(following) or _is_indented(following)):
                end += 1
                continue
            break
        if _is_indented(current):
            end += 1
            continue
        break
    ordered = marker.group(2)[0].isdigit()
    return "list", end, {"ordered": ordered, "start": marker.group(2)}


_MATCHERS: tuple[_Matcher, ...] = (
    _match_fence,
    _match_hr,
    _match_heading,
    _match_footnote,
    _match_table,
    _match_html,
    _match_image,
    _match_quote,
    _match_list,
)

_PARA_INTERRUPTS: tuple[_Matcher, ...] = (
    _match_fence,
    _match_hr,
    _match_heading,
    _match_footnote,
    _match_table,
    _match_quote,
    _match_list,
)


def _starts_block(lines: Sequence[str], index: int) -> bool:
    return any(matcher(lines, index) is not None for matcher in _PARA_INTERRUPTS)


def _match_para(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]]:
    end = start + 1
    while end < len(lines):
        if lines[end].strip() == "" or _starts_block(lines, end):
            break
        end += 1
    return "para", end, {}


def _consume(lines: Sequence[str], start: int) -> tuple[str, int, dict[str, Any]]:
    for matcher in _MATCHERS:
        found = matcher(lines, start)
        if found is not None:
            return found
    return _match_para(lines, start)


def _block_text(kind: str, content: str, attrs: dict[str, Any]) -> str:
    """The text actually sent to the model.

    Block markers are removed where the model must not be able to alter them (headings,
    whose level is rebuilt from ``text_prefix``) and kept where they carry structure the
    model has to reproduce (list markers, quote markers, table pipes).
    """
    if kind == "heading":
        prefix = str(attrs.get("text_prefix", ""))
        body = content[len(prefix) :]
        # CommonMark strips an optional closing run of hashes; doing it here as well
        # keeps the model from being asked to translate a `##` suffix.
        return re.sub(r"[ \t]*#+[ \t]*$", "", body)
    return content


def split_blocks(markdown: str) -> list[Block]:
    """Segment Markdown into blocks, preserving the exact source slices."""
    if markdown == "":
        return []

    lines = markdown.split("\n")
    blocks: list[Block] = []
    index = 0
    previous_end = 0

    while index < len(lines):
        if lines[index].strip() == "":
            index += 1
            continue

        kind, end, attrs = _consume(lines, index)
        # A matcher may stop on a blank line; those blanks belong to the gap, not to
        # the block, otherwise serialize would double them.
        while end > index + 1 and lines[end - 1].strip() == "":
            end -= 1

        content = "\n".join(lines[index:end])
        source_text = _block_text(kind, content, attrs)
        order = len(blocks)
        blocks.append(
            Block(
                id=f"b{order:06d}",
                order=order,
                kind=kind,
                level=int(attrs["level"]) if kind == "heading" else 0,
                source_md=content,
                source_text=source_text,
                translatable=kind not in NON_TRANSLATABLE_KINDS,
                attrs=attrs,
                content_hash=content_hash(source_text),
                gap_after=0,
            ),
        )
        if len(blocks) > 1:
            blocks[-2].gap_after = index - previous_end
        previous_end = end
        index = end

    if not blocks:
        # A document made only of blank lines: keep it as one opaque block so the
        # round-trip stays exact. Degenerate input, but the invariant is absolute.
        stripped = markdown.rstrip("\n")
        blocks.append(
            Block(
                id="b000000",
                order=0,
                kind="para",
                source_md=stripped,
                source_text="",
                translatable=False,
                content_hash=content_hash(""),
                trailing_newlines=(len(lines) - 1) if stripped == "" else 0,
            ),
        )
        return blocks

    blocks[-1].trailing_newlines = len(lines) - previous_end
    return blocks


def detect_chapters(blocks: list[Block]) -> list[Chapter]:
    """Group blocks under headings, following the depth rule in PLAN.md section 4.

    The chapter level is the deepest heading level occurring at least twice; when no
    level qualifies, the shallowest level present. Blocks before the first chapter
    heading form a synthetic preamble chapter.
    """
    counts: dict[int, int] = {}
    for block in blocks:
        if block.kind == "heading":
            counts[block.level] = counts.get(block.level, 0) + 1

    chapter_level = 0
    if counts:
        repeated = [
            level for level, count in counts.items() if count >= _MIN_HEADINGS_FOR_CHAPTER_LEVEL
        ]
        chapter_level = max(repeated) if repeated else min(counts)

    starts: list[int] = [
        block.order for block in blocks if block.kind == "heading" and block.level == chapter_level
    ]

    chapters: list[Chapter] = []
    if not starts or starts[0] > 0:
        last = (starts[0] - 1) if starts else (len(blocks) - 1)
        chapters.append(
            Chapter(
                id=f"ch{len(chapters):06d}",
                order=0,
                title="",
                level=0,
                block_first=0,
                block_last=last,
            ),
        )

    for position, start in enumerate(starts):
        end = (starts[position + 1] - 1) if position + 1 < len(starts) else (len(blocks) - 1)
        title = blocks[start].source_text.strip()
        chapters.append(
            Chapter(
                id=f"ch{len(chapters):06d}",
                order=len(chapters),
                title=title,
                level=chapter_level,
                block_first=start,
                block_last=end,
            ),
        )

    for chapter in chapters:
        for block in blocks[chapter.block_first : chapter.block_last + 1]:
            block.chapter_id = chapter.id

    return chapters


def parse_markdown(markdown: str) -> tuple[list[Block], list[Chapter]]:
    """Segment a document and group it into chapters."""
    blocks = split_blocks(markdown)
    chapters = detect_chapters(blocks)
    return blocks, chapters
