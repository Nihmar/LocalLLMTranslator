"""JSON-RPC 2.0 transport for the sidecar: NDJSON over stdio.

The process speaks one JSON object per line on stdin and answers with one JSON object per
line on stdout. **stdout is the protocol channel**: nothing else may ever write to it.
Diagnostics go to stderr, and :func:`run_stdio` rebinds ``sys.stdout`` to stderr and writes
replies through a private, explicitly UTF-8 stream, so a stray ``print`` from any library
cannot corrupt the wire.

Requests are dispatched **sequentially, in a single loop**, and deliberately so. Two of the
pure modules temporarily repoint process-global stdio state while they run
(:mod:`~llmtranslator_sidecar.extractors.pdf_pymupdf` redirects both PyMuPDF's message sink
and ``sys.stdout``): with a thread pool a second request could observe or restore that state
mid-flight. Sequential dispatch also guarantees a progress notification is only ever written
between two complete lines, which is what keeps the stream parsable. The client correlates
replies by ``id``, so ordering is not required for correctness — the single loop is a safety
choice, not a performance one, and M1 does not need parallel sidecar execution because Rust
owns concurrency.
"""

from __future__ import annotations

import json
import logging
import os
import platform
import sys
from collections.abc import Callable
from pathlib import Path
from typing import NoReturn, TextIO, cast

from . import __version__, chunker, extractors, pandoc, parse, placeholders, qa
from .blocks import Block

#: Module logger; the root logger is configured once in :func:`run_stdio`.
logger = logging.getLogger(__name__)

#: JSON-RPC protocol version carried on every frame.
JSONRPC_VERSION = "2.0"

#: JSON-RPC error codes plus the sidecar's domain codes (see PLAN.md §12.1).
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603

#: Notification method used to report progress on a long call.
PROGRESS_METHOD = "progress"


type JsonObject = dict[str, object]
type ProgressSink = Callable[[str, int, int, str], None]
type Handler = Callable[[JsonObject, ProgressSink], object]


class JsonRpcError(Exception):
    """An error that maps onto a JSON-RPC ``error`` object."""

    code: int = INTERNAL_ERROR

    def __init__(
        self,
        message: str,
        *,
        code: int | None = None,
        data: JsonObject | None = None,
    ) -> None:
        super().__init__(message)
        self.message = message
        if code is not None:
            self.code = code
        self.data = data


class InvalidParamsError(JsonRpcError):
    """The request well-formed but its parameters are missing or of the wrong type."""

    code = INVALID_PARAMS


def _noop_progress(*_args: object) -> None:
    """Default sink for in-process callers; the stdio loop passes its line writer."""


def progress_notification(phase: str, done: int, total: int, message: str) -> JsonObject:
    """Build a server -> client progress notification."""
    return {
        "jsonrpc": JSONRPC_VERSION,
        "method": PROGRESS_METHOD,
        "params": {"phase": phase, "done": done, "total": total, "message": message},
    }


# ---------------------------------------------------------------------------
# Parameter validation
# ---------------------------------------------------------------------------


def _fail(message: str) -> NoReturn:
    """Raise a parameter error; a dedicated raiser keeps every message at the call site."""
    raise InvalidParamsError(message)


def _require_str(params: JsonObject, key: str) -> str:
    value = params.get(key)
    if not isinstance(value, str):
        _fail(f"parameter {key!r} must be a string")
    return value


def _optional_str(params: JsonObject, key: str) -> str | None:
    value = params.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        _fail(f"parameter {key!r} must be a string when present")
    return value


def _optional_bool(params: JsonObject, key: str, *, default: bool = False) -> bool:
    value = params.get(key)
    if value is None:
        return default
    if not isinstance(value, bool):
        _fail(f"parameter {key!r} must be a boolean when present")
    return value


def _require_int(params: JsonObject, key: str) -> int:
    value = params.get(key)
    # bool is a subclass of int; a JSON true/false is not an integer here.
    if isinstance(value, bool) or not isinstance(value, int):
        _fail(f"parameter {key!r} must be an integer")
    return value


def _require_list(params: JsonObject, key: str) -> list[object]:
    value = params.get(key)
    if not isinstance(value, list):
        _fail(f"parameter {key!r} must be an array")
    return cast("list[object]", value)


def _require_object(params: JsonObject, key: str) -> JsonObject:
    value = params.get(key)
    if not isinstance(value, dict):
        _fail(f"parameter {key!r} must be an object")
    return cast("JsonObject", value)


def _coerce_str_list(value: object, key: str) -> list[str]:
    if not isinstance(value, list):
        _fail(f"parameter {key!r} must be an array of strings")
    items: list[str] = []
    for item in cast("list[object]", value):
        if not isinstance(item, str):
            _fail(f"parameter {key!r} must contain only strings")
        items.append(item)
    return items


def _require_str_list(params: JsonObject, key: str) -> list[str]:
    value = params.get(key)
    if value is None:
        _fail(f"parameter {key!r} is required")
    return _coerce_str_list(value, key)


def _optional_str_list(params: JsonObject, key: str) -> list[str] | None:
    value = params.get(key)
    if value is None:
        return None
    return _coerce_str_list(value, key)


def _require_str_map(params: JsonObject, key: str) -> dict[str, str]:
    value = params.get(key)
    if not isinstance(value, dict):
        _fail(f"parameter {key!r} must be an object of strings")
    mapping: dict[str, str] = {}
    for item_key, item_value in cast("JsonObject", value).items():
        if not isinstance(item_value, str):
            _fail(f"parameter {key!r} must map strings to strings")
        mapping[item_key] = item_value
    return mapping


#: A placeholder entry is exactly ``[index, literal]``.
_PLACEHOLDER_PAIR_SIZE = 2


def _placeholders_from(params: JsonObject) -> list[list[object]]:
    """Validate the ``[[index, literal], ...]`` placeholder map shared by several methods."""
    raw = _require_list(params, "placeholders")
    pairs: list[list[object]] = []
    for item in raw:
        if not isinstance(item, list):
            _fail("each placeholder entry must be [integer, string]")
        pair = cast("list[object]", item)
        if (
            len(pair) != _PLACEHOLDER_PAIR_SIZE
            or isinstance(pair[0], bool)
            or not isinstance(pair[0], int)
            or not isinstance(pair[1], str)
        ):
            _fail("each placeholder entry must be [integer, string]")
        pairs.append([pair[0], pair[1]])
    return pairs


def _blocks_from(params: JsonObject) -> list[Block]:
    raw = _require_list(params, "blocks")
    blocks: list[Block] = []
    for item in raw:
        if not isinstance(item, dict):
            _fail("each block must be an object")
        try:
            blocks.append(Block.from_json(cast("JsonObject", item)))
        except (KeyError, TypeError, ValueError) as exc:
            _fail(f"invalid block: {exc}")
    return blocks


def _units_from(params: JsonObject) -> list[dict[str, object]]:
    raw = _require_list(params, "units")
    units: list[dict[str, object]] = []
    for item in raw:
        if not isinstance(item, dict):
            _fail("each unit must be an object")
        unit = cast("JsonObject", item)
        path = unit.get("path")
        if not isinstance(path, str):
            _fail("each unit requires a string 'path'")
        title = unit.get("title", "")
        if not isinstance(title, str):
            _fail("unit 'title' must be a string when present")
        units.append({"path": path, "title": title})
    return units


# ---------------------------------------------------------------------------
# Method handlers -- all delegate to the pure modules, none re-implements them
# ---------------------------------------------------------------------------


def _handle_ping(_params: JsonObject, _notify: ProgressSink) -> JsonObject:
    return {
        "pong": True,
        "version": __version__,
        "python": platform.python_version(),
        "platform": sys.platform,
    }


def _handle_detect_format(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    return extractors.inspect(_require_str(params, "path"))


def _handle_ingest(params: JsonObject, notify: ProgressSink) -> JsonObject:
    path = _require_str(params, "path")
    work_dir = _require_str(params, "work_dir")
    pdf_backend = _optional_str(params, "pdf_backend")
    notify("ingest", 0, 1, f"extracting {Path(path).name}")
    result = extractors.extract(path, work_dir, pdf_backend)
    notify("ingest", 1, 1, "markdown written")
    return result


def _read_text(path: str) -> str:
    try:
        return Path(path).read_text(encoding="utf-8")
    except OSError as exc:
        message = f"cannot read {path}: {exc}"
        raise JsonRpcError(message, code=INTERNAL_ERROR, data={"path": path}) from exc


def _handle_parse_document(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    markdown = _read_text(_require_str(params, "markdown_path"))
    blocks, chapters = parse.parse_markdown(markdown)
    return {
        "blocks": [block.to_json() for block in blocks],
        "chapters": [chapter.to_json() for chapter in chapters],
    }


def _handle_build_chunks(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    blocks = _blocks_from(params)
    budget = _require_int(params, "budget_tokens")
    if budget <= 0:
        _fail("parameter 'budget_tokens' must be positive")
    chunks = chunker.build_chunks(blocks, budget)
    return {"chunks": [chunk.to_json() for chunk in chunks]}


def _handle_prepare_text(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    text = _require_str(params, "text")
    block_ids = _optional_str_list(params, "block_ids")
    llm_text, placeholder_map = placeholders.substitute(text)
    return {
        "llm_text": llm_text,
        "placeholders": placeholder_map,
        "used_blocks": None if block_ids is None else len(block_ids),
    }


def _handle_reinject(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    text = _require_str(params, "text")
    pairs = _placeholders_from(params)
    expected_blocks = _require_int(params, "expected_blocks")
    result = placeholders.reinject(text, pairs)
    # The caller indexes blocks_md positionally against the chunk's block list, so the
    # order split_blocks produces here is load-bearing.
    blocks_md = [block.source_md for block in parse.split_blocks(result.text)]
    return {
        "blocks_md": blocks_md,
        "placeholders_ok": result.ok,
        "missing": result.missing,
        "duplicated": result.duplicated,
        "unknown": result.unknown,
        "block_count_ok": len(blocks_md) == expected_blocks,
    }


def _handle_qa_check(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    return qa.check(
        source_text=_require_str(params, "source_text"),
        target_text=_require_str(params, "target_text"),
        glossary=_require_str_map(params, "glossary"),
        placeholders=_placeholders_from(params),
    )


def _handle_pandoc_build(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    return pandoc.build(
        units=_units_from(params),
        metadata=_require_object(params, "metadata"),
        output_path=_require_str(params, "output_path"),
        output_format=_require_str(params, "output_format"),
        template=_optional_str(params, "template"),
        css=_optional_str(params, "css"),
        resource_path=_optional_str_list(params, "resource_path") or [],
        toc=_optional_bool(params, "toc"),
        lua_filters=_optional_str_list(params, "lua_filters") or [],
        top_level_division=_optional_str(params, "top_level_division"),
    )


def _handle_estimate_tokens(params: JsonObject, _notify: ProgressSink) -> JsonObject:
    texts = _require_str_list(params, "texts")
    return {"counts": [chunker.estimate_tokens(text) for text in texts]}


_HANDLERS: dict[str, Handler] = {
    "ping": _handle_ping,
    "detect_format": _handle_detect_format,
    "ingest": _handle_ingest,
    "parse_document": _handle_parse_document,
    "build_chunks": _handle_build_chunks,
    "prepare_text": _handle_prepare_text,
    "reinject": _handle_reinject,
    "qa_check": _handle_qa_check,
    "pandoc_build": _handle_pandoc_build,
    "estimate_tokens": _handle_estimate_tokens,
}


# ---------------------------------------------------------------------------
# Error mapping
# ---------------------------------------------------------------------------


def _translate(exc: Exception) -> JsonRpcError:
    """Map a domain exception onto its JSON-RPC error, keeping the useful detail."""
    if isinstance(exc, extractors.MissingDependencyError):
        data: JsonObject = {"backend": exc.backend} if exc.backend is not None else {}
        return JsonRpcError(str(exc), code=exc.code, data=data or None)
    if isinstance(exc, extractors.ExtractionError):
        return JsonRpcError(str(exc), code=exc.code)
    if isinstance(exc, pandoc.MissingDependencyError):
        return JsonRpcError(str(exc), code=exc.code)
    if isinstance(exc, pandoc.PandocError):
        detail: JsonObject = {"log": exc.log} if exc.log else {}
        return JsonRpcError(str(exc), code=exc.code, data=detail or None)
    message = str(exc) or type(exc).__name__
    return JsonRpcError(
        message,
        code=INTERNAL_ERROR,
        data={"type": type(exc).__name__, "message": message},
    )


def dispatch(method: str, params: JsonObject, notify: ProgressSink = _noop_progress) -> object:
    """Run one method and return its result, or raise the mapped :class:`JsonRpcError`."""
    handler = _HANDLERS.get(method)
    if handler is None:
        message = f"unknown method: {method}"
        raise JsonRpcError(message, code=METHOD_NOT_FOUND)
    try:
        return handler(params, notify)
    except JsonRpcError:
        raise
    except Exception as exc:
        # The transport must translate every failure: an unmapped exception would kill
        # the loop and leave the client waiting until its timeout. The traceback goes to
        # stderr so the control plane can put it in the diagnostics log.
        logger.exception("method %s failed", method)
        raise _translate(exc) from exc


# ---------------------------------------------------------------------------
# Framing
# ---------------------------------------------------------------------------


def _response(request_id: object, result: object) -> JsonObject:
    return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "result": result}


def _error_response(request_id: object, error: JsonRpcError) -> JsonObject:
    payload: JsonObject = {"code": error.code, "message": error.message}
    if error.data:
        payload["data"] = error.data
    return {"jsonrpc": JSONRPC_VERSION, "id": request_id, "error": payload}


def _request_id(message: JsonObject) -> object:
    value = message.get("id")
    if value is None or isinstance(value, str):
        return value
    if isinstance(value, int) and not isinstance(value, bool):
        return value
    return None


def _id_of(message: object) -> object:
    if isinstance(message, dict):
        return _request_id(cast("JsonObject", message))
    return None


def handle_message(message: object, notify: ProgressSink) -> JsonObject | None:
    """Turn one decoded request into its response object.

    A message without an ``id`` is a notification and never gets a reply, per JSON-RPC.
    """
    if not isinstance(message, dict):
        return _error_response(None, JsonRpcError("invalid request", code=INVALID_REQUEST))
    request = cast("JsonObject", message)
    if "id" not in request:
        return None
    request_id = _request_id(request)
    method = request.get("method")
    if not isinstance(method, str):
        return _error_response(request_id, JsonRpcError("invalid request", code=INVALID_REQUEST))
    raw_params = request.get("params")
    if raw_params is None:
        params: JsonObject = {}
    elif isinstance(raw_params, dict):
        params = cast("JsonObject", raw_params)
    else:
        return _error_response(request_id, InvalidParamsError("params must be an object"))
    try:
        result = dispatch(method, params, notify)
    except JsonRpcError as error:
        return _error_response(request_id, error)
    return _response(request_id, result)


def process_line(line: str, notify: ProgressSink = _noop_progress) -> JsonObject | None:
    """Turn one NDJSON line into the response object, or ``None`` for a notification."""
    try:
        message = json.loads(line)
    except json.JSONDecodeError as exc:
        return _error_response(None, JsonRpcError(f"parse error: {exc.msg}", code=PARSE_ERROR))
    try:
        return handle_message(message, notify)
    except JsonRpcError as error:
        return _error_response(_id_of(message), error)
    except Exception as exc:  # noqa: BLE001 - the loop must survive every failure
        return _error_response(_id_of(message), _translate(exc))


def _write_line(writer: TextIO, payload: JsonObject) -> None:
    writer.write(json.dumps(payload, ensure_ascii=False) + "\n")
    writer.flush()


def serve(reader: TextIO, writer: TextIO) -> int:
    """Read NDJSON requests from ``reader`` and write responses to ``writer`` until EOF."""

    def emit(phase: str, done: int, total: int, message: str) -> None:
        _write_line(writer, progress_notification(phase, done, total, message))

    while True:
        line = reader.readline()
        if line == "":
            logger.info("sidecar stdin closed; exiting")
            return 0
        stripped = line.strip()
        if stripped == "":
            continue
        response = process_line(stripped, emit)
        if response is not None:
            _write_line(writer, response)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def _protocol_stream() -> TextIO:
    """A private UTF-8 stream on a duplicate of the real stdout descriptor.

    Duplicating the descriptor keeps the protocol channel independent of ``sys.stdout``,
    which :func:`run_stdio` rebinds to stderr, and forcing UTF-8 keeps non-ASCII payloads
    from failing on an ASCII locale.
    """
    try:
        sys.stdout.flush()
        descriptor = os.dup(sys.stdout.fileno())
    except (AttributeError, OSError, ValueError):
        return sys.stdout
    return open(descriptor, "w", encoding="utf-8", newline="\n")  # noqa: PTH123 - dup'd fd, not a path


def run_stdio() -> int:
    """Serve JSON-RPC on stdio until EOF, then return the exit code."""
    logging.basicConfig(level=logging.INFO, stream=sys.stderr)
    logger.info(
        "sidecar starting (version %s, python %s, pid %s)",
        __version__,
        platform.python_version(),
        os.getpid(),
    )
    original = sys.stdout
    protocol = _protocol_stream()
    # From here on a stray `print` in the pure modules or their libraries hits stderr
    # instead of the wire; the extractors do the same internally for PyMuPDF.
    sys.stdout = sys.stderr
    try:
        return serve(sys.stdin, protocol)
    finally:
        sys.stdout = original
        protocol.flush()
        if protocol is not original:
            protocol.close()
