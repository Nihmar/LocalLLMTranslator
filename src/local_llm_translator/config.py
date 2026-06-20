from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path

from local_llm_translator.settings import load_settings


@dataclass
class Config:
    input_path: Path
    target_language: str
    style: str
    output_dir: Path
    from_chapter: int | None
    to_chapter: int | None
    output_format: str
    base_url: str
    api_key: str
    model: str
    context_size: int
    timeout: int
    debug: bool


def build_config(argv: list[str] | None = None) -> Config:
    parser = argparse.ArgumentParser(
        prog="llm-translate",
        description="Translate a PDF book using a local LLM.",
    )

    parser.add_argument("--input", required=True, type=Path, help="Input PDF file")
    parser.add_argument("--lang", required=True, help="Target language (e.g. Italiano)")
    parser.add_argument(
        "--style",
        required=True,
        choices=["historian", "fantasy"],
        help="Translation style (selects system prompt)",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=None,
        help="Output directory (default: ./output/<input_stem>/)",
    )
    parser.add_argument("--from", dest="from_chapter", type=int, default=None, help="Start chapter")
    parser.add_argument("--to", dest="to_chapter", type=int, default=None, help="End chapter")
    parser.add_argument(
        "--format",
        dest="output_format",
        default="md",
        choices=["md", "epub", "pdf", "docx"],
        help="Output format (requires pandoc for non-md formats)",
    )
    parser.add_argument("--base-url", default=None, help="OpenAI-compatible API base URL")
    parser.add_argument("--api-key", default=None, help="API key")
    parser.add_argument("--model", default=None, help="Model name")
    parser.add_argument("--context-size", type=int, default=None, help="Max input tokens")
    parser.add_argument(
        "--timeout",
        type=int,
        default=None,
        help="LLM request timeout in seconds (default: 600)",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Enable debug logging (shows full LLM request/response)",
    )
    parser.add_argument(
        "--mock",
        action="store_true",
        help="Shorthand for --base-url http://localhost:8001/v1",
    )

    raw = vars(parser.parse_args(argv))
    settings = load_settings()
    api = settings.get("api", {})
    out = settings.get("output", {})

    def _first(*values: object) -> object:
        return next((v for v in values if v is not None), None)

    base_url: str | None = _first(
        "http://localhost:8001/v1" if raw["mock"] else None,
        raw["base_url"],
        api.get("base_url"),
        "http://localhost:8001/v1",
    )  # type: ignore[assignment]

    output_dir = raw["output"] or out.get("directory")
    if output_dir is None:
        output_dir = raw["input"].parent / raw["input"].stem
    elif isinstance(output_dir, str):
        output_dir = Path(output_dir)

    return Config(
        input_path=raw["input"],
        target_language=raw["lang"],
        style=raw["style"],
        output_dir=output_dir,
        from_chapter=raw["from_chapter"],
        to_chapter=raw["to_chapter"],
        output_format=_first(raw["output_format"], out.get("format"), "md"),  # type: ignore[arg-type]
        base_url=base_url,  # type: ignore[arg-type]
        api_key=_first(raw["api_key"], api.get("api_key"), "sk-mock"),  # type: ignore[arg-type]
        model=_first(raw["model"], api.get("model"), "llama3"),  # type: ignore[arg-type]
        context_size=_first(raw["context_size"], api.get("context_size"), 8192),  # type: ignore[arg-type]
        timeout=_first(raw["timeout"], api.get("timeout"), 600),  # type: ignore[arg-type]
        debug=raw.get("debug", False),
    )
