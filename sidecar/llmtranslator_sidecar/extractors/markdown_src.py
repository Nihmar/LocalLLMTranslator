"""Markdown ingestion: the file is already the target format, so it passes through.

The only work done here is reading optional YAML front matter into the ``metadata``
channel. The front matter block itself stays in the document: stripping it would change
the source, and the source is what the round-trip invariant is measured against.
"""

from __future__ import annotations

from pathlib import Path
from typing import Any, cast

import yaml

from .base import ExtractionError, ExtractResult

#: The front-matter delimiters mandated by the YAML-in-Markdown convention.
_OPEN_DELIMITER = "---"
_CLOSE_DELIMITERS = frozenset({"---", "..."})


def _read_front_matter(text: str, warnings: list[str]) -> dict[str, Any]:
    """Parse the leading ``---`` YAML block, tolerating anything malformed."""
    lines = text.split("\n")
    if not lines or lines[0].strip() != _OPEN_DELIMITER:
        return {}

    closing = next(
        (
            index
            for index, line in enumerate(lines[1:], start=1)
            if line.strip() in _CLOSE_DELIMITERS
        ),
        None,
    )
    if closing is None:
        warnings.append("front matter is not terminated")
        return {}

    try:
        data = yaml.safe_load("\n".join(lines[1:closing]))
    except yaml.YAMLError as exc:
        warnings.append(f"cannot parse YAML front matter: {exc}")
        return {}

    if data is None:
        return {}
    if not isinstance(data, dict):
        warnings.append("front matter is not a mapping")
        return {}
    mapping = cast("dict[object, object]", data)
    return {str(key): value for key, value in mapping.items()}


class MarkdownSourceExtractor:
    """Backend for documents that are already Markdown."""

    format = "markdown"

    def extract(self, path: str) -> ExtractResult:
        source = Path(path)
        try:
            text = source.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            message = f"cannot read Markdown source {source.name}: {exc}"
            raise ExtractionError(message) from exc

        # Strip a UTF-8 BOM before the first line is inspected: otherwise the BOM hides the
        # opening ``---`` delimiter and the front matter is never detected.
        text = text.removeprefix("\ufeff")

        warnings: list[str] = []
        return ExtractResult(
            markdown=text,
            metadata=_read_front_matter(text, warnings),
            warnings=warnings,
        )
