#!/usr/bin/env python3
"""Deterministic, offline, stdlib-only fake of ``llama-server``.

This module emulates the subset of the ``llama-server`` HTTP surface that the
LocalLLMTranslator pipeline depends on, so the whole test-suite can run offline
and deterministically (CI backbone).  It has **no third-party imports**: only
the Python standard library is used, therefore it can be executed with any
Python 3.9+ interpreter, with or without the sidecar virtual-env.

Endpoints
---------
``GET  /health``                -> ``{"status":"ok"}``
``GET  /props``                 -> ``{"total_slots":N,"n_ctx":C,"model_path":"fake",...}``
``GET  /v1/models``             -> OpenAI-shaped model list
``POST /tokenize``              -> ``{"tokens":[ids],"n_tokens":n}``
``POST /v1/tokenize``           -> same as ``/tokenize``
``POST /v1/chat/completions``   -> non-streaming JSON *or* SSE (``stream:true``)

Determinism
-----------
``created`` timestamps are a fixed constant, model ids are fixed, the token
approximation is a pure function of the input length, and the fake translation
is a pure function of the input text.  Two runs over the same input produce byte
identical bodies.

The fake translation (documented transform)
--------------------------------------------
``translate(text, cfg)`` walks the text **line by line** and applies a single
regular-expression substitution per line.  The substitution recognises four
kinds of token, in this priority order:

1. inline code spans  `` `...` ``          -> copied verbatim (never translated)
2. placeholders       ``⟦12⟧``             -> copied verbatim, exactly once,
                                              in the same relative order
3. URLs               ``https?://...``     -> copied verbatim (never translated)
4. translatable words ``[^\\W\\d_]+``        -> wrapped with the markers ``«`` and
                                              ``»`` and, for a small fixed
                                              substitution table of common
                                              English words, replaced by a fixed
                                              Italian pseudo-translation.

Because only *words* are rewritten and every structural character (newlines,
blank lines, ``#`` heading markers, ``-`` / ``*`` / ``1.`` list markers, ``>``
blockquote markers, ``|`` table pipes and the ``|---|`` / ``:---:`` separator
row) is a non-word character, the transform preserves:

* the number of lines and the number of blank-line separated paragraphs;
* list markers, heading levels and table pipes exactly;
* the multiset and the relative order of ``⟦n⟧`` placeholders.

Fenced code blocks (lines inside/around ``` or ~~~ fences) are copied verbatim.

The transform is *reversible*: strip the ``«`` / ``»`` markers and map the
pseudo-translations back through :data:`SUBST_INVERSE` (words not in the table
are identity).  Reversibility is documented for test authors; it is not used by
the server.

Fault injection
---------------
Every fault is configurable by CLI flag **and** by environment variable (the CLI
flag wins when both are given):

======================  =========================================  ==========================================
CLI                     Environment                                Effect
======================  =========================================  ==========================================
``--drop-placeholder N`` ``FAKE_LLAMA_DROP_PLACEHOLDER=N``          remove every ``⟦N⟧`` from the output
``--duplicate-placeholder N`` ``FAKE_LLAMA_DUPLICATE_PLACEHOLDER=N`` duplicate every ``⟦N⟧``
``--reorder-placeholders`` ``FAKE_LLAMA_REORDER_PLACEHOLDERS=1``    reverse the placeholder sequence
``--truncate RATIO``     ``FAKE_LLAMA_TRUNCATE=RATIO``              cut the output short (0<ratio<1)
``--fail-rate P``        ``FAKE_LLAMA_FAIL_RATE=P``                 HTTP 500 for a fraction of chat requests
``--delay-ms MS``        ``FAKE_LLAMA_DELAY_MS=MS``                 sleep before answering chat requests
``--merge-paragraphs``   ``FAKE_LLAMA_MERGE_PARAGRAPHS=1``          drop blank lines (corrupt block structure)
``--echo-prompt-prefix`` ``FAKE_LLAMA_ECHO_PROMPT_PREFIX=1``        echo the prompt preface before the passage (an irregular answer)
``--total-slots N``      ``FAKE_LLAMA_TOTAL_SLOTS=N``               value reported by ``/props``
``--n-ctx N``            ``FAKE_LLAMA_N_CTX=N``                     value reported by ``/props``
``--port PORT``          ``FAKE_LLAMA_PORT=PORT``                   listen port (default 8080, 0 = ephemeral)
``--host HOST``          ``FAKE_LLAMA_HOST=HOST``                   listen host (default 127.0.0.1)
======================  =========================================  ==========================================

Run ``python tools/fake_llama_server.py --selftest`` to execute an internal
assertion pass; it exits non-zero on failure.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import threading
import time
import urllib.error
import urllib.request
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

__version__ = "0.1.0"

# --------------------------------------------------------------------------- #
# Constants
# --------------------------------------------------------------------------- #

#: Fixed ``created`` timestamp so responses are byte-identical across runs.
FIXED_CREATED = 1_700_000_000

#: Fixed model id reported by ``/v1/models`` and echoed in completions.
FAKE_MODEL = "fake-model"

#: Line that introduces the passage to translate inside the user message.
PASSAGE_MARKER = "PASSAGE TO TRANSLATE:"

#: Word-marker characters used by the fake translation.
WRAP_OPEN = "\u00ab"  # «
WRAP_CLOSE = "\u00bb"  # »

#: A small fixed substitution table (English -> Italian pseudo-translation).
#: Values are unique so the map is (almost) a bijection and therefore invertible
#: for tests.
SUBST: dict[str, str] = {
    "the": "il",
    "a": "un",
    "an": "uno",
    "and": "e",
    "or": "oppure",
    "of": "di",
    "to": "verso",
    "in": "dentro",
    "on": "sopra",
    "at": "presso",
    "for": "per",
    "with": "con",
    "without": "senza",
    "from": "da",
    "by": "tramite",
    "as": "come",
    "is": "è",
    "are": "sono",
    "was": "era",
    "were": "erano",
    "be": "essere",
    "been": "stato",
    "has": "possiede",
    "have": "avere",
    "had": "aveva",
    "will": "vorrà",
    "would": "vorrebbe",
    "can": "può",
    "could": "potrebbe",
    "not": "non",
    "no": "nessuno",
    "yes": "sì",
    "but": "ma",
    "if": "qualora",
    "that": "che",
    "this": "questo",
    "these": "questi",
    "those": "quelli",
    "he": "egli",
    "she": "ella",
    "it": "esso",
    "they": "essi",
    "we": "noi",
    "you": "voi",
    "i": "io",
    "his": "suo",
    "her": "sua",
    "their": "loro",
    "our": "nostro",
    "my": "mio",
    "your": "vostro",
    "there": "là",
    "here": "qui",
    "what": "ciò",
    "which": "quale",
    "when": "quando",
    "where": "dove",
    "who": "chi",
    "how": "come-come",
    "all": "tutto",
    "one": "uno-solo",
    "two": "due",
    "three": "tre",
    "first": "primo",
    "last": "ultimo",
    "day": "giorno",
    "night": "notte",
    "light": "luce",
    "water": "acqua",
    "sea": "mare",
    "ship": "nave",
    "book": "libro",
    "word": "parola",
    "man": "uomo",
    "woman": "donna",
    "time": "tempo",
    "world": "mondo",
    "city": "città",
    "king": "re",
    "house": "casa",
    "door": "porta",
    "road": "strada",
    "old": "vecchio",
    "new": "nuovo",
    "good": "buono",
    "great": "grande",
    "small": "piccolo",
}

#: Inverse of :data:`SUBST` for test-side reversal.
SUBST_INVERSE: dict[str, str] = {v: k for k, v in SUBST.items()}

#: Matches inline code spans, placeholders, URLs and translatable words.
TOKEN_RE = re.compile(r"`[^`]*`|\u27e6\d+\u27e7|https?://[^\s)\]>]+|[^\W\d_]+", re.UNICODE)

#: Matches a single ``⟦n⟧`` placeholder, capturing ``n``.
PLACEHOLDER_RE = re.compile(r"\u27e6(\d+)\u27e7")

#: Matches a fenced code block delimiter (``` or ~~~).
FENCE_RE = re.compile(r"^\s*(```|~~~)")


# --------------------------------------------------------------------------- #
# Tokenisation helpers (deterministic, monotonic in input length)
# --------------------------------------------------------------------------- #


def approx_tokens(text: str) -> int:
    """Return a deterministic token approximation for *text*.

    The value is ``ceil(len(text) / 4)`` (``0`` for the empty string).  It is a
    pure function of the length and is therefore **monotonic in input length**
    (a longer input never yields fewer tokens), which is the property the
    tests rely on.
    """
    if not text:
        return 0
    return (len(text) + 3) // 4


def token_ids(text: str, count: int | None = None) -> list[int]:
    """Return a deterministic list of pseudo token ids for *text*.

    ``count`` defaults to :func:`approx_tokens`.  Ids are generated with a
    fixed FNV-1a/LCG mixing of the characters, so they are stable across runs
    but look vocabulary-like (values ``0..31999``).
    """
    if count is None:
        count = approx_tokens(text)
    h = 2166136261
    for ch in text:
        h = ((h ^ ord(ch)) * 16777619) & 0xFFFFFFFF
    out: list[int] = []
    for _ in range(count):
        h = (h * 1103515245 + 12345) & 0x7FFFFFFF
        out.append(h % 32000)
    return out


# --------------------------------------------------------------------------- #
# The fake translation
# --------------------------------------------------------------------------- #


@dataclass
class TranslationConfig:
    """All knobs of the fake server / fake translation."""

    total_slots: int = 1
    n_ctx: int = 4096
    drop_placeholder: int | None = None
    duplicate_placeholder: int | None = None
    reorder_placeholders: bool = False
    truncate: float | None = None
    fail_rate: float = 0.0
    delay_ms: int = 0
    merge_paragraphs: bool = False
    echo_prompt_prefix: bool = False

    def generation_settings(self) -> dict[str, Any]:
        """Return a ``default_generation_settings``-shaped dict."""
        return {
            "n_ctx": self.n_ctx,
            "n_predict": -1,
            "temperature": 0.8,
            "top_k": 40,
            "top_p": 0.95,
            "min_p": 0.05,
            "repeat_penalty": 1.1,
            "seed": -1,
            "mirostat": 0,
        }


def map_word(word: str) -> str:
    """Return the pseudo-translation of a single word (case preserved)."""
    lowered = word.lower()
    mapped = SUBST.get(lowered, lowered)
    if len(word) > 1 and word.isupper():
        return mapped.upper()
    if word[:1].isupper():
        return mapped[:1].upper() + mapped[1:]
    return mapped


def _transform_line(line: str) -> str:
    """Rewrite the translatable words of a single line, keeping structure."""

    def repl(match: re.Match[str]) -> str:
        token = match.group(0)
        if token.startswith("`") or token.startswith("\u27e6") or token.startswith("http"):
            return token  # protected: inline code, placeholder or URL
        return WRAP_OPEN + map_word(token) + WRAP_CLOSE

    return TOKEN_RE.sub(repl, line)


def reorder_placeholders(text: str) -> str:
    """Reverse the relative order of placeholders, keeping the multiset."""
    found = PLACEHOLDER_RE.findall(text)
    if len(found) < 2:
        return text
    reversed_tokens = [f"\u27e6{n}\u27e7" for n in reversed(found)]
    it = iter(reversed_tokens)
    return PLACEHOLDER_RE.sub(lambda _m: next(it), text)


def apply_faults(text: str, cfg: TranslationConfig) -> str:
    """Apply every configured fault injection to *text*."""
    out = text
    if cfg.drop_placeholder is not None:
        out = out.replace(f"\u27e6{cfg.drop_placeholder}\u27e7", "")
    if cfg.duplicate_placeholder is not None:
        token = f"\u27e6{cfg.duplicate_placeholder}\u27e7"
        out = out.replace(token, token + token)
    if cfg.reorder_placeholders:
        out = reorder_placeholders(out)
    if cfg.merge_paragraphs:
        out = "\n".join(line for line in out.split("\n") if line.strip() != "")
    if cfg.truncate is not None and cfg.truncate < 1.0:
        out = out[: max(0, int(len(out) * cfg.truncate))]
    return out


def translate(text: str, cfg: TranslationConfig | None = None) -> str:
    """Translate *text* deterministically, preserving Markdown structure.

    Fenced code blocks are copied verbatim; every other line is rewritten by
    :func:`_transform_line`; finally :func:`apply_faults` runs.
    """
    cfg = cfg or TranslationConfig()
    lines = text.split("\n")
    out: list[str] = []
    in_fence = False
    for line in lines:
        if FENCE_RE.match(line):
            out.append(line)
            in_fence = not in_fence
            continue
        out.append(line if in_fence else _transform_line(line))
    return apply_faults("\n".join(out), cfg)


def split_passage(content: str) -> tuple[str, str]:
    """Split a user message into (preface, passage) around the passage marker.

    Real prompts put the passage after a ``PASSAGE TO TRANSLATE:`` line; the
    preface (chapter title, previous context, instructions, ...) is context for the
    model.  A compliant, instruction-following model returns only the passage and
    not the preface.  When the marker is absent the whole content is the passage.
    """
    idx = content.find(PASSAGE_MARKER)
    if idx == -1:
        return "", content
    end = idx + len(PASSAGE_MARKER)
    return content[:end], content[end:]


def translate_content(content: str, cfg: TranslationConfig | None = None) -> str:
    """Translate only the passage of a chat user-message content.

    By default the preface before ``PASSAGE TO TRANSLATE:`` is dropped, matching a
    compliant model that answers with the translation alone.
    ``TranslationConfig.echo_prompt_prefix`` restores the old behaviour of echoing
    the (untranslated) preface ahead of the passage, which produces an irregular
    answer the pipeline refuses to align (``needs_review``).
    """
    cfg = cfg or TranslationConfig()
    prefix, passage = split_passage(content)
    translated = translate(passage, cfg)
    if cfg.echo_prompt_prefix:
        return prefix + translated
    return translated


def stream_pieces(text: str, size: int = 24) -> list[str]:
    """Split *text* into streaming pieces without breaking a placeholder."""
    pieces: list[str] = []
    i = 0
    n = len(text)
    while i < n:
        j = min(i + size, n)
        # Never cut a ⟦n⟧ placeholder in half.
        open_at = text.rfind("\u27e6", i, j)
        close_after = text.find("\u27e7", j)
        if open_at != -1 and text.find("\u27e7", open_at) == -1:
            if close_after != -1:
                j = close_after + 1
            else:
                j = n
        pieces.append(text[i:j])
        i = j
    return pieces


def _json_dumps(obj: Any) -> bytes:
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


# --------------------------------------------------------------------------- #
# HTTP server
# --------------------------------------------------------------------------- #


class _Counters:
    """Thread-safe request counters used for deterministic fault injection."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._chat = 0

    def next_chat(self) -> int:
        """Return the 1-based index of the next chat request."""
        with self._lock:
            self._chat += 1
            return self._chat


def should_fail(cfg: TranslationConfig, index: int) -> bool:
    """Deterministically decide whether chat request *index* gets a 500."""
    if cfg.fail_rate <= 0:
        return False
    if cfg.fail_rate >= 1:
        return True
    period = max(1, round(1.0 / cfg.fail_rate))
    return index % period == 0


def make_handler(cfg: TranslationConfig) -> type[BaseHTTPRequestHandler]:
    """Build a request-handler class bound to *cfg*."""
    counters = _Counters()

    class FakeHandler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        server_version = "fake-llama-server/" + __version__

        def end_headers(self) -> None:
            """Every response closes its connection.

            HTTP/1.1 would otherwise keep the socket alive, and those lingering sockets get
            reclaimed by the garbage collector after the test that owned them has already
            finished. With warnings promoted to errors in the test suite that surfaces as an
            unraisable-exception failure attributed to an unrelated test.

            Note this runs after the ``Connection`` header has been sent, so it also wins
            over the explicit ``keep-alive`` the SSE path emits: close_connection is what
            the request loop actually consults.
            """
            self.close_connection = True
            super().end_headers()

        # -- helpers ------------------------------------------------------- #
        def log_message(self, fmt: str, *args: Any) -> None:  # noqa: A003
            if os.environ.get("FAKE_LLAMA_LOG"):
                super().log_message(fmt, *args)

        def _read_body(self) -> dict[str, Any]:
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            if not raw:
                return {}
            try:
                parsed = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                return {}
            return parsed if isinstance(parsed, dict) else {}

        def _send_json(self, obj: Any, status: int = 200) -> None:
            payload = _json_dumps(obj)
            try:
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)
            except (BrokenPipeError, ConnectionResetError):
                pass

        def _send_error_json(self, status: int, message: str) -> None:
            self._send_json({"error": {"message": message, "type": "fake_error"}}, status)

        def _send_sse(self, events: list[str]) -> None:
            try:
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.send_header("Connection", "close")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                for event in events:
                    data = event.encode("utf-8")
                    self.wfile.write(f"{len(data):X}\r\n".encode("ascii") + data + b"\r\n")
                self.wfile.write(b"0\r\n\r\n")
            except (BrokenPipeError, ConnectionResetError):
                pass

        # -- routing ------------------------------------------------------- #
        def do_GET(self) -> None:  # noqa: N802
            path = self.path.split("?", 1)[0]
            if path == "/health":
                self._send_json({"status": "ok"})
            elif path == "/props":
                self._send_json(
                    {
                        "total_slots": cfg.total_slots,
                        "n_ctx": cfg.n_ctx,
                        "model_path": "fake",
                        "default_generation_settings": cfg.generation_settings(),
                    }
                )
            elif path == "/v1/models":
                self._send_json(
                    {
                        "object": "list",
                        "data": [
                            {
                                "id": FAKE_MODEL,
                                "object": "model",
                                "created": FIXED_CREATED,
                                "owned_by": "fake",
                            }
                        ],
                    }
                )
            else:
                self._send_error_json(404, f"unknown path {path}")

        def do_POST(self) -> None:  # noqa: N802
            path = self.path.split("?", 1)[0]
            body = self._read_body()
            if path in ("/tokenize", "/v1/tokenize"):
                self._handle_tokenize(body)
            elif path == "/v1/chat/completions":
                self._handle_chat(body)
            else:
                self._send_error_json(404, f"unknown path {path}")

        # -- endpoint implementations -------------------------------------- #
        def _handle_tokenize(self, body: dict[str, Any]) -> None:
            text = body.get("content") or body.get("text") or body.get("prompt") or ""
            if not isinstance(text, str):
                text = str(text)
            count = approx_tokens(text)
            self._send_json({"tokens": token_ids(text, count), "n_tokens": count})

        def _handle_chat(self, body: dict[str, Any]) -> None:
            index = counters.next_chat()
            if cfg.delay_ms > 0:
                time.sleep(cfg.delay_ms / 1000.0)
            if should_fail(cfg, index):
                self._send_error_json(500, "injected failure")
                return

            messages = body.get("messages") or []
            if not isinstance(messages, list):
                messages = []
            model = body.get("model") or FAKE_MODEL
            stream = bool(body.get("stream"))

            prompt_tokens = 0
            last_user = ""
            for message in messages:
                if not isinstance(message, dict):
                    continue
                content = message.get("content") or ""
                if not isinstance(content, str):
                    content = str(content)
                prompt_tokens += approx_tokens(content)
                if message.get("role") == "user":
                    last_user = content

            response_format = body.get("response_format") or {}
            wants_json = isinstance(response_format, dict) and response_format.get("type") in (
                "json_object",
                "json_schema",
            )
            if wants_json:
                # editor/proofreader JSON mode
                text = json.dumps({"verdict": "ok", "issues": []}, ensure_ascii=False)
                finish_reason = "stop"
            else:
                text = translate_content(last_user, cfg)
                finish_reason = "length" if (cfg.truncate is not None and cfg.truncate < 1.0) else "stop"

            completion_tokens = approx_tokens(text)

            if stream:
                self._send_sse(self._sse_events(model, text, finish_reason))
            else:
                self._send_json(
                    {
                        "id": "chatcmpl-fake",
                        "object": "chat.completion",
                        "created": FIXED_CREATED,
                        "model": model,
                        "choices": [
                            {
                                "index": 0,
                                "message": {"role": "assistant", "content": text},
                                "finish_reason": finish_reason,
                            }
                        ],
                        "usage": {
                            "prompt_tokens": prompt_tokens,
                            "completion_tokens": completion_tokens,
                            "total_tokens": prompt_tokens + completion_tokens,
                        },
                    }
                )

        def _sse_events(self, model: str, text: str, finish_reason: str) -> list[str]:
            events: list[str] = []

            def chunk(delta: dict[str, Any], reason: str | None) -> str:
                payload = {
                    "id": "chatcmpl-fake",
                    "object": "chat.completion.chunk",
                    "created": FIXED_CREATED,
                    "model": model,
                    "choices": [{"index": 0, "delta": delta, "finish_reason": reason}],
                }
                return "data: " + json.dumps(payload, ensure_ascii=False) + "\n\n"

            events.append(chunk({"role": "assistant", "content": ""}, None))
            for piece in stream_pieces(text):
                events.append(chunk({"content": piece}, None))
            events.append(chunk({}, finish_reason))
            events.append("data: [DONE]\n\n")
            return events

    return FakeHandler


def create_server(host: str, port: int, cfg: TranslationConfig | None = None) -> ThreadingHTTPServer:
    """Create (but do not start) a :class:`ThreadingHTTPServer`."""
    cfg = cfg or TranslationConfig()
    return ThreadingHTTPServer((host, port), make_handler(cfg))


def create_and_start(host: str = "127.0.0.1", port: int = 0, cfg: TranslationConfig | None = None) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    """Start the server in a daemon thread and return it with its base URL."""
    server = create_server(host, port, cfg)
    thread = threading.Thread(target=server.serve_forever, name="fake-llama-server", daemon=True)
    thread.start()
    return server, thread, f"http://{host}:{server.server_address[1]}"


def _env(name: str, default: str | None = None) -> str | None:
    value = os.environ.get(name)
    return value if value not in (None, "") else default


def _env_int(name: str, default: int | None) -> int | None:
    value = _env(name)
    return int(value) if value is not None else default


def _env_float(name: str, default: float | None) -> float | None:
    value = _env(name)
    return float(value) if value is not None else default


def build_config(args: argparse.Namespace) -> TranslationConfig:
    """Merge CLI arguments and environment variables into a config.

    Explicit CLI flags win over environment variables (``None`` means the flag
    was not given).
    """
    return TranslationConfig(
        total_slots=(
            args.total_slots if args.total_slots is not None else (_env_int("FAKE_LLAMA_TOTAL_SLOTS", 1) or 1)
        ),
        n_ctx=args.n_ctx if args.n_ctx is not None else (_env_int("FAKE_LLAMA_N_CTX", 4096) or 4096),
        drop_placeholder=(
            args.drop_placeholder
            if args.drop_placeholder is not None
            else _env_int("FAKE_LLAMA_DROP_PLACEHOLDER", None)
        ),
        duplicate_placeholder=(
            args.duplicate_placeholder
            if args.duplicate_placeholder is not None
            else _env_int("FAKE_LLAMA_DUPLICATE_PLACEHOLDER", None)
        ),
        reorder_placeholders=(
            args.reorder_placeholders or _env("FAKE_LLAMA_REORDER_PLACEHOLDERS") not in (None, "0")
        ),
        truncate=args.truncate if args.truncate is not None else _env_float("FAKE_LLAMA_TRUNCATE", None),
        fail_rate=args.fail_rate if args.fail_rate is not None else (_env_float("FAKE_LLAMA_FAIL_RATE", 0.0) or 0.0),
        delay_ms=args.delay_ms if args.delay_ms is not None else (_env_int("FAKE_LLAMA_DELAY_MS", 0) or 0),
        merge_paragraphs=(
            args.merge_paragraphs or _env("FAKE_LLAMA_MERGE_PARAGRAPHS") not in (None, "0")
        ),
        echo_prompt_prefix=(
            args.echo_prompt_prefix or _env("FAKE_LLAMA_ECHO_PROMPT_PREFIX") not in (None, "0")
        ),
    )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="fake_llama_server",
        description="Deterministic offline fake of llama-server (stdlib only).",
    )
    parser.add_argument("--host", default=None, help="listen host (default 127.0.0.1)")
    parser.add_argument("--port", type=int, default=None, help="listen port (default 8080, 0 = ephemeral)")
    parser.add_argument("--total-slots", type=int, default=None, help="value reported by /props")
    parser.add_argument("--n-ctx", type=int, default=None, help="context size reported by /props")
    parser.add_argument("--drop-placeholder", type=int, default=None, metavar="N", help="omit placeholder N")
    parser.add_argument("--duplicate-placeholder", type=int, default=None, metavar="N", help="duplicate placeholder N")
    parser.add_argument("--reorder-placeholders", action="store_true", help="reverse placeholder order")
    parser.add_argument("--truncate", type=float, default=None, metavar="RATIO", help="cut output to RATIO of its length")
    parser.add_argument("--fail-rate", type=float, default=None, metavar="P", help="HTTP 500 for a fraction P of chat requests")
    parser.add_argument("--delay-ms", type=int, default=None, metavar="MS", help="sleep MS before answering chat requests")
    parser.add_argument("--merge-paragraphs", action="store_true", help="drop blank lines (corrupt structure)")
    parser.add_argument("--echo-prompt-prefix", action="store_true", help="echo the prompt preface before the passage")
    parser.add_argument("--selftest", action="store_true", help="run internal assertions and exit")
    return parser


# --------------------------------------------------------------------------- #
# Self-test
# --------------------------------------------------------------------------- #


SELFTEST_INPUT = """# The Lantern Keeper

The harbour was quiet that morning.

- iron key
- brass telescope

| Element | Notes |
|:--------|------:|
| Lamps   | glass |

```sql
SELECT 1;
```

The keeper wrote that [^1] and left ⟦1⟧ the ⟦2⟧ room.
"""


def _selftest() -> int:
    failures: list[str] = []

    def check(cond: bool, label: str) -> None:
        if not cond:
            failures.append(label)

    cfg = TranslationConfig(total_slots=4, n_ctx=32768)
    server, thread, base = create_and_start("127.0.0.1", 0, cfg)

    def get(path: str) -> Any:
        with urllib.request.urlopen(base + path, timeout=10) as resp:
            return json.loads(resp.read().decode("utf-8"))

    def post(path: str, obj: dict[str, Any]) -> Any:
        data = json.dumps(obj).encode("utf-8")
        req = urllib.request.Request(base + path, data=data, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read().decode("utf-8"))

    try:
        check(get("/health") == {"status": "ok"}, "/health shape")
        props = get("/props")
        check(props["total_slots"] == 4 and props["n_ctx"] == 32768, "/props values")
        check(props["model_path"] == "fake", "/props model_path")
        check("default_generation_settings" in props, "/props generation settings")
        models = get("/v1/models")
        check(models["object"] == "list" and models["data"][0]["id"] == FAKE_MODEL, "/v1/models shape")

        # tokenize monotonic + consistent across both aliases
        short = post("/tokenize", {"content": "hello world"})["n_tokens"]
        longer = post("/tokenize", {"content": "hello world extended further"})["n_tokens"]
        check(longer >= short, "tokenize monotonic")
        t1 = post("/tokenize", {"content": "same text here"})["n_tokens"]
        t2 = post("/v1/tokenize", {"content": "same text here"})["n_tokens"]
        check(t1 == t2 and t1 > 0, "tokenize aliases agree")

        # non streaming completion
        resp = post(
            "/v1/chat/completions",
            {"model": FAKE_MODEL, "messages": [{"role": "user", "content": SELFTEST_INPUT}]},
        )
        content = resp["choices"][0]["message"]["content"]
        check(resp["choices"][0]["finish_reason"] in ("stop", "length"), "finish_reason present")
        check(resp["usage"]["completion_tokens"] == approx_tokens(content), "usage completion tokens")
        check(resp["usage"]["total_tokens"] == resp["usage"]["prompt_tokens"] + resp["usage"]["completion_tokens"], "usage total")
        check(
            len(PLACEHOLDER_RE.findall(content)) == len(PLACEHOLDER_RE.findall(SELFTEST_INPUT)),
            "placeholders preserved",
        )
        check(content.count("\n") == SELFTEST_INPUT.count("\n"), "line count preserved")
        check("```sql" in content and "SELECT 1;" in content, "code block preserved")

        # SSE framing
        data = json.dumps({"model": FAKE_MODEL, "stream": True, "messages": [{"role": "user", "content": SELFTEST_INPUT}]}).encode()
        req = urllib.request.Request(base + "/v1/chat/completions", data=data, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as resp:
            raw = resp.read().decode("utf-8")
        events = [e for e in raw.split("\n\n") if e]
        check(all(e.startswith("data: ") for e in events), "SSE data: prefix")
        check(events[-1] == "data: [DONE]", "SSE [DONE] terminator")
        streamed = "".join(
            json.loads(e[len("data: "):])["choices"][0]["delta"].get("content", "")
            for e in events[:-1]
            if e != "data: [DONE]"
        )
        check(streamed == content, "SSE reassembles to non-stream content")

        # fault injection (pure function)
        fc = TranslationConfig(drop_placeholder=1)
        dropped = translate(SELFTEST_INPUT, fc)
        check("\u27e61\u27e7" not in dropped and "\u27e62\u27e7" in dropped, "drop-placeholder")
        dc = TranslationConfig(duplicate_placeholder=2)
        check(translate(SELFTEST_INPUT, dc).count("\u27e62\u27e7") == 2, "duplicate-placeholder")
        rc = TranslationConfig(reorder_placeholders=True)
        check(
            PLACEHOLDER_RE.findall(translate(SELFTEST_INPUT, rc)) == list(reversed(PLACEHOLDER_RE.findall(SELFTEST_INPUT))),
            "reorder-placeholders",
        )
        tc = TranslationConfig(truncate=0.5)
        check(len(translate(SELFTEST_INPUT, tc)) < len(translate(SELFTEST_INPUT)), "truncate shortens")
        mc = TranslationConfig(merge_paragraphs=True)
        check("\n\n" not in translate(SELFTEST_INPUT, mc), "merge-paragraphs")

        # default: only the passage, never the echoed preface; the echo flag restores it
        message = f"PREFACE\n\nPASSAGE TO TRANSLATE:\n{SELFTEST_INPUT}"
        preface, passage = split_passage(message)
        check(translate_content(message) == translate(passage), "default drops the preface")
        check(
            translate_content(message, TranslationConfig(echo_prompt_prefix=True))
            == preface + translate(passage),
            "echo-prompt-prefix restores the preface",
        )

        # fail rate 1.0 over HTTP
        fail_server, fail_thread, fail_base = create_and_start("127.0.0.1", 0, TranslationConfig(fail_rate=1.0))
        try:
            req = urllib.request.Request(
                fail_base + "/v1/chat/completions",
                data=json.dumps({"messages": [{"role": "user", "content": "x"}]}).encode(),
                headers={"Content-Type": "application/json"},
            )
            try:
                urllib.request.urlopen(req, timeout=10)
                check(False, "fail-rate 500")
            except urllib.error.HTTPError as exc:
                check(exc.code == 500, "fail-rate 500")
        finally:
            fail_server.shutdown()
            fail_server.server_close()
            fail_thread.join(timeout=5)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)

    if failures:
        sys.stderr.write("SELFTEST FAILED:\n")
        for failure in failures:
            sys.stderr.write(f"  - {failure}\n")
        return 1
    sys.stdout.write("selftest: all checks passed\n")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    if args.selftest:
        return _selftest()

    cfg = build_config(args)
    host = args.host or _env("FAKE_LLAMA_HOST", "127.0.0.1") or "127.0.0.1"
    port = args.port if args.port is not None else (_env_int("FAKE_LLAMA_PORT", 8080) or 8080)

    server = create_server(host, port, cfg)
    actual_port = server.server_address[1]
    sys.stdout.write(f"fake-llama-server listening on http://{host}:{actual_port}\n")
    sys.stdout.flush()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
