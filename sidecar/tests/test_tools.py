"""Support-tooling test entry point.

The support-tooling pytest suite lives in ``tests/test_support_tools.py`` (the
file named by the project's verification command, so a parallel sidecar test
file can safely occupy this ``test_tools.py`` slot).  This module only guards
that the suite is present, so ``pytest tests/test_tools.py`` is meaningful
without collecting every test twice.
"""

from __future__ import annotations

from pathlib import Path

TESTS_DIR = Path(__file__).resolve().parent


def test_support_tooling_suite_is_present() -> None:
    suite = TESTS_DIR / "test_support_tools.py"
    assert suite.is_file(), "the support-tooling suite test_support_tools.py is missing"
    assert suite.read_text(encoding="utf-8").strip(), "the support-tooling suite is empty"
