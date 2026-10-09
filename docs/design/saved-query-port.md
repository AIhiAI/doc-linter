---
id: saved-query-port
role: roadmap-entry
lifecycle: planning
title: Saved-query port to SQLite
summary: Plan item B6 — classifies all 272 saved queries by Cypher feature and category, marks the 30 agent-facing ones, records the hand-translated SQLite SQL in src/store_sqlite/saved_sql, the embedded loader (SqliteDb run_saved_query), and the row-level parity run against Kuzu with every difference listed.
status: draft
updated: 2026-10-08
covers: [sql]
tags: [roadmap, storage]
---

# Saved-query port to SQLite

Companion to [[store-trait]]. That doc recommends typing the hot APIs first and porting the saved queries by result-shape category. This doc is phase 1 of the port: the catalog is classified, and every entry has a SQLite translation that is mechanically checked against the store DDL. Phase 2 embeds the SQL in the binary and proves it against Kuzu row for row (see Parity results).

## What was delivered

> Outcome (2026-10-08): Kuzu is gone, so the Cypher originals, `MANIFEST.tsv` and `scripts/check_saved_sql.py` were deleted with it. The `.sql` files keep a two-line header (`name`, `params`); the catalog (`store::query::saved`) carries name, description, params and the node tables each query needs; the Rust tests `every_builtin_sql_runs_against_an_empty_graph` and `catalog_declared_params_appear_in_sql` replace the script, and the 136 semantic tests plus the goldens in `tests/golden/saved_queries_*.tsv` replace the parity run. The text below is the record of the port.

- `src/store_sqlite/saved_sql/<name>.sql`: one file per catalog entry (272 of 272), each with a header (`-- params:`, `-- columns:`, `-- tier:`, `-- note:`).
- `src/store_sqlite/saved_sql/MANIFEST.tsv`: `name`, `status` (`translated` or `needs-design`), `notes`. Today 272 translated, 0 needs-design.
- `scripts/check_saved_sql.py`: stdlib-only validator, run in CI by the `saved-sql` job. It rebuilds the SQLite DDL by scraping `src/store_sqlite/schema.rs`, runs every file against the empty schema and compares the result column names and order with the header, which was copied from the Cypher `RETURN` aliases. It also checks that `$name` placeholders match the declared params and that the manifest matches the names in `saved.rs`.

The entry count is 272, not the 271 quoted in [[store-trait]]; that count is off by one. The check script derives the list from `saved.rs`, so it stays right.

## Conventions

- Parameters are `$name` placeholders, the form `SqliteDb` binds. A header line `-- params: min_calls=5, pattern` declares them; `name=default` is optional, as in the Cypher catalog.
- Edge tables are quoted (`"CALLS"`), because `REFERENCES` is an SQL keyword and quoting keeps the files uniform.
- `x CONTAINS y` becomes `instr(x, y) > 0` (case-sensitive and wildcard-free, unlike `LIKE`). `STARTS WITH` / `ENDS WITH` become `substr` comparisons. NULL propagation matches Cypher.
- Cypher `RETURN a.id` without an alias names its column `a.id`; the SQL aliases it the same way (`AS "a.id"`).
- List columns read from the side tables (`doc_tags`, `entity_attributes`). A list returned to the caller is a JSON array in a text column, from `json_group_array`. `x IN d.tags` is an `EXISTS` on `doc_tags`.
- Booleans come back as 0 and 1.
- Integer division is the same in both engines (integer over integer truncates), so `a * 100 / b` ports unchanged.

## Translation rules and how often they apply

| Cypher feature | Entries | SQLite form |
|---|---|---|
| OPTIONAL MATCH | 60 | LEFT JOIN / correlated scalar subquery |
| UNWIND | 37 | side table join / UNION ALL of literals |
| collect() | 30 | group_concat / json_group_array |
| size()/length() | 15 | count / length |
| rel-type alternation | 3 | UNION ALL over edge tables |
| undirected edge | 7 | UNION ALL of both directions |
| CONTAINS | 124 | instr(a,b) > 0 |
| STARTS/ENDS WITH | 68 | substr() comparison |
| list column (tags/attributes) | 45 | doc_tags / entity_attributes side tables |
| list literal IN [..] | 27 | IN (..) |
| list index docs[0] | 1 | min()/subquery |
| map literal {..} | 5 | json_object |
| label(r) | 2 | literal edge_type per UNION branch |
| regexp_extract | 3 | replace/rtrim basename idiom |
| CASE | 86 | CASE |
| UNION ALL | 2 | UNION ALL |
| cross-join self pair | 5 | comma join, O(n^2) |
| integer division | 8 | same in SQLite (int / int) |
| shortest path / var-length | 0 | recursive CTE (none in catalog) |
| EXISTS {..} / NOT (pattern) | 9 | EXISTS subquery |
| none of the above | 40 | direct join and filter |

Shortest path and variable-length patterns do not occur in the catalog, so no recursive CTE was needed. If one is added later, use a recursive CTE with a depth cap, as the plan says.

## Categories

| Category | Entries | Notes |
|---|---|---|
| entity-axis | 45 | Entity-centred cards, searches, promotion state, fact/pattern and flavor/lifecycle classifiers, orphan and god-node views. |
| corpus-diagnostics | 42 | Whole-corpus counts and regime classifiers (cold start, ingest state, routing, ratios, language and LOC distributions, authored-doc shape). Mostly single-row aggregates over several node tables. |
| doc-tag-families | 41 | Doc tag analytics: research families, bridges, cookbooks, audits, co-occurrence. All lean on the `Doc.tags` list column, so they move to the `doc_tags` side table. |
| call-graph | 30 | CALLS and USES_TYPE traversals: slices, blast radius, shapes, hubs, cohesion, leaf-with-deps. |
| symbol-lookup | 30 | Function / Type / Field lookups by symbol, file or substring, with no edge traversal. |
| file-layout | 24 | File-table path and language views, clone candidates, generated vs hand-written, prefix and basename surveys. |
| test-surface | 21 | Test-oriented views: untested functions, TEST_FOR coverage, test helpers by stem, pytest pairs. |
| doc-graph | 19 | Doc nodes and doc-to-doc edges: fan-in, leaf docs, freshness, roles and kinds, roadmap deps, phasing. |
| file-coupling | 13 | COUPLED_WITH and IMPORTS views: coupled files, hubs, density, fan-out. |
| other-nodes | 4 | Endpoint, Finding and similar single-table views. |
| context-envelope | 3 | Rich per-node payloads built from `collect` of maps (`node-context-*`). |
| total | 272 | |

## Agent-facing set (30)

An entry is agent-facing when `src/cmd/mcp.rs` (outside its tests), `docs/reference/mcp.md` or `docs/ontology/entities/cypher.md` names it, when launch docs or integration tests lean on it, or when it backs a recipe the MCP `context_for` tool exposes. The MCP surface itself is a single generic `query_saved` tool, so every catalog entry is reachable by name; these are the ones agents are told to use.

| Name | Why it counts | Category |
|---|---|---|
| entity-relations | first catalog entry; entity graph in one view | entity-axis |
| orphan-entities | cypher.md | entity-axis |
| hub-functions | cypher.md | entity-axis |
| endpoint-darkness | cypher.md | entity-axis |
| coverage-by-entity | cypher.md | entity-axis |
| entity-callers | cypher.md | entity-axis |
| dead-files | cypher.md | call-graph |
| type-distribution | cypher.md | symbol-lookup |
| field-references | tests/scip_pipeline.rs | symbol-lookup |
| untested-functions | cypher.md | test-surface |
| function-backward-slice | recipe: callers of a symbol | call-graph |
| function-forward-slice | recipe: callees of a symbol | call-graph |
| function-blast-radius-by-file | recipe: refactor impact | call-graph |
| findings-by-file | cypher.md | file-layout |
| language-distribution | mcp.md, cypher.md | corpus-diagnostics |
| stale-docs-with-impact | docs/launch/first-run-findings.md | doc-graph |
| freshness-by-kind | docs/launch/first-run-findings.md | doc-graph |
| stale-narrative-docs | docs/launch/first-run-findings.md | doc-graph |
| files-by-path-substring | mcp.md | file-layout |
| files-coupled-to | mcp.md | file-coupling |
| docs-by-tag | recipe: tag-axis lookup | doc-tag-families |
| docs-by-kind | mcp.md | doc-graph |
| endpoint-by-kind | cypher.md | other-nodes |
| callgraph-roots | cypher.md | call-graph |
| import-fan-out | mcp.md, cypher.md | file-coupling |
| coupled-files | mcp.md, cypher.md | file-coupling |
| node-context-function | MCP context_for envelope | context-envelope |
| node-context-doc | MCP context_for envelope | context-envelope |
| node-context-entity | MCP context_for envelope | context-envelope |
| concept-work-list | docs/launch/first-run-findings.md | entity-axis |

Count: 30. That is 17 named in `mcp.md`, `cypher.md` or `mcp.rs`, 5 from launch docs or tests, and 8 recipe or context entries.

## Divergences and risks

The check script proves that each file parses against the real DDL and returns the declared columns; `tests/saved_query_parity.rs` proves row-level parity (see Parity results). Predicted places where parity could break, and what the run showed:

- **Aggregates over an empty set.** `sum` over zero rows is NULL in SQLite and the Kuzu result is unconfirmed. Where the value feeds a `CASE` comparison, the SQL uses `coalesce(sum(..), 0)` so an empty corpus classifies as empty.
- **Zero-row quirk fixed.** `corpus-purpose-classifier`, `corpus-cold-start-summary`, `unified-corpus-diagnostic`, `corpus-entities-per-doc-ratio`, `corpus-functions-per-file-ratio` and `corpus-doc-coverage-ratio` used a non-optional `MATCH` after an aggregating `WITH`, so Kuzu returned no rows when the later set was empty. They now use `OPTIONAL MATCH` (Cypher) and no `WHERE` guard (SQL) and return one row of zeros/`EMPTY` on an empty set.
- **Empty `collect`.** `node-context-*` build lists of maps with `OPTIONAL MATCH` + `collect`. With no match, SQLite returns `[]`; Kuzu may return a one-element list of nulls. Consumers should treat both as empty.
- **Row order.** Ties under `ORDER BY` and the element order inside `json_group_array` are unspecified in both engines, so only sorted, tie-free results are comparable row for row.
- **Undirected patterns.** `-[r]-` is a union of both directions; a self-loop counts twice here and may count once in Kuzu.
- **Quadratic self-joins.** `loc-pair-clone-candidates`, `same-name-cross-dir-clones`, `pytest-prod-test-pairs`, `entity-singular-plural-pairs` and `corpus-singular-plural-pair-summary` are O(n^2) in the node count. Fine at current corpus sizes; a `File.basename` column or a hash join would fix it if they get slow.
- **`regexp_extract(path, [^/]+$)`.** Ported with the classic replace/rtrim idiom (strip everything up to the last slash), so it needs no regex extension.
- **Schema drift.** `check_saved_sql.py` re-implements `ddl()` by scraping `schema.rs`. If the shape of those constants changes the script asserts loudly instead of passing on a stale schema.
- **Descriptions and `--list` text** are not duplicated in the `.sql` files; they stay in `saved.rs` (the loader reads the catalog from there).

## Needs design

None. All 272 entries translate with the rules above. The manifest keeps the `needs-design` status for future catalog additions that do not (for example a variable-length path or a vector function).

## Loader

`build.rs` scans `src/store_sqlite/saved_sql/*.sql` and writes `$OUT_DIR/saved_sql_table.rs`, a static `(name, include_str!(path))` table; no dependency. `store_sqlite::saved` includes it (`builtin_sql`, a unit test asserts one entry per catalog name) and exposes `run_saved_query_with_root(db, root, name, params)`.

- Catalog, listing, `name=default` params and missing-param errors are the Kuzu ones: `kuzu_graph::query::saved::{resolve_query, fill_params}` are shared by both runners.
- Extension: a runtime `saved-queries/*.toml` (the directory the Kuzu loader already reads under the corpus root) may carry `sql = "..."` next to `cypher`. `cypher` is now optional in the TOML, so a SQLite-only query parses; the Kuzu runner rejects it with "no cypher form". A corpus-local query with no `sql` is an error on SQLite and never falls back to the built-in of the same name.
- Binding: each `--param` is bound as text to `$name` if the statement mentions it; extras are ignored (the Cypher path substitutes, so unused params were ignored there too). Numeric comparisons rely on SQLite column affinity or an explicit `CAST` in the file.
- Result shape: `{columns, row_count, rows:[{col: value}]}`, like `run_cypher`. A text cell is parsed as JSON only when the statement itself calls `json_group_array`/`json_object`/`json_array`, so ordinary strings that start with `[` survive.

## Parity results

`tests/saved_query_parity.rs` (`cargo test --features sqlite-store --test saved_query_parity -- --test-threads=1`) ingests three corpora with the real binary under each engine (the SQLite root is a `cp -a` of the Kuzu root so mtimes and git history agree), runs every catalog query with discovered params (first entity, symbol, doc, tag and path for the first variant, then generic substrings; `name=default` params use their default once and are overridden after) and compares column lists and row multisets. `SAVED_PARITY_REPORT=<prefix>` writes the per-case report.

| Corpus | Cases | Equal | Differing | Allowlisted | Intentional |
|---|---|---|---|---|---|
| fixture (store_parity fixture: 8 Doc, 4 Function, 1 Type, 5 File) | 480 | 468 | 0 | 0 | 12 |
| own docs (this repo's `docs/` + config) | 480 | 472 | 0 | 0 | 8 |
| rich (fixture + research-tagged docs, test/prod pairs, more commits) | 480 | 467 | 0 | 0 | 13 |

A case is one query with one parameter variant (parameterless queries are one case, others three). The zero-row-quirk allowlist is now empty (the six queries were fixed); both engines return the same rows on all three corpora (zero on docs-only data, one row once code exists). Unexpected differences fail the test.

Comparison normalisation (all deliberate, applied by the test only):

- Numbers compare by value to 5 decimals; bools are 0/1; numeric strings equal the number (Kuzu returns `sum()` over UINT32 as a JSON string, SQLite as a number).
- List elements compare as a sorted multiset, with null elements and all-null maps dropped (Kuzu returns one all-null map for an empty `OPTIONAL MATCH` + `collect` of maps, SQLite returns `[]`).

### Differences found

SQL translation bug, fixed:

- `axis-routing-card-for-vocabulary`: Kuzu groups on the literal (`RETURN 'entity', count(e)`), so a zero count gives no row; SQL gave a `0` row. Added `HAVING count(..) > 0` to both branches.

Intentional (the Kuzu result is a Kuzu quirk and the SQL result is the correct one; each is listed in `INTENTIONAL` in the test with its reason):

| Query | Kuzu | SQLite |
|---|---|---|
| singleton-tags | errors on `docs[0]` ("List extract takes 1-based position") | works |
| entity-to-implementation-files | `collect()` over only-null `OPTIONAL MATCH` rows is NULL, `list + NULL` is NULL, `UNWIND` yields no rows when an entity has no Type mention | returns the function files |
| path-axis-completeness-fingerprint, path-axis-fingerprint-cython-aware | `count(fn)` after an `OPTIONAL MATCH` is 0 while `count(DISTINCT fn.file)` is positive | true count |
| files-by-path-pattern-with-coupling, file-coupling-degree | `count(r)` is 0 and `sum(r.commits)` NULL for a relationship bound by an undirected pattern (changes the `ORDER BY` of the second too) | sums the commits |
| resource-shape-classifier, coupling-density-stats, path-axis-fingerprint-cython-aware | `sum()` over no rows is NULL | `coalesce(..., 0)` |

Two more classes show up as normalisation rather than failures: the INT128 string for `sum()` and the empty-`collect` all-null map (above). The remaining risk is coverage: 183 of 272 queries returned at least one row in at least one corpus; the other 89 compared equal-and-empty (or are intentional rows above), so their filters are only syntax- and column-checked. They need code shapes the fixtures lack (Python test helper stems, Cython files, endpoint nodes, god-node entities, research family bundles with 3+ tags and `iter-` audits, feature-evolution coupling). Add a fixture shape when one of them is changed.

Quirks that were suspected but did not show: "zero-row quirk preserved" matches Kuzu in all three corpora; undirected self-loop double counting and list ordering inside `json_group_array` never differed after sorting.

## Next steps

1. (Done: zero-row quirk fixed in both engines.) `query cypher` stays Kuzu-only until Kuzu is deleted; `query sql` is its SQLite twin and runs read-only `SELECT` / `WITH` (see [[store-trait]], "Kuzu-only paths ported").
2. Grow the fixture so the 89 vacuous queries see rows.
3. `query saved` and MCP `query_saved` now choose the engine from the graph file, so the catalog is runnable end to end on SQLite (`tests/sqlite_engine_cli.rs`).

## Appendix: every entry

Feature codes: OPT = OPTIONAL MATCH, UNW = UNWIND, COLL = collect(), SIZE = size()/length(), ALT = rel-type alternation, UNDIR = undirected edge, CONT = CONTAINS, SEW = STARTS/ENDS WITH, LISTCOL = list column (tags/attributes), INLIST = list literal IN [..], IDX = list index docs[0], MAP = map literal {..}, LABEL = label(r), REGEX = regexp_extract, CASE = CASE, UNION = UNION ALL, SELFJOIN = cross-join self pair, INTDIV = integer division, PATH = shortest path / var-length, EXISTS = EXISTS {..} / NOT (pattern). Tier: A = agent-facing, C = catalog only.

| Name | Params | Category | Features | Tier |
|---|---|---|---|---|
| entity-relations | min_calls=5 | entity-axis | UNION | A |
| orphan-entities | - | entity-axis | EXISTS | A |
| hub-functions | - | entity-axis | - | A |
| endpoint-darkness | - | entity-axis | EXISTS | A |
| coverage-by-entity | - | entity-axis | OPT | A |
| entity-callers | entity | entity-axis | ALT | A |
| dead-files | - | call-graph | EXISTS | A |
| type-distribution | - | symbol-lookup | - | A |
| field-references | name | symbol-lookup | OPT CONT | A |
| types-by-symbol-pattern | pattern | symbol-lookup | CONT | C |
| types-by-file-pattern | pattern | symbol-lookup | CONT | C |
| untested-functions | - | test-surface | CONT EXISTS | A |
| callers-density-hub-test-coverage | - | test-surface | OPT CONT SEW CASE | C |
| untested-by-file-aggregated | - | test-surface | CONT SEW EXISTS | C |
| function-backward-slice | symbol | call-graph | - | A |
| function-forward-slice | symbol | call-graph | - | A |
| function-blast-radius-by-file | symbol | call-graph | - | A |
| function-callees-by-file | symbol | call-graph | - | C |
| function-shape-summary | symbol | call-graph | OPT CASE | C |
| functions-of-shape | shape_label | call-graph | OPT CONT CASE | C |
| function-shape-distribution | - | call-graph | OPT CONT CASE | C |
| function-shape-by-file | - | call-graph | OPT CONT CASE | C |
| file-shape-profile | file_path | call-graph | OPT CONT CASE | C |
| test-coverage-baseline | - | test-surface | OPT CASE | C |
| findings-by-file | - | file-layout | COLL | A |
| language-distribution | - | corpus-diagnostics | - | A |
| mcp-handlers | - | symbol-lookup | - | C |
| tag-axis-loose-ends | - | doc-tag-families | UNW COLL SIZE CONT LISTCOL | C |
| singleton-tags | - | doc-tag-families | UNW COLL LISTCOL IDX | C |
| docs-without-family-tag | - | doc-tag-families | SEW LISTCOL | C |
| docs-via-shared-entity-coverage | doc_id | entity-axis | COLL SIZE | C |
| discriminative-entities | - | entity-axis | - | C |
| entities-via-shared-doc-coverage | entity_id | entity-axis | COLL | C |
| entities-coupled-to-tag | tag | doc-tag-families | LISTCOL | C |
| research-pairs-without-shared-family-tag | - | doc-tag-families | UNW LISTCOL CASE | C |
| research-docs-with-all-singleton-meaningful-tags | - | doc-tag-families | OPT UNW LISTCOL CASE | C |
| established-research-families | - | doc-tag-families | UNW COLL SIZE LISTCOL | C |
| cross-family-concept-tags | - | doc-tag-families | UNW COLL SIZE SEW LISTCOL | C |
| family-tag-absorption-depth | - | doc-tag-families | UNW COLL SIZE SEW LISTCOL | C |
| lessons-for-research-family | tag | doc-tag-families | COLL SIZE SEW LISTCOL | C |
| family-tags-by-code-domain | - | doc-tag-families | UNW COLL SIZE LISTCOL CASE | C |
| audits-mentioning-family-tag | tag | doc-graph | CONT SEW | C |
| audits-by-iter-tag | iter_tag | doc-tag-families | SEW LISTCOL | C |
| family-tag-audit-counts-summary | - | doc-tag-families | UNW COLL SIZE CONT SEW LISTCOL | C |
| family-maturity-2d | - | doc-tag-families | UNW COLL SIZE CONT SEW LISTCOL CASE | C |
| doc-connectivity-ranking | - | doc-graph | UNDIR | C |
| feature-doc-connectivity | - | doc-graph | OPT UNDIR | C |
| doc-hub-tier-classifier | - | corpus-diagnostics | UNDIR CASE | C |
| tier1-architectural-hubs | - | doc-graph | UNDIR | C |
| family-to-feature-bridges | tag | doc-tag-families | LISTCOL | C |
| feature-incoming-from-research | - | doc-tag-families | OPT LISTCOL | C |
| retrieval-pipeline-cookbook | - | doc-tag-families | LISTCOL CASE | C |
| pipeline-stage-coverage | - | doc-tag-families | LISTCOL CASE | C |
| graph-similarity-cookbook | - | doc-tag-families | LISTCOL CASE | C |
| graph-similarity-subfamily-coverage | - | doc-tag-families | LISTCOL CASE | C |
| query-expansion-cookbook | - | doc-tag-families | LISTCOL CASE | C |
| query-expansion-subfamily-coverage | - | doc-tag-families | LISTCOL CASE | C |
| mature-topic-shape-distribution | family_tag | doc-tag-families | UNW SIZE CONT LISTCOL CASE | C |
| mature-topic-shape-census | - | doc-tag-families | UNW COLL SIZE CONT LISTCOL CASE | C |
| cross-family-bridge-papers | family_a, family_b | doc-tag-families | LISTCOL | C |
| cross-family-bridge-density | - | doc-tag-families | UNW SIZE CONT LISTCOL | C |
| shared-layer-source-files | - | file-layout | CONT CASE | C |
| resource-file-cluster | resource_stem | file-layout | CONT CASE | C |
| resource-shape-classifier | resource_stem | file-layout | CONT CASE | C |
| app-route-files | - | file-layout | CONT CASE | C |
| family-top-bridges | family_tag | doc-tag-families | UNW SIZE CONT LISTCOL | C |
| family-rich-cotags | family_tag | doc-tag-families | UNW SIZE CONT LISTCOL | C |
| generated-files | - | file-layout | CONT | C |
| hand-written-source-files | - | file-layout | CONT | C |
| architectural-layers-by-path | - | file-layout | CONT CASE | C |
| imports-edge-count-by-language | - | corpus-diagnostics | - | C |
| pytest-prod-test-pairs | - | test-surface | CONT SEW REGEX SELFJOIN | C |
| python-untested-files | - | test-surface | OPT CONT SEW REGEX | C |
| corpus-shape-classifier | - | corpus-diagnostics | INLIST CASE | C |
| narrative-doc-count | - | corpus-diagnostics | INLIST | C |
| corpus-ingest-state-classifier | - | corpus-diagnostics | OPT CASE | C |
| ingest-pass-status | - | corpus-diagnostics | OPT UNW MAP | C |
| tight-vs-loose-coupling-hubs | - | file-coupling | CASE | C |
| cold-start-overview | - | corpus-diagnostics | OPT INLIST CASE | C |
| corpus-routing-recommendation | - | corpus-diagnostics | OPT INLIST CASE | C |
| corpus-axis-saturation | - | corpus-diagnostics | OPT CASE | C |
| corpus-update-span | - | corpus-diagnostics | - | C |
| corpus-top-entities-by-mention | - | corpus-diagnostics | - | C |
| corpus-top-doc-tags | - | corpus-diagnostics | UNW LISTCOL | C |
| corpus-tag-axis-shape | - | corpus-diagnostics | UNW LISTCOL CASE | C |
| corpus-entity-promotion-state | - | corpus-diagnostics | SEW CASE | C |
| cluster-derived-entities | - | entity-axis | SEW | C |
| entities-by-id-pattern | pattern | entity-axis | CONT | C |
| entities-by-description-substring | pattern | entity-axis | CONT | C |
| entities-by-id-or-description-substring | pattern | entity-axis | CONT | C |
| entity-singular-plural-pairs | - | entity-axis | SEW SELFJOIN | C |
| corpus-singular-plural-pair-summary | - | corpus-diagnostics | SEW CASE SELFJOIN | C |
| entity-to-implementation-files | entity_id | entity-axis | OPT UNW COLL CONT | C |
| entity-implementation-summary | entity_id | entity-axis | OPT CONT | C |
| files-containing-entity-id-in-path | entity_id | file-layout | CONT | C |
| entity-organizational-shape | entity_id | file-layout | CONT CASE | C |
| entity-pair-comparison | entity_a, entity_b | entity-axis | - | C |
| entities-by-display-substring | pattern | entity-axis | CONT | C |
| entity-card-summary | entity_id | entity-axis | CONT SEW CASE | C |
| entity-card-with-file-axis | entity_id | entity-axis | OPT CONT SEW CASE | C |
| mirrored-private-functions-by-suffix | suffix | symbol-lookup | CONT | C |
| protocol-module-functions | protocol_module_substring | symbol-lookup | CONT | C |
| protocol-module-class-methods | protocol_module_substring | symbol-lookup | CONT | C |
| abstract-base-private-hooks | class_path_substring | symbol-lookup | CONT | C |
| abstract-base-private-hooks-non-test | class_path_substring | test-surface | CONT | C |
| entity-fact-vs-pattern-classifier | entity_id | entity-axis | OPT CASE | C |
| entity-fact-vs-pattern-classifier-v2 | entity_id | entity-axis | CONT SEW CASE | C |
| low-density-entities-as-likely-fact | - | entity-axis | CONT SEW | C |
| axis-routing-card-for-vocabulary | term | entity-axis | CONT UNION | C |
| cli-subcommand-pattern-entities | - | entity-axis | SEW | C |
| axis-routing-card-for-vocabulary-v2 | term | entity-axis | OPT UNW CONT | C |
| function-axis-symbol-search | term | symbol-lookup | CONT | C |
| utility-cluster-pattern-entities | - | entity-axis | CONT SEW | C |
| multi-file-extension-target-candidates | - | entity-axis | CONT SEW | C |
| error-type-catalog | - | symbol-lookup | CONT SEW | C |
| types-by-name-substring | term | symbol-lookup | CONT | C |
| failure-mode-class-catalog | - | symbol-lookup | CONT SEW | C |
| signal-class-by-substring | term | symbol-lookup | CONT | C |
| god-node-entities | - | entity-axis | - | C |
| function-entity-touch-card | function_symbol | entity-axis | - | C |
| corpus-calls-coverage-ratio | - | corpus-diagnostics | OPT CASE | C |
| function-direct-callers-count | function_symbol | call-graph | OPT | C |
| corpus-purpose-classifier | - | corpus-diagnostics | INLIST CASE INTDIV | C |
| non-ontology-docs | - | corpus-diagnostics | INLIST | C |
| onboarding-doc-shortlist | - | corpus-diagnostics | INLIST | C |
| session-context-docs | - | corpus-diagnostics | CONT | C |
| ontology-doc-shortlist | - | corpus-diagnostics | INLIST | C |
| ontology-vs-narrative-doc-ratio | - | doc-graph | INLIST CASE INTDIV | C |
| narrative-doc-list | - | corpus-diagnostics | - | C |
| cli-subcommand-pattern-entities-v2 | - | entity-axis | SEW | C |
| functions-in-cli-module | - | symbol-lookup | CONT | C |
| corpus-cold-start-summary | - | corpus-diagnostics | INLIST CASE INTDIV | C |
| corpus-entity-shape-summary | - | corpus-diagnostics | OPT CONT SEW | C |
| files-under-folder-path | folder_path | file-layout | CONT | C |
| methods-on-class-by-substring | class_name | symbol-lookup | CONT | C |
| test-entity-anchors | - | test-surface | SEW | C |
| test-files-for-domain | domain | test-surface | CONT | C |
| functions-mentioning-test-entity | test_entity_id | test-surface | - | C |
| test-functions-by-stem | stem | test-surface | CONT | C |
| test-files-by-stem | stem | test-surface | CONT | C |
| test-writing-card | stem | test-surface | OPT UNW CONT | C |
| test-fixture-helpers-by-stem | stem | test-surface | CONT | C |
| test-private-function-helpers-by-stem | stem | test-surface | CONT | C |
| test-inner-class-helpers-by-stem | stem | test-surface | CONT | C |
| test-fixture-idiom-classifier | - | test-surface | OPT UNW CONT CASE | C |
| corpus-test-shape-summary | - | test-surface | OPT UNW CONT SEW CASE | C |
| unified-corpus-diagnostic | - | entity-axis | OPT CONT INLIST CASE INTDIV | C |
| function-axis-token-distribution-for-stem | stem | symbol-lookup | OPT UNW CONT | C |
| corpus-test-coverage-ratio | - | test-surface | OPT CONT CASE | C |
| corpus-entities-per-doc-ratio | - | corpus-diagnostics | CASE | C |
| corpus-functions-per-file-ratio | - | corpus-diagnostics | CASE | C |
| corpus-doc-coverage-ratio | - | corpus-diagnostics | CASE | C |
| promoted-entities | - | entity-axis | SEW | C |
| promoted-with-pointer-entities | - | entity-axis | CONT SEW | C |
| promoted-entity-pointer-breakdown | - | entity-axis | CONT SEW CASE | C |
| corpus-entity-axis-shape | - | corpus-diagnostics | CASE | C |
| recent-research-by-update | - | doc-tag-families | SEW LISTCOL | C |
| stale-docs-with-impact | - | doc-graph | OPT INLIST | A |
| freshness-by-kind | - | doc-graph | INLIST | A |
| stale-narrative-docs | - | doc-graph | INLIST | A |
| docs-by-update-recency | - | doc-graph | - | C |
| loc-pair-clone-candidates | - | file-layout | CONT SELFJOIN | C |
| same-name-cross-dir-clones | - | file-layout | CONT REGEX SELFJOIN | C |
| function-count-by-language | - | corpus-diagnostics | - | C |
| public-api-functions | - | symbol-lookup | CONT | C |
| public-api-hub-functions | - | entity-axis | CONT | C |
| function-signature-population-by-language | - | corpus-diagnostics | CASE INTDIV | C |
| function-mentions-by-language | - | corpus-diagnostics | - | C |
| workflow-validations | - | doc-tag-families | UNW COLL LISTCOL INLIST | C |
| cross-corpus-validated-patterns | - | doc-tag-families | LISTCOL | C |
| doc-fanin | - | doc-graph | - | C |
| leaf-docs | - | doc-graph | EXISTS | C |
| storyline-leaf-detector | - | doc-tag-families | LISTCOL EXISTS | C |
| recent-leaf-docs | - | doc-graph | EXISTS | C |
| files-by-recency-desc | - | file-layout | - | C |
| files-by-recency-asc | - | file-layout | - | C |
| tests-in-file | path | test-surface | CONT | C |
| functions-in-directory | prefix | symbol-lookup | SEW | C |
| functions-in-file | path | symbol-lookup | - | C |
| types-in-file | path | symbol-lookup | - | C |
| files-by-path-substring | pattern | file-layout | CONT | A |
| function-files-by-pattern | pattern | symbol-lookup | CONT | C |
| path-axis-completeness-fingerprint | pattern | symbol-lookup | OPT CONT CASE | C |
| path-axis-fingerprint-cython-aware | pattern | symbol-lookup | OPT CONT SEW CASE | C |
| cython-presence-by-pattern | pattern | file-layout | CONT SEW CASE | C |
| files-by-path-pattern-with-coupling | pattern | file-coupling | OPT UNDIR CONT | C |
| files-by-pattern-language-split | pattern | file-layout | CONT | C |
| functions-by-symbol-pattern | pattern | symbol-lookup | CONT | C |
| module-cohesion-classifier | pattern | call-graph | OPT CONT CASE INTDIV | C |
| file-language-by-path | file_path | file-layout | - | C |
| function-call-count-between-files | caller_file, callee_file | call-graph | CONT | C |
| corpus-workflow-viability-classifier | - | corpus-diagnostics | OPT CONT SEW CASE | C |
| docs-by-summary-substring | substring | doc-graph | CONT | C |
| file-language-distribution | - | corpus-diagnostics | CONT | C |
| cython-file-count | - | file-layout | CONT SEW | C |
| files-by-path-prefix-impl-count-desc | path_prefix | symbol-lookup | OPT CONT SEW | C |
| files-by-path-prefix-impl-count-asc | path_prefix | symbol-lookup | OPT CONT SEW | C |
| files-by-basename-and-path-prefix-count | basename, path_prefix | file-layout | CONT SEW | C |
| files-by-basename-and-path-prefix | basename, path_prefix | symbol-lookup | OPT CONT SEW | C |
| files-by-basename | basename | symbol-lookup | OPT CONT SEW | C |
| files-by-basename-count | basename | file-layout | CONT SEW | C |
| per-file-cohesion-by-path-prefix | path_prefix | call-graph | OPT UNW COLL CONT SEW CASE INTDIV | C |
| per-file-cohesion-by-path-prefix-asc | path_prefix | call-graph | OPT UNW COLL CONT SEW CASE INTDIV | C |
| files-by-path-prefix | path_prefix | symbol-lookup | OPT CONT SEW | C |
| template-directory-survey | path_prefix | call-graph | OPT COLL CONT SEW CASE | C |
| leaf-with-deps-count-by-basename | basename | call-graph | OPT UNW COLL CONT SEW CASE | C |
| leaf-with-deps-files-by-basename | basename | call-graph | OPT UNW COLL CONT SEW CASE | C |
| leaf-with-deps-files-by-path-prefix | path_prefix | call-graph | OPT UNW COLL CONT SEW CASE | C |
| leaf-with-deps-files | - | call-graph | OPT UNW COLL CONT CASE | C |
| module-external-callees-by-pattern | pattern | call-graph | CONT | C |
| function-callers-of-symbol-pattern | pattern | call-graph | CONT | C |
| function-callees-of-symbol-pattern | pattern | call-graph | CONT | C |
| cython-files-by-pattern | pattern | file-layout | CONT SEW CASE | C |
| files-coupled-to | path | file-coupling | UNDIR | A |
| docs-by-tag | tag | doc-tag-families | LISTCOL | A |
| roles-using-tag | tag | doc-tag-families | LISTCOL | C |
| tag-axis-coverage-summary | - | doc-tag-families | UNW LISTCOL CASE | C |
| storyline-siblings | doc_id | doc-tag-families | UNW LISTCOL | C |
| tag-cooccurrence | - | doc-tag-families | UNW SEW LISTCOL | C |
| entity-cooccurrence | - | entity-axis | - | C |
| architectural-patterns | - | doc-tag-families | UNW COLL LISTCOL INLIST | C |
| docs-by-kind | kind | doc-graph | - | A |
| doc-role-distribution | - | corpus-diagnostics | - | C |
| loop-process-doc-distribution | - | doc-graph | SEW INLIST CASE | C |
| latest-by-loop-process | - | doc-graph | SEW CASE | C |
| loop-process-balance-metric | - | doc-graph | SEW CASE | C |
| corpus-doc-starter-ratio | - | corpus-diagnostics | SEW INLIST CASE | C |
| authored-docs-sample | - | corpus-diagnostics | SEW INLIST | C |
| corpus-narrative-doc-roles-distribution | - | corpus-diagnostics | SEW INLIST | C |
| corpus-narrative-doc-kinds-distribution | - | corpus-diagnostics | SEW INLIST CASE | C |
| authored-docs-by-kind | kind | corpus-diagnostics | SEW INLIST | C |
| corpus-narrative-doc-lifecycles-distribution | - | corpus-diagnostics | SEW INLIST CASE | C |
| hub-functions-by-caller-breadth | - | call-graph | CONT SEW CASE | C |
| hub-spread-distribution | - | call-graph | CONT SEW CASE | C |
| retrieval-failure-modes | - | doc-tag-families | UNW COLL LISTCOL INLIST | C |
| retrieval-mitigation-modes | - | doc-tag-families | UNW COLL LISTCOL INLIST | C |
| endpoint-by-kind | - | other-nodes | - | A |
| callgraph-roots | - | call-graph | EXISTS | A |
| import-fan-out | - | file-coupling | - | A |
| coupled-files | - | file-coupling | - | A |
| coupled-files-without-scaffold | - | file-coupling | - | C |
| coupled-files-by-jaccard-range | min_jaccard, max_jaccard | file-coupling | - | C |
| feature-evolution-coupling-hybrid | - | file-coupling | - | C |
| coupling-density-stats | - | file-coupling | CASE | C |
| rust-test-files-by-pattern | - | test-surface | CONT SEW CASE | C |
| rust-production-source-files | - | file-layout | SEW | C |
| hub-types-by-caller-breadth | - | call-graph | CONT CASE | C |
| type-spread-distribution | - | call-graph | CONT CASE | C |
| function-hub-files | - | call-graph | CONT SEW | C |
| type-hub-files | - | call-graph | CONT | C |
| findings-by-attachment-mode | - | other-nodes | CASE | C |
| unstubbed-concepts-by-doc | - | other-nodes | - | C |
| entity-mention-ratio | - | entity-axis | OPT CONT CASE | C |
| entity-flavor-distribution | - | entity-axis | OPT CONT CASE | C |
| entity-belongs-vs-mentions-ratio | - | entity-axis | OPT CONT CASE | C |
| entity-lifecycle-distribution | - | entity-axis | OPT CONT CASE | C |
| entity-state-grid | - | entity-axis | OPT CONT CASE | C |
| entity-cluster-distribution | - | entity-axis | OPT CONT CASE | C |
| file-coupling-degree | - | file-coupling | UNDIR | C |
| language-loc-distribution | - | corpus-diagnostics | - | C |
| feature-evolution-coupling | - | file-coupling | - | C |
| feature-evolution-coupling-hubs | - | file-coupling | - | C |
| feature-evolution-coupling-by-commits | - | file-coupling | - | C |
| unstubbed-concepts | - | other-nodes | COLL | C |
| roadmap-unmet-deps | - | doc-graph | INLIST | C |
| competitive-tools | - | entity-axis | SIZE LISTCOL | C |
| phasing | - | doc-graph | COLL MAP | C |
| node-context-function | symbol | context-envelope | OPT COLL MAP | A |
| node-context-doc | doc | context-envelope | OPT COLL ALT LISTCOL MAP LABEL | A |
| node-context-entity | entity | context-envelope | OPT COLL MAP | A |
| decisions-to-prior-art | - | doc-tag-families | ALT CONT SEW LISTCOL LABEL | C |
| concept-work-list | - | entity-axis | OPT INLIST | A |
