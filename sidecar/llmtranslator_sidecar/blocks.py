"""Data model shared by the Markdown IR, the chunker and the RPC layer.

These dataclasses are the frozen interface between the parser, the chunker and the JSON-RPC
surface. The wire shape of each one is defined by :meth:`to_json` and must match the ``Block``,
``Chunk`` and ``Chapter`` definitions in ``PLAN.md`` §12.1.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any

#: Every legal value of :attr:`Block.kind`.
BLOCK_KINDS: frozenset[str] = frozenset(
    {
        "heading",
        "para",
        "list",
        "blockquote",
        "table",
        "code",
        "figure",
        "footnote_def",
        "frontmatter",
        "hr",
        "html",
    }
)

#: Blocks that are carried through the pipeline but never sent to the model. YAML front
#: matter belongs here: it is the document's metadata, and a model asked to translate it
#: would rewrite the very fields the pipeline needs to stay reproducible.
NON_TRANSLATABLE_KINDS: frozenset[str] = frozenset(
    {"code", "frontmatter", "hr", "html", "figure"},
)


@dataclass(slots=True)
class Block:
    """A single atomic unit of the document, with a stable, deterministic id.

    ``source_md`` is the exact slice of the source Markdown, so that
    ``serialize(split_blocks(md)) == md`` holds byte for byte. ``source_text`` is what is
    actually sent to the model once block-level markers have been removed where they must not
    be touched (headings): it is the input of the placeholder layer.

    ``gap_after`` and ``trailing_newlines`` are serialization bookkeeping and are
    deliberately absent from :meth:`to_json`. ``gap_after`` is the number of blank
    lines between this block and the next one; ``trailing_newlines`` is only read
    from the last block and holds the blank lines between it and end of document,
    which is what makes an exact round-trip possible without a document wrapper.
    """

    id: str
    order: int
    kind: str
    level: int = 0
    source_md: str = ""
    source_text: str = ""
    translatable: bool = True
    attrs: dict[str, Any] = field(default_factory=dict[str, Any])
    content_hash: str = ""
    chapter_id: str = ""
    gap_after: int = 0
    trailing_newlines: int = 0

    def to_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "chapter_id": self.chapter_id,
            "order": self.order,
            "kind": self.kind,
            "level": self.level,
            "source_md": self.source_md,
            "source_text": self.source_text,
            "translatable": self.translatable,
            "attrs": self.attrs,
            "content_hash": self.content_hash,
        }

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Block:
        return cls(
            id=str(data["id"]),
            order=int(data["order"]),
            kind=str(data["kind"]),
            level=int(data.get("level", 0)),
            source_md=str(data.get("source_md", "")),
            source_text=str(data.get("source_text", "")),
            translatable=bool(data.get("translatable", True)),
            attrs=dict(data.get("attrs") or {}),
            content_hash=str(data.get("content_hash", "")),
            chapter_id=str(data.get("chapter_id", "")),
        )


@dataclass(slots=True)
class Chapter:
    """A contiguous run of blocks grouped under a heading."""

    id: str
    order: int
    title: str
    level: int
    block_first: int
    block_last: int

    def to_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "order": self.order,
            "title": self.title,
            "level": self.level,
            "block_first": self.block_first,
            "block_last": self.block_last,
        }

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Chapter:
        return cls(
            id=str(data["id"]),
            order=int(data["order"]),
            title=str(data.get("title", "")),
            level=int(data.get("level", 0)),
            block_first=int(data.get("block_first", 0)),
            block_last=int(data.get("block_last", 0)),
        )


@dataclass(slots=True)
class Chunk:
    """The unit of work and of checkpointing: always a whole list of blocks, never a byte cut."""

    id: str
    order: int
    block_ids: list[str]
    source_md: str
    token_estimate: int
    chapter_id: str = ""
    context_carrier: dict[str, Any] = field(default_factory=dict[str, Any])
    flags: list[str] = field(default_factory=list[str])

    def to_json(self) -> dict[str, Any]:
        return {
            "id": self.id,
            "chapter_id": self.chapter_id,
            "order": self.order,
            "block_ids": self.block_ids,
            "source_md": self.source_md,
            "token_estimate": self.token_estimate,
            "context_carrier": self.context_carrier,
            "flags": self.flags,
        }

    @classmethod
    def from_json(cls, data: dict[str, Any]) -> Chunk:
        return cls(
            id=str(data["id"]),
            order=int(data["order"]),
            block_ids=[str(b) for b in data.get("block_ids", [])],
            source_md=str(data.get("source_md", "")),
            token_estimate=int(data.get("token_estimate", 0)),
            chapter_id=str(data.get("chapter_id", "")),
            context_carrier=dict(data.get("context_carrier") or {}),
            flags=[str(f) for f in data.get("flags", [])],
        )
