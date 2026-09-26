"""Foundation of the extractor layer: error types, result shape and shared helpers.

Every extractor is a pure ``input -> output`` function with no database, no network and
no state that survives a call, as the data plane requires. The RPC layer turns the two
error classes below into JSON-RPC errors by their name, so their identifiers are part of
the frozen surface: :class:`ExtractionError` maps to code ``1001`` and
:class:`MissingDependencyError` to ``1003``.
"""

from __future__ import annotations

import contextlib
import os
import tempfile
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Protocol, runtime_checkable

from llmtranslator_sidecar.parse import parse_markdown

#: JSON-RPC error code for a failed ingestion (``AGENTS.md`` IPC contract).
INGESTION_FAILED = 1001

#: JSON-RPC error code for an optional backend that is not installed.
MISSING_DEPENDENCY = 1003

#: Document formats the sidecar understands, and the backends each one can use.
BACKENDS: dict[str, list[str]] = {
    "epub": ["ebooklib"],
    "pdf": ["pymupdf4llm", "marker"],
    "markdown": ["source"],
}

_PDF_MAGIC = b"%PDF-"
_ZIP_MAGIC = b"PK\x03\x04"
_EPUB_MIMETYPE = b"application/epub+zip"
_MAGIC_HEAD = 8

_EXTENSIONS: dict[str, str] = {
    ".epub": "epub",
    ".pdf": "pdf",
    ".md": "markdown",
    ".markdown": "markdown",
    ".mdown": "markdown",
    ".mkd": "markdown",
}


class ExtractorError(Exception):
    """Base class for ingestion failures the RPC layer reports as JSON-RPC errors."""

    code: int = -32603

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


class ExtractionError(ExtractorError):
    """Ingestion failed; the RPC layer maps this to code ``1001``."""

    code = INGESTION_FAILED


class MissingDependencyError(ExtractorError):
    """An optional backend is not installed; maps to code ``1003``."""

    code = MISSING_DEPENDENCY

    def __init__(self, message: str, *, backend: str | None = None) -> None:
        super().__init__(message)
        self.backend = backend


@dataclass(slots=True)
class ExtractResult:
    """What a backend produces before the dispatcher writes it to ``work_dir``."""

    markdown: str
    metadata: dict[str, Any] = field(default_factory=dict[str, Any])
    warnings: list[str] = field(default_factory=list[str])
    filename: str = "document.md"


@runtime_checkable
class Extractor(Protocol):
    """One format backend.

    ``format`` names the source family (``epub``/``pdf``/``markdown``) so a caller can
    check which extractor a path dispatched to.
    """

    format: str

    def extract(self, path: str) -> ExtractResult:
        """Turn one source file into canonical Markdown plus metadata and warnings."""
        ...


def normalise_markdown(text: str) -> str:
    """Canonicalise line endings and guarantee a single trailing newline.

    Only line endings are rewritten; the content itself is left alone, because the
    round-trip invariant ``serialize(split_blocks(md)) == md`` is what keeps untranslated
    blocks byte-identical to the source.
    """
    text = text.replace("\r\n", "\n").replace("\r", "\n").removeprefix("\ufeff")
    if text and not text.endswith("\n"):
        text += "\n"
    return text


def atomic_write_text(work_dir: str, filename: str, text: str) -> str:
    """Write ``text`` as UTF-8/LF into ``work_dir`` and return the final path.

    The bytes land in a sibling temp file first and are moved into place with
    ``os.replace``, so a crash mid-write can never leave a half-written document behind
    and a concurrent reader either sees the old file or the complete new one.
    """
    directory = Path(work_dir)
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / filename
    descriptor, temporary = tempfile.mkstemp(dir=directory, prefix=f".{filename}.", suffix=".tmp")
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8", newline="\n") as stream:
            stream.write(text)
        Path(temporary).replace(target)
    except BaseException:
        with contextlib.suppress(OSError):
            Path(temporary).unlink()
        raise
    return str(target)


def chapters_from_markdown(markdown: str) -> list[dict[str, Any]]:
    """Chapter list for ``ingest``, derived from the produced Markdown itself.

    Parsing the written file (rather than the extractor's own idea of the structure)
    guarantees ``ingest`` and the later ``parse_document`` call agree on chapter ids,
    titles and levels.
    """
    _blocks, chapters = parse_markdown(markdown)
    return [
        {"title": chapter.title, "level": chapter.level, "order": chapter.order}
        for chapter in chapters
    ]


def _is_epub(source: Path) -> bool:
    try:
        with zipfile.ZipFile(source) as archive:
            if "mimetype" not in archive.namelist():
                return False
            return archive.read("mimetype").strip() == _EPUB_MIMETYPE
    except (OSError, zipfile.BadZipFile, KeyError):
        return False


def _detect_by_magic(source: Path) -> str | None:
    try:
        with source.open("rb") as stream:
            head = stream.read(_MAGIC_HEAD)
    except OSError as exc:
        message = f"cannot read {source}: {exc}"
        raise ExtractionError(message) from exc
    if head.startswith(_PDF_MAGIC):
        return "pdf"
    if head.startswith(_ZIP_MAGIC) and _is_epub(source):
        return "epub"
    return None


def detect_format(path: str) -> dict[str, Any]:
    """Classify a source file by magic bytes first, then by extension.

    Magic bytes win because a renamed or extension-less file is still a PDF or an EPUB;
    the extension is the tie-breaker for formats with no reliable signature, notably
    plain Markdown.
    """
    source = Path(path)
    if not source.is_file():
        message = f"source file does not exist: {source}"
        raise ExtractionError(message)

    detected = _detect_by_magic(source) or _EXTENSIONS.get(source.suffix.lower())
    if detected is None:
        message = f"unrecognised document format: {source.name}"
        raise ExtractionError(message)
    return {"format": detected, "backends": list(BACKENDS[detected])}
