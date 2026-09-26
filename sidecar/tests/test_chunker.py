"""Chunking invariants.

The chunker is where a document silently loses a paragraph or gains a blank line inside a
table, so the invariants are asserted directly rather than inferred from the output.
"""

from __future__ import annotations

from typing import TYPE_CHECKING

import pytest
from llmtranslator_sidecar.chunker import build_chunks, estimate_tokens
from llmtranslator_sidecar.parse import detect_chapters, split_blocks

if TYPE_CHECKING:
    from llmtranslator_sidecar.blocks import Chunk


def _block_ids(chunks: list[Chunk]) -> list[str]:
    ids: list[str] = []
    for chunk in chunks:
        ids.extend(chunk.block_ids)
    return ids


def test_every_block_appears_exactly_once() -> None:
    markdown = (
        "# Chapter\n\nfirst paragraph\n\n- a\n- b\n\n> a quote\n\n"
        "```\ncode\n```\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nlast paragraph\n"
    )
    blocks = split_blocks(markdown)
    assert _block_ids(build_chunks(blocks, 40)) == [b.id for b in blocks]


def test_no_block_id_is_duplicated_across_chunks() -> None:
    blocks = split_blocks("# H\n\n" + "\n\n".join(f"para {i}" for i in range(40)))
    ids = _block_ids(build_chunks(blocks, 30))
    assert len(ids) == len(set(ids))


def test_source_md_is_the_blocks_joined_for_unsplit_chunks() -> None:
    blocks = split_blocks("# H\n\nalpha\n\nbeta\n\ngamma\n")
    by_id = {b.id: b for b in blocks}
    for chunk in build_chunks(blocks, 1000):
        expected = "\n\n".join(by_id[bid].source_md for bid in chunk.block_ids)
        assert chunk.source_md == expected


def test_non_translatable_blocks_cost_nothing() -> None:
    blocks = split_blocks("```\n" + ("x = 1\n" * 500) + "```\n")
    chunks = build_chunks(blocks, 10)
    assert chunks
    assert all(chunk.token_estimate == 0 for chunk in chunks)


def test_table_is_never_split_across_chunks_when_it_fits() -> None:
    markdown = "before\n\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nafter\n"
    blocks = split_blocks(markdown)
    table = next(b for b in blocks if b.kind == "table")
    holders = [c for c in build_chunks(blocks, 60) if table.id in c.block_ids]
    assert len(holders) == 1
    assert table.source_md in holders[0].source_md


def test_oversized_table_is_split_with_repeated_header() -> None:
    rows = "\n".join(f"| row{i} | value {i} |" for i in range(60))
    blocks = split_blocks(f"| name | value |\n|---|---|\n{rows}\n")
    assert len(blocks) == 1

    chunks = build_chunks(blocks, 80)
    assert len(chunks) > 1
    for chunk in chunks:
        assert "| name | value |" in chunk.source_md
        assert "|---|---|" in chunk.source_md
        assert any(flag.startswith("table_part:") for flag in chunk.flags)

    # The parts together still account for every data row, exactly once and in order.
    recovered = [
        line for chunk in chunks for line in chunk.source_md.split("\n")[2:] if line.strip()
    ]
    original = [line for line in blocks[0].source_md.split("\n")[2:] if line.strip()]
    assert recovered == original


def test_a_table_fragment_never_shares_a_chunk() -> None:
    rows = "\n".join(f"| row{i} | value {i} |" for i in range(60))
    blocks = split_blocks(f"| name | value |\n|---|---|\n{rows}\n")
    for chunk in build_chunks(blocks, 80):
        parts = [flag for flag in chunk.flags if flag.startswith("table_part:")]
        assert len(parts) <= 1


def test_oversized_paragraph_is_split_at_sentence_boundaries() -> None:
    sentence = "Questa e' una frase abbastanza lunga per contare qualcosa. "
    blocks = split_blocks(sentence * 40)
    assert len(blocks) == 1

    chunks = build_chunks(blocks, 60)
    assert len(chunks) > 1
    assert "continues" not in chunks[0].flags
    assert "continues" in chunks[1].flags
    for chunk in chunks:
        # No sentence is ever cut in half.
        assert chunk.source_md.strip().endswith(".")
        assert chunk.token_estimate <= 60


def test_chunk_never_exceeds_budget_unless_indivisible() -> None:
    blocks = split_blocks("```\n" + ("some code line\n" * 500) + "```\n")
    for chunk in build_chunks(blocks, 10):
        if chunk.token_estimate > 10:
            assert "oversized" in chunk.flags or "code" in chunk.flags


def test_chunking_never_crosses_a_chapter_boundary() -> None:
    blocks = split_blocks("# One\n\nalpha\n\n# Two\n\nbeta\n\n# Three\n\ngamma\n")
    detect_chapters(blocks)
    by_id = {b.id: b for b in blocks}
    for chunk in build_chunks(blocks, 1000):
        chapters = {by_id[bid].chapter_id for bid in chunk.block_ids}
        assert len(chapters) == 1


def test_context_carrier_tracks_the_enclosing_headings() -> None:
    blocks = split_blocks("# Book\n\nintro\n\n## Chapter\n\nbody\n\n### Section\n\ndeep\n")
    chunks = build_chunks(blocks, 1000)
    # Each chunk carries the heading chain in effect where it sits, not the whole outline:
    # a heading opens the chunk holding the text it introduces.
    assert [h["text"] for h in chunks[0].context_carrier["headings"]] == ["Book"]
    assert [h["text"] for h in chunks[-1].context_carrier["headings"]] == [
        "Book",
        "Chapter",
        "Section",
    ]
    assert [h["level"] for h in chunks[-1].context_carrier["headings"]] == [1, 2, 3]


def test_chunk_ids_are_deterministic() -> None:
    blocks = split_blocks("# H\n\n" + "\n\n".join(f"p{i}" for i in range(20)))
    first = [c.id for c in build_chunks(blocks, 30)]
    second = [c.id for c in build_chunks(blocks, 30)]
    assert first == second
    assert first == [f"c{i:06d}" for i in range(len(first))]


def test_non_positive_budget_is_rejected() -> None:
    with pytest.raises(ValueError, match="budget_tokens"):
        build_chunks([], 0)


def test_empty_input_yields_no_chunks() -> None:
    assert build_chunks([], 100) == []


def test_estimate_tokens_is_monotonic() -> None:
    assert estimate_tokens("") == 0
    assert estimate_tokens("a") >= 1
    assert estimate_tokens("a" * 300) > estimate_tokens("a" * 30)


# -- M2: a non-translatable block travels with its chunk but is never sent ---------------


def test_front_matter_is_carried_but_costs_no_tokens() -> None:
    markdown = "---\ntitle: X\nauthor: Y\n---\n\n# H\n\nBody text.\n"
    blocks = split_blocks(markdown)
    front = blocks[0]
    assert front.kind == "frontmatter"
    assert front.translatable is False

    chunks = build_chunks(blocks, 1000)

    # The chunker gives a non-translatable block an empty `sent`, so the whole document
    # costs exactly what its translatable blocks cost: nothing is spent on the YAML.
    translatable = sum(estimate_tokens(b.source_text) for b in blocks if b.translatable)
    assert sum(chunk.token_estimate for chunk in chunks) == translatable

    holder = next(chunk for chunk in chunks if front.id in chunk.block_ids)
    assert "frontmatter" in holder.flags


def test_non_translatable_block_is_carried_into_the_chunk_source_md() -> None:
    blocks = split_blocks("---\ntitle: X\n---\n\nBody\n")
    front = blocks[0]
    holder = next(c for c in build_chunks(blocks, 1000) if front.id in c.block_ids)
    # It is never sent, but it does travel so the document can be rebuilt verbatim.
    assert front.source_md in holder.source_md
