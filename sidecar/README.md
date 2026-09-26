# llmtranslator-sidecar

Il **data plane** di LocalLLMTranslator: formati di documento, Markdown IR, chunking,
placeholder e impaginazione Pandoc.

Regole che il pacchetto non viola mai:

- **Senza stato.** Nessuna cache tra chiamate, nessun database, nessun file temporaneo fuori da
  quelli dichiarati in `work_dir`. Ogni metodo RPC è una funzione pura `input → output`, quindi
  è sempre sicuro riavviare il processo e ri-inviare le richieste in volo.
- **Solo dati.** Non conosce la coda dei job, non sa cosa sia un progetto, non parla mai con un
  LLM. Tutto ciò appartiene al control plane in Rust.
- **Fedeltà alla sorgente.** `serialize(split_blocks(md)) == md`, byte per byte.

Uso:

```sh
uv sync --all-groups
uv run python -m llmtranslator_sidecar    # server JSON-RPC 2.0 su stdio, NDJSON
uv run pytest
```
