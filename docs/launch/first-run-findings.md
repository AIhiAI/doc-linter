---
id: first-run-findings
role: doc
kind: explanation
lifecycle: planning
covers: [doc-graph, mcp]
title: First-run findings
summary: What a new user can already see on a fresh repo (coverage table, stale docs, work list), the starter-ontology false-positive story, MCP install and the telemetry audit.
status: draft
updated: 2026-10-08
tags: [launch, first-run]
---

# First-run findings

Static investigation only; nothing here was built or run. Items marked
(unverified) need a real run.

## D. Coverage table and work list

Coverage table: exists.

```bash
doc-linter query coverage-report            # JSON to stdout
doc-linter query coverage-report --write    # also writes docs/coverage-report.md
```

Code: [`src/coverage.rs`](../../src/coverage.rs) (`build_report`, `render_markdown_body`), CLI args in
[`docs/reference/cli.md`](../reference/cli.md) (`## doc-linter query coverage-report`). Per-entity
rows carry `doc_count`, `func_count`, `ratio`, `verdict`, `god_node`. The
`check`-time lint ([`src/cmd/check/coverage.rs`](../../src/cmd/check/coverage.rs), `GraphRead` in `store_sqlite`)
emits `dark-public-function`, `dark-endpoint`, entity gaps and
`coverage-below-min` (`--coverage-min-global`). Related saved queries:
`coverage-by-entity` (functions per entity, no doc side), `endpoint-darkness`.

Work list ("N functions, 0 chapters"): did not exist as a single command.
Closest were `coverage-by-entity` (function count only) and the
`coverage-report` verdicts. Added saved query `concept-work-list`
(`store::query::saved`, end of catalog):

```bash
doc-linter query saved concept-work-list
```

Entities with at least one mentioning function, ranked by narrative-chapter
count ascending then function count descending. Chapters exclude
`ontology-value/-axis/-entity/-migration` and `index` docs. It relies on the
existing guard that runs every saved query on an empty schema to catch
parse errors.

Stale docs: `doc-linter query saved stale-narrative-docs` and
`stale-docs-with-impact` (with inbound wikilinks), `freshness-by-kind`.

## C2. Zero-config first run

Today's path on a fresh repo:

1. `doc-linter init` (writes `.doc-lint.toml`, starter `docs/ontology/`).
2. `doc-linter check` (builds `.doc-lint/graph.sqlite`). `doc-linter query ...`
   also ingests on demand when the DB is absent ([`src/cmd/query.rs`](../../src/cmd/query.rs)), so
   step 2 can be skipped for queries.
3. Optional `doc-linter scip-index` for the code graph. Without SCIP there
   are no Functions, so coverage table and work list are empty (expected).
4. `coverage-report`, `saved stale-narrative-docs`, `saved concept-work-list`.

Why starter docs cause false positives: `init` scaffolds ~20 docs with roles
`ontology-axis`, `ontology-value`, `ontology-entity`, `ontology-migration`
and `index`. They carry fixed `updated` dates that never track code, so any
freshness query that does not filter them reports them as "stale", and any
doc count or chapter count inflates by them.

State of the filters: the stale queries (`stale-narrative-docs`,
`stale-docs-with-impact`, `freshness-by-kind`) and the corpus classifiers
already exclude the three ontology roles. Gaps found:

- The three stale queries exclude only value/axis/entity, not
  `ontology-migration` or `index`. The `kind <> ''` guard in
  `stale-docs-with-impact` masks this there; `stale-narrative-docs` has no
  such guard, so a migration or index doc can still appear. Smallest fix: add
  `'ontology-migration', 'index'` to those lists (not done; trivial, but kept
  separate from this change).
- The new work list had the same risk, so it excludes all five roles.

Update: `stale-narrative-docs`, `stale-docs-with-impact` and
`freshness-by-kind` now exclude all five starter roles (ontology-value,
ontology-axis, ontology-entity, ontology-migration, index) in Cypher and in
the SQL twins, pinned by semantic tests. Other queries that count narrative
docs with the older three-role list (for example `narrative-doc-count` and
the corpus classifiers) are unchanged, since widening them moves
classification thresholds.

`doc-linter report` now exists: it refreshes the graph without needing
`init` or an ontology, then prints the coverage table, the stale docs and
the concept work list, plus candidate concepts mined from the code by the
local proposer (`doc-linter ontology propose`). Code: [`src/cmd/report.rs`](../../src/cmd/report.rs).

Original note, kept for the record. Minimal change for a no-ontology first run: the above filter widening, plus
(optional) a `doc-linter report` wrapper that runs coverage-report, the
stale query and the work list in one call. Not implemented: it would be a new
command needing build and golden tests, and the three commands are already
one line each.

## C7. MCP install

Registration today: a hand-written `.mcp.json` at the workspace root
(`command: ./doc-linter/target/release/doc-linter`, `args: ["mcp","--root",...]`),
and README "MCP setup" / [`docs/quickstart/mcp.md`](../quickstart/mcp.md) show the JSON for Claude
Desktop, Claude Code and Cursor. [`docs/reference/mcp.md`](../reference/mcp.md) is the generated
tool catalog.

Implemented (not compiled, not run): `doc-linter mcp-install --client
claude-code|cursor` in [`src/cmd/mcp_install.rs`](../../src/cmd/mcp_install.rs), wired in [`src/cmd/mod.rs`](../../src/cmd/mod.rs).
It was named `mcp-install` rather than `mcp install` to avoid turning the
existing unit `Mcp` command into a subcommand group (would risk breaking
`doc-linter mcp`).

| Client | File written | Entry | Status |
|---|---|---|---|
| claude-code | `<root>/.mcp.json` | `mcpServers.doc-linter = {command: doc-linter, args: [mcp]}` | documented project scope |
| cursor | `<root>/.cursor/mcp.json` | same, plus `--root <abs path>` | project path from Cursor docs; (unverified) that cwd is not the repo root, hence the pinned root |

Merges into an existing file and preserves other servers; invalid JSON
errors out rather than overwriting. One unit test in the module. Claude
Desktop (global `claude_desktop_config.json`) is not covered. A [`docs/reference/cli.md`](../reference/cli.md)
regen (`doc-linter gen-docs`) is needed after the first build.

## C8. Network and telemetry audit

grep for `reqwest|ureq|TcpStream|hyper|curl|https?://` over `src/`,
`Cargo.toml` and `build.rs`:

- [`src/llm/anthropic.rs`](../../src/llm/anthropic.rs): only `reqwest` user; `reqwest` is behind the
  optional `llm` feature (`Cargo.toml`: `llm = ["dep:reqwest"]`). Opt-in.
- `build.rs`: downloads the pinned bge-small model and tokenizer from
  Hugging Face via system `curl` (SHA-256 verified); skippable with
  `DOC_LINTER_SKIP_MODEL_DOWNLOAD=1`.
- Local `git` subprocesses (`scaffold.rs`, `scip_ingest.rs`,
  `coupling_ingest.rs`, [`src/cmd/homepage.rs`](../../src/cmd/homepage.rs), `endpoint_markers_migrate.rs`):
  no network by themselves.
- [`src/embeddings.rs`](../../src/embeddings.rs) and `saved.rs` matches were comments/query text only.

No telemetry found. Added a "No telemetry" section to `README.md`.
