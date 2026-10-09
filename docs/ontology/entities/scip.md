---
id: entity-scip
role: ontology-entity
title: "Entity: SCIP"
summary: SCIP is the code-index format doc-linter ingests to derive Function / Type / file / symbol facts that anchor every cross-reference between code and docs.
status: stable
updated: 2026-10-08
axis_id: covers
value_id: scip
display: SCIP
description: Sourcegraph's Source Code Intelligence Protocol — a binary index describing every defined and referenced symbol in a codebase with cross-references. doc-linter consumes SCIP files produced by `rust-analyzer scip` (Rust), `scip-python` (Python), and `scip-typescript` (TypeScript) to populate the code half of the doc graph.
synonyms:
  - scip-index
  - code-index
  - scip-symbol
  - symbols
source_modules:
  - src/scip_ingest.rs
  - src/scip_ingest_cache.rs
  - src/cmd/scip_index.rs
  - src/store/symbols/**/*.rs
introduced_in_version: 1
---

# Entity — SCIP

[SCIP](https://github.com/sourcegraph/scip) — Sourcegraph's Source Code
Intelligence Protocol — is a language-agnostic binary format describing
every defined and referenced symbol in a codebase, with cross-references
between them. Each symbol is a structured string (package, descriptor
path, suffix kind) that is stable across files and rebuilds, which makes
it the natural primary key for the code half of the [[entity-doc-graph]].
doc-linter doesn't produce SCIP itself; it consumes indexes emitted by
the language-specific producers — `rust-analyzer scip` for Rust,
`scip-python` for Python, `scip-typescript` for TypeScript.

doc-linter touches SCIP at two well-defined surfaces. `doc-linter
scip-index` ([`src/cmd/scip_index.rs`](../../../src/cmd/scip_index.rs))
shells out to the appropriate indexer with memory caps and idle
scheduling, writing the result to `<repo>/.doc-lint/code.scip`. Then on
the next `doc-linter check`, `scip_ingest::run`
([`src/scip_ingest.rs`](../../../src/scip_ingest.rs)) parses the SCIP
protobuf, classifies each `SymbolInformation` into a `FunctionKind`
(function, method, struct, enum, trait, module, type-alias), and emits
`Function` / `Type` nodes plus `FUNCTION_DEFINED_IN` /
`FUNCTION_MENTIONS` / `TYPE_DEFINED_IN` / `TYPE_MENTIONS` /
`FUNCTION_BELONGS_TO` edges into the SQLite graph. Unchanged `.scip` files short-
circuit through `scip_ingest_cache` so re-runs only rederive the cross-
bucket edges.

The SCIP symbol string is load-bearing: it is the primary key on
`Function.symbol` and `Type.symbol` in the SQLite schema, and the
tokenizer in [`src/store/symbols/`](../../../src/store/symbols/mod.rs)
turns it into the searchable descriptor-path tokens that drive
entity-mention matching. Everything that links code to docs —
`covers` resolution, dark-function detection, query-impact analysis,
`function_context` lookups — ultimately joins on a SCIP symbol.

Authoring rule of thumb: if a finding or query crosses cross-language
code structure (functions, types, files, modules, call graph, import
graph), it goes through SCIP. If it is purely about markdown frontmatter,
wikilinks, or the ontology vocabulary, SCIP is not involved.

## Related

- [[entity-doc-graph]]
- [[lsp]]
- [[axis-covers]]
- [[ontology-mig-0001]]
