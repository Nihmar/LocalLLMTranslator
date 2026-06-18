import asyncio
import logging

from local_llm_translator.app import TranslatorTUI
from local_llm_translator.config import build_config


def main() -> None:
    config = build_config()

    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
    )

    app = TranslatorTUI(config)
    asyncio.run(app.run_async())


if __name__ == "__main__":
    main()
