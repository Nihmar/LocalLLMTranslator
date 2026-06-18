# LocalLLMTranslator

A translator that uses a local LLM.

## Tech stack

| Layer | Tool |
|---|---|
| Project mgmt | `uv` |
| Lint + format | `ruff` (select=ALL, ignore=D/COM812) |
| Types | `pyright` (strict) |
| Tests | `pytest` + `pytest-cov` |
| Git hooks | `pre-commit` (ruff check --fix + ruff-format) |
| CI | GitHub Actions |

## Commands

```sh
uv run ruff check src tests          # lint
uv run ruff format --check src tests # format check (no-op write)
uv run ruff format src tests         # auto-format
uv run pyright                       # type check
uv run pytest                        # test + coverage
uv run llm-translate                 # run the CLI entry point
```

All four checks run in CI in that order.

## Project layout

- **Source:** `src/local_llm_translator/`
- **Tests:** `tests/`
- **CLI entry:** `llm-translate` → `local_llm_translator.__main__:main` (pyproject.toml:13)
- **Build backend:** hatchling (pyproject.toml:23-28)
- **Python:** >=3.14

## Conventions

- Ruff lint with `ALL` minus docstring rules (`D`) and a formatter conflict (`COM812`).
- Ruff format with double quotes, 100-char line length.
- Pyright strict mode — every function needs explicit return types.
- No `print()` in source code (ruff T201); use `logging` with a module-level logger.
- Pre-commit runs `ruff check --fix` then `ruff-format` on every commit.
- Coverage required: `pytest-cov` runs automatically (see `addopts` in pyproject.toml:44).

## State

Greenfield — no application code written yet.
