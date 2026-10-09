---
id: store-schema
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph, sql]
title: Store schema
summary: The shape of the SQLite file that doc-linter check writes, which tables and columns are a public interface, and the version policy for changing them.
status: stable
updated: 2026-10-08
tags: [reference, storage]
---

# Store schema

`doc-linter check` writes the doc graph to `.doc-lint/graph.sqlite`, a plain
SQLite file. Anyone can read it with `sqlite3`, so its shape is a public
interface. This page says which parts you can rely on.

## Version policy

- The file carries one integer schema version, stamped by `check` in two places:
  `PRAGMA user_version` and the `meta` row `schema_version`.
- doc-linter opens a graph only when the stored version equals the version it
  was built with. A missing (`0`), older or newer version does not crash: readers
  (`query`, `report`, `mcp`) print one line telling you to run `doc-linter check`.
- `doc-linter check` (and reingest) never migrates in place. It sees the mismatch,
  starts from an empty graph and writes a fresh file with the current version.
  The graph is derived data, so a rebuild loses nothing.
- The version is bumped on any breaking change to the public surface below.
  Additive changes (a new table or column) do not bump it.

## Stable public surface

Safe to query from outside tools; changes are breaking and bump the version:

- Node tables: `Doc`, `Entity`, `Section`, `Function`, `Type`, `Field`, `File`,
  `Module`, `Finding`, `Endpoint`, with their existing columns.
- Edge tables (`src`, `dst`, plus listed properties), for example `WIKILINK`,
  `COVERS`, `RELATES_TO`, `CALLS`, `TEST_FOR`, `COUPLED_WITH`, `IMPORTS`,
  `FUNCTION_MENTIONS`.
- List side tables `doc_tags`, `doc_covers`, `doc_attributes` and the `entity_*`
  tables (`owner id, value`).
- Saved-query names shown by `doc-linter query saved --list`, with their
  parameters and output columns.
- `meta.schema_version`.
- The `history` table (below).

## The `history` table (coverage history)

Every full `doc-linter check` (and `report`, which runs one) appends one row
after the graph is swapped in. `--file` runs do not. Read it with
`doc-linter query history [--limit N] [--json]` or plain SQL; `doc-linter report`
prints a trend line from it. It is part of the free open core.

| Column | Meaning |
|---|---|
| `id` | Integer primary key, increasing |
| `ts` | UTC time of the run, RFC 3339 |
| `git_rev` | Short `HEAD` revision, `NULL` outside a git checkout |
| `total_docs`, `narrative_docs`, `entities` | Doc count, docs that are not ontology value/axis/entity docs, entity count |
| `functions` | Functions in the code index (0 without `scip-index`) |
| `functions_reaching_entity`, `entity_reach_pct` | The coverage table's reach count and non-exempt percentage |
| `documented_functions`, `documented_pct`, `dark_functions` | Same, from the coverage table |
| `errors`, `warnings` | Finding counts of that run, by severity as configured |

Rules:

- Only the newest 1000 rows are kept; older rows are deleted on each append.
- **It is the one table that survives a rebuild.** `check` builds a new graph
  from a copy of the live file, so history is carried over. When the live file has
  another schema version (so it is not copied), `check` still copies the `history`
  rows across by column name, best effort: if the old file is unreadable or lacks a
  listed column, history restarts empty.
- Adding the table is additive, so the schema version stays at 1. Graphs written
  before the table existed get it on the next `check`; `query history` on such a
  graph prints "No history yet".
- A column added later is also additive; renaming or removing one bumps the version
  and the carry-over copy then drops older rows.

## Internal (may change without notice or a version bump)

- FTS5 tables and their shadow tables (`doc_fts`, `section_fts`, `function_fts`, `*_data`, ...).
- `embed_cache`, the `meta` rows other than `schema_version` (for example `vec_file`),
  and the `graph.vec.*` vector index files.
- `Migration`, `Repo`, `RepoMeta`, index names, and row ids and ordering.
- Any table or column not listed in the stable section.

## Extension room: the `x_` prefix

The prefix `x_` is reserved for tables added by proprietary or enterprise
extensions. The open-core product never creates, reads or depends on an `x_*`
table, and open-core queries must ignore them: never `SELECT` from or `DROP`
one, and never enumerate tables expecting only open-core names. A rebuild writes
a fresh file, so an extension re-creates its `x_*` tables after each `check`.
