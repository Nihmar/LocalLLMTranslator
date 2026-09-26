"""Format detection and ingestion for the sidecar.

The public surface is :func:`detect_format` and :func:`extract`, matching the ``detect_format``
and ``ingest`` JSON-RPC methods in the frozen contract. Everything a backend produces is
written to ``work_dir`` as a single UTF-8/LF Markdown file; the chapter list is derived by
parsing that very file, so it can never disagree with a later ``parse_document`` call.
"""

from __future__ import annotations

from typing import Any

from llmtranslator_sidecar.errors import MissingDependencyError

from .base import (
    ExtractionError,
    Extractor,
    atomic_write_text,
    chapters_from_markdown,
    detect_format,
    normalise_markdown,
)
from .epub import EpubExtractor
from .markdown_src import MarkdownSourceExtractor
from .pdf_marker import MarkerPdfExtractor
from .pdf_pymupdf import PymupdfExtractor

__all__ = [
    "ExtractionError",
    "MissingDependencyError",
    "detect_format",
    "extract",
]


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
