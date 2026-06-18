# LocalLLMTranslator — Plan

## Goal

A TUI application that translates books (native PDFs) using a local LLM.

## Pipeline

```
PDF → pymupdf4llm → Markdown → split by headings → translate sections via LLM → translated Markdown → (optional) pandoc → EPUB/PDF/DOCX
```

## Architecture

```
src/local_llm_translator/
├── __init__.py
├── __main__.py         # argparse → TUI (or --mock flag)
├── app.py              # Textual TUI: log + progress bar
├── config.py           # dataclass from argparse
├── extractor.py        # pymupdf4llm → md string + images on disk
├── splitter.py         # heading parse, token estimate, group ≤ context_size
├── llm.py              # httpx async → /v1/chat/completions
├── translator.py       # orchestration: split → translate → save
├── state.py            # save/load .state.json (resume support)
└── prompts/
    ├── historian.md
    └── fantasy.md

mock_server.py           # standalone: FastAPI /v1/chat/completions → echo
tests/                   # pytest for each module
```

## CLI parameters

```
llm-translate --input PATH --lang TEXT --style STYLE
               [--output DIR] [--from N] [--to N]
               [--format FMT] [--base-url URL] [--api-key KEY]
               [--model NAME] [--context-size N] [--mock]
```

`--mock` is shorthand for `--base-url http://localhost:8001/v1`.

## Key design decisions

- **Token counting:** word_count / 4 (approximate, zero deps)
- **Context window:** each group of sections stays under `--context-size` tokens
- **Resumability:** `TRANSLATION_STATE.json` in output dir; matched by input file hash
- **Previous-context hint:** last 1-2 translated sections prepended to the prompt
- **TUI:** single-screen Textual app with RichLog + ProgressBar
- **Mock server:** standalone script at repo root, `uv run python mock_server.py`

## Implementation order

1. `mock_server.py`
2. `prompts/historian.md`, `prompts/fantasy.md`
3. `config.py`
4. `splitter.py`
5. `extractor.py`
6. `llm.py`
7. `state.py`
8. `translator.py`
9. `app.py`
10. `__main__.py`
11. Tests
12. Update CI / pre-commit / AGENTS.md if needed
