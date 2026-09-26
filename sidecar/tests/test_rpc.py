"""Tests for the JSON-RPC stdio transport.

Two layers are covered:

* in-process dispatch of every method, with inputs built from the project's own fixtures
  (``tools/make_fixtures.py``) so the pure modules get realistic Markdown/EPUB/PDF;
* a subprocess end-to-end run of ``python -m llmtranslator_sidecar`` with NDJSON piped in,
  asserting that stdout carries nothing but well-formed JSON and that the loop survives a
  malformed line, an unknown method and bad parameters.

The last test drives the whole M1 path over the wire -- parse -> chunk -> placeholders ->
(fake translation) -> reinject -> QA -> Pandoc -- because the wire contract, not the pure
modules, is what the Rust client deserialises.
"""

from __future__ import annotations

import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import TYPE_CHECKING, Any, cast

import pytest
from llmtranslator_sidecar import __version__, rpc
from llmtranslator_sidecar.parse import split_blocks

if TYPE_CHECKING:
    from collections.abc import Iterator

SIDECAR_DIR = Path(__file__).resolve().parents[1]
REPO_ROOT = SIDECAR_DIR.parent
MAKE_FIXTURES = REPO_ROOT / "tools" / "make_fixtures.py"

PANDOC = os.environ.get("LLMTRANSLATOR_PANDOC") or shutil.which("pandoc")
MARKER_INSTALLED = importlib.util.find_spec("marker") is not None


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _noop(*_args: object) -> None:
    """Progress sink that ignores its arguments."""


def call(method: str, params: dict[str, Any]) -> dict[str, Any]:
    """Dispatch a method in-process and return its result object."""
    result = rpc.dispatch(method, params)
    assert isinstance(result, dict)
    return cast("dict[str, Any]", result)


def _list(value: Any) -> list[Any]:
    assert isinstance(value, list), value
    return cast("list[Any]", value)


def _dict(value: Any) -> dict[str, Any]:
    assert isinstance(value, dict), value
    return cast("dict[str, Any]", value)


def _ping_frame(request_id: int) -> str:
    return json.dumps({"jsonrpc": "2.0", "id": request_id, "method": "ping", "params": {}})


@pytest.fixture(scope="module")
def fixtures(tmp_path_factory: pytest.TempPathFactory) -> Path:
    """Build the small fixtures once per module and return their directory."""
    out_dir = tmp_path_factory.mktemp("rpc-fixtures")
    process = subprocess.run(
        [sys.executable, str(MAKE_FIXTURES), "--out-dir", str(out_dir), "--small-only", "--quiet"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        timeout=300,
    )
    assert process.returncode == 0, process.stderr
    return out_dir


def _ingest(fixtures: Path, tmp_path: Path, name: str) -> dict[str, Any]:
    return call(
        "ingest",
        {"path": str(fixtures / name), "work_dir": str(tmp_path / "work")},
    )


def _blocks(fixtures: Path, tmp_path: Path) -> list[Any]:
    ingested = _ingest(fixtures, tmp_path, "content.md")
    parsed = call("parse_document", {"markdown_path": ingested["markdown_path"]})
    return _list(parsed["blocks"])


# ---------------------------------------------------------------------------
# In-process dispatch: one test per method
# ---------------------------------------------------------------------------


def test_ping_reports_runtime_identity() -> None:
    result = call("ping", {})
    assert result["pong"] is True
    assert result["version"] == __version__
    assert isinstance(result["python"], str)
    assert result["python"].startswith("3.")
    assert isinstance(result["platform"], str)


def test_detect_format(fixtures: Path) -> None:
    assert call("detect_format", {"path": str(fixtures / "content.md")}) == {
        "format": "markdown",
        "backends": ["source"],
    }
    assert call("detect_format", {"path": str(fixtures / "content.epub")})["format"] == "epub"
    assert call("detect_format", {"path": str(fixtures / "content.pdf")})["format"] == "pdf"


def test_ingest_emits_progress_and_writes_markdown(fixtures: Path, tmp_path: Path) -> None:
    events: list[dict[str, Any]] = []

    def notify(phase: str, done: int, total: int, message: str) -> None:
        events.append({"phase": phase, "done": done, "total": total, "message": message})

    result = cast(
        "dict[str, Any]",
        rpc.dispatch(
            "ingest",
            {"path": str(fixtures / "content.epub"), "work_dir": str(tmp_path)},
            notify,
        ),
    )

    markdown_path = result["markdown_path"]
    assert isinstance(markdown_path, str)
    assert Path(markdown_path).is_file()
    assert isinstance(result["metadata"], dict)
    assert isinstance(result["warnings"], list)

    chapters = _list(result["chapters"])
    assert chapters
    assert set(_dict(chapters[0])) == {"title", "level", "order"}

    assert events, "ingest must report progress"
    assert events[0]["phase"] == "ingest"
    for event in events:
        assert set(event) == {"phase", "done", "total", "message"}
        assert 0 <= event["done"] <= event["total"]


def test_ingest_reports_media_with_the_exact_result_keys(fixtures: Path, tmp_path: Path) -> None:
    ingested = _ingest(fixtures, tmp_path, "content.epub")

    # The Rust client deserialises these keys; the shape must not drift.
    assert set(ingested) == {
        "markdown_path",
        "metadata",
        "chapters",
        "warnings",
        "assets_dir",
        "assets",
    }
    assert ingested["assets"] == ["assets/harbour.png"]
    assert ingested["assets_dir"] == str(tmp_path / "work" / "assets")
    assert Path(str(ingested["assets_dir"]), "harbour.png").is_file()
    assert "assets/harbour.png" in Path(str(ingested["markdown_path"])).read_text("utf-8")


def test_ingest_reports_no_media_for_a_source_without_any(
    fixtures: Path,
    tmp_path: Path,
) -> None:
    ingested = _ingest(fixtures, tmp_path, "content.md")
    assert ingested["assets"] == []
    assert ingested["assets_dir"] is None


def test_front_matter_is_not_handed_to_the_model(tmp_path: Path) -> None:
    source = tmp_path / "book.md"
    source.write_text(
        '---\ntitle: "The Lantern Keeper"\nauthor: Fixture Author\n---\n\n# Title\n\nBody text.\n',
        encoding="utf-8",
    )
    ingested = call("ingest", {"path": str(source), "work_dir": str(tmp_path / "work")})
    parsed = call("parse_document", {"markdown_path": ingested["markdown_path"]})

    blocks = [_dict(block) for block in _list(parsed["blocks"])]
    front = next(block for block in blocks if block["kind"] == "frontmatter")
    assert front["translatable"] is False

    built = call("build_chunks", {"blocks": blocks, "budget_tokens": 1000})
    chunks = [_dict(chunk) for chunk in _list(built["chunks"])]
    front_chunk = next(chunk for chunk in chunks if front["id"] in chunk["block_ids"])

    # The block sits alone and costs nothing of the model's budget: the chunker gives a
    # non-translatable block an empty send, so the YAML never consumes context.
    assert front_chunk["block_ids"] == [front["id"]]
    assert front_chunk["token_estimate"] == 0
    assert "frontmatter" in _list(front_chunk["flags"])

    # prepare_text allocates no placeholder for the YAML either, so there is no token in
    # the metadata the model could be asked to rewrite.
    prepared = call(
        "prepare_text",
        {"text": front_chunk["source_md"], "block_ids": front_chunk["block_ids"]},
    )
    assert prepared["placeholders"] == []
    assert prepared["llm_text"] == front["source_md"]
    assert prepared["used_blocks"] == 1


def test_parse_document_block_and_chapter_shape(fixtures: Path, tmp_path: Path) -> None:
    ingested = _ingest(fixtures, tmp_path, "content.md")
    parsed = call("parse_document", {"markdown_path": ingested["markdown_path"]})

    blocks = _list(parsed["blocks"])
    chapters = _list(parsed["chapters"])
    assert blocks and chapters

    assert set(_dict(blocks[0])) == {
        "id",
        "chapter_id",
        "order",
        "kind",
        "level",
        "source_md",
        "source_text",
        "translatable",
        "attrs",
        "content_hash",
    }
    assert set(_dict(chapters[0])) == {
        "id",
        "order",
        "title",
        "level",
        "block_first",
        "block_last",
    }


def test_build_chunks_preserves_every_block(fixtures: Path, tmp_path: Path) -> None:
    blocks = _blocks(fixtures, tmp_path)
    result = call("build_chunks", {"blocks": blocks, "budget_tokens": 1000})

    chunks = _list(result["chunks"])
    assert chunks
    assert set(_dict(chunks[0])) == {
        "id",
        "chapter_id",
        "order",
        "block_ids",
        "source_md",
        "token_estimate",
        "context_carrier",
        "flags",
    }

    collected = [str(bid) for chunk in chunks for bid in _list(_dict(chunk)["block_ids"])]
    assert sorted(collected) == sorted(str(_dict(block)["id"]) for block in blocks)
    assert all(isinstance(_dict(chunk)["token_estimate"], int) for chunk in chunks)


def test_prepare_text_returns_tokens_and_block_count() -> None:
    result = call(
        "prepare_text",
        {"text": "**The** harbour is `quiet`.", "block_ids": ["b000000", "b000001"]},
    )
    assert result["used_blocks"] == 2
    assert isinstance(result["llm_text"], str)

    pairs = _list(result["placeholders"])
    assert pairs
    for pair in pairs:
        entry = _list(pair)
        assert len(entry) == 2
        assert isinstance(entry[0], int)
        assert isinstance(entry[1], str)
    assert "\u27e61\u27e7" in result["llm_text"]


def test_prepare_text_without_block_ids_reports_null() -> None:
    result = call("prepare_text", {"text": "plain text"})
    assert result["used_blocks"] is None
    assert result["placeholders"] == []


def test_reinject_restores_literals_and_reparses_blocks() -> None:
    source = "# Title\n\n**The** harbour is `quiet` and dark.\n"
    prepared = call("prepare_text", {"text": source})
    translated = str(prepared["llm_text"]).replace("harbour", "porto")

    expected_blocks = len(split_blocks(source))
    result = call(
        "reinject",
        {
            "text": translated,
            "placeholders": prepared["placeholders"],
            "expected_blocks": expected_blocks,
        },
    )

    assert result["placeholders_ok"] is True
    assert result["missing"] == []
    assert result["duplicated"] == []
    assert result["block_count_ok"] is True

    blocks_md = _list(result["blocks_md"])
    assert len(blocks_md) == expected_blocks
    assert "**The**" in "\n\n".join(str(block) for block in blocks_md)


def test_reinject_reports_missing_placeholders() -> None:
    prepared = call("prepare_text", {"text": "**The** tower."})
    stripped = re.sub("\u27e6\\s*\\d+\\s*\u27e7", "", str(prepared["llm_text"]))

    result = call(
        "reinject",
        {"text": stripped, "placeholders": prepared["placeholders"], "expected_blocks": 1},
    )
    assert result["placeholders_ok"] is False
    assert _list(result["missing"]) == [1, 2]


def test_qa_check_returns_typed_findings() -> None:
    result = call(
        "qa_check",
        {
            "source_text": "The harbour was quiet.",
            "target_text": "La baia era tranquilla.",
            "glossary": {"harbour": "porto"},
            "placeholders": [],
        },
    )
    findings = _list(result["findings"])
    assert findings
    assert "glossary_mismatch" in [_dict(finding)["kind"] for finding in findings]
    for finding in findings:
        assert set(_dict(finding)) == {"kind", "severity", "block_id", "details"}


def test_qa_check_reports_an_empty_target() -> None:
    result = call(
        "qa_check",
        {
            "source_text": "Some source text.",
            "target_text": "   ",
            "glossary": {},
            "placeholders": [],
        },
    )
    findings = _list(result["findings"])
    assert [_dict(finding)["kind"] for finding in findings] == ["empty"]
    assert _dict(findings[0])["severity"] == "critical"


def test_estimate_tokens() -> None:
    result = call("estimate_tokens", {"texts": ["", "abc", "hello world"]})
    assert result["counts"] == [0, 1, 3]


@pytest.mark.skipif(PANDOC is None, reason="pandoc binary is not available")
def test_pandoc_build_produces_output(tmp_path: Path) -> None:
    unit = tmp_path / "one.md"
    unit.write_text("# Chapter One\n\nThe harbour was quiet.\n", encoding="utf-8")
    output = tmp_path / "book.html"

    result = call(
        "pandoc_build",
        {
            "units": [{"path": str(unit), "title": "Chapter One"}],
            "metadata": {"title": "The Lantern Keeper"},
            "output_path": str(output),
            "output_format": "html",
        },
    )

    assert result["output_path"] == str(output)
    assert isinstance(result["log"], str)
    assert isinstance(result["duration_ms"], int)
    assert output.is_file()
    assert "Chapter One" in output.read_text(encoding="utf-8")


def test_pandoc_build_forwards_the_resource_path(monkeypatch: pytest.MonkeyPatch) -> None:
    """An optional parameter must reach the module, not be dropped by the dispatcher."""
    captured: dict[str, Any] = {}

    def fake_build(**kwargs: Any) -> dict[str, Any]:
        captured.update(kwargs)
        return {"output_path": "book.epub", "log": "", "duration_ms": 0}

    monkeypatch.setattr(rpc.pandoc, "build", fake_build)
    params: dict[str, Any] = {
        "units": [{"path": "one.md", "title": "Chapter One"}],
        "metadata": {"title": "The Lantern Keeper"},
        "output_path": "book.epub",
        "output_format": "epub",
    }

    call("pandoc_build", params)
    assert captured["resource_path"] == []
    assert captured["toc"] is False
    assert captured["lua_filters"] == []
    assert captured["top_level_division"] is None

    params["resource_path"] = ["/work"]
    params["toc"] = True
    params["lua_filters"] = ["/f/footnotes.lua"]
    params["top_level_division"] = "chapter"
    call("pandoc_build", params)
    assert captured["resource_path"] == ["/work"]
    assert captured["toc"] is True
    assert captured["lua_filters"] == ["/f/footnotes.lua"]
    assert captured["top_level_division"] == "chapter"


# ---------------------------------------------------------------------------
# Error mapping
# ---------------------------------------------------------------------------


def test_unknown_method_maps_to_method_not_found() -> None:
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch("does_not_exist", {})
    assert info.value.code == -32601


def test_missing_parameter_maps_to_invalid_params() -> None:
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch("detect_format", {})
    assert info.value.code == -32602


def test_wrong_parameter_type_maps_to_invalid_params() -> None:
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch("estimate_tokens", {"texts": "not-an-array"})
    assert info.value.code == -32602


def test_non_positive_budget_maps_to_invalid_params() -> None:
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch("build_chunks", {"blocks": [], "budget_tokens": 0})
    assert info.value.code == -32602


def test_extraction_error_maps_to_1001(tmp_path: Path) -> None:
    mystery = tmp_path / "notes.xyz"
    mystery.write_text("no signature, no known extension\n", encoding="utf-8")
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch("detect_format", {"path": str(mystery)})
    assert info.value.code == 1001


def test_missing_source_file_maps_to_internal_error(tmp_path: Path) -> None:
    missing = tmp_path / "absent.md"
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch("parse_document", {"markdown_path": str(missing)})
    assert info.value.code == -32603
    assert _dict(info.value.data)["path"] == str(missing)


@pytest.mark.skipif(MARKER_INSTALLED, reason="marker is installed, so it cannot be missing")
def test_missing_pdf_backend_maps_to_1003(fixtures: Path, tmp_path: Path) -> None:
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch(
            "ingest",
            {
                "path": str(fixtures / "content.pdf"),
                "work_dir": str(tmp_path),
                "pdf_backend": "marker",
            },
        )
    assert info.value.code == 1003
    assert info.value.data == {"backend": "marker"}


def test_pandoc_missing_binary_maps_to_1003(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    monkeypatch.setenv("LLMTRANSLATOR_PANDOC", "definitely-not-a-real-pandoc-binary")
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch(
            "pandoc_build",
            {
                "units": [],
                "metadata": {},
                "output_path": str(tmp_path / "book.html"),
                "output_format": "html",
            },
        )
    assert info.value.code == 1003


@pytest.mark.skipif(PANDOC is None, reason="pandoc binary is not available")
def test_pandoc_failure_maps_to_1002_carrying_the_log(tmp_path: Path) -> None:
    unit = tmp_path / "one.md"
    unit.write_text("Body.\n", encoding="utf-8")
    with pytest.raises(rpc.JsonRpcError) as info:
        rpc.dispatch(
            "pandoc_build",
            {
                "units": [{"path": str(unit), "title": "Chapter"}],
                "metadata": {},
                "output_path": str(tmp_path / "book.html"),
                "output_format": "html",
                "template": str(tmp_path / "does-not-exist.html"),
            },
        )
    assert info.value.code == 1002
    assert "log" in _dict(info.value.data)


# ---------------------------------------------------------------------------
# Framing, in-process
# ---------------------------------------------------------------------------


def test_process_line_round_trips_a_request() -> None:
    response = rpc.process_line(_ping_frame(7), _noop)
    assert response is not None
    assert response["jsonrpc"] == "2.0"
    assert response["id"] == 7
    assert _dict(response["result"])["pong"] is True


def test_process_line_malformed_json_is_parse_error_with_null_id() -> None:
    response = rpc.process_line("{not json", _noop)
    assert response is not None
    assert response["id"] is None
    assert _dict(response["error"])["code"] == -32700


def test_process_line_notification_gets_no_response() -> None:
    frame = json.dumps({"jsonrpc": "2.0", "method": "ping", "params": {}})
    assert rpc.process_line(frame, _noop) is None


def test_serve_ignores_blank_lines_and_survives_a_bad_line() -> None:
    reader = io.StringIO("\n   \n" + _ping_frame(1) + "\n{broken\n")
    writer = io.StringIO()

    assert rpc.serve(reader, writer) == 0

    lines = [line for line in writer.getvalue().split("\n") if line]
    assert len(lines) == 2
    first = _dict(json.loads(lines[0]))
    second = _dict(json.loads(lines[1]))
    assert first["id"] == 1
    assert second["id"] is None
    assert _dict(second["error"])["code"] == -32700


# ---------------------------------------------------------------------------
# Subprocess end-to-end
# ---------------------------------------------------------------------------


def test_subprocess_session_correlates_ids_and_survives_errors(fixtures: Path) -> None:
    frames = [
        _ping_frame(1),
        "",
        "   ",
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "estimate_tokens",
                "params": {"texts": ["", "hello world"]},
            }
        ),
        "this is not json",
        json.dumps({"jsonrpc": "2.0", "id": 3, "method": "does_not_exist", "params": {}}),
        json.dumps({"jsonrpc": "2.0", "id": 4, "method": "detect_format", "params": {}}),
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 5,
                "method": "detect_format",
                "params": {"path": str(fixtures / "content.md")},
            }
        ),
        json.dumps(
            {
                "jsonrpc": "2.0",
                "id": 6,
                "method": "prepare_text",
                "params": {"text": "**citt\u00e0**"},
            }
        ),
    ]

    process = subprocess.run(
        [sys.executable, "-m", "llmtranslator_sidecar"],
        cwd=SIDECAR_DIR,
        input="\n".join(frames) + "\n",
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=120,
    )
    assert process.returncode == 0, process.stderr

    # Every stdout line is one complete JSON object: a stray print would break this parse.
    lines = [line for line in process.stdout.split("\n") if line]
    responses = [cast("dict[str, Any]", json.loads(line)) for line in lines]
    assert len(responses) == 7

    by_id = {response.get("id"): response for response in responses}
    assert _dict(by_id[1]["result"])["pong"] is True
    assert _dict(by_id[2]["result"])["counts"] == [0, 3]
    assert _dict(by_id[None]["error"])["code"] == -32700
    assert _dict(by_id[3]["error"])["code"] == -32601
    assert _dict(by_id[4]["error"])["code"] == -32602
    assert _dict(by_id[5]["result"])["format"] == "markdown"
    assert "citt\u00e0" in str(_dict(by_id[6]["result"])["llm_text"])


class SidecarSession:
    """Interactive NDJSON client over a real ``python -m llmtranslator_sidecar``."""

    def __init__(self, cwd: Path) -> None:
        self._process: subprocess.Popen[str] = subprocess.Popen(
            [sys.executable, "-m", "llmtranslator_sidecar"],
            cwd=str(cwd),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            encoding="utf-8",
        )
        self._next_id = 0
        self.notifications: list[dict[str, Any]] = []

    def call(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        response = self._exchange(method, params)
        result = response.get("result")
        assert isinstance(result, dict), response
        return cast("dict[str, Any]", result)

    def _exchange(self, method: str, params: dict[str, Any]) -> dict[str, Any]:
        self._next_id += 1
        request_id = self._next_id
        stdin = self._process.stdin
        stdout = self._process.stdout
        assert stdin is not None
        assert stdout is not None
        request = {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        stdin.write(json.dumps(request) + "\n")
        stdin.flush()
        while True:
            raw = stdout.readline()
            assert raw != "", "the sidecar closed stdout before answering"
            message = cast("dict[str, Any]", json.loads(raw))
            if "id" not in message:
                self.notifications.append(message)
                continue
            assert message["id"] == request_id
            return message

    def close(self) -> int:
        stdin = self._process.stdin
        stdout = self._process.stdout
        assert stdin is not None
        # Closing stdin is the EOF the server waits for; close stdout after the wait so no
        # TextIOWrapper is left for the garbage collector to report as unclosed.
        stdin.close()
        code = self._process.wait(timeout=60)
        if stdout is not None:
            stdout.close()
        return code


@pytest.fixture
def session() -> Iterator[SidecarSession]:
    client = SidecarSession(SIDECAR_DIR)
    try:
        yield client
    finally:
        assert client.close() == 0


def _translate_one_chunk(
    session: SidecarSession,
    chunk: Any,
    tmp_path: Path,
    index: int,
) -> dict[str, str]:
    """Run one chunk through prepare -> fake translation -> reinject -> QA; return its unit."""
    chunk_data = _dict(chunk)
    block_ids = _list(chunk_data["block_ids"])

    prepared = session.call(
        "prepare_text",
        {"text": chunk_data["source_md"], "block_ids": block_ids},
    )
    assert prepared["used_blocks"] == len(block_ids)
    llm_text = prepared["llm_text"]
    assert isinstance(llm_text, str)
    assert llm_text

    # Stand in for the model: change the prose, keep the placeholder tokens verbatim.
    reinjected = session.call(
        "reinject",
        {
            "text": llm_text.upper(),
            "placeholders": prepared["placeholders"],
            "expected_blocks": len(block_ids),
        },
    )
    assert reinjected["placeholders_ok"] is True
    assert reinjected["missing"] == []
    assert reinjected["duplicated"] == []
    assert reinjected["block_count_ok"] is True

    blocks_md = _list(reinjected["blocks_md"])
    assert len(blocks_md) == len(block_ids)
    target_text = "\n\n".join(str(block) for block in blocks_md)

    qa_result = session.call(
        "qa_check",
        {
            "source_text": chunk_data["source_md"],
            "target_text": target_text,
            "glossary": {"harbour": "porto"},
            "placeholders": prepared["placeholders"],
        },
    )
    assert isinstance(qa_result["findings"], list)

    unit = tmp_path / f"unit_{index}.md"
    unit.write_text(target_text + "\n", encoding="utf-8")
    return {"path": str(unit), "title": f"Chapter {index}"}


def test_pipeline_roundtrip_over_the_wire(
    fixtures: Path, tmp_path: Path, session: SidecarSession
) -> None:
    ingested = session.call(
        "ingest", {"path": str(fixtures / "content.md"), "work_dir": str(tmp_path / "work")}
    )
    markdown_path = ingested["markdown_path"]
    assert isinstance(markdown_path, str)
    assert Path(markdown_path).is_file()

    parsed = session.call("parse_document", {"markdown_path": markdown_path})
    blocks = _list(parsed["blocks"])
    assert blocks

    built = session.call("build_chunks", {"blocks": blocks, "budget_tokens": 1000})
    chunks = _list(built["chunks"])
    assert chunks

    collected = [str(bid) for chunk in chunks for bid in _list(_dict(chunk)["block_ids"])]
    assert sorted(collected) == sorted(str(_dict(block)["id"]) for block in blocks)

    units = [
        _translate_one_chunk(session, chunk, tmp_path, index) for index, chunk in enumerate(chunks)
    ]

    assert session.notifications, "ingest must have produced progress notifications"
    for notification in session.notifications:
        assert notification["method"] == "progress"
        params = _dict(notification["params"])
        assert set(params) == {"phase", "done", "total", "message"}
        assert isinstance(params["done"], int)
        assert isinstance(params["total"], int)
        assert 0 <= params["done"] <= params["total"]

    if PANDOC is None:
        return
    output = tmp_path / "book.html"
    built_doc = session.call(
        "pandoc_build",
        {
            "units": units,
            "metadata": {"title": "The Lantern Keeper"},
            "output_path": str(output),
            "output_format": "html",
        },
    )
    assert built_doc["output_path"] == str(output)
    assert isinstance(built_doc["duration_ms"], int)
    assert output.is_file()
