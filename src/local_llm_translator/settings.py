from __future__ import annotations

import os
import tomllib
from pathlib import Path
from typing import Any

_CONFIG_DIR = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config"))
CONFIG_PATH = _CONFIG_DIR / "local-llm-translator" / "config.toml"

_DataDir = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local" / "share"))
DATA_DIR = _DataDir / "local-llm-translator"


def load_settings() -> dict[str, Any]:
    """Load persistent settings from the XDG config file.

    Returns an empty dict if the file does not exist or cannot be parsed.
    CLI arguments always override these values.
    """
    if not CONFIG_PATH.exists():
        return {}

    try:
        return tomllib.loads(CONFIG_PATH.read_bytes().decode("utf-8"))
    except tomllib.TOMLDecodeError, OSError:
        return {}
