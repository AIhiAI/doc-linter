---
id: local-concept-proposer
role: doc
kind: explanation
lifecycle: implementing
title: Local concept proposer without an API key (C5 and C3)
summary: Design for a non-LLM proposer that clusters identifiers and doc comments of undocumented code into named candidate concepts, plus the `ontology propose` and `ontology accept` CLI that turns accepted candidates into entity docs with a real narrative stub. Implemented in `src/cmd/concepts.rs`; the open questions below were settled when it was built.
status: draft
updated: 2026-10-08
covers: [doc-graph, onnx, scip]
tags: [roadmap, onboarding, concepts]
---

# Local concept proposer

Plan items C5 (no API key) and C3 (vocabulary in one step). Implemented in [`src/cmd/concepts.rs`](../../src/cmd/concepts.rs) (tests in [`tests/concepts_proposer.rs`](../../tests/concepts_proposer.rs), fixture in `tests/fixtures/concepts/`). Where the build departs from the text below, the Decisions section says so.

## What exists today

- LLM use is only the contradiction judge: `Judge` trait [`src/llm/mod.rs`](../../src/llm/mod.rs), Anthropic backend [`src/llm/anthropic.rs`](../../src/llm/anthropic.rs) behind feature `llm`, consumed by [`src/cmd/check/contradictions.rs`](../../src/cmd/check/contradictions.rs) and [`src/cmd/mcp.rs`](../../src/cmd/mcp.rs). Nothing proposes concepts with an LLM, so C5 is not "replace the LLM" but "add a proposer that needs none".
- `doc-linter ontology` is a flat command that dumps the ontology as JSON ([`src/cmd/ontology.rs`](../../src/cmd/ontology.rs), `Commands::Ontology` at [`src/cmd/mod.rs`](../../src/cmd/mod.rs):53,450). It has no subcommands.
- `doc-linter cluster` ([`src/cmd/cluster.rs`](../../src/cmd/cluster.rs)) already runs LPA, Louvain or Leiden over Function, Doc, Entity, Type and Module nodes (`typed::cluster_nodes` and `typed::cluster_edges`), derives `suggested_id`, a synthesized description (`synthesize_description`), `source_files`, and with `--write` emits `status: auto` stubs to `docs/ontology/entities/candidates/` (`emit_candidate_stubs`, `render_candidate_stub`). `--promote <id>` moves a reviewed stub into `docs/ontology/entities/` as `status: stable` (`promote_candidate`). Weaknesses: clusters come from the call graph only, so sparse undocumented code names poorly, and the stub has no narrative doc, which trips the orphan-entity rule (`Issue::OrphanEntity`, [`src/validator/messages.rs`](../../src/validator/messages.rs):278).
- `store::symbol_tokens` splits a SCIP symbol into snake and camel tokens of length 3 or more; [`src/scaffold.rs`](../../src/scaffold.rs) already maps functions to entities through it.
- `store::unstubbed_concepts_ingest` mines proper-noun candidates from `Doc.summary` into `Finding(kind="unstubbed-concept")`. Useful on the doc side, blind to undocumented code.
- Embeddings: `Embedder` trait and tract-ONNX `bge-small-en-v1.5` in [`src/embeddings.rs`](../../src/embeddings.rs) (`default_embedder`, `NoOpEmbedder`); vectors live in `Function.doc_comment_embedding` when `check --embeddings` ran. Opt-in; BM25-only by default.

## Design

Two stages: group, then name. Everything is deterministic and local; the embedding signal is optional.

### Signals (per Function and Type)

1. Identifier tokens: `symbol_tokens(symbol)` plus tokens of the file stem and directory names.
2. Doc-comment tokens: `Function.doc_comment` terms, with stopwords and generic verbs (get, set, new, impl, handle) removed.
3. Structure: existing graph edges (CALLS, USES_TYPE, same file, same module), as in `cluster`.
4. Optional semantic: cosine similarity of stored embeddings, adding an edge between nodes above a threshold (default 0.80) when embeddings are present.

### Grouping

Reuse `cluster::discover_clusters`; no new algorithm. Add two edge sources to its input so sparse code still clusters: token-affinity edges between functions sharing a rare token (weighted by inverse document frequency, fan-out capped per token), and the optional embedding edges. Default algorithm `leiden`, `min_members` 3.

### Naming

Rank candidate names per community by TF-IDF over member tokens (IDF computed against all functions in the repo). Tokens in type names weigh 2x, tokens in doc comments 1.5x. Name = top token, or top bigram when the bigram appears in at least half the members' symbols. Reject names already in the ontology (`TermIndex`), in the symbol stop list (`build_symbol_stop_list`), or shorter than 4 characters. On collision, use the next bigram; never a numeric suffix (drop the candidate with a note instead). Output carries `alt_names` (next three) so the user can rename at accept time.

### Optional local-model hook

Config `[concepts] namer_command = "<program> [args]"` in `.doc-lint.toml`. When set, `propose` pipes one JSON object per candidate (tokens, top doc comments, top symbols) to the program's stdin and reads `{"name","description"}` from stdout, falling back to the TF-IDF name on any error or timeout. This supports llama.cpp, Ollama or any script with no new dependency, no HTTP client and no network code in doc-linter. An in-process model is out of scope.

## CLI surface (C3)

Give `Commands::Ontology` an optional subcommand so bare `doc-linter ontology` keeps printing the JSON dump.

```
doc-linter ontology propose [--top-n 20] [--min-members 3]
                            [--algorithm leiden] [--embeddings]
                            [--json] [--out .doc-lint/proposals.json]
doc-linter ontology accept <name>... [--rename old=new]... [--dry-run]
doc-linter ontology accept --all [--min-confidence 0.5] [--dry-run]
```

- `propose` reads the graph (needs a prior `check`; otherwise errors with "run `doc-linter check` first"), prints a table (name, members, confidence, top files, a sample doc comment) and writes the proposals JSON. That file is the only state `accept` reads, so `accept` never re-clusters and is deterministic between the two commands. `propose` writes nothing under `docs/`.
- `accept` refuses names not in the proposals file and names that already have `docs/ontology/entities/<name>.md` (same rule as `emit_candidate_stubs`). Exit 0 on success, 1 if any name was refused, 2 on internal error.
- `accept` writes only values the repo's ontology registers: the `doc` role, kind `explanation` (else `reference`, else the first registered), lifecycle `draft` (else `implementing`, else the first the role allows). A repo with no ontology has no `doc` role, so `accept` refuses with "run `doc-linter init` first" and writes nothing.
- `cluster --write` and `--promote` stay as the low-level path; `ontology accept` is the first-run path and writes `status: stable` directly, keeping `confidence`.

### What accept writes (two files per concept)

1. `docs/ontology/entities/<name>.md`: the frontmatter of `render_candidate_stub` (`role: ontology-entity`, `axis_id: covers`, `value_id`, `display`, `description`, `synonyms` from the next-ranked tokens, `source_modules`), `status: stable`.
2. `docs/explanations/concept-<name>.md`: a narrative doc, `role: doc`, `kind: explanation`, `lifecycle: draft`, `status: draft`, `covers: [<name>]`, with real content from the code and no empty placeholders:
   - Summary: first sentence of the best member doc comment, else the synthesized description.
   - "What it contains": top 5 functions by in-degree with signature and the first line of their doc comment, plus the cluster's types.
   - "Where it lives": source files and how many functions each contributes.
   - "Doc comments in the code": up to 5 verbatim doc comments with file and line.
   - A short "For the author" list is allowed only after that content, naming what the code could not tell us (why it exists, invariants).

Accept refuses a concept, writing neither file, when fewer than 2 members have identifier or doc-comment content to cite, so no orphan entity is created. This follows the rule that every entity ships with the narrative that justifies it; `OrphanEntity` must not fire on the output.

## Data flow

```
check (SCIP + docs) -> graph (Function, Type, CALLS, doc_comment, [embeddings])
  -> ontology propose: edges (calls + token-affinity + [embedding])
       -> discover_clusters -> name (TF-IDF [+ namer_command]) -> proposals.json + table
  -> ontology accept: read proposals.json, read member details from the graph
       -> write entities/<name>.md + explanations/concept-<name>.md
  -> check (re-run): entity has a covering doc; vocabulary closure picks it up
```

## Test plan

Fixture: `tests/fixtures/concepts/`, three small source trees (about 6 functions each, one with no doc comments) in three domains (invoice billing, user session, report export), plus cross-domain noise functions (`new`, `helper`). Graph built through the helpers the `cluster` tests and [`tests/scip_pipeline.rs`](../../tests/scip_pipeline.rs) already use.

1. Unit: TF-IDF naming returns `invoice` or `billing` for the billing cluster; `new` and `get` are never chosen.
2. Unit: token-affinity edges join two call-disconnected functions sharing a rare token and do not join on a common one.
3. Integration: `ontology propose --json` on the fixture yields at least 3 candidates with expected names and is byte-identical across two runs.
4. Integration: `ontology accept billing` writes both files; the narrative contains a fixture function name and a verbatim fixture doc comment; `check` on the result reports no `OrphanEntity` and no vocabulary errors.
5. Refusal: accepting an existing entity id exits 1 and writes nothing; a thin cluster writes neither file.
6. Backwards compatibility: bare `ontology` prints the same JSON as before.
7. Hook: a stub `namer_command` returning a fixed name is used; failure or timeout falls back to TF-IDF.
8. Embeddings: guarded by the feature and by vector presence; skipped with a clear message under `NoOpEmbedder`.

Graph and MCP tests no longer need `--test-threads=1` now that Kuzu is gone (the repo still runs them that way).

## Decisions taken at build time

1. A single dominant token is an acceptable name (`user`, `invoice`); a stoplist of generic words (get, set, new, handle, helper, util, data, config and similar, plus `vale_ambiguous_words`) rejects the generic ones. `code_comments::COMMENT_STOPWORDS` is not merged: it lists real domain nouns such as Session and Report.
2. Accepted narrative docs are written directly to `docs/explanations/concept-<name>.md` and the entity to `docs/ontology/entities/<name>.md`; there is no staging directory.
3. There is no `accept --all`. Every entity comes with a narrative built from real code (top functions with signatures, types, files, verbatim doc comments with file and line). A cluster with fewer than 2 members that have a signature or doc comment to cite is refused and writes nothing.
4. The proposer reads only through `store::typed` (`concept_symbols`, `concept_calls`, `concept_embeddings`). Stored embeddings live in the vector index and are not readable as a column, so `--embeddings` prints that it found nothing to read and proceeds without those edges.
5. Languages whose indexer emits no SCIP documentation propose from identifiers alone, as before (see [`docs/launch/language-support.md`](../launch/language-support.md)).
6. The narrative uses `lifecycle: draft` when the ontology defines it, else `implementing` (the starter ontology has no `draft`), and `kind: explanation` when defined.
7. `[concepts] namer_command` is split on whitespace (no shell). `DOC_LINTER_NAMER_TIMEOUT_MS` overrides the 10 second timeout, mainly for tests.
