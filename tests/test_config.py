from __future__ import annotations

from local_llm_translator.config import build_config


class TestConfig:
    def test_minimal_args(self) -> None:
        cfg = build_config(["--input", "book.pdf", "--lang", "Italiano", "--style", "historian"])
        assert cfg.input_path.name == "book.pdf"
        assert cfg.target_language == "Italiano"
        assert cfg.style == "historian"
        assert cfg.output_format == "md"
        assert cfg.base_url == "http://localhost:8001/v1"
        assert cfg.api_key == "sk-mock"
        assert cfg.model == "llama3"
        assert cfg.context_size == 8192
        assert cfg.output_dir.name == "book"

    def test_mock_flag(self) -> None:
        cfg = build_config(
            [
                "--input",
                "book.pdf",
                "--lang",
                "Italiano",
                "--style",
                "fantasy",
                "--mock",
            ]
        )
        assert cfg.base_url == "http://localhost:8001/v1"

    def test_base_url_override(self) -> None:
        cfg = build_config(
            [
                "--input",
                "book.pdf",
                "--lang",
                "Italiano",
                "--style",
                "historian",
                "--base-url",
                "http://llm:8080/v1",
            ]
        )
        assert cfg.base_url == "http://llm:8080/v1"

    def test_mock_overrides_base_url(self) -> None:
        cfg = build_config(
            [
                "--input",
                "book.pdf",
                "--lang",
                "Italiano",
                "--style",
                "historian",
                "--base-url",
                "http://other:8080/v1",
                "--mock",
            ]
        )
        assert cfg.base_url == "http://localhost:8001/v1"

    def test_output_format(self) -> None:
        cfg = build_config(
            [
                "--input",
                "book.pdf",
                "--lang",
                "Italiano",
                "--style",
                "historian",
                "--format",
                "epub",
            ]
        )
        assert cfg.output_format == "epub"

    def test_custom_output_dir(self) -> None:
        cfg = build_config(
            [
                "--input",
                "book.pdf",
                "--lang",
                "Italiano",
                "--style",
                "historian",
                "--output",
                "/tmp/my_translation",
            ]
        )
        assert str(cfg.output_dir) == "/tmp/my_translation"

    def test_from_to_chapters(self) -> None:
        cfg = build_config(
            [
                "--input",
                "book.pdf",
                "--lang",
                "Italiano",
                "--style",
                "historian",
                "--from",
                "3",
                "--to",
                "10",
            ]
        )
        assert cfg.from_chapter == 3
        assert cfg.to_chapter == 10

    def test_style_choices(self) -> None:
        cfg = build_config(["--input", "b.pdf", "--lang", "En", "--style", "fantasy"])
        assert cfg.style == "fantasy"

    def test_context_size(self) -> None:
        cfg = build_config(
            [
                "--input",
                "b.pdf",
                "--lang",
                "En",
                "--style",
                "historian",
                "--context-size",
                "4096",
            ]
        )
        assert cfg.context_size == 4096
