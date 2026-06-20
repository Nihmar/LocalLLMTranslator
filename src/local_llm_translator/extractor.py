from __future__ import annotations

import logging
from typing import TYPE_CHECKING, cast

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
    """Extract text from an EPUB using the Table of Contents for chapter detection.

    Uses PyMuPDF's get_toc() to identify chapters and their page ranges.
    Output is markdown with # headings for each TOC entry.
    """
    doc = fitz.open(str(epub_path))  # type: ignore[reportUnknownMemberType]
    try:
        toc = cast("list[tuple[int, str, int]]", doc.get_toc())  # type: ignore[reportUnknownMemberType]
        page_count: int = doc.page_count  # type: ignore[reportUnknownMemberType, reportUnknownVariableType]

        if not toc:
            # No TOC available, fall back to full text extraction
            parts: list[str] = []
            for page in doc:  # type: ignore[reportUnknownVariableType]
                text: str = page.get_text("text")  # type: ignore[reportUnknownMemberType, reportUnknownVariableType]
                if text.strip():
                    parts.append(text)
            return "\n\n".join(parts)

        toc_len = len(toc)  # type: ignore[reportUnknownArgumentType, reportUnknownVariableType]
        _LOGGER.info("EPUB TOC: %d entries, %d pages", toc_len, page_count)  # type: ignore[reportUnknownArgumentType]

        # Build sections from TOC entries
        lines: list[str] = []
        for i, (level, title, start_page) in enumerate(toc):
            start_idx = max(0, start_page - 1)  # TOC pages are 1-based
            end_idx: int = toc[i + 1][2] - 1 if i + 1 < toc_len else page_count  # type: ignore[reportUnknownVariableType]
            end_idx = min(end_idx, page_count)  # type: ignore[reportUnknownArgumentType]

            if start_idx >= end_idx:
                continue

            # Extract text from pages in this range
            section_parts: list[str] = []
            for p in range(start_idx, end_idx):  # type: ignore[reportUnknownArgumentType]
                text: str = doc[p].get_text("text")  # type: ignore[reportUnknownMemberType, reportUnknownVariableType]
                if text.strip():
                    section_parts.append(text.strip())

            body = "\n\n".join(section_parts)
            if not body.strip():
                continue

            heading_mark = "#" * min(level, 6)
            lines.append(f"{heading_mark} {title}\n\n{body}")

        return "\n\n".join(lines)
    finally:
        doc.close()  # type: ignore[reportUnknownMemberType]


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
