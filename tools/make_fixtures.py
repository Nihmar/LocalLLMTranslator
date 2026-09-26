#!/usr/bin/env python3
"""Generate reproducible test documents for the LocalLLMTranslator pipeline.

Run from the repository root (the sidecar's uv project provides ``pymupdf`` and
``ebooklib``)::

    uv run --project sidecar python tools/make_fixtures.py            # small + large
    uv run --project sidecar python tools/make_fixtures.py --small-only

Outputs (default ``tools/fixtures/``, which is meant to be git-ignored — the
generated files are byte-identical across runs and can therefore be diffed):

``content.epub``
    A small EPUB with two nested chapters (h1/h2), a GFM-style table (alignment
    classes), footnote references/definitions, a blockquote, a fenced code
    block, an inline image and rich inline markup (bold, italic, link, code,
    math span).  Exercises the extractor + placeholder layer.
``content.md``
    The same content as Markdown (with a proper GFM table featuring alignment
    colons and ``[^n]:`` footnote definitions).
``content.pdf``
    The same story as a small PDF built with PyMuPDF: headings, paragraphs and
    a drawn table.
``large.epub``
    A large EPUB of roughly 1,000,000 characters of prose across ~50 chapters,
    for robustness and memory tests.

Determinism
-----------
* the OPF ``dcterms:modified`` timestamp is pinned via ebooklib's ``mtime``
  option and defensively rewritten;
* the EPUB identifier is a fixed UUID;
* the generated ZIP entries are rewritten with a fixed timestamp
  (``1980-01-01``) while preserving entry order (so ``mimetype`` stays first and
  uncompressed, as OCF requires);
* the Markdown and prose are pure functions of a fixed seed;
* the PDF metadata dates are pinned.

No ``.gitignore`` edit is performed by design; add ``tools/fixtures/`` to the
repository ``.gitignore`` so the generated artefacts are not committed.
"""

from __future__ import annotations

import argparse
import base64
import datetime
import hashlib
import json
import random
import re
import shutil
import sys
import zipfile
from pathlib import Path

try:  # exercised only when the requested fixtures need the dependency
    import ebooklib
    from ebooklib import epub
except ImportError:  # pragma: no cover - reported at runtime
    ebooklib = None
    epub = None

try:
    import pymupdf as fitz  # PyMuPDF (new canonical module name)
except ImportError:  # pragma: no cover - reported at runtime
    try:
        import fitz  # type: ignore[no-redef]
    except ImportError:
        fitz = None


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT_DIR = REPO_ROOT / "tools" / "fixtures"

FIXED_MTIME = datetime.datetime(2020, 1, 1, 0, 0, 0)
FIXED_UUID = "6f9619ff-8b86-d011-b42d-00c04fc964ff"
FIXED_PDF_DATE = "D:20200101000000Z"
FIXED_PDF_ID = "0123456789abcdef" * 4
ISO_TS_RE = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z")
UUID_RE = re.compile(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")

#: A deterministic 4x4 PNG used as the inline image of the fixtures.
IMAGE_PNG_BYTES = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAIAAAAmkwkpAAAACXBIWXMAAA7EAAAOxAGVKw4bAAAAEUlE"
    "QVR4nGM4cekOHDEQxwEA/uEnYVFgUy4AAAAASUVORK5CYII="
)
IMAGE_NAME = "harbour.png"

#: Deterministic word list for the large fixture.
WORDS = (
    "lantern harbour keeper tide rope salt fog beacon mast gull keel anchor "
    "storm shore light wick glass cotton barrel iron brass telescope chart "
    "compass wave current rope knot sail wind rain bell watch night dawn "
    "shadow silence echo drift ember stone chapel ledger boat pier quay "
    "signal horn reef gale swell cabin hatch rudder coral driftwood".split()
)

BOOK_CSS = """\
body { font-family: serif; line-height: 1.5; }
h1 { text-align: center; }
blockquote { margin-left: 2em; font-style: italic; }
table.data { border-collapse: collapse; }
table.data th, table.data td { border: 1px solid #444; padding: 0.25em 0.5em; }
.left { text-align: left; }
.right { text-align: right; }
.center { text-align: center; }
pre { padding: 0.5em; background: #f4f4f4; }
figure { margin: 1em 0; text-align: center; }
"""


# --------------------------------------------------------------------------- #
# Markdown fixture
# --------------------------------------------------------------------------- #


def markdown_small() -> str:
    """Return the small Markdown fixture (mirrors the EPUB content)."""
    return """\
---
title: "The Lantern Keeper"
author: "Fixture Author"
lang: en
---

# The Lantern Keeper

## Chapter One: The Harbour

The harbour was **quiet** that morning. *Nobody* expected the ship, and the
`keeper` wrote nothing in his log. He had seen the light of
[the northern tower](https://example.org/north) fade twice, then stop.

> The wind rose from the *west* and carried the smell of salt and old rope.

1. Fill the lamps
2. Trim the wicks
3. Lock the door

- iron key
- brass telescope
+ spare oil

| Element | Number | Notes  |
|:--------|-------:|:------:|
| Lamps   |      4 | glass  |
| Wicks   |     12 | cotton |
| Oil     |      3 | barrel |

```sql
SELECT lamp, wick FROM stores WHERE oil > 0 ORDER BY lamp;
```

![The harbour at dawn](harbour.png)

He muttered a phrase learned as a boy[^1] and turned away.

[^1]: A rhyme about storms and lanterns, learned at the knee.

Math: $E = mc^2$ and inline `x = 1` too.

## Chapter Two: The Quiet Sea

The sea was **still** for three days. Nobody spoke of the lantern, and the
keeper slept in the chair by the door.[^2]

[^2]: An old proverb of the coast.

> Silence is the loudest thing in a harbour town.
"""


# --------------------------------------------------------------------------- #
# EPUB fixture
# --------------------------------------------------------------------------- #


def _xhtml(title: str, body: str) -> str:  # noqa: ARG001 - title kept for symmetry with EpubHtml
    """Return the *body inner* fragment ebooklib wraps into a valid XHTML doc.

    ebooklib's ``EpubHtml.content`` must be the inner HTML of ``<body>``: the
    writer injects ``<html>``/``<head>``/``<title>`` and the EPUB namespaces
    (including ``epub:``, needed by ``epub:type`` footnote markup).
    """
    return body


def small_epub_chapters() -> list[dict[str, str]]:
    """Return the small EPUB chapters as ``{title, file_name, html}`` dicts."""
    chapter_one = _xhtml(
        "Chapter One: The Harbour",
        """\
  <h1>The Lantern Keeper</h1>
  <h2 id="ch1">Chapter One: The Harbour</h2>
  <p>The harbour was <strong>quiet</strong> that morning. <em>Nobody</em> expected the ship, and
  the <code>keeper</code> wrote nothing in his log. He had seen the light of
  <a href="https://example.org/north">the northern tower</a> fade twice, then stop.</p>
  <blockquote><p>The wind rose from the <em>west</em> and carried the smell of salt and old rope.</p></blockquote>
  <ol><li>Fill the lamps</li><li>Trim the wicks</li><li>Lock the door</li></ol>
  <ul><li>iron key</li><li>brass telescope</li><li>spare oil</li></ul>
  <table class="data">
    <caption>Stores</caption>
    <thead><tr><th class="left">Element</th><th class="right">Number</th><th class="center">Notes</th></tr></thead>
    <tbody>
      <tr><td class="left">Lamps</td><td class="right">4</td><td class="center">glass</td></tr>
      <tr><td class="left">Wicks</td><td class="right">12</td><td class="center">cotton</td></tr>
      <tr><td class="left">Oil</td><td class="right">3</td><td class="center">barrel</td></tr>
    </tbody>
  </table>
  <pre><code class="language-sql">SELECT lamp, wick FROM stores WHERE oil &gt; 0 ORDER BY lamp;</code></pre>
  <figure><img src="harbour.png" alt="The harbour at dawn"/><figcaption>The harbour at dawn</figcaption></figure>
  <p>He muttered a phrase learned as a boy<a href="#fn1" id="ref-fn1" epub:type="noteref">[1]</a> and turned away.</p>
  <p>Math: <span class="math">E = mc^2</span> and inline <code>x = 1</code> too.</p>
  <aside id="fn1" epub:type="footnote"><p><a href="#ref-fn1">[1]</a> A rhyme about storms and lanterns, learned at the knee.</p></aside>""",
    )
    chapter_two = _xhtml(
        "Chapter Two: The Quiet Sea",
        """\
  <h2 id="ch2">Chapter Two: The Quiet Sea</h2>
  <p>The sea was <strong>still</strong> for three days. Nobody spoke of the lantern, and the
  keeper slept in the chair by the door.<a href="#fn2" id="ref-fn2" epub:type="noteref">[1]</a></p>
  <blockquote><p>Silence is the loudest thing in a harbour town.</p></blockquote>
  <aside id="fn2" epub:type="footnote"><p><a href="#ref-fn2">[1]</a> An old proverb of the coast.</p></aside>""",
    )
    return [
        {"title": "Chapter One: The Harbour", "file_name": "chap_01.xhtml", "html": chapter_one},
        {"title": "Chapter Two: The Quiet Sea", "file_name": "chap_02.xhtml", "html": chapter_two},
    ]


def large_epub_chapters(target_chars: int, seed: int) -> tuple[list[dict[str, str]], int]:
    """Return deterministically generated large chapters and their char count."""
    rng = random.Random(seed)
    chapters: list[dict[str, str]] = []
    total = 0
    index = 0
    paragraphs_per_chapter = 12
    words_per_paragraph = 240
    while total < target_chars:
        index += 1
        body_parts = [f'  <h2 id="ch{index}">Chapter {index}: The Long Watch</h2>']
        for _ in range(paragraphs_per_chapter):
            words = [rng.choice(WORDS) for _ in range(words_per_paragraph)]
            words[0] = words[0].capitalize()
            sentence = " ".join(words) + "."
            total += len(sentence)
            body_parts.append(f"  <p>{sentence}</p>")
        html = _xhtml(f"Chapter {index}", "\n".join(body_parts))
        chapters.append({"title": f"Chapter {index}", "file_name": f"chap_{index:03d}.xhtml", "html": html})
        if index > 5000:  # hard safety valve against an unbounded loop
            break
    return chapters, total


def build_epub(
    chapters: list[dict[str, str]],
    out_path: Path,
    with_image: bool,
    nested_toc: bool = False,
) -> None:
    """Build a deterministic EPUB from *chapters* and write it to *out_path*."""
    assert epub is not None, "ebooklib is required to build EPUB fixtures"
    book = epub.EpubBook()
    book.set_identifier("urn:uuid:" + FIXED_UUID)
    book.set_title("The Lantern Keeper")
    book.set_language("en")
    book.add_author("Fixture Author")

    css = epub.EpubItem(
        uid="style_book",
        file_name="style/book.css",
        media_type="text/css",
        content=BOOK_CSS,
    )
    book.add_item(css)

    items = []
    for chapter in chapters:
        item = epub.EpubHtml(title=chapter["title"], file_name=chapter["file_name"], lang="en")
        item.content = chapter["html"]
        item.add_item(css)
        book.add_item(item)
        items.append(item)

    if with_image:
        image = epub.EpubItem(
            uid="img_harbour",
            file_name=IMAGE_NAME,
            media_type="image/png",
            content=IMAGE_PNG_BYTES,
        )
        book.add_item(image)

    book.add_item(epub.EpubNcx())
    book.add_item(epub.EpubNav())
    links = tuple(epub.Link(chapter["file_name"], chapter["title"], chapter["file_name"]) for chapter in chapters)
    if nested_toc and chapters:
        # A (section, children) pair produces an h1/h2-shaped NAV: a top-level
        # "book" entry with the chapters nested underneath.
        book.toc = ((epub.Section("The Lantern Keeper", href=chapters[0]["file_name"]), links),)
    else:
        book.toc = links
    book.spine = ["nav", *items]

    epub.write_epub(str(out_path), book, {"mtime": FIXED_MTIME})
    normalize_epub(out_path)


def normalize_epub(path: Path) -> None:
    """Rewrite *path* so its bytes are reproducible across runs."""
    with zipfile.ZipFile(path) as zin:
        entries = [(info.filename, info.compress_type, zin.read(info.filename)) for info in zin.infolist()]

    fixed_entries = []
    for name, compress_type, data in entries:
        if name.endswith((".opf", ".xhtml", ".html", ".ncx")):
            text = data.decode("utf-8")
            text = ISO_TS_RE.sub("2020-01-01T00:00:00Z", text)
            text = UUID_RE.sub(FIXED_UUID, text)
            data = text.encode("utf-8")
        fixed_entries.append((name, compress_type, data))

    with zipfile.ZipFile(path, "w") as zout:
        for name, compress_type, data in fixed_entries:
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = compress_type
            info.external_attr = 0o644 << 16
            zout.writestr(info, data)


# --------------------------------------------------------------------------- #
# PDF fixture
# --------------------------------------------------------------------------- #


def build_pdf(out_path: Path) -> None:
    """Build a small deterministic PDF with headings, paragraphs and a table."""
    assert fitz is not None, "pymupdf is required to build the PDF fixture"
    doc = fitz.open()

    title_page = doc.new_page()
    title_page.insert_text((72, 120), "The Lantern Keeper", fontsize=26, fontname="Times-Bold")
    title_page.insert_text((72, 150), "Fixture Author", fontsize=14, fontname="Times-Roman")
    title_page.insert_text((72, 700), "LocalLLMTranslator fixture", fontsize=9, fontname="Helvetica")

    page = doc.new_page()
    page.insert_text((72, 72), "Chapter One: The Harbour", fontsize=18, fontname="Helvetica-Bold")
    paragraphs = [
        "The harbour was quiet that morning. Nobody expected the ship, and the keeper "
        "wrote nothing in his log. He had seen the light of the northern tower fade "
        "twice, then stop.",
        "The wind rose from the west and carried the smell of salt and old rope across "
        "the pier, where the gulls argued over scraps.",
        "He muttered a phrase learned as a boy and turned away from the water.",
    ]
    y = 104.0
    for paragraph in paragraphs:
        rect = fitz.Rect(72, y, 523, y + 90)
        page.insert_textbox(rect, paragraph, fontsize=11, fontname="Helvetica")
        y += 96

    table_page = doc.new_page()
    table_page.insert_text((72, 72), "Stores", fontsize=16, fontname="Helvetica-Bold")
    rows = [
        ("Element", "Number", "Notes"),
        ("Lamps", "4", "glass"),
        ("Wicks", "12", "cotton"),
        ("Oil", "3", "barrel"),
    ]
    x0, y0 = 72.0, 100.0
    row_height = 22.0
    col_widths = (140.0, 90.0, 160.0)
    for row_index, row in enumerate(rows):
        y_top = y0 + row_index * row_height
        x = x0
        for col_index, cell in enumerate(row):
            table_page.draw_rect(fitz.Rect(x, y_top, x + col_widths[col_index], y_top + row_height), color=(0, 0, 0))
            table_page.insert_text((x + 4, y_top + 15), cell, fontsize=10, fontname="Helvetica")
            x += col_widths[col_index]

    doc.set_metadata(
        {
            "title": "The Lantern Keeper",
            "author": "Fixture Author",
            "creationDate": FIXED_PDF_DATE,
            "modDate": FIXED_PDF_DATE,
        }
    )
    doc.save(str(out_path), deflate=True, garbage=4)
    doc.close()
    normalize_pdf(out_path)


def normalize_pdf(path: Path) -> None:
    """Pin the trailer ``/ID`` of *path* so the PDF bytes are reproducible.

    MuPDF writes a fresh random ``/ID`` on every ``save()`` and may render each
    element either as ``<hex>`` or as a literal ``(...)`` string.  We locate the
    ``/ID`` array in the trailer, scan to its matching ``]`` (skipping over PDF
    literal strings, which may contain escapes and brackets) and replace the
    whole array with a fixed value.  The trailer follows the cross-reference
    table, so changing its length leaves every offset in the file valid.
    """
    data = path.read_bytes()
    idx = data.rfind(b"/ID")
    if idx == -1:
        return
    open_idx = data.find(b"[", idx)
    if open_idx == -1:
        return

    i = open_idx + 1
    length = len(data)
    paren_depth = 0
    close_idx = -1
    while i < length:
        byte = data[i : i + 1]
        if paren_depth:
            if byte == b"\\":
                i += 2
                continue
            if byte == b"(":
                paren_depth += 1
            elif byte == b")":
                paren_depth -= 1
            i += 1
            continue
        if byte == b"(":
            paren_depth = 1
        elif byte == b"]":
            close_idx = i
            break
        i += 1

    if close_idx == -1:
        return
    fixed = FIXED_PDF_ID.encode("ascii")
    replacement = b"/ID[<" + fixed + b"><" + fixed + b">]"
    path.write_bytes(data[:idx] + replacement + data[close_idx + 1 :])


# --------------------------------------------------------------------------- #
# Driver
# --------------------------------------------------------------------------- #


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(65536), b""):
            digest.update(block)
    return digest.hexdigest()


def require(module: object, name: str) -> None:
    if module is None:
        sys.stderr.write(
            f"error: {name} is required to build this fixture; install it in the sidecar uv project.\n"
        )
        raise SystemExit(3)


def generate(
    out_dir: Path,
    *,
    small: bool = True,
    large: bool = True,
    seed: int = 20200101,
    large_chars: int = 1_000_000,
    quiet: bool = False,
) -> dict[str, object]:
    """Generate the requested fixtures and return a manifest dict."""
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest: dict[str, object] = {"seed": seed, "large_target_chars": large_chars, "files": {}}
    files: dict[str, dict[str, object]] = manifest["files"]  # type: ignore[assignment]

    if small:
        require(ebooklib, "ebooklib")
        require(fitz, "pymupdf")

        (out_dir / IMAGE_NAME).write_bytes(IMAGE_PNG_BYTES)
        files[IMAGE_NAME] = {"bytes": len(IMAGE_PNG_BYTES), "sha256": sha256(out_dir / IMAGE_NAME)}

        md_path = out_dir / "content.md"
        md_path.write_text(markdown_small(), encoding="utf-8")
        files["content.md"] = {"bytes": md_path.stat().st_size, "sha256": sha256(md_path)}

        epub_path = out_dir / "content.epub"
        build_epub(small_epub_chapters(), epub_path, with_image=True, nested_toc=True)
        files["content.epub"] = {
            "bytes": epub_path.stat().st_size,
            "sha256": sha256(epub_path),
            "chapters": len(small_epub_chapters()),
        }

        pdf_path = out_dir / "content.pdf"
        build_pdf(pdf_path)
        files["content.pdf"] = {"bytes": pdf_path.stat().st_size, "sha256": sha256(pdf_path)}

    if large:
        require(ebooklib, "ebooklib")
        chapters, char_count = large_epub_chapters(large_chars, seed)
        large_path = out_dir / "large.epub"
        build_epub(chapters, large_path, with_image=False)
        files["large.epub"] = {
            "bytes": large_path.stat().st_size,
            "sha256": sha256(large_path),
            "chapters": len(chapters),
            "prose_chars": char_count,
        }
        manifest["large_epub_chars"] = char_count

    manifest_path = out_dir / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    if not quiet:
        for name, info in files.items():
            extra = ""
            if "prose_chars" in info:
                extra = f" prose_chars={info['prose_chars']}"
            elif "chapters" in info:
                extra = f" chapters={info['chapters']}"
            sys.stdout.write(f"[make_fixtures] {out_dir / name}  bytes={info['bytes']}{extra}\n")
        if large:
            sys.stdout.write(f"[make_fixtures] large fixture prose characters: {manifest.get('large_epub_chars')}\n")

    return manifest


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="make_fixtures",
        description="Generate reproducible EPUB/Markdown/PDF test fixtures.",
    )
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT_DIR, help="target directory")
    parser.add_argument("--small-only", action="store_true", help="generate only the small fixtures")
    parser.add_argument("--large-only", action="store_true", help="generate only the large EPUB")
    parser.add_argument("--seed", type=int, default=20200101, help="seed for the large fixture prose")
    parser.add_argument("--large-chars", type=int, default=1_000_000, help="target prose characters for large.epub")
    parser.add_argument("--quiet", action="store_true", help="suppress the summary output")
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    small = not args.large_only
    large = not args.small_only
    if args.small_only and args.large_only:
        sys.stderr.write("error: --small-only and --large-only are mutually exclusive\n")
        return 2
    generate(
        args.out_dir,
        small=small,
        large=large,
        seed=args.seed,
        large_chars=args.large_chars,
        quiet=args.quiet,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
