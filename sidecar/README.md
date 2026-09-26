# llmtranslator-sidecar

The **data plane** of LocalLLMTranslator: document formats, Markdown IR, chunking, placeholders
and Pandoc typesetting.

Rules the package never breaks:

- **Stateless.** No cache between calls, no database, no temporary files beyond those declared
  in `work_dir`. Every RPC method is a pure `input → output` function, so restarting the process
  and re-sending in-flight requests is always safe.
- **Data only.** It does not know about the job queue, it does not know what a project is, and
  it never talks to an LLM. All of that belongs to the Rust control plane.
- **Faithful to the source.** `serialize(split_blocks(md)) == md`, byte for byte.

Usage:

```sh
uv sync --all-groups
uv run python -m llmtranslator_sidecar    # JSON-RPC 2.0 server over stdio, NDJSON
uv run pytest
```
