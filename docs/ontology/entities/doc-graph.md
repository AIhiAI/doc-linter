---
id: entity-doc-graph
role: ontology-entity
title: "Entity: Doc Graph"
summary: The unified SQLite-backed property graph doc-linter assembles from markdown frontmatter, wikilinks, source-code doc comments, and a SCIP code index. Every CLI / MCP / LSP surface reads from this single graph; entities, narrative docs, functions, types, files, modules, and endpoints are nodes and their relationships are typed edges.
status: stable
updated: 2026-10-08
axis_id: covers
value_id: doc-graph
display: Doc Graph
description: The connected property graph of docs, ontology entities, source-level functions/types, files, modules, and endpoints — backed by an embedded SQLite database and queried via CLI, MCP, or LSP.
synonyms: [knowledge-graph, doc-knowledge-graph, sqlite-graph]
source_modules:
  # The doc-linter crate is the doc-graph implementation top-to-bottom
  # — ingest, schema, query, MCP / LSP surfaces — so the entire `src/`
  # tree belongs to this entity.
  - src/**/*.rs
introduced_in_version: 1
---

# Entity — Doc Graph

The doc graph is doc-linter's core data structure: a single property
graph whose nodes are the docs, the ontology vocabulary (axes, values,
entities), and the source-code structural surface (functions, types,
files, modules, endpoints, findings). Typed edges connect them —
`COVERS`, `WIKILINK`, `FUNCTION_MENTIONS`, `CALLS`, `IMPLEMENTS`,
`METHOD_OF`, `ENDPOINT_HANDLED_BY`, and so on.

Concretely the graph lives at `<repo>/.doc-lint/graph.sqlite` as an
embedded SQLite database (node and edge tables, FTS5 text indexes, and a
usearch vector index file beside it). `doc-linter check` populates it; everything
else (`query`, `mcp`, `lsp`, `cluster`, `explain`, `homepage`) reads
from it. The schema is self-describing — `doc-linter query schema`
or the `query_schema` MCP tool dumps every table and row count.

Authoring rule of thumb: a finding, a fact, or a recommendation that
crosses two of {docs, ontology, code} almost always has its right
home as a query over this graph rather than as an ad-hoc script.

## Related

- [[axis-covers]]
- [[ontology-mig-0001]]
