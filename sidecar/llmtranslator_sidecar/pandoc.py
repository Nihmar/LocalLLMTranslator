"""Pandoc bridge: combine the per-chapter Markdown units and render the final book.

The sidecar is stateless, so this module builds its own throwaway work directory: the
combined Markdown and the ``metadata.yaml`` live in a :mod:`tempfile` directory that is
removed before the function returns. The writer writes to a temporary file next to
``output_path`` and that file is then renamed into place, so a crashed build never replaces
a good export with a half-written one. The only durable artefact is ``output_path``.

Every run is bounded: a pandoc that does not finish within ``LLMTRANSLATOR_PANDOC_TIMEOUT``
seconds (default 600) is killed and reported as a build failure. The RPC loop is sequential,
so a hung writer would otherwise block every later request until the client abandons it.

The metadata document follows the same YAML conventions as the Rust ``BookMetadata``:
``title`` first, then ``author``/``language``/``date``/``publisher``/``identifier`` when
present, then any extra front-matter key, with a scalar-quoting rule that keeps a value
such as ``a: b`` from being read back as a mapping.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import TYPE_CHECKING, Any

from .errors import (
    MISSING_DEPENDENCY,
    PANDOC_FAILURE,
    MissingDependencyError,
    PandocError,
)

if TYPE_CHECKING:
    from collections.abc import Mapping, Sequence

#: RPC error code carried on :class:`PandocError` (see ``PLAN.md`` §12.1).
PANDOC_FAILURE_CODE = PANDOC_FAILURE

#: RPC error code carried on :class:`MissingDependencyError`.
MISSING_DEPENDENCY_CODE = MISSING_DEPENDENCY

#: Environment variable that overrides the ``pandoc`` binary path.
PANDOC_ENV = "LLMTRANSLATOR_PANDOC"

#: Environment variable that overrides the pandoc run timeout, in seconds.
PANDOC_TIMEOUT_ENV = "LLMTRANSLATOR_PANDOC_TIMEOUT"

#: A pandoc run that takes longer than this is killed. The sidecar's JSON-RPC loop is
#: sequential, so a hung pandoc would block every later request until the client times out
#: and abandons it, leaving the worker unable to make progress.
DEFAULT_TIMEOUT_SECONDS = 600.0

#: Requested output format -> pandoc writer. An unknown format falls back to its own name.
_WRITERS: dict[str, str] = {
    "epub": "epub",
    "pdf": "pdf",
    "docx": "docx",
    "html": "html",
    "html5": "html",
    "markdown": "markdown",
    "md": "markdown",
    "latex": "latex",
    "tex": "latex",
}

#: Metadata keys emitted in this order, before any extra front-matter key.
_ORDERED_KEYS: tuple[str, ...] = ("title", "author", "language", "date", "publisher", "identifier")

#: Characters that force a YAML scalar to be quoted, mirroring the Rust ``yaml_scalar``.
_YAML_RISKY = frozenset(":#\n\"'[]{},&*!|>%@`")


def _needs_quoting(value: str) -> bool:
    return (
        any(char in _YAML_RISKY for char in value)
        or value.startswith((" ", "-", "?"))
        or value.endswith(" ")
    )


def _yaml_scalar(value: str) -> str:
    """Quote a string when YAML would otherwise reinterpret it."""
    if value == "":
        return '""'
    if _needs_quoting(value):
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'
    return value


def _metadata_value(value: object) -> str:
    if isinstance(value, str):
        return _yaml_scalar(value)
    return json.dumps(value, ensure_ascii=False)


def _render_metadata(metadata: Mapping[str, Any]) -> str:
    """Render a ``metadata.yaml`` document from the book metadata mapping."""
    lines: list[str] = []
    emitted: set[str] = set()
    for key in _ORDERED_KEYS:
        if key in metadata and metadata[key] is not None:
            lines.append(f"{_yaml_scalar(key)}: {_metadata_value(metadata[key])}")
            emitted.add(key)
    for key, value in metadata.items():
        if value is None or key in emitted or key in _ORDERED_KEYS:
            continue
        lines.append(f"{_yaml_scalar(str(key))}: {_metadata_value(value)}")
    return "".join(f"{line}\n" for line in lines)


def _writer_for(output_format: str) -> str:
    normalized = output_format.strip().lower()
    return _WRITERS.get(normalized, normalized)


def timeout_seconds() -> float:
    """The pandoc timeout, overridable for tests and for unusually slow builds."""
    raw = os.environ.get(PANDOC_TIMEOUT_ENV)
    if raw is None:
        return DEFAULT_TIMEOUT_SECONDS
    try:
        value = float(raw)
    except ValueError:
        return DEFAULT_TIMEOUT_SECONDS
    return value if value > 0 else DEFAULT_TIMEOUT_SECONDS


def _resolve_binary() -> str:
    """Return the pandoc executable: the env override wins, then ``PATH``."""
    override = os.environ.get(PANDOC_ENV)
    candidate = shutil.which(override) if override else shutil.which("pandoc")
    if candidate is None:
        msg = f"pandoc binary not found: {override or 'pandoc'}"
        raise MissingDependencyError(msg)
    return candidate


def _combine_units(units: Sequence[Mapping[str, Any]]) -> str:
    """Join the units into one document, each chapter introduced by its own heading."""
    parts: list[str] = []
    for unit in units:
        title = str(unit.get("title", "")).strip()
        source = Path(str(unit["path"]))
        try:
            body = source.read_text(encoding="utf-8").strip("\n")
        except OSError as exc:
            # The units are the build's inputs: an unreadable one is a build failure, not
            # a generic internal error, so it is reported with the domain error type.
            msg = f"cannot read unit {source}: {exc}"
            raise PandocError(msg) from exc
        if title:
            parts.append(f"# {title}")
        if body:
            parts.append(body)
    if not parts:
        return ""
    return "\n\n".join(parts) + "\n"


def build(  # noqa: PLR0913 - the keyword signature is frozen by PLAN.md §12.1
    *,
    units: Sequence[Mapping[str, Any]],
    metadata: Mapping[str, Any],
    output_path: str,
    output_format: str,
    template: str | None = None,
    css: str | None = None,
    resource_path: Sequence[str] | None = None,
    toc: bool = False,
    lua_filters: Sequence[str] | None = None,
    top_level_division: str | None = None,
) -> dict[str, Any]:
    """Render ``units`` to ``output_path`` in ``output_format``.

    ``units`` are ``{"path", "title"}`` per-chapter Markdown files, combined in the given
    order with each title as a top-level heading. ``resource_path`` lists the directories
    pandoc searches for relative targets (extracted media such as ``assets/<name>``), since
    the combined document lives in a throwaway directory. ``toc`` adds the table of contents,
    ``lua_filters`` the ordered ``--lua-filter`` arguments (footnotes, tables, EPUB cleanup)
    and ``top_level_division`` the book-style top-level heading (``chapter`` for LaTeX/PDF).
    Returns ``output_path``, the combined pandoc output as ``log``, and the wall time as
    ``duration_ms``.
    """
    binary = _resolve_binary()
    writer = _writer_for(output_format)
    document = _combine_units(units)

    target = Path(output_path)
    target.parent.mkdir(parents=True, exist_ok=True)

    started = time.perf_counter()
    timeout = timeout_seconds()
    with tempfile.TemporaryDirectory(prefix="llmtranslator-pandoc-") as work:
        work_dir = Path(work)
        (work_dir / "metadata.yaml").write_text(_render_metadata(metadata), encoding="utf-8")
        (work_dir / "input.md").write_text(document, encoding="utf-8")

        handle, temporary = tempfile.mkstemp(
            dir=target.parent,
            prefix=f".{target.name}.",
            suffix=target.suffix,
        )
        os.close(handle)

        command = [
            binary,
            str(work_dir / "input.md"),
            "--standalone",
            f"--to={writer}",
            f"--output={temporary}",
            f"--metadata-file={work_dir / 'metadata.yaml'}",
        ]
        if template:
            command.append(f"--template={template}")
        if css:
            command.append(f"--css={css}")
        if toc:
            command.append("--toc")
        if top_level_division:
            command.append(f"--top-level-division={top_level_division}")
        command.extend(f"--lua-filter={lua_filter}" for lua_filter in lua_filters or ())
        if resource_path:
            command.append(f"--resource-path={os.pathsep.join(resource_path)}")

        try:
            completed = subprocess.run(  # noqa: S603 - argv is built here, pandoc only
                command,
                capture_output=True,
                text=True,
                check=False,
                timeout=timeout,
            )
        except subprocess.TimeoutExpired as exc:
            Path(temporary).unlink(missing_ok=True)
            partial = "".join(
                stream if isinstance(stream, str) else (stream or b"").decode("utf-8", "replace")
                for stream in (exc.stdout, exc.stderr)
            )
            msg = f"pandoc timed out after {timeout:g}s"
            raise PandocError(msg, log=partial) from exc
        except OSError as exc:
            Path(temporary).unlink(missing_ok=True)
            msg = f"failed to run pandoc: {exc}"
            raise PandocError(msg) from exc

        log = completed.stdout + completed.stderr
        if completed.returncode != 0:
            Path(temporary).unlink(missing_ok=True)
            msg = f"pandoc exited with status {completed.returncode}"
            raise PandocError(msg, log=log)
        try:
            Path(temporary).replace(target)
        except OSError as exc:
            # The rename can fail on its own (target is a directory, permission denied,
            # out of space): the temp file must not leak and the failure is a pandoc
            # build failure, not an unmapped internal error.
            Path(temporary).unlink(missing_ok=True)
            msg = f"failed to move the pandoc output into {target}: {exc}"
            raise PandocError(msg, log=log) from exc

    duration_ms = int((time.perf_counter() - started) * 1000)
    return {"output_path": output_path, "log": log, "duration_ms": duration_ms}
