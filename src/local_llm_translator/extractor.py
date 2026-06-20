from __future__ import annotations

import logging
from typing import TYPE_CHECKING

import fitz  # pyright: ignore[reportMissingTypeStubs]
import pymupdf4llm  # pyright: ignore[reportMissingTypeStubs]

if TYPE_CHECKING:
    from pathlib import Path

_LOGGER = logging.getLogger(__name__)


def _extract_pdf(pdf_path: Path, images_dir: Path) -> str:
    """Extract markdown from a PDF using pymupdf4llm."""
    return pymupdf4llm.to_markdown(  # type: ignore[reportUnknownMemberType]
        str(pdf_path),
        write_images=True,
        image_path=str(images_dir),
        page_chunks=False,
        use_ocr=False,
    )


def _extract_epub(epub_path: Path) -> str:
    """Extract text from an EPUB using PyMuPDF."""
    doc = fitz.open(str(epub_path))  # type: ignore[reportUnknownMemberType]
    parts: list[str] = []
    try:
        for page in doc:  # type: ignore[reportUnknownVariableType]
            text: str = page.get_text("text")  # type: ignore[reportUnknownMemberType, reportUnknownVariableType]
            if text.strip():
                parts.append(text)
    finally:
        doc.close()  # type: ignore[reportUnknownMemberType]
    return "\n\n".join(parts)


def extract_markdown(input_path: Path, output_dir: Path) -> str:
    """Extract markdown text from a PDF or EPUB file.

    For PDFs, images are saved to ``output_dir / "images"``.
    EPUBs are extracted text-only (no images).

    Returns the full markdown string.
    """
    _LOGGER.info("Extracting markdown from %s", input_path)

    suffix = input_path.suffix.lower()

    if suffix == ".epub":
        md_text = _extract_epub(input_path)
    else:
        images_dir = output_dir / "images"
        images_dir.mkdir(parents=True, exist_ok=True)
        md_text = _extract_pdf(input_path, images_dir)

    _LOGGER.info("Extraction complete (%d chars)", len(md_text))
    return md_text
