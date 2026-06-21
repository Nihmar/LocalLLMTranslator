import asyncio
import logging

from local_llm_translator.app import TranslatorTUI
from local_llm_translator.config import build_config


def main() -> None:
    config = build_config()

    config.output_dir.mkdir(parents=True, exist_ok=True)
    log_path = config.output_dir / "translation.log"

    logging.basicConfig(
        level=logging.DEBUG if config.debug else logging.INFO,
        format="%(asctime)s [%(levelname)s] %(name)s: %(message)s",
        handlers=[
            logging.FileHandler(str(log_path), encoding="utf-8"),
            logging.StreamHandler(),
        ],
    )

    app = TranslatorTUI(config)
    asyncio.run(app.run_async())


if __name__ == "__main__":
    main()
