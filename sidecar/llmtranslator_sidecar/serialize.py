"""Reconstruction and rendering of a block list.

Two different jobs, deliberately kept apart:

- :func:`serialize` rebuilds the source byte for byte. It exists to hold the parser
  honest and to emit documents that have not been translated at all.
- :func:`render` builds the *output*: translated blocks are emitted with their rebuilt
  block marker, everything else passes through verbatim, so an untranslated footnote or
  table can never be damaged by the pipeline.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from collections.abc import Mapping

    from .blocks import Block


def _separator(previous: Block) -> str:
    """Newlines between two blocks: one to close the previous line, plus its blank gap."""
    return "\n" * (1 + previous.gap_after)


def serialize(blocks: list[Block]) -> str:
    """Rebuild the exact source text from a block list.

    ``trailing_newlines`` is read from the last block only; it carries the blank elements
    that followed the final block before end of document.
    """
    if not blocks:
        return ""

    parts: list[str] = []
    for position, block in enumerate(blocks):
        if position > 0:
            parts.append(_separator(blocks[position - 1]))
        parts.append(block.source_md)
    parts.append("\n" * blocks[-1].trailing_newlines)
    return "".join(parts)


def render(blocks: list[Block], translations: Mapping[str, str]) -> str:
    """Build the output Markdown.

    A translatable block with a translation is emitted as its block marker plus the
    translated text; anything else — non-translatable blocks and blocks that were never
    translated — is emitted unchanged.
    """
    parts: list[str] = []
    for position, block in enumerate(blocks):
        if position > 0:
            parts.append(_separator(blocks[position - 1]))

        translation = translations.get(block.id)
        if translation is not None and block.translatable:
            prefix = str(block.attrs.get("text_prefix", "")) if block.kind == "heading" else ""
            parts.append(prefix + translation)
        else:
            parts.append(block.source_md)

    # Normalise the tail: a rendered document ends with exactly one newline, whatever
    # spacing the source happened to have.
    text = "".join(parts)
    return text.rstrip("\n") + "\n" if text else ""
