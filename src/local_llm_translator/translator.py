from __future__ import annotations

import asyncio
import logging
import re
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Any

from local_llm_translator.extractor import extract_markdown
from local_llm_translator.llm import LLMClient
from local_llm_translator.splitter import Section, estimate_tokens, split_markdown
from local_llm_translator.state import TranslationState

if TYPE_CHECKING:
    from local_llm_translator.config import Config

_LOGGER = logging.getLogger(__name__)

_PROMPTS_DIR = Path(__file__).parent / "prompts"

ProgressCallback = Callable[[int, int, str], None]
HeadingCallback = Callable[[str], None]


def _load_prompt(style: str) -> str:
    path = _PROMPTS_DIR / f"{style}.md"
    return path.read_text(encoding="utf-8")


def _render_prompt(template: str, target_language: str, previous_context: str, text: str) -> str:
    ctx = ""
    if previous_context:
        ctx = f"Previous context (already translated):\n{previous_context}\n"

    return (
        template.replace("{target_language}", target_language)
        .replace("{previous_context}", ctx)
        .replace("{text}", text)
    )


def _build_system_message(template: str, target_language: str) -> str:
    return (
        template.replace("{target_language}", target_language)
        .split("{previous_context}")[0]
        .split("{text}")[0]
        .strip()
    )


async def _translate_section(
    llm: LLMClient,
    system_content: str,
    user_content: str,
    max_tokens: int,
    section_index: int,
) -> str | None:
    """Translate a single section with retries.

    Returns the translated text, or None if all retries fail.
    """
    max_retries = 3
    for attempt in range(1, max_retries + 1):
        try:
            return await llm.translate(system_content, user_content, max_tokens)
        except Exception:  # noqa: BLE001
            _LOGGER.warning(
                "Section %d attempt %d/%d failed",
                section_index,
                attempt,
                max_retries,
                exc_info=attempt == max_retries,
            )
            if attempt < max_retries:
                await asyncio.sleep(2 * attempt)

    _LOGGER.error("Skipping section %d after %d failed attempts", section_index, max_retries)
    return None


def _cleanup_empty_sections(state: TranslationState, output_dir: Path) -> None:
    """Clear translations for sections whose original_text is empty."""
    count = 0
    for i, s in enumerate(state.sections):
        if not s.get("original_text", "").strip() and s.get("translated_text"):
            _LOGGER.warning("Section %d has empty original_text — clearing stale translation", i)
            s["translated_text"] = None
            s["translated_heading"] = None
            count += 1
    if count:
        _LOGGER.info("Cleaned %d stale section(s) with empty original_text", count)
        state.save(output_dir)


def _check_integrity(state: TranslationState, output_dir: Path) -> set[int]:
    """Rebuild completed_indices from actual section data."""
    _cleanup_empty_sections(state, output_dir)
    stated = set(state.completed_indices)
    actual = {i for i, s in enumerate(state.sections) if s.get("translated_text")}
    if stated != actual:
        _LOGGER.warning(
            "State mismatch: completed_indices=%s but actual=%s — correcting",
            sorted(stated),
            sorted(actual),
        )
        state.completed_indices = sorted(actual)
        state.save(output_dir)
    return actual


async def _translate_heading(
    llm: LLMClient,
    heading: str,
    target_language: str,
    section_index: int,
    prompt_template: str,
) -> str | None:
    """Translate a single heading using the same prompt style as body translation."""
    if not heading.strip():
        _LOGGER.warning("Heading %d: empty heading, skipping", section_index)
        return None
    system_content = _build_system_message(prompt_template, target_language)
    user_content = _render_prompt(prompt_template, target_language, "", heading)
    _LOGGER.info("Heading %d translating: %r", section_index, heading)
    max_retries = 3
    for attempt in range(1, max_retries + 1):
        try:
            result = await llm.translate(system_content, user_content, max_tokens=256)
        except Exception as exc:  # noqa: BLE001
            _LOGGER.warning(
                "Heading %d attempt %d/%d failed: %s",
                section_index,
                attempt,
                max_retries,
                exc,
            )
        else:
            _LOGGER.info("Heading %d raw response: %r", section_index, result)
            cleaned = result.strip()
            cleaned = re.sub(r"^#{1,6}\s+", "", cleaned)
            cleaned = cleaned.rstrip(".。").strip()
            if cleaned:
                return cleaned
            _LOGGER.warning(
                "Heading %d attempt %d/%d: empty after cleaning (raw=%r)",
                section_index,
                attempt,
                max_retries,
                result,
            )
        if attempt < max_retries:
            await asyncio.sleep(2 * attempt)
    return None


async def _ensure_heading_translated(
    state: TranslationState,
    section_index: int,
    llm: LLMClient,
    prompt_template: str,
    on_heading: HeadingCallback | None = None,
) -> None:
    """Translate a section heading if not already done, saving state on success."""
    sec_data = state.sections[section_index]
    heading: str = sec_data.get("heading", "")
    if not heading or sec_data.get("translated_heading"):
        return
    if on_heading:
        on_heading(heading)
    _LOGGER.info("Translating heading %d: %s", section_index, heading)
    result = await _translate_heading(
        llm, heading, state.target_language, section_index, prompt_template
    )
    if result:
        sec_data["translated_heading"] = result
        state.save(Path(state.output_dir))
    else:
        _LOGGER.warning("Heading %d translation failed, keeping original", section_index)


def _skip_if_empty(
    section: Section,
    completed: set[int],
    state: TranslationState,
) -> bool:
    """If section body is empty, mark as done. Returns True if skipped."""
    if not section.text.strip():
        _LOGGER.info("Section %d has empty body — skipping", section.index)
        state.sections[section.index]["translated_text"] = ""
        completed.add(section.index)
        state.completed_indices = sorted(completed)
        state.save(Path(state.output_dir))
        return True
    return False


async def _translate_missing_headings(
    state: TranslationState,
    index_range: tuple[int, int],
    llm: LLMClient,
    prompt_template: str,
    on_heading: HeadingCallback | None,
) -> None:
    """Translate headings for all sections in range that are missing translation."""
    for i in range(index_range[0], index_range[1]):
        await _ensure_heading_translated(
            state,
            i,
            llm,
            prompt_template,
            on_heading,
        )


async def run_translation(
    config: Config,
    on_progress: ProgressCallback | None = None,
    on_heading: HeadingCallback | None = None,
) -> None:
    """Run the full translation pipeline.

    Handles resume from existing state, extraction, splitting,
    LLM translation per section, and state persistence.
    """
    output_dir = config.output_dir
    output_dir.mkdir(parents=True, exist_ok=True)

    # --- Load or init state ---
    state = TranslationState.load(output_dir)

    if state is not None and state.is_compatible(config.input_path):
        _LOGGER.info(
            "Resuming translation (%d sections completed out of %d)",
            len(state.completed_indices),
            len(state.sections),
        )
    else:
        state = TranslationState.new(
            config.input_path, output_dir, config.target_language, config.style
        )

    # --- Build section list (only if not already from state) ---
    if not state.sections:
        # --- Extract PDF to markdown ---
        md_text = extract_markdown(config.input_path, output_dir)
        sections = split_markdown(md_text, config.context_size)
        state.sections = [
            {
                "heading": s.heading,
                "level": s.level,
                "original_text": s.text,
                "translated_text": None,
                "translated_heading": None,
            }
            for s in sections
        ]
        state.save(output_dir)
    else:
        sections = [
            Section(
                heading=s["heading"],
                level=s["level"],
                text=s["original_text"],
                token_estimate=0,
                index=i,
            )
            for i, s in enumerate(state.sections)
        ]

    # --- Integrity check: rebuild completed_indices from actual section data ---
    completed = _check_integrity(state, output_dir)

    # --- Apply --from / --to filters ---
    from_idx = 0 if config.from_chapter is None else config.from_chapter - 1
    to_idx = len(sections) if config.to_chapter is None else config.to_chapter
    to_process = [s for s in sections[from_idx:to_idx] if s.index not in completed]

    # --- Load prompt template ---
    prompt_template = _load_prompt(config.style)

    # --- Init LLM client ---
    llm = LLMClient(
        base_url=config.base_url,
        api_key=config.api_key,
        model=config.model,
        timeout=config.timeout,
    )

    # --- Translate missing headings for ALL sections (including already-completed ones) ---
    await _translate_missing_headings(state, (from_idx, to_idx), llm, prompt_template, on_heading)

    if not to_process:
        _LOGGER.info("All requested sections already translated, nothing to do")
        return

    # --- Translate ---
    done = len(completed)
    total = len(to_process) + done
    translated_history: list[str] = []

    for section in to_process:
        if _skip_if_empty(section, completed, state):
            done += 1
            continue

        prev_ctx = "\n\n".join(translated_history[-2:]) if translated_history else ""

        system_content = _build_system_message(prompt_template, config.target_language)
        user_content = _render_prompt(
            prompt_template, config.target_language, prev_ctx, section.text
        )

        input_tokens = estimate_tokens(system_content) + estimate_tokens(user_content)
        max_tokens = max(256, int(config.context_size * 0.9) - input_tokens)

        _LOGGER.info(
            "Translating section %d/%d: %s (est. input=%d tokens, max_output=%d)",
            done + 1,
            total,
            section.heading or "(no heading)",
            input_tokens,
            max_tokens,
        )

        if on_progress:
            on_progress(done, total, section.heading)

        translated = await _translate_section(
            llm, system_content, user_content, max_tokens, section.index
        )

        if translated is not None:
            translated_history.append(translated)
            state.sections[section.index]["translated_text"] = translated
            completed.add(section.index)
            state.completed_indices = sorted(completed)
            state.save(output_dir)
            if on_progress:
                on_progress(done + 1, total, section.heading)

        done += 1

    # --- Write final translated markdown ---
    _write_translated_markdown(output_dir, state.sections, config.output_format)

    _LOGGER.info("Translation complete: %d sections", len(completed))


def _write_translated_markdown(
    output_dir: Path, sections: list[dict[str, Any]], output_format: str
) -> None:
    output_path = output_dir / f"translated.{output_format}"
    lines: list[str] = []
    for sec in sections:
        heading = sec.get("translated_heading") or sec.get("heading")
        if heading:
            heading_mark = "#" * sec["level"]
            lines.append(f"{heading_mark} {heading}")
        if sec.get("translated_text"):
            lines.append(sec["translated_text"])
        lines.append("")

    text = "\n".join(lines)
    output_path.write_text(text, encoding="utf-8")

    if output_format != "md":
        _LOGGER.info(
            "Pandoc conversion to %s not yet implemented, saved as .md instead",
            output_format,
        )
