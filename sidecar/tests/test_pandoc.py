"""The pandoc bridge: unit combination, metadata, format mapping and atomic output.

The suite skips cleanly when the binary is absent (and the PDF case when no LaTeX engine
is available), because invoking a real pandoc is the whole point of the module.
"""

from __future__ import annotations

import os
import shutil
import zipfile
from pathlib import Path

import pytest
from llmtranslator_sidecar.pandoc import MissingDependencyError, PandocError, build

REPO_ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = REPO_ROOT / "pandoc" / "templates" / "book.html"
CSS = REPO_ROOT / "pandoc" / "styles" / "book.css"


def _find_latex_engine() -> str | None:
    for engine in ("pdflatex", "xelatex", "lualatex", "tectonic"):
        found = shutil.which(engine)
        if found is not None:
            return found
    return None


PANDOC = os.environ.get("LLMTRANSLATOR_PANDOC") or shutil.which("pandoc")
LATEX_ENGINE = _find_latex_engine()

pytestmark = pytest.mark.skipif(PANDOC is None, reason="pandoc binary is not available")
requires_latex = pytest.mark.skipif(LATEX_ENGINE is None, reason="no LaTeX engine is available")


def _unit(tmp_path: Path, name: str, title: str, body: str) -> dict[str, str]:
    path = tmp_path / name
    path.write_text(body, encoding="utf-8")
    return {"path": str(path), "title": title}


def test_html_build_returns_result_and_writes_output(tmp_path: Path) -> None:
    units = [_unit(tmp_path, "one.md", "Chapter One", "The harbour was quiet.\n")]
    output = tmp_path / "book.html"
    result = build(
        units=units,
        metadata={"title": "Test Book", "author": "Fixture Author"},
        output_path=str(output),
        output_format="html",
        template=str(TEMPLATE),
        css=str(CSS),
    )

    assert result["output_path"] == str(output)
    assert isinstance(result["log"], str)
    assert isinstance(result["duration_ms"], int)
    assert result["duration_ms"] >= 0

    html = output.read_text(encoding="utf-8")
    assert "Chapter One" in html
    assert "The harbour was quiet." in html
    assert "Test Book" in html
    assert "Fixture Author" in html


def test_units_are_combined_in_the_given_order(tmp_path: Path) -> None:
    units = [
        _unit(tmp_path, "1.md", "First Chapter", "Alpha body.\n"),
        _unit(tmp_path, "2.md", "Second Chapter", "Beta body.\n"),
    ]
    output = tmp_path / "book.html"
    build(units=units, metadata={"title": "T"}, output_path=str(output), output_format="html")

    html = output.read_text(encoding="utf-8")
    assert html.index("First Chapter") < html.index("Alpha body.")
    assert html.index("Alpha body.") < html.index("Second Chapter")
    assert html.index("Second Chapter") < html.index("Beta body.")


def test_epub_is_a_zip_archive(tmp_path: Path) -> None:
    output = tmp_path / "book.epub"
    build(
        units=[_unit(tmp_path, "one.md", "Chapter One", "The harbour was quiet.\n")],
        metadata={"title": "Test Book", "language": "it"},
        output_path=str(output),
        output_format="epub",
        template=str(TEMPLATE),
        css=str(CSS),
    )
    assert output.stat().st_size > 0
    assert zipfile.is_zipfile(output)


def test_docx_is_a_zip_archive(tmp_path: Path) -> None:
    output = tmp_path / "book.docx"
    build(
        units=[_unit(tmp_path, "one.md", "Chapter One", "The harbour was quiet.\n")],
        metadata={"title": "Test Book"},
        output_path=str(output),
        output_format="docx",
    )
    assert output.stat().st_size > 0
    assert zipfile.is_zipfile(output)


@requires_latex
def test_pdf_starts_with_the_pdf_header(tmp_path: Path) -> None:
    output = tmp_path / "book.pdf"
    build(
        units=[_unit(tmp_path, "one.md", "Chapter One", "The harbour was quiet.\n")],
        metadata={"title": "Test Book", "author": "Fixture Author"},
        output_path=str(output),
        output_format="pdf",
    )
    assert output.read_bytes().startswith(b"%PDF")


def test_output_directory_is_created(tmp_path: Path) -> None:
    output = tmp_path / "nested" / "deep" / "book.html"
    build(
        units=[_unit(tmp_path, "one.md", "Chapter One", "Body.\n")],
        metadata={"title": "T"},
        output_path=str(output),
        output_format="html",
    )
    assert output.is_file()


def test_existing_output_is_replaced(tmp_path: Path) -> None:
    output = tmp_path / "book.html"
    output.write_text("stale content", encoding="utf-8")
    build(
        units=[_unit(tmp_path, "one.md", "Chapter One", "Fresh body.\n")],
        metadata={"title": "T"},
        output_path=str(output),
        output_format="html",
    )
    html = output.read_text(encoding="utf-8")
    assert "stale content" not in html
    assert "Fresh body." in html


def test_missing_binary_raises_missing_dependency(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("LLMTRANSLATOR_PANDOC", "definitely-not-a-real-pandoc-binary")
    with pytest.raises(MissingDependencyError) as info:
        build(
            units=[],
            metadata={},
            output_path=str(tmp_path / "book.html"),
            output_format="html",
        )
    assert info.value.code == 1003


def test_pandoc_failure_raises_pandoc_error_carrying_the_log(tmp_path: Path) -> None:
    output = tmp_path / "book.html"
    with pytest.raises(PandocError) as info:
        build(
            units=[_unit(tmp_path, "one.md", "Chapter One", "Body.\n")],
            metadata={"title": "T"},
            output_path=str(output),
            output_format="html",
            template=str(tmp_path / "does-not-exist.html"),
        )
    assert info.value.code == 1002
    assert info.value.log
    # Atomic write: a failed build never leaves a partial export behind.
    assert not output.exists()
