"""Tests for the support tooling owned by the ``tools``/``prompts``/``pandoc`` work.

Covers:
  * ``tools/fake_llama_server.py``   — endpoints, SSE framing, structural
    invariants of the fake translation, fault injection, ``--selftest``;
  * ``tools/make_fixtures.py``       — EPUB/Markdown/PDF fixtures and the large
    ~1M-character EPUB, validity and reproducibility;
  * ``prompts/``                     — Jinja2 templates parse and render;
  * ``pandoc/``                      — Lua filters and templates apply cleanly.

The tool scripts are loaded by file path (never imported as packages) and are
executed out-of-process when a CLI is being exercised.  ``pandoc`` tests skip
cleanly when the binary is absent.
"""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import re
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.request
import zipfile
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]
TOOLS_DIR = REPO_ROOT / "tools"
PROMPTS_DIR = REPO_ROOT / "prompts"
PANDOC_DIR = REPO_ROOT / "pandoc"
USER_MARKER = "---USER---"
PANDOC_BIN = shutil.which("pandoc")
PDF_ENGINE = shutil.which("pdflatex") or shutil.which("xelatex") or shutil.which("lualatex")


# --------------------------------------------------------------------------- #
# Module loading (by file path, never as an installed package)
# --------------------------------------------------------------------------- #


def _load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec is not None and spec.loader is not None, f"cannot load {path}"
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


fake = _load_module("fake_llama_server", TOOLS_DIR / "fake_llama_server.py")


# --------------------------------------------------------------------------- #
# HTTP helpers and the server fixture
# --------------------------------------------------------------------------- #
#
# urllib raises HTTPError for a 4xx/5xx response, and that object owns the connection. Since
# the exception propagates out of the ``with`` statement, the context manager never runs and
# the socket is only released by the garbage collector — which, with warnings promoted to
# errors, surfaces as an unraisable-exception failure in a later, unrelated test. Closing it
# explicitly before re-raising is what keeps the socket lifetime inside the test that made it.


def _urlopen(url: str | urllib.request.Request):
    try:
        return urllib.request.urlopen(url, timeout=15)
    except urllib.error.HTTPError as error:
        error.close()
        raise


def http_get(base: str, path: str):
    with _urlopen(base + path) as response:
        return json.loads(response.read().decode("utf-8"))


def http_post(base: str, path: str, payload: dict):
    request = urllib.request.Request(
        base + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with _urlopen(request) as response:
        return json.loads(response.read().decode("utf-8"))


def http_post_text(base: str, path: str, payload: dict) -> str:
    request = urllib.request.Request(
        base + path,
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
    )
    with _urlopen(request) as response:
        return response.read().decode("utf-8")


@pytest.fixture
def start_server():
    """Factory that starts fake servers on ephemeral ports and cleans them up."""
    started: list[tuple] = []

    def _start(cfg=None, host: str = "127.0.0.1", port: int = 0) -> str:
        server, thread, base = fake.create_and_start(host, port, cfg)
        started.append((server, thread))
        return base

    yield _start

    for server, thread in started:
        server.shutdown()
        server.server_close()
        thread.join(timeout=10)


# --------------------------------------------------------------------------- #
# A non-trivial input that stresses every structural rule of the pipeline
# --------------------------------------------------------------------------- #

SAMPLE_INPUT = """\
# The Lantern Keeper

The harbour was quiet that morning and the keeper wrote nothing.

- iron key
- brass telescope
+ spare oil

1. Fill the lamps
2. Trim the wicks

| Element | Number |
|:--------|-------:|
| Lamps   |      4 |

```sql
SELECT lamp FROM stores;
```

> The wind rose from the west.

He muttered a phrase learned as a boy ⟦1⟧ and turned to the ⟦2⟧ machine.

## Chapter Two

The sea was still for three days.⟦3⟧
"""

_HEADING_RE = re.compile(r"^(#{1,6})\s")
_LIST_RE = re.compile(r"^\s*([-*+]|\d+[.)])\s")


def _blank_mask(text: str) -> list[bool]:
    return [line.strip() == "" for line in text.split("\n")]


def _heading_levels(text: str) -> list[int]:
    return [len(m.group(1)) for line in text.split("\n") if (m := _HEADING_RE.match(line))]


def _list_markers(text: str) -> list[str]:
    return [m.group(1) for line in text.split("\n") if (m := _LIST_RE.match(line))]


def _pipe_counts(text: str) -> list[int]:
    return [line.count("|") for line in text.split("\n") if "|" in line]


def assert_structure_preserved(source: str, translated: str) -> None:
    """Assert the invariants the real pipeline validates."""
    assert translated.split("\n") and len(translated.split("\n")) == len(source.split("\n")), (
        "line count changed"
    )
    assert _blank_mask(translated) == _blank_mask(source), "blank-line separation changed"
    assert _heading_levels(translated) == _heading_levels(source), "heading levels changed"
    assert _list_markers(translated) == _list_markers(source), "list markers changed"
    assert _pipe_counts(translated) == _pipe_counts(source), "table pipes changed"
    assert fake.PLACEHOLDER_RE.findall(translated) == fake.PLACEHOLDER_RE.findall(source), (
        "placeholders changed"
    )
    assert translated != source, "no translation happened"
    assert "\u00ab" in translated, "translation marker missing"


# --------------------------------------------------------------------------- #
# Fake server: endpoints
# --------------------------------------------------------------------------- #


def test_health_props_and_models(start_server):
    base = start_server(fake.TranslationConfig(total_slots=4, n_ctx=32768))

    assert http_get(base, "/health") == {"status": "ok"}

    props = http_get(base, "/props")
    assert props["total_slots"] == 4
    assert props["n_ctx"] == 32768
    assert props["model_path"] == "fake"
    assert isinstance(props["default_generation_settings"], dict)
    assert props["default_generation_settings"]["n_ctx"] == 32768

    models = http_get(base, "/v1/models")
    assert models["object"] == "list"
    assert models["data"][0]["id"] == fake.FAKE_MODEL
    assert models["data"][0]["object"] == "model"


def test_tokenize_endpoints(start_server):
    base = start_server()

    short = http_post(base, "/tokenize", {"content": "hello world"})
    long = http_post(base, "/tokenize", {"content": "hello world and a much longer passage"})
    assert short["n_tokens"] > 0
    assert long["n_tokens"] >= short["n_tokens"], "token count must be monotonic in length"
    assert len(short["tokens"]) == short["n_tokens"]

    # both aliases agree and are deterministic
    a = http_post(base, "/tokenize", {"content": "same text"})
    b = http_post(base, "/v1/tokenize", {"content": "same text"})
    c = http_post(base, "/v1/tokenize", {"content": "same text"})
    assert a == b == c

    # empty input is zero tokens
    assert http_post(base, "/tokenize", {"content": ""})["n_tokens"] == 0

    # monotonic on the pure helper too
    assert (
        fake.approx_tokens("abcd") <= fake.approx_tokens("abcde") <= fake.approx_tokens("abcdefg")
    )


def test_chat_completions_non_streaming(start_server):
    base = start_server()
    response = http_post(
        base,
        "/v1/chat/completions",
        {"model": fake.FAKE_MODEL, "messages": [{"role": "user", "content": SAMPLE_INPUT}]},
    )
    choice = response["choices"][0]
    assert choice["message"]["role"] == "assistant"
    assert choice["finish_reason"] in ("stop", "length")
    content = choice["message"]["content"]
    assert content.strip()

    usage = response["usage"]
    assert usage["prompt_tokens"] > 0
    assert usage["completion_tokens"] == fake.approx_tokens(content)
    assert usage["total_tokens"] == usage["prompt_tokens"] + usage["completion_tokens"]

    assert_structure_preserved(SAMPLE_INPUT, content)


def test_translation_is_deterministic(start_server):
    base = start_server()
    payload = {"messages": [{"role": "user", "content": SAMPLE_INPUT}]}
    first = http_post(base, "/v1/chat/completions", payload)
    second = http_post(base, "/v1/chat/completions", payload)
    assert first["choices"][0]["message"]["content"] == second["choices"][0]["message"]["content"]


def test_sse_framing_is_well_formed(start_server):
    base = start_server()
    raw = http_post_text(
        base,
        "/v1/chat/completions",
        {"stream": True, "messages": [{"role": "user", "content": SAMPLE_INPUT}]},
    )

    assert raw.endswith("\n\n"), "SSE body must end with a blank line"
    events = [event for event in raw.split("\n\n") if event]
    assert events, "no SSE events"
    assert all(event.startswith("data: ") for event in events), "every event must be data:-prefixed"
    assert events[-1] == "data: [DONE]", "stream must be terminated by data: [DONE]"

    assembled = ""
    for event in events:
        if event == "data: [DONE]":
            continue
        payload = json.loads(event[len("data: ") :])
        assembled += payload["choices"][0]["delta"].get("content", "")

    # reassembling the stream gives the same text as the non-streaming endpoint
    non_stream = http_post(
        base, "/v1/chat/completions", {"messages": [{"role": "user", "content": SAMPLE_INPUT}]}
    )
    assert assembled == non_stream["choices"][0]["message"]["content"]
    assert_structure_preserved(SAMPLE_INPUT, assembled)


def test_json_response_format_mode(start_server):
    base = start_server()
    response = http_post(
        base,
        "/v1/chat/completions",
        {
            "messages": [{"role": "user", "content": "anything"}],
            "response_format": {"type": "json_schema", "json_schema": {"name": "editor"}},
        },
    )
    content = json.loads(response["choices"][0]["message"]["content"])
    assert content["verdict"] == "ok"
    assert content["issues"] == []


def test_book_profile_json_schema_returns_the_candidate_profile(start_server):
    """A ``book_profile`` schema request answers with the deterministic profile."""
    base = start_server()
    response = http_post(
        base,
        "/v1/chat/completions",
        {
            "messages": [{"role": "user", "content": "analyse this book"}],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": fake.BOOK_PROFILE_SCHEMA_NAME},
            },
        },
    )
    content = json.loads(response["choices"][0]["message"]["content"])
    assert content == fake.BOOK_PROFILE
    # Both glossary branches must be present for the pipeline tests.
    assert {entry["kind"] for entry in content["proper_nouns"]} == {
        "proper_noun",
        "do_not_translate",
    }


def test_chapter_summary_json_schema_returns_the_rolling_memory(start_server):
    """A ``chapter_summary`` schema request answers with the deterministic summary."""
    base = start_server()
    response = http_post(
        base,
        "/v1/chat/completions",
        {
            "messages": [{"role": "user", "content": "summarise this chapter"}],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": fake.CHAPTER_SUMMARY_SCHEMA_NAME},
            },
        },
    )
    content = json.loads(response["choices"][0]["message"]["content"])
    assert content == fake.CHAPTER_SUMMARY
    assert {entry["kind"] for entry in content["new_terms"]} == {
        "term",
        "do_not_translate",
    }
    assert content["style_notes"]


def test_editor_issue_mode_returns_a_proposed_correction(start_server):
    """``--editor-issues`` drives the suggestion pipeline deterministically."""
    base = start_server(fake.TranslationConfig(editor_issues=True))
    response = http_post(
        base,
        "/v1/chat/completions",
        {
            "messages": [{"role": "user", "content": "compare these"}],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": fake.EDITOR_SCHEMA_NAME},
            },
        },
    )
    content = json.loads(response["choices"][0]["message"]["content"])
    assert content == fake.EDITOR_ISSUES
    assert content["issues"][0]["block_index"] == 0


def test_proofreader_request_is_answered_unchanged(start_server):
    """A monolingual proofreader request gets the input back (no corrections)."""
    base = start_server()
    text = "Primo blocco.\n\n<!-- block -->\n\nSecondo blocco."
    response = http_post(
        base,
        "/v1/chat/completions",
        {
            "messages": [
                {"role": "system", "content": "You are a monolingual proofreader for Italian."},
                {"role": "user", "content": text},
            ],
        },
    )
    content = response["choices"][0]["message"]["content"]
    assert content == text


def test_recon_broken_answers_prose_not_json(start_server):
    """``--recon-broken`` exercises the control plane's parse-failure branch."""
    base = start_server(fake.TranslationConfig(recon_broken=True))
    response = http_post(
        base,
        "/v1/chat/completions",
        {
            "messages": [{"role": "user", "content": "analyse this book"}],
            "response_format": {
                "type": "json_schema",
                "json_schema": {"name": fake.BOOK_PROFILE_SCHEMA_NAME},
            },
        },
    )
    content = response["choices"][0]["message"]["content"]
    assert content == fake.RECON_BROKEN_TEXT
    with pytest.raises(json.JSONDecodeError):
        json.loads(content)


def test_default_returns_only_the_translated_passage(start_server):
    """A compliant model answers with the passage alone; the preface is not echoed."""
    base = start_server()
    message = f"CHAPTER: One\n\nPASSAGE TO TRANSLATE:\n{SAMPLE_INPUT}"
    content = http_post(
        base, "/v1/chat/completions", {"messages": [{"role": "user", "content": message}]}
    )["choices"][0]["message"]["content"]
    assert not content.startswith("CHAPTER: One"), "the preface must not be echoed"
    assert "PASSAGE TO TRANSLATE:" not in content
    # the passage keeps its leading newline, so compare against the exact source
    assert_structure_preserved("\n" + SAMPLE_INPUT, content)


def test_echo_prompt_prefix_fault_restores_the_preface(start_server):
    """``--echo-prompt-prefix`` reproduces the irregular answer the pipeline rejects."""
    base = start_server(fake.TranslationConfig(echo_prompt_prefix=True))
    message = f"CHAPTER: One\n\nPASSAGE TO TRANSLATE:\n{SAMPLE_INPUT}"
    content = http_post(
        base, "/v1/chat/completions", {"messages": [{"role": "user", "content": message}]}
    )["choices"][0]["message"]["content"]
    assert content.startswith("CHAPTER: One"), "the echo flag must restore the preface"
    translated_passage = content.split("PASSAGE TO TRANSLATE:", 1)[1]
    assert_structure_preserved("\n" + SAMPLE_INPUT, translated_passage)


# --------------------------------------------------------------------------- #
# Fake server: fault injection
# --------------------------------------------------------------------------- #


def test_fault_drop_placeholder():
    out = fake.translate(SAMPLE_INPUT, fake.TranslationConfig(drop_placeholder=1))
    assert "\u27e61\u27e7" not in out
    assert "\u27e62\u27e7" in out and "\u27e63\u27e7" in out


def test_fault_duplicate_placeholder():
    out = fake.translate(SAMPLE_INPUT, fake.TranslationConfig(duplicate_placeholder=2))
    assert out.count("\u27e62\u27e7") == 2
    assert out.count("\u27e61\u27e7") == 1


def test_fault_reorder_placeholders():
    out = fake.translate(SAMPLE_INPUT, fake.TranslationConfig(reorder_placeholders=True))
    source = fake.PLACEHOLDER_RE.findall(SAMPLE_INPUT)
    assert sorted(fake.PLACEHOLDER_RE.findall(out)) == sorted(source)
    assert fake.PLACEHOLDER_RE.findall(out) == list(reversed(source))


def test_fault_truncate():
    baseline = fake.translate(SAMPLE_INPUT)
    truncated = fake.translate(SAMPLE_INPUT, fake.TranslationConfig(truncate=0.5))
    assert len(truncated) < len(baseline)
    assert len(truncated) <= int(len(baseline) * 0.5) + 1


def test_fault_merge_paragraphs():
    merged = fake.translate(SAMPLE_INPUT, fake.TranslationConfig(merge_paragraphs=True))
    assert "\n\n" not in merged
    assert len(merged.split("\n")) < len(SAMPLE_INPUT.split("\n"))


def test_fault_drop_placeholder_over_http(start_server):
    base = start_server(fake.TranslationConfig(drop_placeholder=2))
    content = http_post(
        base, "/v1/chat/completions", {"messages": [{"role": "user", "content": SAMPLE_INPUT}]}
    )["choices"][0]["message"]["content"]
    assert "\u27e62\u27e7" not in content


def test_fault_fail_rate_returns_500(start_server):
    base = start_server(fake.TranslationConfig(fail_rate=1.0))
    with pytest.raises(urllib.error.HTTPError) as excinfo:
        http_post(base, "/v1/chat/completions", {"messages": [{"role": "user", "content": "x"}]})
    assert excinfo.value.code == 500


def test_fault_zero_fail_rate_is_healthy(start_server):
    base = start_server(fake.TranslationConfig(fail_rate=0.0))
    response = http_post(
        base, "/v1/chat/completions", {"messages": [{"role": "user", "content": "x"}]}
    )
    assert response["choices"][0]["message"]["content"]


def test_fault_delay(start_server):
    base = start_server(fake.TranslationConfig(delay_ms=80))
    started = time.monotonic()
    http_post(base, "/v1/chat/completions", {"messages": [{"role": "user", "content": "x"}]})
    assert time.monotonic() - started >= 0.05


def test_truncate_sets_length_finish_reason(start_server):
    base = start_server(fake.TranslationConfig(truncate=0.4))
    response = http_post(
        base, "/v1/chat/completions", {"messages": [{"role": "user", "content": SAMPLE_INPUT}]}
    )
    assert response["choices"][0]["finish_reason"] == "length"


def test_config_from_environment(monkeypatch):
    monkeypatch.setenv("FAKE_LLAMA_TOTAL_SLOTS", "7")
    monkeypatch.setenv("FAKE_LLAMA_N_CTX", "12345")
    monkeypatch.setenv("FAKE_LLAMA_DROP_PLACEHOLDER", "2")
    monkeypatch.setenv("FAKE_LLAMA_TRUNCATE", "0.5")
    monkeypatch.setenv("FAKE_LLAMA_MERGE_PARAGRAPHS", "1")
    monkeypatch.setenv("FAKE_LLAMA_FAIL_RATE", "0.25")
    monkeypatch.setenv("FAKE_LLAMA_ECHO_PROMPT_PREFIX", "1")

    args = fake.build_parser().parse_args([])
    cfg = fake.build_config(args)
    assert cfg.total_slots == 7
    assert cfg.n_ctx == 12345
    assert cfg.drop_placeholder == 2
    assert cfg.truncate == pytest.approx(0.5)
    assert cfg.merge_paragraphs is True
    assert cfg.fail_rate == pytest.approx(0.25)
    assert cfg.echo_prompt_prefix is True

    # the default keeps the compliant behaviour (no echoed preface)
    monkeypatch.delenv("FAKE_LLAMA_ECHO_PROMPT_PREFIX")
    assert fake.build_config(fake.build_parser().parse_args([])).echo_prompt_prefix is False

    # CLI flags win over the environment
    override = fake.build_config(fake.build_parser().parse_args(["--total-slots", "3"]))
    assert override.total_slots == 3


def test_selftest_passes():
    proc = subprocess.run(
        [sys.executable, str(TOOLS_DIR / "fake_llama_server.py"), "--selftest"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert proc.returncode == 0, proc.stderr
    assert "passed" in proc.stdout


def test_module_is_stdlib_only():
    """The fake server must not import third-party packages."""
    source = (TOOLS_DIR / "fake_llama_server.py").read_text(encoding="utf-8")
    for banned in (
        "import requests",
        "import flask",
        "import numpy",
        "import pymupdf",
        "import jinja2",
    ):
        assert banned not in source


# --------------------------------------------------------------------------- #
# Fixture generator
# --------------------------------------------------------------------------- #


def _run_make_fixtures(out_dir: Path, extra: list[str]) -> Path:
    proc = subprocess.run(
        [
            sys.executable,
            str(TOOLS_DIR / "make_fixtures.py"),
            "--out-dir",
            str(out_dir),
            "--quiet",
            *extra,
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=300,
    )
    assert proc.returncode == 0, proc.stderr
    return out_dir


@pytest.fixture(scope="session")
def small_fixtures(tmp_path_factory) -> Path:
    out = tmp_path_factory.mktemp("fixtures-small")
    return _run_make_fixtures(out, ["--small-only"])


@pytest.fixture(scope="session")
def large_fixtures(tmp_path_factory) -> Path:
    out = tmp_path_factory.mktemp("fixtures-large")
    return _run_make_fixtures(out, ["--large-only"])


def test_small_epub_is_valid_and_rich(small_fixtures):
    ebooklib = pytest.importorskip("ebooklib")
    from ebooklib import epub

    book = epub.read_epub(str(small_fixtures / "content.epub"))
    assert book.get_metadata("DC", "title")[0][0] == "The Lantern Keeper"

    documents = list(book.get_items_of_type(ebooklib.ITEM_DOCUMENT))
    images = list(book.get_items_of_type(ebooklib.ITEM_IMAGE))
    assert documents, "no document items"
    assert images, "no image items"

    joined = b"\n".join(document.get_content() for document in documents).decode("utf-8", "replace")
    assert "<h1" in joined and "<h2" in joined, "nested h1/h2 headings missing"
    assert "<table" in joined, "table missing"
    assert "<blockquote" in joined, "blockquote missing"
    assert "<pre" in joined, "fenced code block missing"
    assert "<img" in joined, "inline image missing"
    assert "noteref" in joined and "footnote" in joined, "footnote markup missing"


def test_small_pdf_is_valid(small_fixtures):
    pymupdf = pytest.importorskip("pymupdf")

    document = pymupdf.open(str(small_fixtures / "content.pdf"))
    try:
        assert document.page_count >= 2
        text = "\n".join(page.get_text() for page in document)
    finally:
        document.close()
    assert "Chapter One: The Harbour" in text
    assert "Stores" in text
    assert "Lamps" in text


def test_markdown_fixture_exercises_the_placeholder_layer(small_fixtures):
    markdown = (small_fixtures / "content.md").read_text(encoding="utf-8")
    assert "# The Lantern Keeper" in markdown
    assert "## Chapter One: The Harbour" in markdown
    assert "|:--------|-------:|:------:|" in markdown, "GFM alignment colons missing"
    assert "[^1]:" in markdown and "[^2]:" in markdown, "footnote definitions missing"
    assert "```sql" in markdown
    assert "![The harbour at dawn](harbour.png)" in markdown
    assert "> The wind rose" in markdown
    assert "**quiet**" in markdown and "*Nobody*" in markdown
    assert "[the northern tower](https://example.org/north)" in markdown
    assert "`keeper`" in markdown
    assert "$E = mc^2$" in markdown


def test_large_fixture_crosses_one_million_characters(large_fixtures):
    ebooklib = pytest.importorskip("ebooklib")
    from ebooklib import epub

    large = large_fixtures / "large.epub"
    assert large.exists() and large.stat().st_size > 0

    manifest = json.loads((large_fixtures / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["large_epub_chars"] >= 1_000_000

    # independently: the decompressed chapter markup also crosses 1M characters
    with zipfile.ZipFile(large) as archive:
        decompressed = sum(
            len(archive.read(info.filename))
            for info in archive.infolist()
            if info.filename.endswith((".xhtml", ".html"))
        )
    assert decompressed >= 1_000_000

    book = epub.read_epub(str(large))
    assert len(list(book.get_items_of_type(ebooklib.ITEM_DOCUMENT))) >= 20


def test_fixtures_are_deterministic(tmp_path):
    first = _run_make_fixtures(tmp_path / "run1", ["--small-only"])
    second = _run_make_fixtures(tmp_path / "run2", ["--small-only"])

    def digest(path: Path) -> str:
        return hashlib.sha256(path.read_bytes()).hexdigest()

    for name in ("content.epub", "content.md", "content.pdf", "harbour.png"):
        assert digest(first / name) == digest(second / name), f"{name} is not reproducible"


# --------------------------------------------------------------------------- #
# Prompt templates
# --------------------------------------------------------------------------- #

REPRESENTATIVE_CONTEXTS: dict[str, dict[str, object]] = {
    "translator.md": {
        "source_language": "English",
        "target_language": "Italian",
        "style_guide": "Keep the register formal.",
        "glossary": "lantern => lanterna",
        "book_title": "The Lantern Keeper",
        "book_author": "Fixture Author",
        "synopsis": "A keeper guards a harbour light.",
        "chapter_title": "Chapter One",
        "chapter_summary_so_far": "The keeper wakes.",
        "previous_context": "The harbour was quiet.",
        "text": "The harbour was quiet \u27e61\u27e7.",
    },
    "translator.table.md": {
        "source_language": "English",
        "target_language": "Italian",
        "style_guide": "Keep the register formal.",
        "glossary": "lantern => lanterna",
        "book_title": "The Lantern Keeper",
        "book_author": "Fixture Author",
        "synopsis": "A keeper guards a harbour light.",
        "chapter_title": "Chapter One",
        "chapter_summary_so_far": "",
        "previous_context": "",
        "text": "| Element | Number |\n|:--------|-------:|\n| Lamps   | 4 |",
    },
    "editor.md": {
        "source_language": "English",
        "target_language": "Italian",
        "source_text": "The harbour was quiet.",
        "target_text": "Il porto era tranquillo.",
        "response_schema": '{"type":"object"}',
    },
    "proofreader.md": {
        "source_language": "English",
        "target_language": "Italian",
        "text": "Il porto era tranquillo \u27e61\u27e7.",
    },
    "summarizer.md": {
        "source_language": "English",
        "target_language": "Italian",
        "chapter_title": "Chapter One",
        "excerpt": "The harbour was quiet that morning.",
    },
    "orchestrator.md": {
        "source_language": "English",
        "target_language": "Italian",
        "book_title": "The Lantern Keeper",
        "project_state": '{"pending_chunks": 12}',
    },
    "analyze_book.md": {
        "source_language": "English",
        "target_language": "Italian",
        "metadata": '{"title": "The Lantern Keeper", "author": "Fixture Author"}',
        "excerpts": "The harbour was quiet that morning.",
        "pasted_text": "",
    },
}


def test_prompt_templates_exist_and_have_headers():
    expected = {
        "translator.md",
        "translator.table.md",
        "editor.md",
        "proofreader.md",
        "summarizer.md",
        "orchestrator.md",
        "analyze_book.md",
    }
    available = {path.name for path in PROMPTS_DIR.glob("*.md")}
    assert expected <= available, f"missing: {expected - available}"

    for name in expected:
        text = (PROMPTS_DIR / name).read_text(encoding="utf-8")
        assert text.lstrip().startswith("<!--"), f"{name} must start with an HTML comment header"
        assert "USER-EDITABLE" in text, f"{name} header must say it is user-editable"
        assert USER_MARKER in text.split("\n"), f"{name} must contain the {USER_MARKER} marker line"


def test_prompt_templates_parse_and_render_without_undefined():
    jinja2 = pytest.importorskip("jinja2")

    strict = jinja2.Environment(undefined=jinja2.StrictUndefined, keep_trailing_newline=True)
    for name, context in REPRESENTATIVE_CONTEXTS.items():
        source = (PROMPTS_DIR / name).read_text(encoding="utf-8")
        template = jinja2.Environment(keep_trailing_newline=True).from_string(source)
        rendered = template.render(**context)
        assert "Undefined" not in rendered, f"{name} leaked an Undefined variable"
        # a complete context renders cleanly even under StrictUndefined
        strict_rendered = strict.from_string(source).render(**context)
        assert "Undefined" not in strict_rendered
        assert rendered.strip()


def test_translator_marker_splits_system_and_user():
    text = (PROMPTS_DIR / "translator.md").read_text(encoding="utf-8")
    system, _, user = text.partition(f"\n{USER_MARKER}\n")
    assert system and user, "marker did not split the file"
    # the volatile variables belong to the user half only
    assert "{{ text }}" not in system
    assert "{{ previous_context }}" not in system
    assert "{{ text }}" in user and "{{ chapter_title }}" in user
    # the stable variables live in the system half
    assert "{{ source_language }}" in system
    assert "{{ glossary }}" in system


def test_editor_schema_matches_plan():
    schema = json.loads((PROMPTS_DIR / "editor.schema.json").read_text(encoding="utf-8"))
    assert schema["type"] == "object"
    assert schema["required"] == ["verdict", "issues"]
    assert schema["properties"]["verdict"]["enum"] == ["ok", "needs_fix"]

    issue = schema["properties"]["issues"]["items"]
    assert issue["properties"]["severity"]["enum"] == ["critical", "major", "minor"]
    assert issue["properties"]["kind"]["enum"] == [
        "meaning",
        "omission",
        "addition",
        "terminology",
        "register",
        "markup",
        "placeholder",
    ]
    assert issue["required"] == ["block_index", "kind", "quote", "suggested", "reason"]


def test_analyze_book_marker_and_schema_match_plan():
    text = (PROMPTS_DIR / "analyze_book.md").read_text(encoding="utf-8")
    system, _, user = text.partition(f"\n{USER_MARKER}\n")
    assert system and user, "marker did not split the file"
    # The evidence is volatile and belongs to the user half only.
    assert "{{ metadata }}" in user and "{{ excerpts }}" in user and "{{ pasted_text }}" in user
    assert "{{ excerpts }}" not in system and "{{ pasted_text }}" not in system

    schema = json.loads((PROMPTS_DIR / "analyze_book.schema.json").read_text(encoding="utf-8"))
    assert schema["type"] == "object"
    assert schema["required"] == [
        "source_language",
        "genre",
        "audience",
        "era",
        "narrative_voice",
        "register",
        "style_notes",
        "themes",
        "synopsis",
        "proper_nouns",
        "field_basis",
    ]
    proper_noun = schema["properties"]["proper_nouns"]["items"]
    assert proper_noun["properties"]["kind"]["enum"] == ["proper_noun", "do_not_translate"]
    assert proper_noun["required"] == ["source", "kind"]
    # The caps exist in the schema; the control plane clamps again in Rust.
    assert schema["properties"]["synopsis"]["maxLength"] > 0
    assert schema["properties"]["style_notes"]["maxItems"] == 8


def test_summarizer_schema_matches_plan():
    schema = json.loads((PROMPTS_DIR / "summarizer.schema.json").read_text(encoding="utf-8"))
    assert schema["type"] == "object"
    assert schema["required"] == ["summary", "new_terms", "style_notes"]

    term = schema["properties"]["new_terms"]["items"]
    assert term["properties"]["kind"]["enum"] == ["term", "proper_noun", "do_not_translate"]
    assert term["required"] == ["source", "target", "kind"]
    assert schema["properties"]["new_terms"]["maxItems"] == 8
    assert schema["properties"]["style_notes"]["maxItems"] == 8


# --------------------------------------------------------------------------- #
# Pandoc assets
# --------------------------------------------------------------------------- #

PANDOC_MD = """\
---
title: "Pandoc Asset Test"
author: "Tester"
lang: en
toc: true
---

# Chapter One {#ch1}

A paragraph with a note.[^n] Another with a placeholder \u27e67\u27e7 to clean.

[^n]: The note body.

Table 1: Stores overview

| Element | Number |
|:--------|-------:|
| Lamps   |      4 |

<!-- b000001 -->

Some text [with a marker]{.block-marker} and an empty paragraph follows:

```python
print(1)
```

# Chapter Two {#ch2}

More text.[^m]

[^m]: A second note.
"""

PANDOC_CLEAN_MD = """\
---
title: "Pandoc PDF Test"
author: "Tester"
lang: en
toc: true
---

# Chapter One

A clean paragraph without placeholder tokens, followed by a table and a note.[^n]

[^n]: A clean note.

| Left | Right |
|:-----|------:|
| a    | 1     |
"""


def _run_pandoc(args: list[str], cwd: Path, timeout: int = 120) -> subprocess.CompletedProcess:
    return subprocess.run(
        [PANDOC_BIN, *args],
        cwd=str(cwd),
        capture_output=True,
        text=True,
        timeout=timeout,
    )


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_lua_filters_are_valid_and_apply(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_MD, encoding="utf-8")
    filters = {
        "footnotes": PANDOC_DIR / "filters" / "footnotes.lua",
        "tables": PANDOC_DIR / "filters" / "tables.lua",
        "epub_cleanup": PANDOC_DIR / "filters" / "epub_cleanup.lua",
    }
    for name, path in filters.items():
        assert path.exists(), f"{name} filter missing"
        proc = _run_pandoc(["-t", "native", "--lua-filter", str(path), "in.md"], tmp_path)
        assert proc.returncode == 0, f"{name} filter failed:\n{proc.stderr}"
        assert proc.stdout.strip(), f"{name} filter produced no output"


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_tables_filter_keeps_caption_and_alignment(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_MD, encoding="utf-8")
    proc = _run_pandoc(
        ["-t", "html5", "--lua-filter", str(PANDOC_DIR / "filters" / "tables.lua"), "in.md"],
        tmp_path,
    )
    assert proc.returncode == 0, proc.stderr
    assert "<caption>" in proc.stdout and "Table 1: Stores overview" in proc.stdout
    assert "text-align: right" in proc.stdout, "column alignment was lost"


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_footnotes_filter_default_and_endnotes(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_MD, encoding="utf-8")
    filter_path = str(PANDOC_DIR / "filters" / "footnotes.lua")

    keep = _run_pandoc(["-t", "html5", "--lua-filter", filter_path, "in.md"], tmp_path)
    assert keep.returncode == 0, keep.stderr
    assert 'class="footnote-ref"' in keep.stdout

    endnotes_src = PANDOC_MD.replace("lang: en", "lang: en\nfootnotes-endnotes: true")
    (tmp_path / "endnotes.md").write_text(endnotes_src, encoding="utf-8")
    moved = _run_pandoc(["-t", "html5", "--lua-filter", filter_path, "endnotes.md"], tmp_path)
    assert moved.returncode == 0, moved.stderr
    assert "Notes" in moved.stdout
    assert "[1]" in moved.stdout and "[2]" in moved.stdout
    assert 'class="footnote-ref"' not in moved.stdout


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_epub_cleanup_removes_artefacts(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_MD, encoding="utf-8")
    proc = _run_pandoc(
        ["-t", "html5", "--lua-filter", str(PANDOC_DIR / "filters" / "epub_cleanup.lua"), "in.md"],
        tmp_path,
    )
    assert proc.returncode == 0, proc.stderr
    assert "\u27e67\u27e7" not in proc.stdout, "placeholder leaked"
    assert "b000001" not in proc.stdout, "block marker leaked"
    assert "block-marker" not in proc.stdout, "marker span leaked"


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_book_html_template(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_MD, encoding="utf-8")
    out = tmp_path / "out.html"
    proc = _run_pandoc(
        [
            "-t",
            "html5",
            "-s",
            "--template",
            str(PANDOC_DIR / "templates" / "book.html"),
            "--css",
            str(PANDOC_DIR / "styles" / "book.css"),
            "-o",
            str(out),
            "in.md",
        ],
        tmp_path,
    )
    assert proc.returncode == 0, proc.stderr
    html = out.read_text(encoding="utf-8")
    assert 'id="title-block-header"' in html
    assert "book.css" in html
    assert "Chapter One" in html


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_book_tex_template_renders_to_latex(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_CLEAN_MD, encoding="utf-8")
    proc = _run_pandoc(
        [
            "-t",
            "latex",
            "--template",
            str(PANDOC_DIR / "templates" / "book.tex"),
            "--top-level-division=chapter",
            "in.md",
        ],
        tmp_path,
    )
    assert proc.returncode == 0, proc.stderr
    assert "\\documentclass" in proc.stdout
    assert "\\tableofcontents" in proc.stdout
    assert "A clean paragraph" in proc.stdout


@pytest.mark.skipif(PDF_ENGINE is None, reason="no LaTeX engine available")
def test_book_tex_template_builds_pdf(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_CLEAN_MD, encoding="utf-8")
    out = tmp_path / "book.pdf"
    proc = _run_pandoc(
        [
            "-o",
            str(out),
            "--template",
            str(PANDOC_DIR / "templates" / "book.tex"),
            "--pdf-engine",
            os.path.basename(PDF_ENGINE),
            "--toc",
            "--top-level-division=chapter",
            "in.md",
        ],
        tmp_path,
        timeout=300,
    )
    assert proc.returncode == 0, proc.stderr
    assert out.exists()
    assert out.read_bytes().startswith(b"%PDF")
    assert out.stat().st_size > 1024


@pytest.mark.skipif(PANDOC_BIN is None, reason="pandoc is not installed")
def test_epub_export_with_book_assets(tmp_path):
    (tmp_path / "in.md").write_text(PANDOC_CLEAN_MD, encoding="utf-8")
    out = tmp_path / "book.epub"
    proc = _run_pandoc(
        [
            "-o",
            str(out),
            "--template",
            str(PANDOC_DIR / "templates" / "book.html"),
            "--css",
            str(PANDOC_DIR / "styles" / "book.css"),
            "--lua-filter",
            str(PANDOC_DIR / "filters" / "footnotes.lua"),
            "--lua-filter",
            str(PANDOC_DIR / "filters" / "tables.lua"),
            "--lua-filter",
            str(PANDOC_DIR / "filters" / "epub_cleanup.lua"),
            "in.md",
        ],
        tmp_path,
    )
    assert proc.returncode == 0, proc.stderr
    assert out.exists() and out.stat().st_size > 0
    with zipfile.ZipFile(out) as archive:
        assert "mimetype" in archive.namelist()
