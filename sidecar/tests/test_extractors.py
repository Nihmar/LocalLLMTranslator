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
import os
import re
import shutil
import subprocess
import sys
import types
import zipfile
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
from llmtranslator_sidecar.placeholders import reinject, substitute
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


#: A minimal, valid EPUB 3 package assembled by hand, so the extractor can be exercised on
#: arbitrary body markup without depending on the untyped ebooklib writer in this suite.
_EPUB_CONTAINER = """<?xml version="1.0" encoding="utf-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>
"""

_EPUB_OPF = """<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:00000000-0000-0000-0000-000000000000</dc:identifier>
    <dc:title>Synthetic</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="chap" href="chap.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="chap"/>
  </spine>
</package>
"""


def _write_epub(path: Path, body: str) -> None:
    """Write ``path`` as an EPUB whose single chapter body is ``body``."""
    xhtml = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>x</title></head>'
        f"<body>{body}</body></html>\n"
    )
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        archive.writestr("META-INF/container.xml", _EPUB_CONTAINER)
        archive.writestr("OEBPS/content.opf", _EPUB_OPF)
        archive.writestr("OEBPS/chap.xhtml", xhtml)


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
    # The caption is its own paragraph, blank-line separated from the delimiter row, so a
    # GFM parser does not fold it into the table and lose the table itself.
    assert "Stores\n\n| Element | Number | Notes |" in markdown
    assert "```sql" in markdown
    assert "![The harbour at dawn](assets/harbour.png)" in markdown
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

    # An EPUB carries its image inside the archive, so extraction adds the assets
    # directory next to the document; markdown and PDF sources produce no media.
    expected = ["assets", "document.md"] if result["assets"] else ["document.md"]
    assert sorted(entry.name for entry in tmp_path.iterdir()) == expected
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


# -- C1: bare text nodes inside a container must not be dropped -------------------------


def test_epub_keeps_bare_text_nodes_and_warns(tmp_path: Path) -> None:
    source = tmp_path / "bare.epub"
    # ebooklib normalises the XHTML while reading and drops any text before the first
    # element, so the bare text sits where it actually reaches the extractor: between and
    # after the block children of <body>.
    _write_epub(
        source,
        "<p>First paragraph.</p>Between the blocks.<p>Second paragraph.</p>Trailing text.",
    )

    result = extract(str(source), str(tmp_path / "work"))
    markdown = produced(result)

    assert "First paragraph." in markdown
    assert "Between the blocks." in markdown
    assert "Second paragraph." in markdown
    assert "Trailing text." in markdown
    assert any("flatten" in warning for warning in result["warnings"])
    assert serialize(split_blocks(markdown)) == markdown


def test_epub_keeps_bare_text_directly_inside_a_div(tmp_path: Path) -> None:
    source = tmp_path / "div.epub"
    _write_epub(source, "<div>Inside a div.</div>")

    result = extract(str(source), str(tmp_path / "work"))
    assert "Inside a div." in produced(result)
    assert any("flatten" in warning for warning in result["warnings"])


def test_epub_drops_whitespace_only_text_without_a_warning(tmp_path: Path) -> None:
    source = tmp_path / "spaces.epub"
    _write_epub(source, "\n  <p>Only a paragraph.</p>\n\t")

    result = extract(str(source), str(tmp_path / "work"))
    assert "Only a paragraph." in produced(result)
    assert result["warnings"] == []


# -- m3: alt text / href characters that would break Markdown are escaped ---------------


def test_epub_escapes_markdown_breakers_in_images_and_links(tmp_path: Path) -> None:
    body = (
        '<p><img src="maps/a)1.png" alt="harbour] map"/></p>'
        '<p><a href="https://example.org/a)b">the pier</a></p>'
    )
    source = tmp_path / "escaping.epub"
    _write_epub(source, body)

    markdown = produced(extract(str(source), str(tmp_path / "work")))
    assert "harbour\\] map" in markdown
    assert "maps/a\\)1.png" in markdown
    assert "https://example.org/a\\)b" in markdown
    assert serialize(split_blocks(markdown)) == markdown


# -- M3: the NDJSON transport requires that extract never writes to fd 1 ----------------


@pytest.mark.parametrize("name", sorted(FORMATS))
def test_extract_writes_nothing_to_stdout(fixtures: Path, tmp_path: Path, name: str) -> None:
    captured = tmp_path / "stdout.bin"
    saved = os.dup(1)
    sink = captured.open("wb")
    try:
        sys.stdout.flush()
        os.dup2(sink.fileno(), 1)
        extract(str(fixtures / name), str(tmp_path / "work"))
        sys.stdout.flush()
    finally:
        sys.stdout.flush()
        os.dup2(saved, 1)
        os.close(saved)
        sink.close()

    assert captured.read_bytes() == b"", "extract() must not write to stdout (fd 1)"


# -- m2: a version-mismatched marker is a missing dependency, not a raw ImportError ------


def test_marker_import_failure_maps_to_missing_dependency(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    from llmtranslator_sidecar.extractors import pdf_marker

    def boom(name: str) -> object:
        raise ImportError(name)

    monkeypatch.setattr(pdf_marker, "_marker_available", lambda: True)
    monkeypatch.setattr(pdf_marker, "importlib", types.SimpleNamespace(import_module=boom))

    with pytest.raises(MissingDependencyError) as error:
        pdf_marker.MarkerPdfExtractor().extract("whatever.pdf", str(tmp_path))
    assert error.value.code == 1003
    assert error.value.backend == "marker"


# -- m4: a UTF-8 BOM must not hide the YAML front matter --------------------------------


def test_markdown_front_matter_survives_a_bom(tmp_path: Path) -> None:
    source = tmp_path / "bom.md"
    source.write_text("\ufeff---\ntitle: BOM Book\nauthor: BOM Author\n---\n\nBody.\n", "utf-8")

    result = extract(str(source), str(tmp_path / "work"))
    assert result["metadata"]["title"] == "BOM Book"
    markdown = produced(result)
    assert not markdown.startswith("\ufeff")
    assert serialize(split_blocks(markdown)) == markdown


# -- m1: extractor and pandoc share a single MissingDependencyError class ----------------


def test_missing_dependency_error_is_shared_across_modules() -> None:
    from llmtranslator_sidecar import errors, pandoc

    assert MissingDependencyError is pandoc.MissingDependencyError
    assert issubclass(MissingDependencyError, errors.SidecarError)


# -- M2: embedded media is materialised so image hrefs never dangle ---------------------

#: Distinct byte payloads so a test can tell which item landed in which asset file.
_PNG_ONE = b"\x89PNG\r\n\x1a\n" + b"asset-one"
_PNG_TWO = b"\x89PNG\r\n\x1a\n" + b"asset-two"


def _epub_xhtml(body: str) -> str:
    return (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<html xmlns="http://www.w3.org/1999/xhtml"><head><title>x</title></head>'
        f"<body>{body}</body></html>\n"
    )


def _write_epub_tree(
    path: Path,
    *,
    chapter_href: str,
    body: str,
    images: dict[str, bytes],
) -> None:
    """Write an EPUB whose one chapter and images live at chosen archive paths.

    ``chapter_href`` and the keys of ``images`` are manifest hrefs relative to the OPF
    (``OEBPS/``), which is what the extractor must resolve a chapter's ``src`` against.
    The flat :func:`_write_epub` cannot express a chapter nested in a directory, so the
    relative ``../images/x.png``-style references need this richer builder.
    """
    manifest = [
        f'    <item id="chap" href="{chapter_href}" media-type="application/xhtml+xml"/>',
        *(
            f'    <item id="img{index}" href="{href}" media-type="image/png"/>'
            for index, href in enumerate(images)
        ),
    ]
    opf = (
        '<?xml version="1.0" encoding="utf-8"?>\n'
        '<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">\n'
        '  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">\n'
        '    <dc:identifier id="bookid">urn:uuid:00000000-0000-0000-0000-000000000000'
        "</dc:identifier>\n"
        "    <dc:title>Synthetic</dc:title>\n"
        "    <dc:language>en</dc:language>\n"
        "  </metadata>\n"
        "  <manifest>\n" + "\n".join(manifest) + "\n  </manifest>\n"
        '  <spine>\n    <itemref idref="chap"/>\n  </spine>\n'
        "</package>\n"
    )
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        archive.writestr("META-INF/container.xml", _EPUB_CONTAINER)
        archive.writestr("OEBPS/content.opf", opf)
        archive.writestr(f"OEBPS/{chapter_href}", _epub_xhtml(body))
        for href, data in images.items():
            archive.writestr(f"OEBPS/{href}", data)


def test_epub_materialises_image_bytes_and_rewrites_the_href(
    fixtures: Path,
    tmp_path: Path,
) -> None:
    result = extract(str(fixtures / "content.epub"), str(tmp_path))

    assert result["assets"] == ["assets/harbour.png"]
    assert result["assets_dir"] == str(tmp_path / "assets")

    written = tmp_path / "assets" / "harbour.png"
    assert written.is_file()
    assert written.read_bytes() == (fixtures / "harbour.png").read_bytes()

    markdown = produced(result)
    assert "![The harbour at dawn](assets/harbour.png)" in markdown
    assert "(harbour.png)" not in markdown, "the archive-relative href must be gone"
    # Rewriting the href does not disturb the IR round-trip.
    assert serialize(split_blocks(markdown)) == markdown


def test_epub_resolves_image_paths_relative_to_the_chapter(tmp_path: Path) -> None:
    source = tmp_path / "nested.epub"
    _write_epub_tree(
        source,
        chapter_href="text/chap.xhtml",
        body='<p><img src="../images/pic.png" alt="pic"/></p>',
        images={"images/pic.png": _PNG_ONE},
    )

    result = extract(str(source), str(tmp_path / "work"))
    assert result["assets"] == ["assets/pic.png"]
    assert (tmp_path / "work" / "assets" / "pic.png").read_bytes() == _PNG_ONE
    assert "![pic](assets/pic.png)" in produced(result)
    assert result["warnings"] == []


def test_epub_disambiguates_name_collisions_deterministically(tmp_path: Path) -> None:
    source = tmp_path / "collision.epub"
    _write_epub_tree(
        source,
        chapter_href="chap.xhtml",
        body=(
            '<p><img src="images/harbour.png" alt="A"/></p>'
            '<p><img src="other/harbour.png" alt="B"/></p>'
        ),
        images={"images/harbour.png": _PNG_ONE, "other/harbour.png": _PNG_TWO},
    )

    result = extract(str(source), str(tmp_path / "work"))
    assert result["assets"] == ["assets/harbour.png", "assets/harbour-2.png"]
    assets = tmp_path / "work" / "assets"
    assert (assets / "harbour.png").read_bytes() == _PNG_ONE
    assert (assets / "harbour-2.png").read_bytes() == _PNG_TWO

    markdown = produced(result)
    assert "![A](assets/harbour.png)" in markdown
    assert "![B](assets/harbour-2.png)" in markdown


def test_epub_unresolvable_image_keeps_its_href_and_warns(tmp_path: Path) -> None:
    source = tmp_path / "missing.epub"
    _write_epub(source, '<p><img src="missing.png" alt="gone"/></p><p>Body.</p>')

    result = extract(str(source), str(tmp_path / "work"))
    assert "![gone](missing.png)" in produced(result)
    assert result["assets"] == []
    assert result["assets_dir"] is None
    assert any("missing.png" in warning for warning in result["warnings"]), result["warnings"]
    assert not (tmp_path / "work" / "assets").exists()


def test_epub_re_extraction_leaves_no_stale_asset(fixtures: Path, tmp_path: Path) -> None:
    work_dir = tmp_path / "work"
    extract(str(fixtures / "content.epub"), str(work_dir))

    stale = work_dir / "assets" / "stale.png"
    stale.write_bytes(b"stale")
    assert stale.is_file()

    result = extract(str(fixtures / "content.epub"), str(work_dir))
    assert result["assets"] == ["assets/harbour.png"]
    assert not stale.exists(), "a re-extraction must clear stale media"
    assert sorted(entry.name for entry in (work_dir / "assets").iterdir()) == ["harbour.png"]


def test_epub_two_extractions_produce_byte_identical_output(
    fixtures: Path,
    tmp_path: Path,
) -> None:
    first = extract(str(fixtures / "content.epub"), str(tmp_path / "a"))
    second = extract(str(fixtures / "content.epub"), str(tmp_path / "b"))

    assert produced(first) == produced(second)
    assert first["assets"] == second["assets"]
    for href in first["assets"]:
        left = (tmp_path / "a" / href).read_bytes()
        right = (tmp_path / "b" / href).read_bytes()
        assert left == right


# -- M2: stable ids -- the same source extracted twice yields identical block identity ---


@pytest.mark.parametrize("name", sorted(FORMATS))
def test_two_extractions_yield_identical_block_identities(
    fixtures: Path,
    tmp_path: Path,
    name: str,
) -> None:
    first = produced(extract(str(fixtures / name), str(tmp_path / "a")))
    second = produced(extract(str(fixtures / name), str(tmp_path / "b")))

    def identity(markdown: str) -> list[tuple[str, int, str, str]]:
        return [(b.id, b.order, b.kind, b.content_hash) for b in split_blocks(markdown)]

    assert identity(first) == identity(second)
    assert identity(first), "the fixture must produce at least one block"


# -- M2: footnotes are a reference plus a definition that survive the model ---------------


def test_epub_footnotes_become_references_and_definitions(fixtures: Path, tmp_path: Path) -> None:
    blocks = split_blocks(produced(extract(str(fixtures / "content.epub"), str(tmp_path))))

    definitions = [b for b in blocks if b.kind == "footnote_def"]
    assert [b.attrs["ref"] for b in definitions] == ["^1", "^2"]
    # Only the note body is prose; the ``[^n]`` marker is hidden from the model by the
    # placeholder layer, so the definition itself stays translatable.
    assert all(b.translatable is True for b in definitions)

    referencing = [
        b
        for b in blocks
        if b.kind != "footnote_def" and re.search(r"\[\^[^\]]+\]", b.source_md) is not None
    ]
    assert referencing, "a paragraph must carry the [^n] reference"


def test_footnote_reference_and_definition_survive_the_model(
    fixtures: Path,
    tmp_path: Path,
) -> None:
    blocks = split_blocks(produced(extract(str(fixtures / "content.epub"), str(tmp_path))))
    definition = next(b for b in blocks if b.kind == "footnote_def" and b.attrs["ref"] == "^1")
    reference = next(b for b in blocks if b.kind != "footnote_def" and "[^1]" in b.source_md)
    pair = f"{reference.source_md}\n\n{definition.source_md}\n"

    llm_text, placeholders = substitute(pair)
    translated = reinject(llm_text.replace("muttered", "mormorò"), placeholders)
    assert translated.ok, (translated.missing, translated.duplicated)

    rebuilt = split_blocks(translated.text)
    refs = [b for b in rebuilt if b.kind != "footnote_def" and "[^1]" in b.source_md]
    defs = [b for b in rebuilt if b.kind == "footnote_def"]
    assert len(refs) == 1
    assert len(defs) == 1
    assert defs[0].attrs["ref"] == "^1"
    reference_number = re.search(r"\[\^([^\]]+)\]", refs[0].source_md)
    assert reference_number is not None
    assert reference_number.group(1) == defs[0].attrs["ref"].lstrip("^")


# -- M2: a table is a single block that keeps its alignment ------------------------------


def test_epub_table_is_a_single_block_with_alignments(fixtures: Path, tmp_path: Path) -> None:
    blocks = split_blocks(produced(extract(str(fixtures / "content.epub"), str(tmp_path))))

    tables = [b for b in blocks if b.kind == "table"]
    assert len(tables) == 1
    assert tables[0].attrs["align"] == ["l", "r", "c"]
    assert tables[0].translatable is True
