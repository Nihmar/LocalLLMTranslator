"""Regression tests for the shipped ``pandoc/`` export assets.

A small multi-chapter Markdown document is pushed through the real ``pandoc``
binary and through each template, filter and stylesheet under ``pandoc/``:

  * ``pandoc/templates/book.tex``   — must compile to a real PDF (the LaTeX
    writer path that previously aborted on a hyperref key/value parse error);
  * ``pandoc/templates/book.html``  — must render the title block and the TOC;
  * ``pandoc/styles/book.css`` and ``pandoc/styles/book.tex`` — must be usable
    with those templates;
  * ``pandoc/filters/*.lua``        — must load and apply without error.

Everything is skipped cleanly when ``pandoc`` (or, for the PDF case, a LaTeX
engine) is absent, because invoking the real toolchain is the whole point.
"""

from __future__ import annotations

import base64
import os
import shutil
import subprocess
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
PANDOC_DIR = REPO_ROOT / "pandoc"
TEMPLATE_TEX = PANDOC_DIR / "templates" / "book.tex"
TEMPLATE_HTML = PANDOC_DIR / "templates" / "book.html"
STYLE_CSS = PANDOC_DIR / "styles" / "book.css"
STYLE_TEX = PANDOC_DIR / "styles" / "book.tex"
FILTERS = (
    PANDOC_DIR / "filters" / "footnotes.lua",
    PANDOC_DIR / "filters" / "tables.lua",
    PANDOC_DIR / "filters" / "epub_cleanup.lua",
)

PANDOC = os.environ.get("LLMTRANSLATOR_PANDOC") or shutil.which("pandoc")


def _find_latex_engine() -> str | None:
    for engine in ("pdflatex", "xelatex", "lualatex"):
        found = shutil.which(engine)
        if found is not None:
            return found
    return None


LATEX_ENGINE = _find_latex_engine()

# An 8x8 RGB PNG: large enough for graphicx/fontspec to embed it without a
# "could not determine size" warning, small enough to inline as a fixture.
_FIGURE_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAaklEQVR4nAXB0QAAAAhDwZGMJJJI"
    "+nwUkYwkou4kYVGixYgVESckY1OmzZg1MWekwkUVXUyxRYorpMZNNd1Ms02aa6TBQw09zLBDhhuk"
    "xUstvcyyS5ZbpOBQocOEDQkXpMNHHX3MsUeOOx6wLFSBm6x5zwAAAABJRU5ErkJggg=="
)

# A multi-chapter document exercising every structure the template must render:
# title page + subtitle, TOC, headings, lists, blockquote, table, fenced code,
# footnote and an image.
DOCUMENT = """\
---
title: "The Lantern Keeper"
subtitle: "A Tale of the Harbour Light"
author:
  - "Fixture Author"
date: "2024-01-01"
lang: en
toc: true
toc-title: "Contents"
colorlinks: true
---

# Chapter One: The Harbour {#ch1}

The harbour was quiet that morning.[^note]

[^note]: A note body with *emphasis* and `code`.

## A Section

A paragraph with **bold**, *italic*, `inline code`, a [link](https://example.org/),
math $E = mc^2$, and a placeholder \u27e67\u27e7 the cleanup filter removes.

- iron key
- brass telescope

1. Fill the lamps
2. Trim the wicks

> The wind rose from the west.

Table 1: Stores overview

| Element | Number |
|:--------|-------:|
| Lamps   |      4 |
| Wicks   |     12 |

```python
def keep(lamp: int) -> int:
    return lamp * 2
```

![The harbour at dawn](figure.png)

# Chapter Two: The Sea {#ch2}

The sea was still for three days.[^m]

[^m]: A second note.
"""

# Markers that any of these would mean the template or the LaTeX run went wrong.
_TEMPLATE_ERROR_MARKERS = (
    "[ERROR]",
    "[WARNING]",
    "Error compiling template",
    "Paragraph ended before",
    "Missing \\begin{document}",
    "Undefined control sequence",
    "! LaTeX Error",
    "! Package",
    "! Font",
    "! pdfTeX error",
    "Fatal error occurred",
)


@pytest.fixture(scope="session")
def pandoc_bin() -> str:
    if PANDOC is None:
        pytest.skip("pandoc binary is not available")
    return PANDOC


@pytest.fixture(scope="session")
def latex_engine() -> str:
    if LATEX_ENGINE is None:
        pytest.skip("no LaTeX engine is available")
    return LATEX_ENGINE


def _run(
    binary: str, args: list[str], cwd: Path, timeout: int = 300
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [binary, *args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
        check=False,
        timeout=timeout,
    )


def _assert_clean(proc: subprocess.CompletedProcess[str]) -> None:
    assert proc.returncode == 0, f"pandoc exited with {proc.returncode}:\n{proc.stderr}"
    hits = [marker for marker in _TEMPLATE_ERROR_MARKERS if marker in proc.stderr]
    assert not hits, f"stderr reports a template problem {hits}:\n{proc.stderr}"


def _write_document(tmp_path: Path) -> Path:
    (tmp_path / "figure.png").write_bytes(_FIGURE_PNG)
    document = tmp_path / "input.md"
    document.write_text(DOCUMENT, encoding="utf-8")
    return document


def test_book_tex_template_builds_a_pdf(pandoc_bin: str, latex_engine: str, tmp_path: Path) -> None:
    document = _write_document(tmp_path)
    output = tmp_path / "book.pdf"

    proc = _run(
        pandoc_bin,
        [
            str(document),
            "--standalone",
            "--to=pdf",
            f"--pdf-engine={Path(latex_engine).name}",
            f"--template={TEMPLATE_TEX}",
            "--top-level-division=chapter",
            "--toc",
            "-o",
            str(output),
        ],
        tmp_path,
    )

    _assert_clean(proc)
    assert output.exists(), "no PDF was produced"
    payload = output.read_bytes()
    assert payload.startswith(b"%PDF"), "the artifact is not a PDF"
    assert len(payload) > 1024, "the PDF is suspiciously small"


def test_book_tex_style_include_is_usable(
    pandoc_bin: str, latex_engine: str, tmp_path: Path
) -> None:
    document = _write_document(tmp_path)
    output = tmp_path / "book.pdf"

    proc = _run(
        pandoc_bin,
        [
            str(document),
            "--standalone",
            "--to=pdf",
            f"--pdf-engine={Path(latex_engine).name}",
            f"--template={TEMPLATE_TEX}",
            f"--include-in-header={STYLE_TEX}",
            "--top-level-division=chapter",
            "-o",
            str(output),
        ],
        tmp_path,
    )

    _assert_clean(proc)
    assert output.read_bytes().startswith(b"%PDF")


def test_book_html_template_renders_title_and_toc(pandoc_bin: str, tmp_path: Path) -> None:
    document = _write_document(tmp_path)
    output = tmp_path / "book.html"

    proc = _run(
        pandoc_bin,
        [
            str(document),
            "--standalone",
            "--to=html5",
            f"--template={TEMPLATE_HTML}",
            f"--css={STYLE_CSS}",
            "--toc",
            "-o",
            str(output),
        ],
        tmp_path,
    )

    _assert_clean(proc)
    html = output.read_text(encoding="utf-8")
    assert "The Lantern Keeper" in html, "the title is missing"
    assert 'id="title-block-header"' in html, "the title block is missing"
    assert 'id="TOC"' in html, "the table of contents is missing"
    assert STYLE_CSS.name in html, "the stylesheet was not referenced"
    assert "Chapter One" in html and "Chapter Two" in html
    assert len(html) > 1024, "the HTML is suspiciously small"


def test_lua_filters_load_and_apply(pandoc_bin: str, tmp_path: Path) -> None:
    document = _write_document(tmp_path)
    for filter_path in FILTERS:
        assert filter_path.is_file(), f"missing filter {filter_path}"
        proc = _run(
            pandoc_bin,
            ["-t", "native", f"--lua-filter={filter_path}", str(document)],
            tmp_path,
        )
        _assert_clean(proc)
        assert proc.stdout.strip(), f"{filter_path.name} produced no output"


def test_filters_and_templates_compose(pandoc_bin: str, tmp_path: Path) -> None:
    document = _write_document(tmp_path)
    output = tmp_path / "book.html"

    filter_args = [arg for path in FILTERS for arg in ("--lua-filter", str(path))]
    proc = _run(
        pandoc_bin,
        [
            str(document),
            "--standalone",
            "--to=html5",
            f"--template={TEMPLATE_HTML}",
            f"--css={STYLE_CSS}",
            *filter_args,
            "-o",
            str(output),
        ],
        tmp_path,
    )

    _assert_clean(proc)
    html = output.read_text(encoding="utf-8")
    assert "The Lantern Keeper" in html
    assert "\u27e67\u27e7" not in html, "the placeholder leaked through the filters"
