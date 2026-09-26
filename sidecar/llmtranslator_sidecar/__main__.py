"""Console entry point for the sidecar.

Both ``python -m llmtranslator_sidecar`` and the ``llmtranslator-sidecar`` script declared
in ``pyproject.toml`` resolve here and run the same JSON-RPC stdio loop.
"""

from __future__ import annotations

from .rpc import run_stdio


def main() -> int:
    """Run the JSON-RPC 2.0 stdio server until EOF and return the process exit code."""
    return run_stdio()


if __name__ == "__main__":
    raise SystemExit(main())
