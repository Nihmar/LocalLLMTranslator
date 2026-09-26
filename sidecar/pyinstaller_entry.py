"""PyInstaller entry point for the packaged sidecar.

The real entry point is :mod:`llmtranslator_sidecar.__main__`; this module exists
because PyInstaller analyses a script, and a script that imports the package is
more reliable than pointing the bundler at ``__main__.py`` (whose relative
imports assume the package form).
"""

from __future__ import annotations

from llmtranslator_sidecar.__main__ import main

if __name__ == "__main__":
    raise SystemExit(main())
