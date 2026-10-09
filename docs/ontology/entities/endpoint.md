---
id: entity-endpoint
role: ontology-entity
title: "Entity: Endpoint"
summary: Endpoint is the typed node for any handler the linter discovers (axum, clap, mcp, fastapi, flask, express), the foothold for the dark-endpoint and entity-reach diagnostics.
status: stable
updated: 2026-10-08
axis_id: covers
value_id: endpoint
display: Endpoint
description: The typed [[entity-doc-graph]] node representing any handler doc-linter can statically discover from source. Six `EndpointKind` variants are supported today — `axum`, `clap`, `mcp`, `fastapi`, `flask`, `express` — each with its own per-framework extractor.
synonyms:
  - endpoint-handler
  - endpoint-row
  - dark-endpoint
source_modules:
  - src/endpoint_extract.rs
  - src/store/endpoint_ingest.rs
  - src/store_sqlite/ingest/misc.rs
  - src/endpoint_markers_migrate.rs
introduced_in_version: 1
---

# Entity — Endpoint

`Endpoint` is the typed node doc-linter uses for any handler it can
statically discover from source. The SQLite schema declares it as a
first-class node table
([`src/store_sqlite/schema.rs`](../../../src/store_sqlite/schema.rs)) with
seven columns: `id` (primary key, shaped `"<kind>:<method>:<path>"` —
e.g. `"axum:GET:/v1/outlets/:id"`, `"clap:cli:scip-index"`,
`"mcp:mcp:propose_rule_edit"`), `kind`, `method`, `path`,
`handler_symbol`, `source_file`, and `source_line`. The
`handler_symbol` column carries the [[entity-scip]] symbol of the
implementing function, which is what joins Endpoint back into the
Function / Type code half of the [[entity-doc-graph]]. Six per-framework
extractors in [`src/endpoint_extract.rs`](../../../src/endpoint_extract.rs)
emit rows for the `EndpointKind` variants — `Axum`, `Clap`, `Mcp`,
`Fastapi`, `Flask`, `Express` — and
[`src/store_sqlite/ingest/misc.rs`](../../../src/store_sqlite/ingest/misc.rs)
writes them in a single pass during `doc-linter check`, resolving each
handler through the `FunctionIndex` in
[`src/store/endpoint_ingest.rs`](../../../src/store/endpoint_ingest.rs).

Two outgoing edges anchor the typed surface. `ENDPOINT_HANDLED_BY`
(Endpoint → Function) joins each handler row to the SCIP-derived
Function node that implements it, so any blast-radius / impact query
that walks `CALLS` backwards from a handler crosses this edge first.
`ENDPOINT_TOUCHES_ENTITY` (Endpoint → Entity) is populated from the
handler's doc-comment: every wikilink or ontology reference in the prose
above the handler resolves into one of these edges. An endpoint with
zero `ENDPOINT_TOUCHES_ENTITY` edges is a "dark endpoint" — a handler
whose author never wrote prose connecting it to the ontology, which is
the lint surface the `dark-endpoint` diagnostic in
[`docs/error-codes.md`](../../error-codes.md) reports against.

Author-facing surfaces all flow from those two edges. The `dark-endpoint`
finding fires from
[`src/cmd/check/coverage.rs`](../../../src/cmd/check/coverage.rs) on
every `doc-linter check`; the `doc-linter query endpoints --dark` CLI
subcommand
([`src/query.rs`](../../../src/query.rs)) lists every offender as JSON
filterable by `--kind {axum,clap,mcp,fastapi,flask,express}`; and the
`query_endpoints` MCP tool
([`src/cmd/mcp.rs`](../../../src/cmd/mcp.rs)) exposes the same query
under JSON-RPC with `dark: true`, plus the recently-added `entity_id`
filter (PR #219, issue #207) that turns the query around — "which
endpoints touch this entity?" — so an agent can scope a refactor to
every handler that references a given ontology node.

Authoring rule of thumb: if a handler has its own per-framework macro
or decorator — an axum router method (`.route("/p", get(h))`), a clap
subcommand attribute, an MCP `tools/call` registration, a FastAPI
`@router.get` decorator, a Flask `@app.route`, an Express
`app.get("/p", h)` — it belongs in the Endpoint table. Adding a seventh
framework is a two-file change: a new variant on
`EndpointKind` in [`src/endpoint_extract.rs`](../../../src/endpoint_extract.rs)
with its extractor function and a tree-sitter query, plus a token in
the `as_str` / `parse` pair. The schema and the ingest path in
[`src/store_sqlite/ingest/misc.rs`](../../../src/store_sqlite/ingest/misc.rs)
are stable and do not move on a new framework.

## Related

- [[entity-doc-graph]]
- [[entity-scip]]
- [[axis-covers]]
- [[ontology-mig-0001]]
