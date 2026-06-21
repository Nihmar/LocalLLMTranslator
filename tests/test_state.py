from __future__ import annotations

from pathlib import Path

from local_llm_translator.state import TranslationState


class TestTranslationState:
    def test_new_state(self, tmp_output: Path) -> None:
        pdf = tmp_output / "test.pdf"
        pdf.write_text("dummy content")
        state = TranslationState.new(pdf, tmp_output, "Italiano", "historian")
        assert state.target_language == "Italiano"
        assert state.style == "historian"
        assert state.sections == []
        assert state.completed_indices() == []

    def test_save_and_load(self, tmp_output: Path) -> None:
        pdf = tmp_output / "test.pdf"
        pdf.write_text("same content")

        state = TranslationState.new(pdf, tmp_output, "Italiano", "historian")
        state.sections = [
            {"heading": "Ch1", "level": 2, "original_text": "text", "translated_text": None},
        ]
        state.save(tmp_output)

        loaded = TranslationState.load(tmp_output)
        assert loaded is not None
        assert len(loaded.sections) == 1
        assert loaded.sections[0]["heading"] == "Ch1"
        assert loaded.completed_indices() == []

    def test_resume_matching_hash(self, tmp_output: Path) -> None:
        pdf = tmp_output / "test.pdf"
        pdf.write_text("same content")

        state = TranslationState.new(pdf, tmp_output, "Italiano", "historian")
        state.save(tmp_output)

        loaded = TranslationState.load(tmp_output)
        assert loaded is not None
        assert loaded.is_compatible(pdf) is True

    def test_resume_wrong_hash(self, tmp_output: Path) -> None:
        pdf1 = tmp_output / "a.pdf"
        pdf1.write_text("content a")
        pdf2 = tmp_output / "b.pdf"
        pdf2.write_text("content b")

        state = TranslationState.new(pdf1, tmp_output, "Italiano", "historian")
        state.save(tmp_output)

        loaded = TranslationState.load(tmp_output)
        assert loaded is not None
        assert loaded.is_compatible(pdf2) is False

    def test_load_nonexistent(self, tmp_output: Path) -> None:
        loaded = TranslationState.load(tmp_output / "nonexistent")
        assert loaded is None

    def test_load_corrupted(self, tmp_output: Path) -> None:
        path = tmp_output / "TRANSLATION_STATE.json"
        path.write_text("not json", encoding="utf-8")
        loaded = TranslationState.load(tmp_output)
        assert loaded is None

    def test_section_state_tracking(self, tmp_output: Path) -> None:
        pdf = tmp_output / "test.pdf"
        pdf.write_text("content")

        state = TranslationState.new(pdf, tmp_output, "Italiano", "historian")
        state.sections = [
            {"heading": "Ch1", "level": 2, "original_text": "text1", "translated_text": None},
            {"heading": "Ch2", "level": 2, "original_text": "text2", "translated_text": "ciao"},
        ]
        state.save(tmp_output)

        loaded = TranslationState.load(tmp_output)
        assert loaded is not None
        assert loaded.completed_indices() == [1]
        assert loaded.sections[0]["translated_text"] is None
        assert loaded.sections[1]["translated_text"] == "ciao"

        # Complete section 0
        loaded.sections[0]["translated_text"] = "translated1"
        loaded.save(tmp_output)

        re_loaded = TranslationState.load(tmp_output)
        assert re_loaded is not None
        assert re_loaded.sections[0]["translated_text"] == "translated1"
        assert re_loaded.completed_indices() == [0, 1]
