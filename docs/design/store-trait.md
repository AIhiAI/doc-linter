---
id: store-trait
role: doc
kind: explanation
lifecycle: planning
title: Storage seam (Store trait) and the Kuzu to SQLite path
summary: Plan items B1 to B8 — the Store/StoreRead seam, the SQLite store (rusqlite + FTS5 + usearch) and the removal of Kuzu. Records what was done at each step, the trait sketch, saved-query result-shape categories, the SQLite/usearch mapping, and the final cut that made SQLite the only engine. The sections before "Kuzu removal" are the migration record and describe the dual-engine period.
status: draft
updated: 2026-10-08
covers: [doc-graph]
tags: [roadmap, storage]
---

# Storage seam (Store trait)

## Kuzu removal (plan items B7 and B8, 2026-10-08)

SQLite is the only engine and `src/kuzu_graph` is now `src/store`. What changed relative to everything below, which describes the dual-engine period:

- `Engine::default()` and the `DOC_LINTER_STORE` variable are gone; `check` writes `.doc-lint/graph.sqlite` and every reader opens it. `rusqlite` is an unconditional dependency (the `sqlite-store` feature no longer exists); `vector-usearch` is a default feature, and `--no-default-features` falls back to the exact index.
- `query cypher`, the MCP `cypher` tool, the Cypher halves of `graph_read::dual` (now `graph_read::query_sql`), the Cypher DDL, the Kuzu writers and readers, `kuzu_graph::typed`'s Cypher and the `kuzu` crate are deleted, together with the `cxx-build` pin, the simsimd compiler flags in the cargo config, the cmake steps in CI and the install script, and the cxx advisory ignore in `deny.toml` (cxx moved to 1.0.202). `GraphRead::run_sql` and `embedding_nearest` return plain results.
- `src/store` keeps what never cared about the engine: `Value`, the `StoreRead`/`Store` traits (`traits.rs`), the row projections, the pure planners the SQLite ingest uses (corpus walk, SCIP fact dedupe and mention pairs, coupling, findings, unstubbed concepts, ...) and the saved-query catalog.
- A graph file left by an older version (`.doc-lint/graph.kuzu`) is ignored; `check` prints one notice, builds `graph.sqlite` beside it and the old file can be deleted.
- Tests: the Kuzu-versus-SQLite parity harnesses could not outlive Kuzu. Their guarantee is kept as goldens recorded from Kuzu (`tests/golden/`: saved queries on two corpora, every typed read, 14 MCP tools and the CLI read surface, the embedding search, the `check` report and `sitemap.json`), checked against SQLite; `RECORD_GOLDEN=1` re-records from SQLite. 136 of the 168 Kuzu-seeded saved-query unit tests moved to the `saved_tests` module of `store_sqlite` on a test kit that accepts the old Cypher `CREATE` notation; the 32 that only pinned Cypher spelling were dropped. The cache-hit scenario, the read-only guards and the MCP tests run on SQLite alone.
- The goldens exposed a bug shared by both engines: coupling jaccard used a set of commit timestamps, so commits within one second collapsed (jaccard above 1). It is keyed by commit now.
- Measured size, build time and latency: [[kuzu-removal-results]].

## Status

Steps 1 to 3 of the plan are landed (`store.rs` and `typed.rs` in the former `kuzu_graph` module, now `src/store`):

- `Db` and `Conn<'_>` are opaque newtypes over the Kuzu handles. Raw `query` / `prepare` / `execute` on `Conn` are `pub(in crate::kuzu_graph)`; nothing else in the crate or the binary can call them or name a `kuzu` type.
- `kuzu_graph::Value` is the engine-neutral enum (`Null, Bool, Int, Float, Str, List`) with `as_str` / `as_i64` / `as_u64_or_zero` / `str_or_empty` helpers. `to_kuzu` / `from_kuzu` convert at the boundary: `run_cypher_with_params` bindings and the `StoreRead` impl.
- `StoreRead::query(stmt, params) -> Vec<Row>` and `Store::exec(stmt, params)` are implemented for `Conn` (Kuzu only). Tests outside `kuzu_graph` seed fixtures through `Store::exec`.
- The 21 raw call sites outside `kuzu_graph` are gone. Their Cypher now lives in `kuzu_graph::typed`: coverage (`count_functions*`, `entity_doc_counts`, `entity_func_counts`, `crate_function_totals`, `crate_function_reach`, `dark_function_symbols`), cluster discovery (`cluster_nodes`, `cluster_edges`), contradictions (`design_doc_summaries`, `insert_contradiction_finding`) and the SCIP cache-hit path (`file_import_counts`, `clear_source_derived_rows`). They are written against the traits, so a second engine implements them rather than parsing Cypher.

Deliberately not done: inside `kuzu_graph` the ingest writers, `schema.rs`, `query/*` and `projections.rs` still use `kuzu::Value` and the `val_*` coercion helpers directly. That is the Kuzu implementation and moves with step 3 (typed APIs) as each family gets a trait method; converting it earlier would only add a second conversion layer. Small behaviour notes: the contradictions insert now prepares per finding instead of once; `file_import_counts` accepts any integer width for `import_count`.

## SQLite backend (step 5, first cut)

`src/store_sqlite/` behind cargo feature `sqlite-store` (off by default; `vector-usearch` adds the HNSW index). Kuzu is untouched.

- `schema.rs`: every Kuzu node table as a table (embedding columns dropped, `UINT32` as `INTEGER`), 8 list columns as side tables (`doc_tags`, `entity_synonyms`, ...) indexed on `value`, 32 rel tables as `(src, dst, props)` edge tables indexed on both ends (names quoted; `REFERENCES` is an SQL keyword), and FTS5 external-content tables `doc_fts` (title, summary), `section_fts` (heading, text), `function_fts` (symbol, doc_comment) with porter stemming. FTS is synced by `rebuild_fts`, which `build_and_swap` runs before the swap; direct rw writers must call it themselves.
- `mod.rs`: `SqliteDb` implements `StoreRead` / `Store` with `$name` bound parameters (an unknown name is an error; lists bind as JSON text). `open_rw` uses WAL, `open_ro` is `SQLITE_OPEN_READ_ONLY`. `build_and_swap` does `VACUUM INTO` a staging file (or starts empty), runs the closure, rebuilds FTS, sets `journal_mode=DELETE` so the file is self-contained, drops stale `-wal`/`-shm` of the live name and renames. `graph_identity` is dev+inode, as in Kuzu. A reader holding the old handle keeps reading the old inode until it reopens.
- `vector.rs`: `VectorIndex { add, search(k), save, load }` keyed by the owning row's `rowid`. `UsearchIndex` (HNSW, cosine, f32) is the product path; `ExactIndex` is a brute-force oracle for tests only. Index files live beside the db and are the caller's to rebuild inside `build_and_swap`.
- Linking caveat: usearch's default `simsimd` feature defines the same C symbols as the simsimd statically linked into libkuzu, so the final link fails with multiple definitions. The dependency is declared with `default-features = false, features = ["fp16lib"]` (portable distance kernels). This goes away when Kuzu does; until then SIMD distance is off.
- Tests (`--features vector-usearch`, `--test-threads=1`): insert/read roundtrip, swap visibility after reopen plus failed-build rollback, FTS5 bm25 ranking, ExactIndex save/load, usearch recall@10 on 5k random 384-dim vectors against exact (0.977).

Inside `src/kuzu_graph`, 26 files do raw queries, plus the ingest writers, DDL in `schema.rs`, and `build_and_swap` (file copy plus atomic rename, engine-agnostic while the store is a single file; SQLite needs `VACUUM INTO` or a WAL checkpoint before the copy).

## SQLite ingest (plan items B3 and B4)

`DOC_LINTER_STORE=sqlite doc-linter check` writes `.doc-lint/graph.sqlite` instead of the Kuzu graph. It needs a build with `--features sqlite-store` (`vector-usearch` for embeddings); unset or `kuzu` keeps today's behaviour. `crate::engine::Engine` parses the variable; an unknown value, or `sqlite` without the feature, is an error.

- Entry point: `store_sqlite::ingest::pipeline::ingest_corpus`, run inside `store_sqlite::build_and_swap` from [`src/cmd/check/mod.rs`](../../src/cmd/check/mod.rs). Order follows the Kuzu block: docs, migrations, repo meta, repos, unstubbed concepts, SCIP, endpoints, files/modules, imports, test_for, coupling, findings, embeddings. Each step after docs warns on failure like the Kuzu pipeline; SCIP failure aborts the swap.
- Writers under `src/store_sqlite/ingest/`: `docs.rs` (Doc, Entity, Section, Doc-Doc edges, COVERS, RELATES_TO), `code.rs` (Function, Type, Field, METHOD_OF, USES_TYPE, REFERENCES, IMPLEMENTS, EXTENDS, DEFINED_IN, MENTIONS, BELONGS_TO, CALLS, ENTITY_CALLS, god nodes), `misc.rs` (files, modules, DESCRIBED_BY, IMPORTS, TEST_FOR, COUPLED_WITH, findings, endpoints, migrations, repos, repo meta, unstubbed concepts). `mod.rs` holds the helpers: `upsert` (`INSERT .. ON CONFLICT(pk) DO UPDATE`), `insert_edges` (`INSERT .. SELECT .. WHERE EXISTS(src) AND EXISTS(dst)`, so an edge to a missing endpoint is dropped exactly like Kuzu's `MATCH .. CREATE`), `write_list` (side tables), `reset` (the row-deleting twin of `reset_schema_with_mode`, since the staging file starts as a copy of the live one) and `SqliteDb::transaction` / `write_rows` (one prepared statement per batch, one transaction per step).
- The Kuzu writers are not rewritten. Row derivation is shared: `build_plan`/`IngestPlan`, `dedupe_function_facts`, `build_mention_pairs`, `partition_mention_pairs`, `classify_god_nodes`, `collect_findings`, `plan_test_for`, `plan_unstubbed`, `in_module_pairs`, `import_counts`, `collect_files`/`collect_modules` and the coupling git helpers were made `pub(crate)` or split out of the Kuzu function that used to hold them. The derive queries (`ENTITY_CALLS`, god-node counts, `ENDPOINT_TOUCHES_ENTITY`, `DESCRIBED_BY`) are SQL joins.
- Vectors: `store_sqlite::vectors::populate` embeds Doc summaries, Entity display names, Function/Type doc comments and Section text into one `VectorIndex` under the key `table_tag << 56 | rowid`, saved at `<staging>.vec`. `build_and_swap` renames it to a unique `graph.vec.<pid>-<nanos>`, records that name in `meta.vec_file` inside the staged db, and renames the db last, so the db rename stays the single commit point; the previous index file is deleted after the swap (readers that loaded it hold it in memory). A failed build removes the staged index. `embed_cache(fp, emb)` is the twin of Kuzu's `EmbedCache` and survives resets: a `check` without `--embeddings` rebuilds the index from the cache (cache-only `populate`), `--embeddings` embeds only cache misses and prunes stale fingerprints. `vectors::load_index` is the reader side.
- Parity harness: `store_parity.rs` (`cargo test --features sqlite-store --test store_parity -- --test-threads=1`) runs the real binary twice per corpus (Kuzu, then `DOC_LINTER_STORE=sqlite`) and compares row counts for all 13 node tables, counts for every rel table, and element counts of the 8 list columns. Corpora: a generated fixture (two crates, a SCIP index with a call, a struct with a method and a type reference, entities with `source_modules`, a git history for coupling, TODO/FIXME comments) and this repo's own `docs/` plus `.doc-lint.toml`. Both pass with zero mismatches (fixture: 8 Doc, 4 Function, 1 Type, 5 File, 2 Finding, 1 CALLS, 1 ENTITY_CALLS, 1 COUPLED_WITH; own docs: 53 Doc, 8 Entity, 359 Section, 6 Finding, 169 WIKILINK, 43 COVERS, 59 doc_tags).

The parity harness `store_parity.rs` does not cover embeddings (the ONNX backend is not available in CI); `vectors.rs` has its own swap/restore/prune test with a fake embedder and `ExactIndex`, `store_sqlite::tests::usearch_recall_at_10_vs_exact` covers the HNSW index, and the Kuzu-only port below compares the embedding backend of both engines with a fake embedder. The SCIP cache-hit fast path is ported (see "Kuzu-only paths ported"). The post-ingest `check` consumers and the readers are covered in the next two sections.

## Typed reads on both engines (plan item B7, step 3)

`crate::graph_read::GraphRead` (supertrait `StoreRead`) lists the typed query APIs callers need: docs (`list_all_docs`, `get_doc`, `outbound`, `inbound`, `covers_inbound_for_entity_doc`, `shortest_path`), entities (`ranked_entities`, `get_entity`, `top_entities_in_crate`, `query_entity_subgraph`, `query_entity_ego_graph`), endpoints, functions (`function_context`, `function_mentions`, `function_siblings`, `docs_covering_entities`, `search_functions_by_symbol_substring`, `functions_mentioning`), `query_at`, `query_impact`, `list_dead_code`, the coverage family and `run_saved_query`. Callers hold a `&dyn GraphRead`.

- Kuzu: `impl GraphRead for Db` delegates to the unchanged `kuzu_graph::query::*` functions, so the default path did not move. `Db` also became a `StoreRead` (one short-lived connection per statement).
- SQLite: `store_sqlite::read` re-expresses each function as SQL with the same projection structs and the same Rust-side ranking, dedup and sorting. `query_impact` is a recursive CTE with a depth cap (`UNION` dedupes `(symbol, depth)`, so call cycles end at the cap and a caller's level is its shortest distance). `shortest_path` is a recursive CTE over the undirected Doc-Doc edge view that computes the BFS level of every reachable doc up to `max_hops`, then walks back from the target one level closer at a time (lowest id first). A path-carrying CTE would enumerate every simple path when the target is unreachable; the level form is polynomial. Ties between equal-length paths are engine-chosen on both sides.
- `kuzu_graph::typed` (coverage counts, cluster nodes and edges, contradictions, import counts, source-derived reset) now takes `&(impl StoreRead + ?Sized)` and picks Cypher or SQL from `StoreRead::is_sql`; the `DETACH DELETE` of `clear_source_derived_rows` becomes row deletes plus the touched edge tables.
- Proof: `typed_read_parity.rs` (`cargo test --features sqlite-store --test typed_read_parity -- --test-threads=1`) ingests the fixture, a richer fixture and this repo's docs under both engines and compares every method (and the typed helpers) per doc, entity, function and file: 139, 171 and 580 checks, zero mismatches. List results compare as sorted multisets except where the API sorts (`list_all_docs`, `ranked_entities`, `top_entities_in_crate`). `shortest_path` compares hop count and endpoints. Kinds that stay empty-on-both-sides in the fixtures: `list_dark_endpoints`, `list_dark_public_functions`, `list_dark_functions_in_crate`, `list_all_dark_public_functions`.

## Engine choice and the read side (plan item B7, step 4)

- Writers: `DOC_LINTER_STORE` (`Engine::from_env`). Readers (`query`, `explain`, `scaffold-coverage`, `cluster`, `migrate-anchor-required`, `mcp`): `Engine::for_read` takes the variable when set, else Kuzu if `graph.kuzu` exists, else SQLite if only `graph.sqlite` exists, else Kuzu (whose open reports the missing graph). A SQLite graph with a build lacking `--features sqlite-store` is an error that says so.
- `graph_read::ReadGraph` (Kuzu or SQLite, read-only) derefs to `dyn GraphRead`, so call sites changed from `kuzu_graph::f(db, ..)` to `db.f(..)` and nothing else. `identity()` is the per-engine file identity MCP uses to notice a swapped graph.
- `query::run`, `coverage::build_report`, `scaffold::build_proposals`, `explain::build_explanation`, `discover_clusters` and the `check` steps take `&dyn GraphRead` (or `&impl StoreRead`). Removed as dead code: the "cold graph, ingest on demand" branches of `query`, `explain` and `migrate-anchor-required`, which sat behind an `open_db_read_only` that had already failed on a missing graph.
- `check` on SQLite now runs the contradiction check (`--contradictions`), writes `sitemap.json` under `docs`, and runs the coverage and cluster lints inside the same `build_and_swap` as the ingest, via `GraphRead`. `check::run` gained an `engine` argument so MCP follows the engine its graph was opened on instead of the environment.
- MCP: `Server.db` is a `ReadGraph`. Tools on the typed calls (`list_docs`, `list_entities`, `query_path`, `query_entity`, `function_context`, `query_at`, `query_impact`, `query_dead_code`, `query_endpoints`, `query_saved`, `reingest`) use the engine-agnostic trait; the CLI test exercises `list_docs`, `list_entities`, `query_saved` and `reingest` over stdio, the rest are covered by the typed-read parity test of the same calls. `reingest` on SQLite runs `check` with `Engine::Sqlite` (no handle to share; the staging file swaps in and the server reopens on the new inode), and the cold-start build in `open_serving_db` does the same. `empty_tables` counts rows with SQL.
- The Cypher-only commands and tools that this section first listed as deliberately not ported are ported; see "Kuzu-only paths ported" below. `query cypher` and the MCP `cypher` tool stay Kuzu-only and say so, pointing at `query sql` / `sql`.
- Proof: [`tests/sqlite_engine_cli.rs`](../../tests/sqlite_engine_cli.rs) builds the same corpus (with `@endpoint` markers, a Python test pair, research docs) under both engines and checks that 21 read commands print byte-identical output (`query list|backlinks|neighbors|context|path|subgraph|functions-mentioning|function-context|coverage-report|endpoints|at|impact|dead-code|saved|map`, `explain`, `scaffold-coverage`, `cluster`) with no environment variable on the SQLite side; that `check --format json` (including `dark-public-function` / `entity-coverage-gap` diagnostics) and `sitemap.json` are identical; that `query cypher` fails with the pointed message; and that an `mcp` server on the SQLite graph answers typed tools, saved queries and `reingest` (new inode, no `graph.kuzu` created).

## Kuzu-only paths ported (plan item B, final cut)

After this step no command or MCP tool needs the Kuzu engine except the raw `query cypher` / `cypher` escape hatches, which exist only because Kuzu does. Every port is compared against Kuzu on three corpora (the generated fixture, the rich fixture, this repo's `docs/`) by `kuzu_only_port_parity.rs`.

Mechanism (all in `graph_read.rs` unless noted):

- `GraphRead::run_sql(sql, params)` is the SQL twin of `run_cypher_with_params`; `None` on Kuzu. `graph_read::dual(db, cypher, sql, params)` runs whichever dialect the open graph speaks and returns the same `{columns, row_count, rows}` JSON, so a call site carries both statements side by side instead of a translator. List columns come back as JSON arrays on both engines (SQL builds them with `json_group_array` over the side table, in insertion order).
- `GraphRead::schema_tables()` returns every node and rel table with columns, endpoints and row count (`ColumnInfo`, `TableInfo`). Kuzu reads `show_tables()` / `TABLE_INFO` / `show_connection`; SQLite reads `pragma_table_info` and folds the list side tables back into their owner as `STRING[]`.
- `GraphRead::embedding_nearest(NearestRequest)` is the vector search: SQLite loads the usearch index named by `meta.vec_file` once per handle (`SqliteDb` caches it), asks for `k` nearest keys, keeps those of the requested table whose row passes the repo / tag filters, and widens `k` until `top` rows survive or the index is exhausted. `corpus_size` counts rows that carry a vector. Score is `1 - cosine distance`, the dot product Kuzu ranks by. Kuzu returns `None` and `query::embedding_hits` falls back to fetching the corpus with its vector column. `query::embedding_rank` is the public entry the parity test calls with a fixed query vector.
- `regexp(pattern, text)` is registered on every `SqliteDb` (rusqlite feature `functions`, `regex` crate), so `text REGEXP 'pat'` works. It matches the whole string, like Cypher's `=~` (RE2 full match): `REGEXP 'doc'` does not match `doc-a`. A bad pattern is an SQL error.
- Read-only SQL: `store_sqlite::saved::run_readonly_sql` accepts one statement that starts with `SELECT` or `WITH` (comments skipped), refuses text after a `;`, and then asks SQLite whether the compiled statement is read-only (`sqlite3_stmt_readonly`), which also rejects `WITH ... DELETE` and `PRAGMA` writes. Parameters bind by `$name`.

Surface:

- CLI `query sql '<SELECT|WITH ...>' [--param name=value]` (the SQLite twin of `query cypher`, which keeps working on Kuzu and on SQLite says to use `query sql`); `query schema`, `query types`, `query similar` (BM25 and embedding), `query graph-summary` run on both engines.
- MCP `sql` tool (`query`, optional `params`, `max_rows`); `tools/list` advertises `cypher` on a Kuzu graph and `sql` on a SQLite one, and both stay callable (the wrong one answers with a pointer to the right one). Ported to both engines: `query_schema`, `query_similar`, `audit_doc_region` (BM25 and embedding), `suggest_tags_for_doc`, `classify_file_coupling`, `context_for` (doc, entity and function centres), `list_files`, `list_findings`, `list_migrations`, `list_modules`, `list_repos`, `list_types`, `query_doc_neighbors`, `query_entity_neighbors`.
- Corpus fetchers for `query similar` BM25 now `ORDER BY id` on both engines so tied scores rank the same everywhere; `list_findings` breaks ties by `id`.

SCIP cache-hit ingest on SQLite (`store_sqlite::ingest::pipeline`):

- The snapshot lives in `ingest-cache.sqlite.json` in the `.doc-lint` directory, apart from Kuzu's `ingest-cache.json`, because the two engines hold separate graphs. `check` writes it only after `build_and_swap` has committed (the pipeline returns the snapshot), so a failed build can never leave a snapshot describing rows that are not live.
- A run hits when `--rebuild` is absent, the SCIP file matches the snapshot (schema version, mtime, length, SHA-256) and the staged graph, a copy of the live one, still holds exactly the snapshot's Function and Type row counts. Anything else is a full ingest, so deleting the live graph or a failed earlier swap falls back safely.
- On a hit: the vault tables are rebuilt with `ResetMode::VaultAndCrossBucket`; `code::rederive_cross_bucket` redraws only the edges that point into the vault (defined-in, mentions, belongs-to) and the derivations over them (ENTITY_CALLS, mention counts, god nodes) from Function and Type rows read back as facts; File, Module, Finding, IMPORTS, COUPLED_WITH and DEFINED_IN_FILE are cleared and rebuilt from the current tree (IMPORTS counts are read back first as the facts they came from); endpoints are re-extracted; TEST_FOR is preserved, not recomputed. The SCIP file is not parsed. Unstubbed-concept findings are now written after the SCIP step, so the clear on a hit cannot drop them (Kuzu's cache-hit path loses them; not fixed there since Kuzu is going away).

mcp unit tests run on both engines: `Eng` plus a thread-local pick the engine `server()` builds on, `seed_server(&[Seed::..])` seeds rows as Cypher `CREATE` on Kuzu and `INSERT` on SQLite, and 55 tests that touch a server are written once as `<name>_body` and registered twice (`<name>` and `<name>_sqlite`). Tests that exercise Kuzu internals (the `cypher_*` guards, raw Kuzu connections, the real-model embedding round trip) stay Kuzu-only.

Proof (`cargo test --features vector-usearch --test kuzu_only_port_parity -- --test-threads=1`, zero mismatches except the documented differences below):

- CLI and MCP comparison: fixture 222 checks, rich fixture 245, repo docs 275 (`query types|similar|graph-summary|schema`, eight Cypher/SQL dialect twins including `=~` against `REGEXP`, and the 14 MCP tools above, each with several argument shapes and unknown ids).
- Embedding backend, fake bag-of-words embedder, 215 checks per corpus over every corpus kind (doc, function, entity, section, type, all) crossed with repo, `with_tag` and `without_tag` filters: 0 mismatches on all three corpora.
- Cache hit: a hit rebuilds a graph identical to a forced full ingest of the same tree (per-table counts and sampled saved queries), picks up a new doc while keeping every Function and Type row, misses on a changed SCIP file, refreshes the snapshot on the miss, and falls back to a full ingest when the live graph has been removed.
- Read-only guard: `DELETE`, `INSERT`, `UPDATE`, `DROP`, `PRAGMA`, `ATTACH`, `WITH ... DELETE`, `SELECT 1; DELETE ...` and the empty string are all refused by both `query sql` and the `sql` tool, and the graph is intact afterwards.

Documented intentional differences (the test normalises exactly these):

1. `query schema` / `query_schema` column types are spelled `TEXT` / `INTEGER` / `REAL` on SQLite (`STRING` / `UINT32` / `INT64` / `DOUBLE` on Kuzu), SQLite has no `EmbedCache` table and no `*_embedding` columns (vectors live in the usearch file), and its `examples` are SQL.
2. Booleans are `0` / `1` in raw SQL results (`is_god_node`, `generated`, `applied`, `inferred`, `dispatch`); `list_migrations` converts `applied` back to a boolean.
3. `context_for`: neighbours come in scan order on Kuzu and sorted by edge kind then id on SQLite; once a node has more neighbours than the cap, which ones survive is engine-chosen. The comparison sorts, and compares only the centre and the count when capped.
4. Array members built with `collect(DISTINCT ..)` against `json_group_array(DISTINCT ..)` (`shared_entities`, `shared_docs`) have engine-chosen order; compared sorted.
5. Embedding scores agree to rounding (usearch stores f32 and reports a cosine distance), and hits tied at the zero-similarity floor have an engine-chosen order; the comparison keeps hits above 0.05 and compares `corpus_size` exactly. With a real model the HNSW search is approximate (recall 0.977 on 5k random vectors).
6. A SCIP cache hit on Kuzu drops unstubbed-concept findings; on SQLite it keeps them.


## Trait sketch

```rust
pub enum Value { Null, Bool(bool), Int(i64), Float(f64), Str(String), List(Vec<Value>) }
pub type Row = Vec<Value>;
pub type Params<'a> = &'a [(&'a str, Value)];

pub trait StoreRead {
    fn query(&self, stmt: &str, params: Params) -> Result<Vec<Row>>;
}
pub trait Store: StoreRead {
    fn exec(&self, stmt: &str, params: Params) -> Result<()>;
    fn transaction<T>(&self, f: impl FnOnce(&Self) -> Result<T>) -> Result<T>;
}
// free fns today (kuzu_graph/mod.rs), trait-level tomorrow:
fn open_rw(root) -> Result<impl Store>;            // open_db
fn open_ro(root) -> Result<impl StoreRead>;        // open_db_read_only
fn build_and_swap<T>(root, f: impl FnOnce(&impl Store) -> Result<T>) -> Result<T>;
fn graph_identity(root) -> Option<(u64, u64)>;     // unchanged (dev, inode)
```

Handles stay opaque (`Db`, `Conn`); `Conn` borrows `Db` today and can stay a thin wrapper. Rows are eagerly collected; Kuzu's lazy iterator only matters for the large cluster scans (`read_graph_nodes/edges`), which can page by `LIMIT/OFFSET`.

The raw-query escape hatch is the real problem: the 271 saved queries and the `query cypher` CLI are Cypher text, which a SQL engine cannot run. Options: (a) a Cypher-to-SQL translator for the subset used, (b) type the hot APIs and leave `query cypher` as a Kuzu-only feature. Recommended: (b) first, then port saved queries by result-shape category.

## APIs that must become typed

These build structs from Cypher today, so they move behind `StoreRead` methods (one impl per engine) rather than taking query strings:

- `shortest_path` (query/docs.rs): uses `[* SHORTEST 1..N]`. SQLite: recursive CTE carrying a visited-path string, ordered by depth with `LIMIT 1`, or BFS in Rust over the loaded edge list (the graph is megabytes).
- `query_impact` (query/impact.rs): depth-limited fan-out, already iterative (2 queries per visited node); port as recursive CTE or keep the loop.
- `query_at`, `query_entity_subgraph`, `function_context`, `function_siblings`, `docs_covering_entities`, `endpoint_reach_by_kind`, `global_function_reach`, `list_dead_code`, `list_dark_*`, `list_entity_coverage_gaps`, `ranked_entities`, `search_functions_by_symbol_substring`, `inbound/outbound`, `get_doc/get_entity/list_all_docs`.
- coverage.rs scalar/count helpers, cluster.rs graph readers, contradictions.rs, scip_pipeline `facts_for_cache_hit`.
- `run_cypher`, `run_cypher_with_params`, `run_saved_query`: stay engine-specific; they return `serde_json::Value`, so no Kuzu type escapes.
- mcp.rs: about 25 inline queries with `Value::String` bindings; migrate onto the typed methods above.

## Saved queries (271) by result shape

Approximate grep counts over the former Kuzu saved-query module (`saved.rs`) (a query can match several): 497 `WITH` stages, 306 `count(`, 236 `ORDER BY`, 131 `OPTIONAL MATCH`, 54 `collect(`, 50 `UNWIND`, 29 `size(`, 7 `UNION`, 1 variable-length path, 0 vector functions, 589 `CONTAINS/STARTS WITH/=~`.

Categories, easiest first:

1. Scalar counts / single-row stats (`count(*) AS n`).
2. Flat listings: `MATCH (n:T) WHERE .. RETURN cols ORDER BY .. LIMIT` (the bulk).
3. Aggregated groupings: implicit-key `count/collect`; `collect` maps to `json_group_array` / `group_concat`.
4. Edge joins with `OPTIONAL MATCH` (131): LEFT JOIN on the per-edge-kind tables.
5. `UNWIND` over list columns (50): needs the side tables below, then a plain JOIN.
6. `UNION ALL` (7): same in SQL.
7. Traversals (1 variable-length, plus `shortest_path`): recursive CTE.
8. Regex / substring (`=~`, CONTAINS): SQLite has no built-in regex; register a `regexp` function through rusqlite, or use FTS5 / LIKE.

The catalog uses `$param` string substitution rather than bound parameters; porting also means moving to real bound parameters.

## SQLite / usearch mapping

- Node tables Doc / Entity / Function become tables; each rel table becomes an edge table `(src, dst, props...)` indexed on both ends. One table per edge kind stays (it is what `EDGE_SCHEMA` models).
- List columns (`tags`, `covers`, `attributes`, `synonyms`, `bounded_contexts`, `scanner_coverage`, `source_modules`): side tables `doc_tags(doc_id, value)` etc., indexed on `value`. `UNWIND` and list membership become joins / `EXISTS`. JSON1 arrays are cheaper but cannot be indexed.
- `FLOAT[384]` embeddings (`summary_embedding`, `display_embedding`, EmbedCache): drop the column; keep vectors in a usearch HNSW index keyed by the owning row's `rowid`, in a file beside the db and rebuilt inside `build_and_swap`. Cosine search returns rowids joined back to rows.
- BM25: an FTS5 virtual table over title/summary/body with `bm25()` ranking.
- `build_and_swap`: checkpoint the WAL (or `VACUUM INTO staging`), build, rename. `graph_identity` keeps working. WAL mode permits readers during a writer, so the swap could later be dropped.
- `open_db_read_only` maps to `SQLITE_OPEN_READ_ONLY`. The Kuzu `MAX_DB_SIZE` address-space workaround disappears.
- Ingest uses explicit `BEGIN TRANSACTION`; same in SQLite.

## Remaining steps

1. Done: opaque `Db` / `Conn` newtypes and the neutral `Value` enum.
2. Done: `StoreRead` / `Store` (Kuzu impl) and the outside-`kuzu_graph` call sites moved to `kuzu_graph::typed`.
3. Convert the typed APIs above one family at a time (docs, functions, endpoints, impact, coverage, mcp) onto the traits, retiring `kuzu::Value` inside `query/*` and `projections.rs`.
4. Done: saved queries ported by category (272 of 272); `query cypher` stays on Kuzu until Kuzu is deleted, with `query sql` as its SQLite twin.
5. Second impl: landed behind `sqlite-store` (storage, ingest including the SCIP cache-hit path, saved queries, typed reads, engine choice, the `check` consumers, MCP serving and `reingest`, and every formerly Kuzu-only command and tool). Nothing in the product path requires the Kuzu engine any more. Still to do before Kuzu can be deleted: re-point the Kuzu-seeded integration tests (`kuzu_roundtrip`, `scip_pipeline`, `force_growth`, `mcp_supervisor_swap`, ...) at the SQLite engine, decide the default engine (`Engine::default`), and drop `query cypher`, the MCP `cypher` tool and the Cypher halves of `graph_read::dual`.
