from __future__ import annotations

from local_llm_translator.splitter import (
    Section,
    _detect_chapter_level,
    estimate_tokens,
    split_markdown,
)
from local_llm_translator.splitter import _parse_headings as parse_headings


class TestEstimateTokens:
    def test_empty(self) -> None:
        assert estimate_tokens("") == 1

    def test_four_words(self) -> None:
        assert estimate_tokens("one two three four") == 1

    def test_eight_words(self) -> None:
        assert estimate_tokens("one two three four five six seven eight") == 2


class TestDetectChapterLevel:
    def test_deepest_with_multiple(self, sample_markdown: str) -> None:
        # sample_markdown has 3 h2s and 2 h1s → should pick h2
        assert _detect_chapter_level(sample_markdown) == 2

    def test_flat_book(self, flat_markdown: str) -> None:
        # flat_markdown has 2 h1s and no h2s → h1
        assert _detect_chapter_level(flat_markdown) == 1

    def test_no_headings(self) -> None:
        assert _detect_chapter_level("Just text\n\nNo headings here.") == 1


class TestParseHeadings:
    def test_parse_at_h1(self, flat_markdown: str) -> None:
        sections = parse_headings(flat_markdown, chapter_level=1)
        assert len(sections) == 2
        assert sections[0].heading == "Chapter 1"
        assert sections[1].heading == "Chapter 2"

    def test_parse_at_h2_with_preamble(self, sample_markdown: str) -> None:
        sections = parse_headings(sample_markdown, chapter_level=2)
        assert len(sections) == 4
        # First section is the intro text between first h1 and first h2
        assert sections[0].heading == "Part One: The Beginning"
        assert sections[0].level == 1
        assert "introductory text" in sections[0].text.lower()
        # Then chapters (Part Two has no text → skipped)
        assert sections[1].heading == "Chapter 1: The Start"
        assert sections[2].heading == "Chapter 2: The Middle"
        assert sections[3].heading == "Chapter 3: The Finish"

    def test_no_data_loss(self, sample_markdown: str) -> None:
        sections = parse_headings(sample_markdown, chapter_level=2)
        reconstructed = []
        for s in sections:
            if s.heading:
                reconstructed.append(f"{'#' * s.level} {s.heading}")
            reconstructed.append(s.text)
        result = "\n".join(reconstructed)
        # Check that key content is preserved
        assert "introductory text" in result
        assert "first chapter content" in result
        assert "Middle chapter here" in result
        assert "Final chapter content" in result


class TestSplitMarkdown:
    def test_split_flat_book(self, flat_markdown: str) -> None:
        sections = split_markdown(flat_markdown, context_size=8192)
        assert len(sections) >= 2
        assert all(isinstance(s, Section) for s in sections)
        assert all(s.token_estimate > 0 for s in sections)

    def test_split_multi_part(self, sample_markdown: str) -> None:
        sections = split_markdown(sample_markdown, context_size=8192)
        # Should split at h2 level → 3 sections (intro + 2)
        assert len(sections) >= 3

    def test_oversized_section(self) -> None:
        big = f"# Chapter 1\n\n{'hello world ' * 2000}\n\n# Chapter 2\n\nsmall"
        sections = split_markdown(big, context_size=512)  # ~1000 words / 4 ≈ 250 tokens
        # Should split ch1 because it exceeds 512 tokens
        assert len(sections) >= 2
        assert all(s.token_estimate <= 512 for s in sections)

    def test_contiguous_indices(self) -> None:
        md = f"# A\n\n{'x ' * 100}\n\n# B\n\n{'y ' * 100}"
        sections = split_markdown(md, context_size=10000)
        for i, s in enumerate(sections):
            assert s.index == i
