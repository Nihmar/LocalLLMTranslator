from __future__ import annotations

import time
from typing import TYPE_CHECKING

from textual.app import App, ComposeResult
from textual.reactive import reactive
from textual.widgets import Header, ProgressBar, RichLog, Static

from local_llm_translator.translator import run_translation

if TYPE_CHECKING:
    from local_llm_translator.config import Config


class TranslatorTUI(App[None]):
    TITLE = "LocalLLMTranslator"

    CSS = """
    Screen {
        layout: vertical;
    }

    #info-bar {
        height: 3;
        padding: 0 1;
    }

    #log {
        height: 1fr;
        border: solid $primary;
        margin: 0 1;
    }

    #progress-area {
        height: 3;
        padding: 0 1;
    }

    ProgressBar {
        width: 1fr;
    }
    """

    progress = reactive(0.0)
    status_text = reactive("Starting...")

    def __init__(self, config: Config) -> None:
        super().__init__()
        self._config = config
        self._start_time: float = 0.0
        self._total = 0
        self._done = 0

    def compose(self) -> ComposeResult:
        yield Header()
        lang = self._config.target_language
        yield Static(
            f"[bold]{self._config.input_path.name}[/] → {lang} ({self._config.style})",
            id="info-bar",
        )
        yield RichLog(id="log", highlight=True, markup=True)
        yield ProgressBar(id="progress-bar", total=100)
        yield Static(id="timer", classes="text-center")

    def on_mount(self) -> None:
        self._start_time = time.monotonic()
        self.run_translation_worker()  # type: ignore[unused_coroutine]

    async def run_translation_worker(self) -> None:
        def on_progress(done: int, total: int, heading: str) -> None:
            self.call_from_thread(self._update_ui, done, total, heading)

        await run_translation(self._config, on_progress=on_progress)

        elapsed = time.monotonic() - self._start_time
        mins, secs = divmod(int(elapsed), 60)
        self.call_from_thread(
            self.query_one("#timer", Static).update,
            f"[green]Done in {mins}:{secs:02d}[/]",
        )
        self.call_from_thread(
            self.query_one("#log", RichLog).write,
            "[bold green]✓ Translation complete![/]",
        )

    def _update_ui(self, done: int, total: int, heading: str) -> None:
        self._total = total
        self._done = done
        pct = (done / total) * 100 if total else 0

        log = self.query_one("#log", RichLog)
        log.write(
            f"{'[green]✓[/]' if done > 0 else '[yellow]⟳[/]'} "
            f"[{done}/{total}] {heading or '(no heading)'}"
        )

        self.query_one("#progress-bar", ProgressBar).update(progress=pct)

        elapsed = time.monotonic() - self._start_time
        mins, secs = divmod(int(elapsed), 60)
        remaining_secs = (elapsed / done * (total - done)) if done else 0
        rem_mins, rem_secs = divmod(int(remaining_secs), 60)
        self.query_one("#timer", Static).update(
            f"Elapsed: {mins}:{secs:02d}  Remaining: {rem_mins}:{rem_secs:02d}"
        )
