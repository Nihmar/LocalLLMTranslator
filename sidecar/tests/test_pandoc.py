"""The pandoc bridge: unit combination, metadata, format mapping and atomic output.

The suite skips cleanly when the binary is absent (and the PDF case when no LaTeX engine
is available), because invoking a real pandoc is the whole point of the module.
"""

from __future__ import annotations

import base64
import os
import shutil
import zipfile
from pathlib import Path

import pytest
from llmtranslator_sidecar.pandoc import MissingDependencyError, PandocError, build

REPO_ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = REPO_ROOT / "pandoc" / "templates" / "book.html"
CSS = REPO_ROOT / "pandoc" / "styles" / "book.css"
FOOTNOTES_FILTER = REPO_ROOT / "pandoc" / "filters" / "footnotes.lua"

#: A 4x4 PNG, the same bytes the fixture generator embeds in `content.epub`.
PNG_NAME = "harbour.png"
PNG_BYTES = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAACXBIWXMAAA7EAAAOxAGVKw4bAAAAEUlE"
    "QVR4nGM4cekOHDEQxwEA/uEnYVFgUy4AAAAASUVORK5CYII="
)


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


def test_output_path_that_is_a_directory_raises_pandoc_error(tmp_path: Path) -> None:
    target = tmp_path / "book_dir"
    target.mkdir()

    with pytest.raises(PandocError) as info:
        build(
            units=[_unit(tmp_path, "one.md", "Chapter One", "Body.\n")],
            metadata={"title": "T"},
            output_path=str(target),
            output_format="html",
        )
    assert info.value.code == 1002
    assert isinstance(info.value.log, str)
    # The failed rename must not leak the sibling temp file next to the target.
    assert not list(tmp_path.glob(f".{target.name}.*"))
    assert target.is_dir()


def test_unreadable_unit_raises_pandoc_error(tmp_path: Path) -> None:
    with pytest.raises(PandocError) as info:
        build(
            units=[{"path": str(tmp_path / "gone.md"), "title": "Chapter"}],
            metadata={"title": "T"},
            output_path=str(tmp_path / "book.html"),
            output_format="html",
        )
    assert info.value.code == 1002


def _media_book(tmp_path: Path) -> tuple[list[dict[str, str]], Path]:
    """A one-chapter book whose Markdown references `assets/harbour.png` relatively."""
    work = tmp_path / "work"
    assets = work / "assets"
    assets.mkdir(parents=True)
    (assets / PNG_NAME).write_bytes(PNG_BYTES)
    units = [_unit(work, "one.md", "Chapter One", f"![The harbour at dawn](assets/{PNG_NAME})\n")]
    return units, work


def _embedded_images(output: Path) -> list[str]:
    with zipfile.ZipFile(output) as archive:
        return [name for name in archive.namelist() if name.lower().endswith(".png")]


def test_resource_path_resolves_relative_media(tmp_path: Path) -> None:
    """The combined document lives in a throwaway directory, so a relative `assets/...`
    href only resolves through the resource path the caller passes."""
    units, work = _media_book(tmp_path)
    output = work / "book.epub"

    build(
        units=units,
        metadata={"title": "The Lantern Keeper"},
        output_path=str(output),
        output_format="epub",
        resource_path=[str(work)],
    )

    assert _embedded_images(output), "the image must be embedded, not dropped"


def test_without_a_resource_path_relative_media_is_dropped(tmp_path: Path) -> None:
    """The flag is load-bearing: without it pandoc cannot find the image at all."""
    units, work = _media_book(tmp_path)
    output = work / "book.epub"

    build(
        units=units,
        metadata={"title": "The Lantern Keeper"},
        output_path=str(output),
        output_format="epub",
    )

    assert not _embedded_images(output)
    assert output.is_file(), "a missing resource is a warning, not a failed build"


def test_toc_and_lua_filters_are_applied(tmp_path: Path) -> None:
    """`toc` and `lua_filters` reach pandoc, and the filters actually run."""
    units = [
        _unit(
            tmp_path,
            "one.md",
            "Chapter One",
            "The harbour was quiet.[^1]\n\n[^1]: A note kept for the end.\n",
        )
    ]
    output = tmp_path / "book.html"
    build(
        units=units,
        metadata={"title": "Test Book", "footnotes-endnotes": True},
        output_path=str(output),
        output_format="html",
        toc=True,
        lua_filters=[str(FOOTNOTES_FILTER)],
    )

    html = output.read_text(encoding="utf-8")
    # The TOC is generated from the unit heading.
    assert 'id="TOC"' in html
    # The footnotes filter turned the note into an endnote section, which is the
    # filter's documented deterministic form when `footnotes-endnotes` is set.
    assert "A note kept for the end." in html
    assert "endnote" in html
    assert "[1]" in html


def test_top_level_division_is_forwarded(tmp_path: Path) -> None:
    """`top_level_division=chapter` is accepted by the LaTeX writer (skipped without one)."""
    if LATEX_ENGINE is None:
        pytest.skip("no LaTeX engine is available")
    units = [_unit(tmp_path, "one.md", "Chapter One", "Body.\n")]
    output = tmp_path / "book.pdf"
    build(
        units=units,
        metadata={"title": "Test Book"},
        output_path=str(output),
        output_format="pdf",
        top_level_division="chapter",
    )
    assert output.read_bytes().startswith(b"%PDF")


def test_a_hung_pandoc_is_killed_and_reported(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A pandoc that never returns must not block the sequential RPC loop forever."""
    slow = tmp_path / "slow-pandoc"
    slow.write_text("#!/bin/sh\nsleep 30\n", encoding="utf-8")
    slow.chmod(0o755)
    monkeypatch.setenv("LLMTRANSLATOR_PANDOC", str(slow))
    monkeypatch.setenv("LLMTRANSLATOR_PANDOC_TIMEOUT", "0.2")

    output = tmp_path / "book.html"
    with pytest.raises(PandocError, match="timed out"):
        build(
            units=[_unit(tmp_path, "one.md", "Chapter One", "Body.\n")],
            metadata={"title": "Test Book"},
            output_path=str(output),
            output_format="html",
        )

    # No half-written artefact and no leaked temp file next to the target.
    assert not output.exists()
    assert list(tmp_path.glob(".book.html.*")) == []


def test_timeout_defaults_when_the_override_is_invalid(monkeypatch: pytest.MonkeyPatch) -> None:
    from llmtranslator_sidecar.pandoc import DEFAULT_TIMEOUT_SECONDS, timeout_seconds

    monkeypatch.delenv("LLMTRANSLATOR_PANDOC_TIMEOUT", raising=False)
    assert timeout_seconds() == DEFAULT_TIMEOUT_SECONDS
    for invalid in ("nonsense", "0", "-3"):
        monkeypatch.setenv("LLMTRANSLATOR_PANDOC_TIMEOUT", invalid)
        assert timeout_seconds() == DEFAULT_TIMEOUT_SECONDS
    monkeypatch.setenv("LLMTRANSLATOR_PANDOC_TIMEOUT", "1.5")
    assert timeout_seconds() == 1.5
