from __future__ import annotations

import fitz  # pyright: ignore[reportMissingTypeStubs]

from local_llm_translator.extractor import extract_markdown


class TestExtractor:
    def test_extract_simple_pdf(self, tmp_output):
        """Extract from a tiny programmatically-generated PDF."""
        pdf_path = tmp_output / "simple.pdf"
        doc = fitz.open()
        page = doc.new_page()
        page.insert_text((50, 50), "# Chapter 1", fontsize=14)
        page.insert_text((50, 80), "Hello world content.", fontsize=11)
        doc.save(str(pdf_path))
        doc.close()

        md = extract_markdown(pdf_path, tmp_output)
        assert "Chapter 1" in md
        assert "Hello world content" in md

    def test_extract_with_images(self, tmp_output):
        """Images directory is created."""
        pdf_path = tmp_output / "with_img.pdf"
        doc = fitz.open()
        page = doc.new_page()
        page.insert_text((50, 50), "Just text", fontsize=11)
        doc.save(str(pdf_path))
        doc.close()

        extract_markdown(pdf_path, tmp_output)
        images_dir = tmp_output / "images"
        assert images_dir.exists()
        assert images_dir.is_dir()

    def test_extract_nonexistent_file(self, tmp_output):
        import pytest

        with pytest.raises(Exception):
            extract_markdown(tmp_output / "nope.pdf", tmp_output)

    def test_extract_epub(self, tmp_output):
        """Extract from a programmatically-generated EPUB."""
        epub_path = tmp_output / "simple.epub"
        doc = fitz.open()
        page = doc.new_page()
        page.insert_text((50, 50), "# Prologue", fontsize=14)
        page.insert_text((50, 80), "Once upon a time.", fontsize=11)
        doc.save(str(epub_path))
        doc.close()

        md = extract_markdown(epub_path, tmp_output)
        assert "Prologue" in md
        assert "Once upon a time" in md
