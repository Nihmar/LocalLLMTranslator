"""Format detection and ingestion for the sidecar.

The public surface is :func:`detect_format` and :func:`extract`, matching the ``detect_format``
and ``ingest`` JSON-RPC methods in the frozen contract. Everything a backend produces is
written to ``work_dir`` as a single UTF-8/LF Markdown file; the chapter list is derived by
parsing that very file, so it can never disagree with a later ``parse_document`` call.
"""

from __future__ import annotations

from typing import Any

from llmtranslator_sidecar.errors import MissingDependencyError
from llmtranslator_sidecar.langdetect import guess_language

from .base import (
    ExtractionError,
    Extractor,
    atomic_write_text,
    chapters_from_markdown,
    detect_format,
    normalise_markdown,
)
from .epub import EpubExtractor
from .epub import peek as peek_epub
from .markdown_src import MarkdownSourceExtractor
from .markdown_src import peek as peek_markdown
from .pdf_marker import MarkerPdfExtractor
from .pdf_pymupdf import PymupdfExtractor
from .pdf_pymupdf import peek as peek_pdf

__all__ = [
    "ExtractionError",
    "MissingDependencyError",
    "detect_format",
    "extract",
    "inspect",
]

#: What ``inspect`` reports about a document, all optional.
_HINT_KEYS = ("title", "author", "language")
#: Enough text for the function-word count, read without converting the whole book.
_SAMPLE_CHARS = 30_000


def inspect(path: str) -> dict[str, Any]:
    """``detect_format`` plus what the file says about itself: title, author, language.

    The language comes from the text when it is clear (``langdetect``): declared metadata is
    often wrong, a French novel tagged ``en`` included. The hints only pre-fill the "new book"
    form, so a file that cannot be peeked into still inspects, with none: the real error
    surfaces at ingestion, where it belongs.
    """
    detected = detect_format(path)
    raw: dict[str, Any] = {}
    sample = ""
    peekers = {"epub": peek_epub, "markdown": peek_markdown, "pdf": peek_pdf}
    try:
        raw, sample = peekers[detected["format"]](path, _SAMPLE_CHARS)
    except Exception:  # noqa: BLE001 - a hint is optional; ingestion reports the real failure
        raw, sample = {}, ""
    if "language" not in raw and "lang" in raw:
        raw["language"] = raw["lang"]
    hints: dict[str, str] = {}
    for key in _HINT_KEYS:
        value = raw.get(key)
        if isinstance(value, str) and value.strip():
            hints[key] = value.strip()
    if "language" in hints:
        # `fr-FR`, `fr_FR` -> `fr`: the project stores the primary language subtag.
        hints["language"] = hints["language"].replace("_", "-").split("-")[0].lower()
    guessed = guess_language(sample)
    if guessed is not None:
        hints["language"] = guessed
    return {**detected, "metadata": hints}


def _pdf_extractor(backend: str | None) -> PymupdfExtractor | MarkerPdfExtractor:
    if backend is None or backend in {"pymupdf4llm", "pymupdf"}:
        return PymupdfExtractor()
    if backend == "marker":
        return MarkerPdfExtractor()
    message = f"unknown PDF backend: {backend!r}"
    raise ExtractionError(message)


def _select(path: str, pdf_backend: str | None) -> Extractor:
    detected = detect_format(path)["format"]
    if detected == "markdown":
        return MarkdownSourceExtractor()
    if detected == "epub":
        return EpubExtractor()
    return _pdf_extractor(pdf_backend)


def extract(path: str, work_dir: str, pdf_backend: str | None = None) -> dict[str, Any]:
    """Ingest ``path`` into ``work_dir`` and describe the produced document.

    Returns the path of the one Markdown file written, its metadata, the chapter skeleton
    and any warnings raised while degrading the source, plus the media it extracted:
    ``assets`` are the hrefs as they appear in the Markdown (relative to ``document.md``)
    and ``assets_dir`` is the absolute directory that holds them (``None`` when the source
    carried no media). Extraction is a pure function of the input file: repeated calls
    produce byte-identical output.
    """
    extractor = _select(path, pdf_backend)
    result = extractor.extract(path, work_dir)

    markdown = normalise_markdown(result.markdown)
    markdown_path = atomic_write_text(work_dir, result.filename, markdown)

    return {
        "markdown_path": markdown_path,
        "metadata": result.metadata,
        "chapters": chapters_from_markdown(markdown),
        "warnings": result.warnings,
        "assets_dir": result.assets_dir,
        "assets": result.assets,
    }
