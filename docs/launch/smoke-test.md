---
id: smoke-test
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph]
title: Smoke test of the SQLite-only build
summary: End-to-end run of the SQLite-only build on a tiny directory and a copy of this repo, with the commands used and what each produced.
status: draft
updated: 2026-10-08
tags: [launch, testing]
---

# Smoke test

End-to-end run of the SQLite-only build (branch `chore/apache-2-licence`, debug binary) on
two clean directories under a scratch dir: `tiny/` (3 markdown docs, one Rust file and a
`Cargo.toml`, not a git repo, no `init`, no ontology, no config) and `copy/` (this repo
copied with `tar` minus `target/` and `.git`). `$DL` below is the built `doc-linter` binary
and every command runs with the named directory as the working directory.

## 1. `report` with no ontology and no init (tiny, before any code index)

```
$ doc-linter report
doc-linter: repo ingest — 1 row(s)
doc-linter: file/module ingest — 1 files, 0 modules, ...
doc-linter: coupling ingest — `git log` exited non-zero (exit status: 128); skipping COUPLED_WITH derivation
COVERAGE
  No code index yet (0 functions). Run `doc-linter scip-index`, then `doc-linter report` again.
STALE DOCS (stalest first; starter ontology docs excluded)
  2026-10-08              billing                      Billing
  2026-10-08              readme                       Overview
  2026-10-08              shipping                     Shipping
CONCEPT WORK LIST
  Nothing to list yet: no entity is mentioned by code and no candidate concept was found.
```

Result: exit 0, no crash, no `init` needed. The `git log` line is expected in a non-git directory.

The same command on `copy/` (this repo, its own `.doc-lint.toml`) lists the 10 stalest docs
and the same "No code index yet" hint.

## 2. `scip-index`, `check`, `query saved concept-work-list`, `ontology propose` (tiny)

```
$ doc-linter scip-index
doc-linter: wrote .../tiny/.doc-lint/code.scip (1469 bytes from 1 indexer(s))
$ doc-linter check
README.md: missing frontmatter — add `---\n...\n---` block at the top with id/role/title/summary/status/updated
docs/billing.md: missing frontmatter — ...
docs/billing.md: line 3: vale[warning] DocLinter.AmbiguousBare: Bare ambiguous noun 'service' — ...
docs/shipping.md: missing frontmatter — ...
4 error(s), 0 warning(s) across 3 file(s)
$ doc-linter query saved concept-work-list
{ "columns": ["entity","functions","chapters"], "row_count": 0, "rows": [] }
$ doc-linter ontology propose
No candidate concepts found (min-members 3).
```

Result: `check` exits 1 on the missing frontmatter, as designed. The empty work list and the
empty proposal are correct for a two-function repo (the proposer needs at least 3 members).
Before `scip-index`, `ontology propose` instead says
`the graph has no Function or Type rows: run doc-linter scip-index then doc-linter check first`.

## 3. This repo's copy

```
$ doc-linter check          # no SCIP index
... 274 error(s), 0 warning(s) across 13 file(s)
$ doc-linter query saved concept-work-list     # empty, row_count 0
$ doc-linter ontology propose
doc-linter: the graph has no Function or Type rows: run `doc-linter scip-index` then `doc-linter check` first
$ doc-linter scip-index
rust-analyzer: Loading building proc-macros: doc-linter
memory allocation of 98304 bytes failed
doc-linter: rust indexer exited with signal: 6 (SIGABRT) — likely the 4096 MiB cap; raise DOC_LINTER_SCIP_MEMORY_MB
```

Notes: the 274 errors are mostly `endpoint ... has no ontology entity link` because the copy
has no code index; the real repo lints clean with its index. The SCIP run hit the default
4 GiB indexer cap on the shared machine (the message names the fix); it was not retried
with a bigger cap, so a full-repo SCIP run is not covered here.

## 4. `mcp-install`

```
$ mkdir mi && cd mi && doc-linter mcp-install --client claude-code
wrote doc-linter MCP entry to .../mi/.mcp.json
$ cat .mcp.json
{"mcpServers": {"doc-linter": {"args": ["mcp"], "command": "doc-linter"}}}
```

## 5. `mcp` handshake over stdio (tiny)

Input (three lines): `initialize`, the `notifications/initialized` notification, `tools/list`.

```
$ doc-linter mcp < req.jsonl
{"id":1,"jsonrpc":"2.0","result":{"capabilities":{"tools":{}},"protocolVersion":"2024-11-05","serverInfo":{"name":"doc-linter","version":"0.3.1"}}}
{"id":2,"jsonrpc":"2.0","result":{"tools":[{"description":"Run a read-only SQL query over the entity-doc-graph (SQLite). ...
```

`tools/list` returned 29 tools (`sql`, `list_entities`, `list_docs`, `query_saved`,
`query_schema`, `context_for`, `reingest`, ...). The process exits 0 at end of input.

## 6. Store schema version (tiny)

The graph carries `PRAGMA user_version = 1` and `meta.schema_version = '1'`. After
setting `user_version` to 99 with Python's `sqlite3`:

```
$ doc-linter query saved concept-work-list
doc-linter: doc graph .../.doc-lint/graph.sqlite has store schema version 99, this doc-linter needs 1 — run `doc-linter check` to rebuild it     (exit 2)
$ doc-linter mcp < tool-call.jsonl
doc-linter: open the graph for mcp: doc graph ... has store schema version 99, this doc-linter needs 1 — run `doc-linter check` to rebuild it
$ doc-linter report        # refreshes the graph: rebuilds, prints the normal report
$ doc-linter check         # rebuilds as well; saved queries work again
```

See [Store schema](../reference/store-schema.md) for the policy.

## Bugs found

None needing a fix. Observations: the git-exit-128 line in a non-git directory is noisy but
harmless; a full SCIP index of this repo needs more than the 4 GiB default cap on this machine.
