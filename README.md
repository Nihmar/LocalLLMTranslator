# LocalLLMTranslator

A TUI application that translates books (native PDFs) using a local LLM.

## Pipeline

```
PDF → pymupdf4llm → Markdown → split by chapters → LLM translation → translated Markdown → (optional) pandoc → EPUB/PDF/DOCX
```

## Quick start

```sh
# Prerequisites: Python >=3.14, uv installed
uv sync --all-groups

# Start the mock server (for testing without a real LLM)
uv run python mock_server.py &

# Translate a book
uv run llm-translate --input book.pdf --lang Italiano --style historian --mock
```

## Usage

```sh
uv run llm-translate --input <pdf> --lang <language> --style <historian|fantasy> [options]
```

### Required

| Flag | Description |
|---|---|
| `--input PATH` | Input PDF file |
| `--lang TEXT` | Target language (e.g. `Italiano`, `English`) |
| `--style STYLE` | Translation style: `historian` or `fantasy` |

### Options

| Flag | Default | Description |
|---|---|---|
| `--output DIR` | `./output/<pdf-stem>/` | Output directory |
| `--from N` | — | Start from chapter N (1-based) |
| `--to N` | — | End at chapter N (1-based) |
| `--format FMT` | `md` | Output format: `md`, `epub`, `pdf`, `docx` (non-md requires pandoc) |
| `--base-url URL` | `http://localhost:8001/v1` | OpenAI-compatible API base URL |
| `--api-key KEY` | `sk-mock` | API key |
| `--model NAME` | `llama3` | Model name |
| `--context-size N` | `8192` | Max input tokens per request |
| `--mock` | — | Shortcut for `--base-url http://localhost:8001/v1` |

### Examples

```sh
# Full translation
uv run llm-translate --input book.pdf --lang Italiano --style historian --format epub

# Range of chapters, custom API
uv run llm-translate --input book.pdf --lang English --style fantasy --from 5 --to 10 \
    --base-url http://192.168.1.100:8080/v1 --api-key mykey --model llama3

# Resume interrupted translation (re-run with same output directory)
uv run llm-translate --input book.pdf --lang Italiano --style historian
```

## Resumability

If the process is interrupted, re-run the exact same command. The translator detects the
`TRANSLATION_STATE.json` file in the output directory and resumes from where it left off.
Already-translated sections are skipped.

## Architecture

```
src/local_llm_translator/
├── __init__.py
├── __main__.py         # CLI entry point
├── app.py              # Textual TUI (RichLog + ProgressBar)
├── config.py           # CLI arg parser + settings merge
├── settings.py         # XDG config file loader
├── extractor.py        # pymupdf4llm PDF → Markdown
├── splitter.py         # Chapter detection, heading splitting, token estimation
├── llm.py              # OpenAI-compatible async HTTP client
├── translator.py       # Orchestration: split → translate → save → resume
├── state.py            # JSON state persistence for resume
└── prompts/
    ├── historian.md    # Academic/historical translation style
    └── fantasy.md      # Fantasy/narrative translation style

mock_server.py           # Standalone mock OpenAI API (/v1/chat/completions → echo)
```

### Chapter splitting

The splitter automatically detects the natural chapter level:
- **Multi-part books** (e.g. Lord of the Rings: parts → chapters): splits at h2 (chapters)
- **Flat books** (only h1 headings): splits at h1
- **Preamble text** between a part heading and the first chapter is preserved as
  its own introductory section
- **Oversized chapters** are recursively split by sub-headings or word-chunked

## Persistent settings

Create a TOML config file at `~/.config/local-llm-translator/config.toml` to set defaults:

```toml
[api]
base_url = "http://localhost:8001/v1"
api_key = "sk-mock"
model = "llama3"
context_size = 8192

[output]
directory = "~/translations"
format = "md"

[defaults]
language = "Italiano"
style = "historian"
```

CLI arguments always override config file values.

## Development

```sh
uv run ruff check src tests          # lint
uv run ruff format --check src tests # format check
uv run ruff format src tests         # auto-format
uv run pyright                       # type check
uv run pytest                        # test + coverage
uv run llm-translate --help          # CLI help
```

All four checks run in CI in this order: **lint → format → typecheck → test**.

### Tech stack

| Layer | Tool |
|---|---|
| Project mgmt | `uv` |
| Lint + format | `ruff` (select=ALL, ignore=D/COM812) |
| Types | `pyright` (strict) |
| Tests | `pytest` + `pytest-cov` |
| Git hooks | `pre-commit` (ruff check --fix + ruff-format) |
| CI | GitHub Actions |

## Building as AppImage

AppImage allows you to distribute `llm-translate` as a single self-contained executable.

### Prerequisites

- `docker` or `podman` (for the build container)
- `linuxdeploy` and `appimagetool` (or use the container-based build)

### Build steps

```sh
# 1. Create a build directory
mkdir -p build/AppDir

# 2. Install Python + dependencies into AppDir
uv pip install --python 3.14 --target build/AppDir/usr/lib/python3.14/site-packages -e .
uv pip install --python 3.14 --target build/AppDir/usr/lib/python3.14/site-packages pyinstaller

# 3. Build with PyInstaller
uv run pyinstaller --onefile --name llm-translate \
    --distpath build/AppDir/usr/bin \
    src/local_llm_translator/__main__.py

# 4. Create AppDir structure
cp build/AppDir/usr/bin/llm-translate build/AppDir/usr/bin/
mkdir -p build/AppDir/usr/share/applications
cat > build/AppDir/usr/share/applications/llm-translate.desktop <<EOF
[Desktop Entry]
Name=LLM Translator
Exec=llm-translate
Terminal=true
Type=Application
Categories=Office;
EOF

# 5. Build AppImage using linuxdeploy
wget -q https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
chmod +x linuxdeploy-x86_64.AppImage
./linuxdeploy-x86_64.AppImage --appdir build/AppDir --output appimage

# Result: LLM_Translator-x86_64.AppImage
```

After building, you can run:

```sh
chmod +x LLM_Translator-x86_64.AppImage
./LLM_Translator-x86_64.AppImage --input book.pdf --lang Italiano --style historian
```

### Changing settings in an AppImage

The AppImage itself is read-only. Persistent settings are stored in your home directory:

| Path | Purpose |
|---|---|
| `~/.config/local-llm-translator/config.toml` | User configuration (CLI defaults) |
| `~/.local/share/local-llm-translator/` | Translation output data |

To change settings after installing the AppImage:

```sh
# Create or edit the config file
mkdir -p ~/.config/local-llm-translator
nano ~/.config/local-llm-translator/config.toml

# Example: change base URL and default language
cat > ~/.config/local-llm-translator/config.toml <<EOF
[api]
base_url = "http://192.168.1.100:8080/v1"
api_key = "my-secret-key"
model = "llama3"

[defaults]
language = "Français"
style = "fantasy"
EOF
```

The next time you run the AppImage, it will use these values. CLI flags still override them.

### Notes

- The AppImage requires FUSE to run (or `--appimage-extract` + run the extracted binary)
- Non-md output formats (`epub`, `pdf`, `docx`) require `pandoc` installed on the host system
- For native PDFs only (scanned documents/OCR are not supported)
