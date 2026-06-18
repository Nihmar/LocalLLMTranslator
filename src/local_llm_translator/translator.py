from __future__ import annotations

import logging
from collections.abc import Callable
from pathlib import Path
from typing import TYPE_CHECKING, Any

from local_llm_translator.extractor import extract_markdown
from local_llm_translator.llm import LLMClient
from local_llm_translator.splitter import Section, split_markdown
from local_llm_translator.state import TranslationState

if TYPE_CHECKING:
    from local_llm_translator.config import Config

_LOGGER = logging.getLogger(__name__)

_PROMPTS_DIR = Path(__file__).parent / "prompts"

ProgressCallback = Callable[[int, int, str], None]


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


async def run_translation(
    config: Config,
    on_progress: ProgressCallback | None = None,
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

    # --- Extract PDF to markdown ---
    md_text = extract_markdown(config.input_path, output_dir)

    # --- Build section list (only if not already from state) ---
    if not state.sections:
        sections = split_markdown(md_text, config.context_size)
        state.sections = [
            {
                "heading": s.heading,
                "level": s.level,
                "original_text": s.text,
                "translated_text": None,
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

    completed: set[int] = set(state.completed_indices)
    already_done = len(completed)

    # --- Apply --from / --to filters ---
    from_idx = 0 if config.from_chapter is None else config.from_chapter - 1
    to_idx = len(sections) if config.to_chapter is None else config.to_chapter

    to_process = [s for s in sections[from_idx:to_idx] if s.index not in completed]

    if not to_process:
        _LOGGER.info("All requested sections already translated, nothing to do")
        return

    # --- Load prompt template ---
    prompt_template = _load_prompt(config.style)

    # --- Init LLM client ---
    llm = LLMClient(
        base_url=config.base_url,
        api_key=config.api_key,
        model=config.model,
    )

    # --- Translate ---
    total = len(sections[from_idx:to_idx])
    done = already_done

    # Track translated text for previous-context injection
    translated_history: list[str] = []

    for section in to_process:
        prev_ctx = ""
        if translated_history:
            # Last 1-2 translated sections as context
            prev_ctx = "\n\n".join(translated_history[-2:])

        system_content = _build_system_message(prompt_template, config.target_language)
        user_content = _render_prompt(
            prompt_template, config.target_language, prev_ctx, section.text
        )

        _LOGGER.info(
            "Translating section %d/%d: %s",
            done + 1,
            total,
            section.heading or "(no heading)",
        )

        if on_progress:
            on_progress(done, total, section.heading)

        try:
            translated = await llm.translate(system_content, user_content)
        except Exception:
            _LOGGER.exception("Failed to translate section %d", section.index)
            raise

        translated_history.append(translated)

        # Update state
        state.sections[section.index]["translated_text"] = translated
        completed.add(section.index)
        state.completed_indices = sorted(completed)
        state.save(output_dir)
        done += 1

        if on_progress:
            on_progress(done, total, section.heading)

    # --- Write final translated markdown ---
    _write_translated_markdown(output_dir, state.sections, config.output_format)

    _LOGGER.info("Translation complete: %d sections", len(completed))


def _write_translated_markdown(
    output_dir: Path, sections: list[dict[str, Any]], output_format: str
) -> None:
    output_path = output_dir / f"translated.{output_format}"
    lines: list[str] = []
    for sec in sections:
        if sec["heading"]:
            heading_mark = "#" * sec["level"]
            lines.append(f"{heading_mark} {sec['heading']}")
        if sec["translated_text"]:
            lines.append(sec["translated_text"])
        lines.append("")

    text = "\n".join(lines)
    output_path.write_text(text, encoding="utf-8")

    if output_format != "md":
        _LOGGER.info(
            "Pandoc conversion to %s not yet implemented, saved as .md instead",
            output_format,
        )
