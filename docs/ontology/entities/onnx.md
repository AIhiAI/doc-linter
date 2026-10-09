---
id: entity-onnx
role: ontology-entity
title: "Entity: ONNX"
summary: ONNX is the inference runtime format used for the FLOAT[384] embedding vectors backing doc-linter's semantic similarity search.
status: stable
updated: 2026-10-08
axis_id: covers
value_id: onnx
display: ONNX
description: The portable neural-network model format doc-linter uses to embed Doc / Function / Entity text into 384-dim sentence vectors. The default bundled model is `bge-small-en-v1.5`; the runtime is the pure-Rust [`tract`](https://github.com/sonos/tract) crate, so no Python or C++ ONNX Runtime binary is required.
synonyms:
  - onnx-runtime
  - embeddings-runtime
  - tract
source_modules:
  - src/embeddings.rs
  - src/store_sqlite/vectors.rs
  - src/query.rs
  - build.rs
introduced_in_version: 1
---

# Entity — ONNX

ONNX (Open Neural Network Exchange) is a portable model-serialisation format that lets a model trained in PyTorch / TensorFlow / JAX run under any conformant inference runtime. doc-linter uses it for one specific job: turning the prose on every `Doc`, `Function`, and `Entity` node into a 384-dim sentence vector so `query similar --backend embedding` can rank by cosine similarity rather than BM25 term overlap. The default bundled model is [`bge-small-en-v1.5`](https://huggingface.co/BAAI/bge-small-en-v1.5) (33M params, 384-dim output, BERT-family encoder); the runtime is the pure-Rust [`tract`](https://github.com/sonos/tract) crate. Picking tract over `ort` keeps the build hermetic — no C++ toolchain, no auto-downloaded ONNX Runtime shared library — so a default `cargo install doc-linter` produces a working semantic-search binary on every host with a Rust toolchain.

The embedder factory in [`embeddings::default_embedder`](../../../src/embeddings.rs) has three resolution layers, tried in order:

1. **Operator override** — when `DOC_LINTER_EMBED_MODEL` and `DOC_LINTER_EMBED_TOKENIZER` are both set, `OnnxEmbedder::try_from_env` loads the model + `tokenizer.json` they point at. This is how an operator plugs in a different fine-tune, a different language, or a larger model.
2. **Build-time bundle** — `build.rs` (gated by `--features embeddings`, on by default) downloads the quantized `bge-small-en-v1.5` export from Xenova's HuggingFace mirror into the user cache (`$XDG_CACHE_HOME/doc-linter/models/bge-small-en-v1.5/`) and bakes the absolute paths into the binary via `DOC_LINTER_BUNDLED_EMBED_MODEL` / `DOC_LINTER_BUNDLED_EMBED_TOKENIZER`. `option_env!()` reads them at runtime, so this is the "just works after `cargo install --features embeddings`" path.
3. **NoOp fallback** — neither layer resolved (default feature disabled, or both download and override failed). The pipeline still runs end-to-end; no vectors are indexed and `query similar --backend embedding` errors with a clear "rebuild with default features" hint rather than silently returning empty hits.

Where the vectors land: [`store_sqlite::vectors::populate`](../../../src/store_sqlite/vectors.rs) runs once near the end of `doc-linter check --embeddings` and embeds `Doc` summaries, `Entity` display names, `Function` / `Type` doc comments and `Section` text into one usearch HNSW index file beside the graph (`.doc-lint/graph.vec.*`, named by `meta.vec_file` inside the db and swapped in with it); a text-fingerprint cache table keeps vectors across rebuilds. From there both the `query similar --backend embedding` CLI command and the `query_similar` MCP tool (with `backend: "embedding"`) compute the query vector with the same `default_embedder()` and ask the index for the nearest rows by cosine similarity (a dot product, because every indexed vector is L2-normalised at write time).

Authoring rule of thumb: ONNX-backed semantic search is opt-in at two layers. The `embeddings` Cargo feature is on by default but can be dropped via `--no-default-features` for air-gapped CI; the `--embeddings` flag on `doc-linter check` is off by default because the populate pass is the dominant cost on a full rebuild (re-encoding every Doc / Function / Entity row through a transformer). BM25 over the same text columns is the always-on fallback, so `query similar` returns meaningful results even on a fresh checkout that has never run the populate pass.

## Related

- [[entity-doc-graph]]
- [[axis-covers]]
- [[ontology-mig-0001]]
