"""Shared domain error types for the sidecar's pure modules.

The RPC layer maps these onto JSON-RPC errors **by class name** (see ``rpc.py``), so how
they are named and the numbers they carry are part of the frozen IPC surface
(``AGENTS.md``): :class:`ExtractionError` -> ``1001``, :class:`PandocError` -> ``1002``,
:class:`MissingDependencyError` -> ``1003``. Both the extractor backends and the pandoc
bridge raise the very same classes from here, so a caller can catch a single
:class:`MissingDependencyError` and handle every backend, whichever module raised it.
"""

from __future__ import annotations

#: JSON-RPC error code for a failed ingestion (``AGENTS.md`` IPC contract).
INGESTION_FAILED = 1001

#: JSON-RPC error code for a failed pandoc build.
PANDOC_FAILURE = 1002

#: JSON-RPC error code for an optional backend that is not installed.
MISSING_DEPENDENCY = 1003

#: Fallback JSON-RPC code for an unexpected internal failure.
INTERNAL_ERROR = -32603


class SidecarError(Exception):
    """Base class for failures the RPC layer reports as JSON-RPC errors."""

    code: int = INTERNAL_ERROR

    def __init__(self, message: str) -> None:
        super().__init__(message)
        self.message = message


class ExtractionError(SidecarError):
    """Ingestion failed; the RPC layer maps this to code ``1001``."""

    code = INGESTION_FAILED


class PandocError(SidecarError):
    """Pandoc failed; the RPC layer maps this to code ``1002``.

    :attr:`log` carries pandoc's combined stdout and stderr so the caller can surface it.
    """

    code = PANDOC_FAILURE

    def __init__(self, message: str, log: str = "") -> None:
        super().__init__(message)
        self.log = log


class MissingDependencyError(SidecarError):
    """An optional backend is not installed; maps to code ``1003``."""

    code = MISSING_DEPENDENCY

    def __init__(self, message: str, *, backend: str | None = None) -> None:
        super().__init__(message)
        self.backend = backend
