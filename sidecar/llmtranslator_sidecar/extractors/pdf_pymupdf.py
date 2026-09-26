# pyright: reportUnknownMemberType=false, reportUnknownVariableType=false, reportUnknownArgumentType=false
"""PDF ingestion via ``pymupdf4llm``, with metadata and text-layer checks from PyMuPDF.

``pymupdf4llm`` is the default PDF backend because it is a normal wheel with no model
downloads; the richer ``marker`` backend lives in :mod:`pdf_marker` and stays optional.

The library writes its progress and layout notes to the process stdout via
``pymupdf.message``. That is fatal here: the sidecar speaks newline-delimited JSON-RPC on
stdout, so any stray line corrupts the protocol. The call is therefore wrapped so that both
``print`` and PyMuPDF's own message sink are diverted to a throwaway buffer and the previous
sink is restored afterwards.

The unknown-type checks are relaxed file-wide because PyMuPDF exposes much of its API from
the C extension, where pyright has no types to work with; every value below is validated at
run time instead.
"""

from __future__ import annotations

import contextlib
import io
from pathlib import Path
from typing import Any

import pymupdf

from .base import ExtractionError, ExtractResult

#: Metadata keys lifted from the PDF document information dictionary, in output order.
_METADATA_KEYS: tuple[tuple[str, str], ...] = (
    ("title", "title"),
    ("author", "author"),
    ("subject", "subject"),
    ("keywords", "keywords"),
    ("creator", "creator"),
    ("producer", "producer"),
    ("creationDate", "creation_date"),
    ("modDate", "mod_date"),
)


def _open(path: str) -> pymupdf.Document:
    try:
        return pymupdf.open(path)
    except (RuntimeError, OSError) as exc:
        message = f"cannot open PDF {Path(path).name}: {exc}"
        raise ExtractionError(message) from exc


def pdf_metadata(path: str) -> dict[str, Any]:
    """Read the document information dictionary through PyMuPDF.

    Shared with the optional marker backend so both PDF paths report the same keys.
    """
    document = _open(path)
    try:
        raw = document.metadata or {}
        metadata: dict[str, Any] = {}
        for source_key, output_key in _METADATA_KEYS:
            value = raw.get(source_key)
            if value:
                metadata[output_key] = str(value)
        metadata["page_count"] = int(document.page_count)
        return metadata
    finally:
        document.close()


def _has_text_layer(path: str) -> bool:
    """True when at least one page carries selectable text."""
    document = _open(path)
    try:
        for page in document:
            text = page.get_text()
            if isinstance(text, str) and text.strip():
                return True
        return False
    finally:
        document.close()


def _to_markdown(path: str) -> str:
    """Render the PDF, keeping PyMuPDF's chatter away from the JSON-RPC stdout."""
    sink = io.StringIO()
    # The sink is a private global; getattr keeps it off the public surface and there is no
    # other supported way to read the current destination so it can be restored afterwards.
    previous = getattr(pymupdf, "_g_out_message")  # noqa: B009
    pymupdf.set_messages(stream=sink)
    try:
        with contextlib.redirect_stdout(sink):
            # Imported lazily, and inside the stdout guard: pymupdf4llm drags in
            # onnxruntime, which is heavy and is only needed when a PDF is rendered, so
            # markdown/EPUB ingestion never pays for it — and any import-time chatter ends
            # up in the throwaway sink instead of the NDJSON wire.
            import pymupdf4llm  # noqa: PLC0415

            return str(pymupdf4llm.to_markdown(path))
    finally:
        pymupdf.set_messages(stream=previous)


class PymupdfExtractor:
    """Default PDF backend."""

    format = "pdf"

    def extract(self, path: str) -> ExtractResult:
        warnings: list[str] = []
        metadata = pdf_metadata(path)

        if not _has_text_layer(path):
            warnings.append("PDF has no text layer; extracted text may be incomplete")

        try:
            markdown = _to_markdown(path)
        except Exception as exc:
            message = f"pymupdf4llm failed to extract {Path(path).name}: {exc}"
            raise ExtractionError(message) from exc
        if not markdown.strip():
            warnings.append("PDF produced no Markdown content")

        return ExtractResult(markdown=markdown, metadata=metadata, warnings=warnings)
