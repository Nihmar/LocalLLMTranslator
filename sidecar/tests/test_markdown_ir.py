"""The Markdown IR must be lossless.

``serialize(split_blocks(md)) == md`` is the invariant everything else rests on: if the
parser normalises the source, untranslated blocks get silently rewritten and the output
stops being a faithful edition of the original.
"""

from __future__ import annotations

import pytest
from llmtranslator_sidecar.blocks import NON_TRANSLATABLE_KINDS
from llmtranslator_sidecar.parse import detect_chapters, parse_markdown, split_blocks
from llmtranslator_sidecar.serialize import render, serialize

NASTY_CORPUS: dict[str, str] = {
    "empty": "",
    "single_line_no_newline": "A",
    "single_line_with_newline": "A\n",
    "two_paragraphs": "A\n\nB\n",
    "many_trailing_newlines": "A\n\n\n\n",
    "only_blank_lines": "\n\n\n",
    "only_newline": "\n",
    "blank_lines_between_paragraphs": "A\n\n\n\nB\n",
    "multiline_paragraph": "one\ntwo\nthree\n",
    "headings_all_levels": "# a\n## b\n### c\n#### d\n##### e\n###### f\n",
    "heading_closing_hashes": "## Title ##\n\nBody\n",
    "hash_without_space_is_not_heading": "#hashtag\n\nBody\n",
    "nested_and_mixed_lists": "- a\n  - b\n    1. c\n- d\n\nAfter\n",
    "ordered_list": "1. one\n2. two\n10. ten\n",
    "list_with_continuation": "- item\n  continued line\n- next\n",
    "loose_list": "- a\n\n- b\n",
    "table_with_alignment": "| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |\n",
    "table_without_outer_pipes": "a | b\n--- | ---\n1 | 2\n",
    "dash_line_is_not_a_table": "Text\n\n---\n\nMore\n",
    "fenced_code_with_markdown_inside": "```markdown\n# not a heading\n| not | a table |\n```\n\nAfter\n",
    "fenced_code_unclosed": "```python\nx = 1\n",
    "fenced_code_tilde": "~~~\ncode\n~~~\n",
    "fenced_code_longer_fence_closes": "````\n```\ninner\n```\n````\n",
    "blockquote": "> a\n> b\n\nAfter\n",
    "blockquote_with_blank_line": "> a\n>\n> b\n\nAfter\n",
    "nested_blockquote": "> > deep\n\nAfter\n",
    "footnote_definition": "[^a]: text\n    continued\n\nBody[^a]\n",
    "footnote_definition_multiline": "[^long]:\n    first\n    second\n",
    "html_block": '<div class="x">\n<p>hi</p>\n</div>\n\nAfter\n',
    "html_comment": "<!-- a comment -->\n\nAfter\n",
    "horizontal_rules": "***\n\n___\n\n- - -\n",
    "standalone_image": "![alt](img.png)\n\nAfter\n",
    "setext_lookalike": "Title\n===\n\nAfter\n",
    "trailing_spaces": "A  \nB\n",
    "unicode_and_placeholders": "Città ⟦1⟧però⟦2⟧ 日本語\n",
    "crlf_free_but_dense": "A\n\n#H\n\n- x\n\n|a|b|\n|-|-|\n",
    # YAML front matter must be one opaque block, not hr + para + hr.
    "front_matter": '---\ntitle: "X"\nauthor: "Y"\n---\n\n# H\n\nBody\n',
    "front_matter_closed_with_dots": "---\ntitle: X\n...\n\nBody\n",
    "front_matter_empty": "---\n---\n\nBody\n",
    "front_matter_only": "---\ntitle: X\n---\n",
    "front_matter_unterminated_is_a_rule": "---\ntitle: X\n\nBody\n",
    "front_matter_not_leading_is_a_rule": "Text\n\n---\n\ntitle: X\n",
    # A table cell may carry a footnote reference and an image inline.
    "table_cell_with_footnote_and_image": (
        "| a | b |\n|---|---|\n| see[^1] | ![x](img.png) |\n\n[^1]: note\n"
    ),
}


@pytest.mark.parametrize("name", sorted(NASTY_CORPUS))
def test_round_trip_is_exact(name: str) -> None:
    markdown = NASTY_CORPUS[name]
    assert serialize(split_blocks(markdown)) == markdown


@pytest.mark.parametrize("name", sorted(NASTY_CORPUS))
def test_ids_are_deterministic(name: str) -> None:
    markdown = NASTY_CORPUS[name]
    first = split_blocks(markdown)
    second = split_blocks(markdown)
    assert [b.id for b in first] == [b.id for b in second]
    assert [b.order for b in first] == list(range(len(first)))
    assert [b.id for b in first] == [f"b{i:06d}" for i in range(len(first))]


def test_kinds_are_classified() -> None:
    blocks = split_blocks(
        "# H\n\npara\n\n- a\n- b\n\n> q\n\n```\ncode\n```\n\n---\n\n"
        "| a |\n|---|\n| 1 |\n\n![i](p.png)\n\n[^n]: note\n\n<div>x</div>\n",
    )
    assert [b.kind for b in blocks] == [
        "heading",
        "para",
        "list",
        "blockquote",
        "code",
        "hr",
        "table",
        "figure",
        "footnote_def",
        "html",
    ]


def test_non_translatable_kinds_are_flagged() -> None:
    blocks = split_blocks("```\ncode\n```\n\n---\n\n<div>x</div>\n\n![i](p.png)\n\nProse\n")
    flags = {b.kind: b.translatable for b in blocks}
    for kind in NON_TRANSLATABLE_KINDS:
        if kind in flags:
            assert flags[kind] is False
    assert flags.get("para") is True


def test_heading_keeps_prefix_out_of_the_translatable_text() -> None:
    (block,) = split_blocks("### Deep Title\n")
    assert block.kind == "heading"
    assert block.level == 3
    assert block.attrs["text_prefix"] == "### "
    assert block.source_text == "Deep Title"
    assert block.source_md == "### Deep Title"


def test_content_hash_ignores_trailing_whitespace_and_is_stable() -> None:
    a = split_blocks("Some text\n")
    b = split_blocks("Some text   \n")
    assert a[0].content_hash == b[0].content_hash


def test_render_passes_untranslated_blocks_through_verbatim() -> None:
    markdown = "# Title\n\nProse\n\n```\ncode\n```\n\n| a |\n|---|\n| 1 |\n"
    blocks = split_blocks(markdown)
    assert render(blocks, {}) == markdown


def test_render_rebuilds_heading_marker_from_attrs() -> None:
    blocks = split_blocks("## Original\n\nBody\n")
    out = render(blocks, {blocks[0].id: "Tradotto"})
    assert "## Tradotto" in out
    assert "Original" not in out
    # The body was not translated, so it survives untouched.
    assert "Body" in out


def test_render_never_touches_non_translatable_blocks() -> None:
    blocks = split_blocks("```\ncode = 1\n```\n")
    out = render(blocks, {blocks[0].id: "IGNORED"})
    assert "code = 1" in out
    assert "IGNORED" not in out


def test_single_heading_line_is_not_absorbed_into_a_paragraph() -> None:
    blocks = split_blocks("para\n# heading\n")
    assert [b.kind for b in blocks] == ["para", "heading"]


def test_trailing_newline_count_is_recorded_on_the_last_block() -> None:
    blocks = split_blocks("A\n\nB\n\n\n")
    assert blocks[-1].trailing_newlines == 3


def test_detect_chapters_uses_deepest_repeated_level() -> None:
    markdown = "# Part\n\nintro\n\n## One\n\na\n\n## Two\n\nb\n"
    blocks, chapters = parse_markdown(markdown)
    titles = [c.title for c in chapters]
    # h2 occurs twice and is deeper than h1, so h2 is the chapter level; the text under
    # the part heading before it becomes the preamble.
    assert titles == ["", "One", "Two"]
    assert chapters[0].level == 0
    assert blocks[0].chapter_id == chapters[0].id


def test_detect_chapters_falls_back_to_shallowest_level() -> None:
    blocks = split_blocks("# Only\n\na\n\n## Deeper\n\nb\n")
    chapters = detect_chapters(blocks)
    # No level repeats, so the shallowest (h1) wins and everything is one chapter.
    assert len(chapters) == 1
    assert chapters[0].title == "Only"
    assert chapters[0].level == 1


def test_detect_chapters_with_no_headings_yields_one_preamble_chapter() -> None:
    blocks = split_blocks("just prose\n\nmore prose\n")
    chapters = detect_chapters(blocks)
    # Nothing to split on, so the whole document is one chapter. Every block still gets a
    # chapter id, which is what lets the chunker treat every document uniformly.
    assert len(chapters) == 1
    assert chapters[0].title == ""
    assert chapters[0].level == 0
    assert chapters[0].block_first == 0
    assert chapters[0].block_last == len(blocks) - 1
    assert all(b.chapter_id == chapters[0].id for b in blocks)


def test_chapter_ranges_cover_every_block_exactly_once() -> None:
    markdown = "# A\n\ntext\n\n# B\n\nmore\n\n# C\n\nlast\n"
    blocks, chapters = parse_markdown(markdown)
    covered: list[int] = []
    for chapter in chapters:
        covered.extend(range(chapter.block_first, chapter.block_last + 1))
    assert covered == list(range(len(blocks)))


# -- M2: YAML front matter is metadata, never a translatable paragraph -------------------


def test_leading_front_matter_is_one_non_translatable_block() -> None:
    markdown = (
        '---\ntitle: "The Lantern Keeper"\nauthor: "Fixture Author"\nlang: en\n---\n\n# H\n\nBody\n'
    )
    blocks = split_blocks(markdown)

    front = blocks[0]
    assert front.kind == "frontmatter"
    assert front.translatable is False
    assert front.source_md == (
        '---\ntitle: "The Lantern Keeper"\nauthor: "Fixture Author"\nlang: en\n---'
    )
    # One block, not the hr + para + hr the YAML body used to be segmented into.
    assert [b.kind for b in blocks] == ["frontmatter", "heading", "para"]
    assert "hr" not in {b.kind for b in blocks}


def test_front_matter_is_recognised_only_at_the_very_start() -> None:
    blocks = split_blocks("Paragraph\n\n---\n\nMore\n")
    assert [b.kind for b in blocks] == ["para", "hr", "para"]


def test_unterminated_front_matter_stays_a_rule() -> None:
    blocks = split_blocks("---\ntitle: X\n\nBody\n")
    # No closing delimiter: guessing the end would swallow the document, so it stays an hr.
    assert blocks[0].kind == "hr"


def test_render_emits_front_matter_verbatim() -> None:
    markdown = "---\ntitle: X\n---\n\nBody\n"
    blocks = split_blocks(markdown)
    front, para = blocks[0], blocks[-1]

    assert render(blocks, {para.id: "Corpo"}) == "---\ntitle: X\n---\n\nCorpo\n"
    # A translation keyed to the front matter's id can never reach the output: the block is
    # not translatable, so render always falls back to its source slice.
    assert render(blocks, {front.id: "TAMPERED"}) == markdown


def test_table_cell_may_hold_a_footnote_reference_and_an_image() -> None:
    markdown = "| a | b |\n|---|---|\n| see[^1] | ![x](img.png) |\n\n[^1]: note\n"
    blocks = split_blocks(markdown)

    assert blocks[0].kind == "table"
    assert "[^1]" in blocks[0].source_md
    assert "![x](img.png)" in blocks[0].source_md
    assert blocks[1].kind == "footnote_def"
    assert serialize(blocks) == markdown
