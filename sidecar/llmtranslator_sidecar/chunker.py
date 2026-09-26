"""Structural chunking.

A chunk is never a fixed-size cut through the text: it is always a whole list of blocks.
Cutting at an arbitrary character offset is what tears a table in half or strands a
footnote reference from its definition, and no amount of prompt engineering recovers
from that.

What is sent to the model excludes non-translatable blocks (code, rules, raw HTML,
figures): they travel with the chunk so the document can be rebuilt, but there is no
reason to spend context on them.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from typing import Any

from .blocks import Block, Chunk

#: Only prose-like blocks can be split at all.
_SPLIT_KINDS = frozenset({"para", "list", "blockquote"})

#: A pipe table needs a header row, a delimiter row and at least one data row.
_MIN_TABLE_LINES = 3

_SENTENCE_RE = re.compile(r"(?<=[.!?\u2026\u3002\uff01\uff1f])\s+")


def estimate_tokens(text: str) -> int:
    """Character-based estimate, used when the server's ``/tokenize`` is unavailable.

    Empty text costs nothing, which is what lets a non-translatable block travel with a
    chunk without consuming any of the model's budget.
    """
    if not text:
        return 0
    return max(1, len(text) // 3)


@dataclass(slots=True)
class _Piece:
    """One block, or one fragment of a block that had to be split."""

    block: Block
    markdown: str
    sent: str
    split: bool = False
    flags: list[str] = field(default_factory=list[str])


def _pack_units(units: list[str], budget: int, separator: str) -> list[str]:
    """Greedily pack units into groups of at most ``budget`` tokens each."""
    chunks: list[str] = []
    current: list[str] = []
    for unit in units:
        candidate = separator.join([*current, unit])
        if current and estimate_tokens(candidate) > budget:
            chunks.append(separator.join(current))
            current = [unit]
        else:
            current.append(unit)
    if current:
        chunks.append(separator.join(current))
    return chunks


def _split_prose(text: str, budget: int) -> list[str]:
    """Split prose at sentence boundaries, falling back to lines then to words."""
    sentences = [part for part in _SENTENCE_RE.split(text) if part != ""]
    parts = _pack_units(sentences, budget, " ")
    if len(parts) > 1:
        return parts

    # A single sentence longer than the budget: cut on lines, then on words.
    lines = text.split("\n")
    if len(lines) > 1:
        return _pack_units(lines, budget, "\n")
    return _pack_units(text.split(" "), budget, " ")


def _split_table(block: Block, budget: int) -> list[_Piece]:
    """Split an oversized table by rows, repeating the header in every part."""
    lines = block.source_md.split("\n")
    if len(lines) < _MIN_TABLE_LINES:
        return [_Piece(block=block, markdown=block.source_md, sent=block.source_text)]

    header = "\n".join(lines[:2])
    rows = lines[2:]
    room = max(budget - estimate_tokens(header), 1)
    groups = _pack_units(rows, room, "\n")
    total = len(groups)

    return [
        _Piece(
            block=block,
            markdown=f"{header}\n{group}",
            sent=f"{header}\n{group}",
            split=True,
            flags=["table", f"table_part:{position + 1}/{total}"],
        )
        for position, group in enumerate(groups)
    ]


def _split_oversized(block: Block, budget: int) -> list[_Piece]:
    if block.kind == "table":
        return _split_table(block, budget)
    if block.kind in _SPLIT_KINDS:
        parts = _split_prose(block.source_text, budget)
        return [
            _Piece(
                block=block,
                markdown=part,
                sent=part,
                split=True,
                flags=[] if position == 0 else ["continues"],
            )
            for position, part in enumerate(parts)
        ]
    # Indivisible and oversized (a huge code block, for instance): it travels whole.
    return [
        _Piece(
            block=block,
            markdown=block.source_md,
            sent=block.source_text,
            flags=["oversized"],
        ),
    ]


def _piece_for(block: Block, budget: int) -> list[_Piece]:
    if block.translatable and estimate_tokens(block.source_text) > budget:
        return _split_oversized(block, budget)
    if not block.translatable:
        return [
            _Piece(block=block, markdown=block.source_md, sent="", flags=[block.kind]),
        ]
    return [_Piece(block=block, markdown=block.source_md, sent=block.source_text)]


class _ChunkBuilder:
    """Accumulates pieces into chunks, keeping the heading chain in step."""

    def __init__(self, budget: int) -> None:
        self.budget = budget
        self.chunks: list[Chunk] = []
        self.headings: list[dict[str, Any]] = []
        self.pieces: list[_Piece] = []
        self.tokens = 0

    def flush(self) -> None:
        if not self.pieces:
            return
        order = len(self.chunks)
        self.chunks.append(
            Chunk(
                id=f"c{order:06d}",
                order=order,
                block_ids=[piece.block.id for piece in self.pieces],
                source_md="\n\n".join(piece.markdown for piece in self.pieces),
                token_estimate=self.tokens,
                chapter_id=self.pieces[0].block.chapter_id,
                context_carrier={"headings": list(self.headings)},
                flags=list(dict.fromkeys(f for p in self.pieces for f in p.flags)),
            ),
        )
        self.pieces = []
        self.tokens = 0

    def open_heading(self, block: Block) -> None:
        """A heading should open the chunk carrying the text it introduces."""
        if any(piece.block.kind != "heading" for piece in self.pieces):
            self.flush()
        self.headings = [item for item in self.headings if int(item["level"]) < block.level]
        self.headings.append({"level": block.level, "text": block.source_text})

    def offer_split(self, piece: _Piece) -> None:
        """A fragment always lands in a chunk of its own.

        Packing two fragments together would join them with a blank line, which inside a
        table is not a cosmetic problem: it ends the table.
        """
        self.flush()
        self.pieces = [piece]
        self.tokens = estimate_tokens(piece.sent)
        self.flush()

    def offer(self, piece: _Piece) -> None:
        cost = estimate_tokens(piece.sent)
        if self.pieces and self.tokens + cost > self.budget:
            self.flush()
        self.pieces.append(piece)
        self.tokens += cost


def build_chunks(blocks: list[Block], budget_tokens: int) -> list[Chunk]:
    """Group blocks into chunks that fit ``budget_tokens``.

    Invariants, all covered by the tests:

    1. every block id appears in exactly one chunk — nothing lost, nothing duplicated;
    2. for a chunk with no split piece, ``source_md`` is the blocks' markdown joined by a
       blank line;
    3. ``token_estimate`` counts only what is sent to the model, so non-translatable
       blocks never contribute to it;
    4. a chunk never exceeds the budget unless it holds a single indivisible block;
    5. chunk ids are deterministic;
    6. chunking never crosses a chapter boundary;
    7. a fragment of a split block is never packed together with another fragment.
    """
    if budget_tokens <= 0:
        msg = "budget_tokens must be positive"
        raise ValueError(msg)

    builder = _ChunkBuilder(budget_tokens)
    chapter_id: str | None = None
    seen_chapter = False

    for block in blocks:
        if seen_chapter and block.chapter_id != chapter_id:
            builder.flush()
        chapter_id = block.chapter_id
        seen_chapter = True

        if block.kind == "heading":
            builder.open_heading(block)

        for piece in _piece_for(block, budget_tokens):
            if piece.split:
                builder.offer_split(piece)
            else:
                builder.offer(piece)

    builder.flush()
    return builder.chunks
