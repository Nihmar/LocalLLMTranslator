import argparse
from dataclasses import dataclass
from pathlib import Path


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
    parser.add_argument(
        "--base-url",
        default="http://localhost:8001/v1",
        help="OpenAI-compatible API base URL",
    )
    parser.add_argument("--api-key", default="sk-mock", help="API key")
    parser.add_argument("--model", default="llama3", help="Model name")
    parser.add_argument(
        "--context-size",
        type=int,
        default=8192,
        help="Max input tokens per request",
    )
    parser.add_argument(
        "--mock",
        action="store_true",
        help="Shorthand for --base-url http://localhost:8001/v1 (overrides --base-url)",
    )

    args = parser.parse_args(argv)

    if args.mock:
        args.base_url = "http://localhost:8001/v1"

    if args.output is None:
        args.output = Path("output") / args.input.stem

    return Config(
        input_path=args.input,
        target_language=args.lang,
        style=args.style,
        output_dir=args.output,
        from_chapter=args.from_chapter,
        to_chapter=args.to_chapter,
        output_format=args.output_format,
        base_url=args.base_url,
        api_key=args.api_key,
        model=args.model,
        context_size=args.context_size,
    )
