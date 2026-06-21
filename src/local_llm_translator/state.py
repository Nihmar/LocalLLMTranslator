from __future__ import annotations

import json
import logging
from dataclasses import asdict, dataclass, field
from datetime import UTC, datetime
from hashlib import sha256
from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from pathlib import Path

_LOGGER = logging.getLogger(__name__)


@dataclass
class SectionState:
    heading: str
    level: int
    original_text: str
    translated_text: str | None


_FILENAME = "TRANSLATION_STATE.json"

_SectionDict = dict[str, Any]


@dataclass
class TranslationState:
    input_hash: str
    output_dir: str
    target_language: str
    style: str
    sections: list[_SectionDict] = field(default_factory=list)  # pyright: ignore[reportUnknownVariableType]
    created_at: str = ""
    updated_at: str = ""

    def completed_indices(self) -> list[int]:
        """Return indices of sections that have a translation (computed, not stored)."""
        return sorted(i for i, s in enumerate(self.sections) if s.get("translated_text"))

    @staticmethod
    def _hash_file(path: Path) -> str:
        return sha256(path.read_bytes()).hexdigest()

    @classmethod
    def new(
        cls, input_path: Path, output_dir: Path, target_language: str, style: str
    ) -> TranslationState:
        now = datetime.now(UTC).isoformat()
        return cls(
            input_hash=cls._hash_file(input_path),
            output_dir=str(output_dir),
            target_language=target_language,
            style=style,
            created_at=now,
            updated_at=now,
        )

    @classmethod
    def load(cls, output_dir: Path) -> TranslationState | None:
        path = output_dir / _FILENAME
        if not path.exists():
            return None
        try:
            data: dict[str, Any] = json.loads(path.read_text(encoding="utf-8"))
            # Drop legacy field if present
            data.pop("completed_indices", None)
            return cls(**data)
        except (json.JSONDecodeError, KeyError, TypeError) as e:
            _LOGGER.warning("Failed to load state file: %s", e)
            return None

    def save(self, output_dir: Path) -> None:
        self.updated_at = datetime.now(UTC).isoformat()
        path = output_dir / _FILENAME
        data = asdict(self)
        path.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
        _LOGGER.debug("State saved to %s", path)

    def is_compatible(self, input_path: Path) -> bool:
        return self._hash_file(input_path) == self.input_hash
