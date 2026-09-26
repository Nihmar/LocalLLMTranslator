"""Optional ``marker`` PDF backend.

``marker`` is a heavy extra (it drags ``torch`` and downloads models on first use), so it
is never imported eagerly and never part of the default path. When it is missing the
backend must not silently fall back to ``pymupdf4llm``: the caller asked for marker
quality and deserves to know it is not available, hence the dedicated error that the RPC
layer maps to code ``1003``.
"""

from __future__ import annotations

import importlib
import importlib.util
from pathlib import Path
from typing import Any, cast

from llmtranslator_sidecar.errors import MissingDependencyError

from .base import ExtractionError, ExtractResult
from .pdf_pymupdf import pdf_metadata


def _marker_available() -> bool:
    """True when the optional ``marker`` distribution can be imported.

    ``find_spec`` never imports the package, so probing for marker stays cheap and does
    not pull ``torch`` into the default path.
    """
    try:
        return importlib.util.find_spec("marker") is not None
    except (ImportError, ValueError):
        return False


def _extract_with_marker(path: str) -> str:
    """Run marker's high-level converter and return its Markdown rendering.

    marker ships no type information, so its modules are widened to ``Any`` at the import
    boundary; the opaque values never reach a statically checked call site. The imports and
    the attribute wiring run inside the guarded block: a marker whose internal layout has
    drifted (a version mismatch) is a broken install, and must surface as a missing
    dependency rather than as an opaque internal error.
    """
    try:
        converters = cast("Any", importlib.import_module("marker.converters.pdf"))
        models = cast("Any", importlib.import_module("marker.models"))
        output = cast("Any", importlib.import_module("marker.output"))
        converter = converters.PdfConverter(models.create_model_dict())
        render = output.text_from_rendered
    except (ImportError, AttributeError) as exc:
        message = (
            "the installed 'marker' backend is incompatible or incomplete "
            f"({exc}); reinstall the sidecar's optional 'marker' extra"
        )
        raise MissingDependencyError(message, backend="marker") from exc

    try:
        rendered = converter(str(path))
        markdown, _images, _metadata = render(rendered)
    except Exception as exc:
        message = f"marker failed to extract {Path(path).name}: {exc}"
        raise ExtractionError(message) from exc
    return str(markdown)


class MarkerPdfExtractor:
    """Optional high-quality PDF backend."""

    format = "pdf"

    def extract(self, path: str) -> ExtractResult:
        if not _marker_available():
            message = (
                "the 'marker' PDF backend is not installed; "
                "install the sidecar's optional 'marker' extra to enable it"
            )
            raise MissingDependencyError(message, backend="marker")

        markdown = _extract_with_marker(path)
        return ExtractResult(markdown=markdown, metadata=pdf_metadata(path), warnings=[])
