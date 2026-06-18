from __future__ import annotations

import logging
from typing import TYPE_CHECKING

import pymupdf4llm  # pyright: ignore[reportMissingTypeStubs]

if TYPE_CHECKING:
    from pathlib import Path

_LOGGER = logging.getLogger(__name__)


def extract_markdown(pdf_path: Path, output_dir: Path) -> str:
    """Extract a PDF to markdown text using pymupdf4llm.

    Images are saved to ``output_dir / "images"`` automatically by pymupdf4llm.

    Returns the full markdown string.
    """
    _LOGGER.info("Extracting markdown from %s", pdf_path)
    images_dir = output_dir / "images"
    images_dir.mkdir(parents=True, exist_ok=True)

    md_text: str = pymupdf4llm.to_markdown(  # type: ignore[reportUnknownMemberType]
        str(pdf_path),
        save_images=True,
        image_path=str(images_dir),
        page_chunks=False,
    )

    _LOGGER.info("Extraction complete (%d chars)", len(md_text))
    return md_text
