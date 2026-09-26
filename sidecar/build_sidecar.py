"""Build the PyInstaller ``onedir`` bundle for the sidecar (PLAN.md §13, M7).

Run it through the ``package`` extra, so PyInstaller is available without
dragging it into the development environment:

    cd sidecar && uv run --extra package python -m build_sidecar

The result is ``sidecar/packaging/llmtranslator_sidecar/`` (a directory with
the executable and its libraries), which the Tauri bundle declares as the
``llmtranslator_sidecar`` resource. The directory is tracked through a
``.gitkeep`` placeholder so ``cargo check`` works on a clean tree; the build
rewrites it. ``onedir`` is deliberate: faster start-up and fewer antivirus false
positives than ``onefile``, and the supervisor expects a path to an executable.

The heavy optional backends stay out of the bundle (``marker``/``torch`` are an
opt-in extra, PLAN.md constraints).
"""

from __future__ import annotations

import argparse
import importlib.util
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent
DIST = REPO / "packaging"
BUILD = REPO / "build"
NAME = "llmtranslator_sidecar"

#: The placeholder that keeps the resource directory in git, so `cargo check`
#: finds the declared Tauri resource on a tree where the sidecar was never built.
PLACEHOLDER = ".gitkeep"

#: Never bundled: optional heavy backends and unused GUI/tooling imports.
EXCLUDES = (
    "marker",
    "torch",
    "tkinter",
    "IPython",
    "matplotlib",
    "pytest",
    "ruff",
    "pyright",
)


def main() -> int:
    parser = argparse.ArgumentParser(description="Build the PyInstaller onedir sidecar")
    parser.add_argument(
        "--clean",
        action="store_true",
        help="remove build/ and dist/ before building",
    )
    args = parser.parse_args()

    if importlib.util.find_spec("PyInstaller") is None:
        print(
            "PyInstaller is not installed: run `uv run --extra package python -m build_sidecar`",
            file=sys.stderr,
        )
        return 1

    if args.clean:
        shutil.rmtree(BUILD, ignore_errors=True)
        shutil.rmtree(DIST, ignore_errors=True)

    command = [
        sys.executable,
        "-m",
        "PyInstaller",
        "--noconfirm",
        "--clean",
        "--onedir",
        "--console",
        "--name",
        NAME,
        "--distpath",
        str(DIST),
        "--workpath",
        str(BUILD),
        "--specpath",
        str(BUILD),
        "--paths",
        str(REPO),
    ]
    for module in EXCLUDES:
        command += ["--exclude-module", module]
    command.append(str(REPO / "pyinstaller_entry.py"))

    print("+ " + " ".join(command), flush=True)
    completed = subprocess.run(command, check=False)  # noqa: S603 - argv is built here
    if completed.returncode != 0:
        return completed.returncode

    executable = DIST / NAME / NAME
    if sys.platform == "win32":
        executable = executable.with_suffix(".exe")
    if not executable.is_file():
        print(f"expected {executable} after the build", file=sys.stderr)
        return 1
    # PyInstaller replaces the whole output directory; keep the directory tracked.
    (DIST / NAME / PLACEHOLDER).write_text("", encoding="utf-8")
    print(f"built {executable}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
