from __future__ import annotations

import tempfile
from collections.abc import Generator
from pathlib import Path

import fitz  # pyright: ignore[reportMissingTypeStubs]
import pytest


@pytest.fixture
def tmp_output() -> Generator[Path]:
    with tempfile.TemporaryDirectory() as d:
        yield Path(d)


@pytest.fixture
def sample_pdf(tmp_output: Path) -> Path:
    """Create a tiny 3-page PDF with headings and paragraphs."""
    path = tmp_output / "sample.pdf"
    doc = fitz.open()
    pages = [
        ("# Chapter 1", "First chapter.\n\nContent.\n\n![img](images/fig1.png)"),
        ("## Section 1.1", "Sub-section content here.\n\nMore text for testing."),
        ("# Chapter 2", "Second chapter begins here.\n\nIt continues."),
    ]
    for heading, body in pages:
        page = doc.new_page()
        page.insert_text((50, 50), heading, fontsize=14)
        page.insert_text((50, 80), body, fontsize=11)

    doc.save(str(path))
    doc.close()
    return path


@pytest.fixture
def sample_markdown() -> str:
    return """# Part One: The Beginning

Some introductory text before the first chapter.

## Chapter 1: The Start

This is the first chapter content.

## Chapter 2: The Middle

Middle chapter here.

# Part Two: The End

## Chapter 3: The Finish

Final chapter content.
"""


@pytest.fixture
def flat_markdown() -> str:
    return """# Chapter 1

Content of chapter one.

# Chapter 2

Content of chapter two.
"""
