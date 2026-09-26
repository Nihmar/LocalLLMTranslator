"""Tests for the extractor layer: format detection and ingestion.

The fixtures are produced by the project's own generator (``tools/make_fixtures.py``) in a
temporary directory, so the suite stays hermetic and offline: nothing is downloaded and no
repository file is written. Running the generator out of process also keeps its optional,
untyped dependencies out of this module.

The load-bearing assertion is the round-trip invariant ``serialize(split_blocks(md)) == md``:
whatever a backend emits must survive the Markdown IR unchanged, otherwise untranslated
blocks would be silently rewritten downstream.
"""

from __future__ import annotations

import importlib.util
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

import pytest
from llmtranslator_sidecar.extractors import (
    ExtractionError,
    MissingDependencyError,
    detect_format,
    extract,
)
from llmtranslator_sidecar.parse import parse_markdown, split_blocks
from llmtranslator_sidecar.serialize import serialize

REPO_ROOT = Path(__file__).resolve().parents[2]
MAKE_FIXTURES = REPO_ROOT / "tools" / "make_fixtures.py"

#: The three sample documents built by the fixture generator.
FORMATS = {
    "content.md": "markdown",
    "content.epub": "epub",
    "content.pdf": "pdf",
}

MARKER_IS_INSTALLED = importlib.util.find_spec("marker") is not None


@pytest.fixture(scope="module")
def fixtures(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Build the small fixtures once per module and return their directory."""
    out_dir = tmp_path_factory.mktemp("extractor-fixtures")
    process = subprocess.run(
        [sys.executable, str(MAKE_FIXTURES), "--out-dir", str(out_dir), "--small-only", "--quiet"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=300,
    )
    assert process.returncode == 0, process.stderr
    return out_dir


def produced(result: dict[str, Any]) -> str:
    """Read back the single Markdown file an ``extract`` call produced."""
    return Path(str(result["markdown_path"])).read_text(encoding="utf-8")


def test_detect_format_reports_format_and_backends(fixtures: Path) -> None:
    assert detect_format(str(fixtures / "content.md")) == {
        "format": "markdown",
        "backends": ["source"],
    }
    assert detect_format(str(fixtures / "content.epub")) == {
        "format": "epub",
        "backends": ["ebooklib"],
    }
    assert detect_format(str(fixtures / "content.pdf")) == {
        "format": "pdf",
        "backends": ["pymupdf4llm", "marker"],
    }


def test_detect_format_uses_magic_bytes_over_extension(fixtures: Path, tmp_path: Path) -> None:
    renamed_pdf = tmp_path / "renamed.dat"
    renamed_epub = tmp_path / "also_renamed.dat"
    shutil.copyfile(fixtures / "content.pdf", renamed_pdf)
    shutil.copyfile(fixtures / "content.epub", renamed_epub)

    assert detect_format(str(renamed_pdf))["format"] == "pdf"
    assert detect_format(str(renamed_epub))["format"] == "epub"


def test_detect_format_unknown_extension_raises(tmp_path: Path) -> None:
    mystery = tmp_path / "notes.xyz"
    mystery.write_text("no signature, no known extension\n", encoding="utf-8")
    with pytest.raises(ExtractionError):
        detect_format(str(mystery))


def test_detect_format_missing_file_raises(tmp_path: Path) -> None:
    with pytest.raises(ExtractionError):
        detect_format(str(tmp_path / "absent.md"))


def test_extract_unknown_format_raises(tmp_path: Path) -> None:
    mystery = tmp_path / "notes.xyz"
    mystery.write_text("no signature, no known extension\n", encoding="utf-8")
    with pytest.raises(ExtractionError):
        extract(str(mystery), str(tmp_path / "work"))


def test_markdown_passthrough_keeps_content_and_reads_front_matter(
    fixtures: Path, tmp_path: Path
) -> None:
    source = (fixtures / "content.md").read_text(encoding="utf-8")
    result = extract(str(fixtures / "content.md"), str(tmp_path))

    assert result["metadata"]["title"] == "The Lantern Keeper"
    assert result["metadata"]["author"] == "Fixture Author"
    assert result["metadata"]["lang"] == "en"

    markdown = produced(result)
    assert markdown == source, "passthrough must not rewrite the Markdown source"
    assert markdown.startswith("---\n"), "the front matter block stays in the document"
    assert "# The Lantern Keeper" in markdown
    assert "|:--------|-------:|:------:|" in markdown
    assert "[^1]:" in markdown


def test_extract_epub_yields_headings_paragraphs_and_metadata(
    fixtures: Path, tmp_path: Path
) -> None:
    result = extract(str(fixtures / "content.epub"), str(tmp_path))
    markdown = produced(result)

    assert result["metadata"]["title"] == "The Lantern Keeper"
    assert result["metadata"]["author"] == "Fixture Author"
    assert result["metadata"]["language"] == "en"

    assert "# The Lantern Keeper" in markdown
    assert "## Chapter One: The Harbour" in markdown
    assert "## Chapter Two: The Quiet Sea" in markdown
    assert "The harbour was **quiet** that morning." in markdown

    # lists, blockquote, table, fenced code, image link and footnotes all survive
    assert "1. Fill the lamps" in markdown
    assert "> The wind rose from the *west*" in markdown
    assert "| Element | Number | Notes |" in markdown
    assert "```sql" in markdown
    assert "![The harbour at dawn](harbour.png)" in markdown
    assert "[^1]:" in markdown and "[^2]:" in markdown

    titles = [chapter["title"] for chapter in result["chapters"]]
    assert "Chapter One: The Harbour" in titles
    assert "Chapter Two: The Quiet Sea" in titles


def test_extract_pdf_yields_text_without_warnings(fixtures: Path, tmp_path: Path) -> None:
    result = extract(str(fixtures / "content.pdf"), str(tmp_path))
    markdown = produced(result)

    assert result["warnings"] == []
    assert result["metadata"]["title"] == "The Lantern Keeper"
    assert result["metadata"]["author"] == "Fixture Author"
    assert result["metadata"]["page_count"] >= 2

    assert "The Lantern Keeper" in markdown
    assert "Chapter One: The Harbour" in markdown
    assert "The harbour was quiet that morning." in markdown
    assert "Lamps" in markdown


@pytest.mark.skipif(MARKER_IS_INSTALLED, reason="marker is installed, so it cannot be 'missing'")
def test_marker_backend_raises_when_not_installed(fixtures: Path, tmp_path: Path) -> None:
    with pytest.raises(MissingDependencyError) as error:
        extract(str(fixtures / "content.pdf"), str(tmp_path), pdf_backend="marker")
    assert error.value.code == 1003
    assert error.value.backend == "marker"


@pytest.mark.parametrize("name", sorted(FORMATS))
def test_produced_markdown_round_trips(fixtures: Path, tmp_path: Path, name: str) -> None:
    markdown = produced(extract(str(fixtures / name), str(tmp_path)))
    assert serialize(split_blocks(markdown)) == markdown


@pytest.mark.parametrize("name", sorted(FORMATS))
def test_chapters_match_a_later_parse(fixtures: Path, tmp_path: Path, name: str) -> None:
    result = extract(str(fixtures / name), str(tmp_path))
    _blocks, chapters = parse_markdown(produced(result))
    expected = [{"title": c.title, "level": c.level, "order": c.order} for c in chapters]
    assert result["chapters"] == expected
    assert [chapter["order"] for chapter in result["chapters"]] == list(
        range(len(result["chapters"]))
    )


@pytest.mark.parametrize("name", sorted(FORMATS))
def test_atomic_write_leaves_only_the_document(fixtures: Path, tmp_path: Path, name: str) -> None:
    result = extract(str(fixtures / name), str(tmp_path))

    assert sorted(entry.name for entry in tmp_path.iterdir()) == ["document.md"]
    assert not list(tmp_path.glob("*.tmp"))

    data = Path(str(result["markdown_path"])).read_bytes()
    assert b"\r\n" not in data
    assert data.endswith(b"\n")


def test_extract_creates_the_work_dir(fixtures: Path, tmp_path: Path) -> None:
    work_dir = tmp_path / "nested" / "work"
    result = extract(str(fixtures / "content.md"), str(work_dir))
    assert work_dir.is_dir()
    assert Path(str(result["markdown_path"])).parent == work_dir


def test_extraction_error_carries_the_ingestion_code(tmp_path: Path) -> None:
    mystery = tmp_path / "notes.xyz"
    mystery.write_text("nope\n", encoding="utf-8")
    with pytest.raises(ExtractionError) as error:
        detect_format(str(mystery))
    assert error.value.code == 1001
