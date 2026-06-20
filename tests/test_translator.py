from __future__ import annotations

import json
from pathlib import Path

import pytest

from local_llm_translator.config import Config
from local_llm_translator.translator import run_translation


class TestTranslator:
    """Integration test with mocked LLM API."""

    @pytest.fixture
    def config(self, tmp_output: Path) -> Config:
        pdf = tmp_output / "book.pdf"
        pdf.write_text("dummy pdf content, not actually parsed here")

        # We need a real markdown file for extraction — but the extractor
        # uses pymupdf4llm which needs a real PDF. Instead we'll test
        # the state/resume logic with a pre-populated state file.
        return Config(
            input_path=pdf,
            target_language="Italiano",
            style="historian",
            output_dir=tmp_output,
            from_chapter=None,
            to_chapter=None,
            output_format="md",
            base_url="http://mock/v1",
            api_key="sk-mock",
            model="mock",
            context_size=8192,
            timeout=600,
            debug=False,
        )

    async def test_resume_from_state(self, tmp_output: Path, config: Config) -> None:
        """When state exists with completed sections, translator skips them."""
        # Pre-create state file with 2 completed sections
        state = {
            "input_hash": "abc",
            "output_dir": str(tmp_output),
            "target_language": "Italiano",
            "style": "historian",
            "sections": [
                {"heading": "Ch1", "level": 1, "original_text": "hi", "translated_text": "ciao"},
            ],
            "completed_indices": [0],
            "created_at": "2025-01-01T00:00:00",
            "updated_at": "2025-01-01T00:00:00",
        }
        state_path = tmp_output / "TRANSLATION_STATE.json"
        state_path.write_text(json.dumps(state), encoding="utf-8")

        # Pretend the input hash matches by writing the state
        # directly with the correct hash for this specific input
        from hashlib import sha256

        real_hash = sha256(config.input_path.read_bytes()).hexdigest()
        state["input_hash"] = real_hash
        state_path.write_text(json.dumps(state), encoding="utf-8")

        # Since all sections are completed, translator should detect
        # nothing to do and return without calling the LLM
        # (the real extractor will fail since it's not a real PDF,
        # but since state has sections, extraction is skipped)
        await run_translation(config)

        # Verify: nothing crashes, no more sections added
        loaded = json.loads(state_path.read_text(encoding="utf-8"))
        assert len(loaded["sections"]) == 1

    async def test_from_to_chapters_filter_empty(self, tmp_output: Path, config: Config) -> None:
        """--from and --to exclude all sections → nothing to do."""
        config.from_chapter = 1
        config.to_chapter = 0  # invalid range, should result in nothing

        # Pre-populate with one section to avoid extraction
        from local_llm_translator.state import TranslationState

        state = TranslationState.new(config.input_path, tmp_output, "Italiano", "historian")
        state.sections = [
            {"heading": "Ch1", "level": 1, "original_text": "text", "translated_text": None},
        ]
        state.save(tmp_output)

        # Should not crash
        await run_translation(config)
