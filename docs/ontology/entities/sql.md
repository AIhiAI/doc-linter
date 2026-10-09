---
id: entity-sql
role: ontology-entity
title: "Entity: SQL"
summary: SQL is the query language of the doc-graph's embedded SQLite store, used for every cross-bucket query, the saved-query catalog and the read-only escape hatch.
status: stable
updated: 2026-10-08
axis_id: covers
value_id: sql
display: SQL
description: "The SQLite dialect of SQL, as executed by the embedded SQLite store (FTS5 for text search, recursive CTEs for graph walks). The linter exposes it in three places: the `sql` MCP tool (read-only, guarded by SQLite itself), the `query sql` CLI subcommand, and the curated `query saved` catalog of high-value views."
synonyms:
  - sql-query
  - sqlite-dialect
  - sql-dialect
source_modules:
  - src/store_sqlite/read.rs
  - src/store_sqlite/saved.rs
  - src/store/query/saved.rs
introduced_in_version: 1
---

# Entity — SQL

SQL is a declarative query language; doc-linter's dialect is SQLite's.
The [[entity-doc-graph]] lives in one SQLite file
(`.doc-lint/graph.sqlite`): one table per node kind (`Doc`, `Entity`,
`Function`, ...), one table per edge kind with `(src, dst, props...)`
columns (`COVERS`, `WIKILINK`, `CALLS`, ...), and side tables for the list
columns (`doc_tags`, `entity_synonyms`, ...). Every cross-bucket question
doc-linter answers — "which entities are dark?", "what calls into
`function_context`?", "which files import this one?" — is a join over
those tables, for example
`SELECT d.id FROM Doc d JOIN COVERS c ON c.src = d.id WHERE c.dst = 'pricing-rule'`.
SQLite was chosen because it is a single embedded file, builds with the
`cc` crate (no cmake), has FTS5 for BM25 and recursive CTEs for the one
graph walk (`shortest_path`), and pairs with a usearch HNSW index for
vector search.

The linter exposes SQL at three surfaces. (1) The typed readers in
[`store_sqlite/read.rs`](../../../src/store_sqlite/read.rs) and the
helpers in [`store/typed.rs`](../../../src/store/typed.rs) own their
statements and return plain Rust data. (2) `doc-linter query sql '<QUERY>'`
exposes the raw escape hatch to human operators for one-off
investigations. (3) The MCP `sql` tool
([`src/cmd/mcp.rs`](../../../src/cmd/mcp.rs)) exposes the same escape
hatch to agents. Both accept exactly one `SELECT` or `WITH` statement and
ask SQLite whether the compiled statement is read-only
(`sqlite3_stmt_readonly`), so `INSERT`, `DELETE`, `PRAGMA`, `ATTACH` and
`WITH ... DELETE` are refused with JSON-RPC `-32602` before anything
changes. `REGEXP` is registered and matches the whole string.

The curated saved-query catalog
([`src/store/query/saved.rs`](../../../src/store/query/saved.rs) for
names, descriptions and parameters; the statements are the files under
`src/store_sqlite/saved_sql/`) is the supported authoring surface for
views that are worth shipping with the binary. A corpus can add its own in
`saved-queries/*.toml` with a `sql` field. `query saved --list`
enumerates them and `query saved <name> --param k=v` runs one; `$k`
placeholders are bound parameters.

Authoring rule of thumb: reach for `query saved` before composing a SQL
string by hand. If a saved query nearly fits but not quite, copy the
statement into a new `saved_sql/<name>.sql` plus a catalog entry rather
than letting the ad-hoc string fork — the catalog is the authoring
surface, and it is the set of queries that survives schema bumps.

## Related

- [[entity-doc-graph]]
- [[axis-covers]]
- [[ontology-mig-0001]]
