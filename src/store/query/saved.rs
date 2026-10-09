//! Roadmap issue #29 (v0.3.0): curated catalog of high-value saved views
//! shipped with the binary. Agents reach for these instead of composing
//! `query sql` strings by hand.
//!
//! ## What's here
//!
//! A static slice of `SavedQuery` entries. Each carries:
//!   - `name`       — kebab-case identifier the user types.
//!   - `description` — one-line summary printed under `--list`.
//!   - `params`     — array of placeholder names that `--param key=val`
//!                    must supply (e.g. `["entity"]`); `name=default`
//!                    declares an optional one (`["min_calls=5"]`).
//!   - `needs`      — node tables the query must find rows in; the MCP
//!                    listing hides a query whose table is empty.
//!
//! The SQL itself lives in `store_sqlite/saved_sql/<name>.sql` (embedded by
//! `build.rs`; `$key` placeholders are bound to the `--param` values).
//! A corpus can add or override queries with `<root>/saved-queries/*.toml`
//! (`name`, `description`, `params`, `sql`).

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Static catalog entry. Held as `&'static` because the catalog is
/// hardcoded; runtime TOML queries use the owned [`RuntimeSavedQuery`].
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SavedQuery {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [&'static str],
    /// Node tables the query needs rows in to return anything (the labels
    /// bound by its non-optional matches). An empty table hides the query
    /// from the MCP listing.
    pub needs: &'static [&'static str],
}

/// Built-in catalog. Order is the `--list` output order — keep
/// related queries grouped (entity / endpoint / function) so an
/// agent scanning the list can find the right view fast.
pub const SAVED_QUERIES: &[SavedQuery] = &[
    SavedQuery {
        name: "entity-relations",
        description: "entity -> entity relations in one place: authored RELATES_TO with its \
                      type (origin `authored`) plus ENTITY_CALLS, the code-derived call \
                      density, with frequency >= $min_calls (default 5) as type `calls` \
                      (origin `derived`)",
        params: &["min_calls=5"],
        needs: &["Entity"],
    },
    SavedQuery {
        name: "orphan-entities",
        description: "entities with no covering Doc AND no FUNCTION_BELONGS_TO",
        params: &[],
        needs: &["Entity"],
    },
    SavedQuery {
        name: "hub-functions",
        description: "functions ranked by FUNCTION_MENTIONS count (likely god-functions)",
        params: &[],
        needs: &["Entity", "Function"],
    },
    SavedQuery {
        name: "endpoint-darkness",
        description: "Endpoints with no ENDPOINT_TOUCHES_ENTITY edge",
        params: &[],
        needs: &["Endpoint"],
    },
    SavedQuery {
        name: "coverage-by-entity",
        description: "how many functions cover each entity, ranked",
        params: &[],
        needs: &["Entity"],
    },
    SavedQuery {
        name: "entity-callers",
        description: "every function in any chain calling into entity $entity (depth 1)",
        params: &["entity"],
        needs: &["Entity", "Function"],
    },
    SavedQuery {
        name: "dead-files",
        description: "Files whose functions have zero inbound CALLS and no Endpoint binding",
        params: &[],
        needs: &["File", "Function"],
    },
    // Roadmap issue #32: distribution over the Type table's
    // kind column. Useful as a sanity check that the dual-write
    // landed (non-empty rows) and to spot ratios that look off
    // for the repo's domain.
    SavedQuery {
        name: "type-distribution",
        description: "Type rows grouped by kind (struct / enum / trait / module / type_alias)",
        params: &[],
        needs: &["Type"],
    },
    // Per iter 275 (sibling to iter-271 functions-by-
    // symbol-pattern at the Type axis): Type-axis pattern
    // discovery. Closes the symmetry gap — Function axis
    // got iter-271's pattern primitive but Type axis only
    // had iter-110's whole-corpus kind distribution. Agent
    // asking "show me Type rows for X" can now compose this
    // with type-distribution + iter-110 query_similar type=
    // type for a complete Type-axis routing.
    // Legacy gap 7: "where is property X read or written".
    SavedQuery {
        name: "field-references",
        description: "Fields / properties whose name CONTAINS `$name` (case-sensitive substring, so a partial name works as a fuzzy lookup), with every function that reads or writes each one: the function, its file and the line of its first reference. Fields come from `Type#name.` SCIP symbols (C# properties and fields, TS class members, Dart fields, Rust struct fields, Java fields). Fields with no reader come back with an empty function. LIMIT 50.",
        params: &["name"],
        needs: &["Field"],
    },
    SavedQuery {
        name: "types-by-symbol-pattern",
        description: "Types whose symbol CONTAINS the given `$pattern`, with test-path filtering. Type-axis sibling of [[functions-by-symbol-pattern]] (iter-271). Returns each Type's symbol + kind (struct / enum / trait / type_alias) + file path + line. Implementation-only via `NOT t.file CONTAINS 'tests/'` + `NOT t.symbol CONTAINS 'tests/'` per iter-225 [[feedback_test_for_sparse_universal]] convention. On doc-linter source with `$pattern='Config'` should surface LintConfig + DocLintConfig types from the config module; on example-app with $pattern='Pattern' should surface Pattern + PatternDetector type rows. LIMIT 50.",
        params: &["pattern"],
        needs: &["Type"],
    },
    // Per iter 275 (file-axis Type lookup): given a file
    // path substring, return Types defined in matching
    // files. Lets the agent answer "what types live in
    // file X?" in 1 call. Composes with iter-269
    // files-by-path-substring + type-distribution to give
    // the agent a per-file structural overview.
    SavedQuery {
        name: "types-by-file-pattern",
        description: "Types defined in files whose path CONTAINS the given `$pattern`. File-axis Type lookup — composes with [[files-by-path-substring]] to answer 'what types live in module X?'. Returns each Type's symbol + kind + file + line, grouped by file. Test-exclusion is two-pronged per [[user-probe-067]] Finding B: BOTH `NOT t.file CONTAINS 'tests/'` AND `NOT t.symbol CONTAINS 'tests/'`. The symbol-side filter catches in-file `#[cfg(test)] mod tests` blocks that SCIP indexes as Type rows on non-test file paths (e.g., src/store/query/dead_code.rs has a symbol ending `dead_code/tests/`). Aligns with sibling [[types-by-symbol-pattern]] (iter-275) + [[functions-by-symbol-pattern]] (iter-271). On doc-linter source with `$pattern='store/query'` should surface SavedQuery + the at.rs structs but NOT the dead_code/tests/ module; on example-app with `$pattern='pattern_detector'` should surface Pattern + PatternDetector types from pattern_detector.py. LIMIT 50.",
        params: &["pattern"],
        needs: &["Type"],
    },
    // Roadmap issue #24: surfaces Functions with no inbound
    // TEST_FOR edge. Combined with the dark-public-function
    // diag this gives an actionable "needs a test" view.
    SavedQuery {
        name: "untested-functions",
        description: "Functions with no inbound TEST_FOR edge — candidates for new tests",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 218 (user-probe-052 Finding C — hub-axis
    // companion to untested-functions). The Function-axis
    // alone (untested-functions) returns thousands of raw
    // rows on full-scip corpora and lacks architectural-
    // importance signal. THIS query intersects with CALLS
    // density to surface the ARCHITECTURALLY CRITICAL
    // untested functions — functions called by ≥5 others
    // that lack test coverage. Categorical emission per
    // iter-203/211 pattern: `critical-untested-hub` (≥10
    // callers / 0 tests), `untested-hub` (5-9 callers / 0
    // tests), `tested-hub` (any callers / has tests).
    // Excludes `.pyi` per Finding B (type-stub interfaces
    // aren't directly testable; tests target the underlying
    // .pyx implementations).
    SavedQuery {
        name: "callers-density-hub-test-coverage",
        description: "Function hubs (≥5 callers) with categorical test-coverage classification per iter-217 user-probe-052: `critical-untested-hub` (≥10 callers + 0 TEST_FOR edges — architectural blast risk), `untested-hub` (5-9 callers + 0 tests), `tested-hub` (callers + has tests). Excludes `.pyi` type-stub files per Finding B (stubs declare interfaces tested via underlying .pyx implementations; surfacing them as untested is a false positive). Excludes /tests/ path. On spacy this surfaces Example.from_dict (180c/0t), Language.initialize (176c/0t), Token.text (160c/0t) as critical-untested-hubs at the very top. Use to prioritize test writing by ARCHITECTURAL importance, not just file-axis untested count.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 218 (user-probe-052 Q2 + Finding B): file-
    // axis companion to untested-functions that aggregates
    // and excludes `.pyi` type-stub files. The raw
    // untested-functions return is thousands of rows on
    // full-scip corpora; this surface gives the agent a
    // per-file density signal it can read at a glance.
    // Excludes `.pyi` per Finding B + /tests/ per existing
    // convention.
    SavedQuery {
        name: "untested-by-file-aggregated",
        description: "Per-file count of untested Functions (no inbound TEST_FOR edge), excluding .pyi type-stub files + tests paths. The aggregated form of iter-170 untested-functions — file-axis density signal an agent can read at a glance instead of paging thousands of raw rows. Per user-probe-052 Finding B: .pyi exclusion mirrors iter-153 generated-files filter discipline. Per user-probe-053 Finding B (iter 220): path filter is `tests/` (no leading slash) so it catches BOTH nested (spacy/tests/) AND root-level (tests/) conventions. On spacy this surfaces spacy/util.py (102 untested), spacy/language.py (73), spacy/pipeline/spancat.py (27); on example-app surfaces example_app/cross_session_memory.py (18) + example_app/project_trajectory.py (15). **CALIBRATION CAVEAT** per [[feedback_test_for_sparse_on_python]]: on Python corpora the 0-tests signal is unreliable — pair this query with `test-coverage-baseline` to gauge trust.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 228 (research-sridharan-thin-slicing's
    // depth-2 recommendation): function-axis program-slice
    // primitive. Given a target Function symbol, returns
    // the set of upstream callers within 2 hops along the
    // CALLS graph. This is a function-level thin-backward-
    // slice — answers "what code feeds into this function?"
    // at the precision Sridharan argued was the right
    // engineer-comprehension granularity. Companion to
    // function-forward-slice (the symmetric variant).
    // Depth is hardcoded to 2 per the thin-slicing
    // precision argument; a parameterized variant would
    // need numeric-param support which the saved-query
    // system currently lacks (all params are string-quoted
    // per saved.rs line ~1730).
    SavedQuery {
        name: "function-backward-slice",
        description: "Per-function backward thin-slice on the CALLS graph at depth 1. Given a Function `$symbol`, returns its direct callers — functions that call the target. Per [[research-sridharan-thin-slicing]] this is the producer-precision floor; depth-2 was attempted in iter-228 but Kuzu's binder rejected the variable-length-match scoping (filed as a Type D candidate to revisit). Use this when scoping a refactor: 'who calls THIS function directly?' Companion to function-forward-slice. Empty result IS architectural signal — the function is a leaf-of-the-callgraph or entry-point.",
        params: &["symbol"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "function-forward-slice",
        description: "Per-function forward thin-slice on the CALLS graph at depth 1. Given a Function `$symbol`, returns its direct callees — what code the target REACHES at 1 hop. Per [[research-sridharan-thin-slicing]] depth-1 is the producer-precision floor; depth-2 (Kuzu-binder-blocked per iter-228 attempt) would extend to indirect callees. Companion to function-backward-slice. Empty result = the function is a leaf (calls nothing) — interesting architectural signal for pure-function detection.",
        params: &["symbol"],
        needs: &["Function"],
    },
    // Per iter 231 (user-probe-055 Finding D — file-axis
    // companion to function-backward-slice). The
    // function-backward-slice query returns per-CALLER
    // rows; on hubs like val_string this produces 150+
    // rows of CALLS-edge enumeration that's hard to
    // route by. THIS query aggregates to FILE granularity:
    // per file containing callers, return the file path
    // + distinct-caller count. Reduces the val_string
    // result from 150 edges to 8 files (per the iter-230
    // measurement) — a 5.5x compression that surfaces
    // subsystem ownership directly.
    SavedQuery {
        name: "function-blast-radius-by-file",
        description: "Per-FILE aggregation of function-backward-slice. Given a Function `$symbol`, returns the set of files containing its direct callers + per-file distinct-caller count, ordered by distinct count DESC. Per user-probe-055 Finding D: edge-count is misleading on hubs (val_string has 150 CALLS edges but only 27 distinct callers across 8 files); the file-axis aggregation gives an actionable subsystem-routing signal. On doc-linter MCP for val_string: store/query/functions.rs 8 + store/query/coverage.rs 5 + store/code_ingest.rs 3 + ... = 8 files all within store/* (subsystem-internal utility). Use this to assess refactor blast radius at subsystem granularity.",
        params: &["symbol"],
        needs: &["Function"],
    },
    // Per iter 231 (forward companion to function-blast-
    // radius-by-file). Same file-axis aggregation but
    // FORWARD: per-file count of direct callees the target
    // reaches. Surfaces "what subsystems does this
    // function touch?" at file granularity.
    SavedQuery {
        name: "function-callees-by-file",
        description: "Per-FILE aggregation of function-forward-slice. Given a Function `$symbol`, returns the set of files containing its direct callees + per-file distinct-callee count. The forward direction analog of function-blast-radius-by-file. Use to assess at subsystem granularity what the target function reads/depends on. Empty result = function is a leaf at file granularity (a pure self-contained operation). Returns LIMIT 25 ordered by distinct_callees DESC.",
        params: &["symbol"],
        needs: &["Function"],
    },
    // Per iter 233 (user-probe-056 Finding E + iter-211
    // cold-start-overview composite pattern): the function-
    // shape categorical classifier. Composes the 4-query
    // refactor-impact matrix into 1 call emitting per-
    // direction counts + categorical shape label. Per
    // user-probe-056 Q1+Q2+Q3 measurement (tool_text_result
    // = 17/1/0/0 → pure-leaf-factory) + iter-230 Q1+Q2+Q3
    // (parse_doc = 15/13/1/1 → thin-entry-point) the
    // 4-axis composition cleanly distinguishes architectural
    // shapes. Sibling to iter-228 function-backward-slice
    // + iter-231 function-blast-radius-by-file at the
    // composition layer.
    SavedQuery {
        name: "function-shape-summary",
        description: "Per-function architectural-shape categorical classifier — composes the iter-228 + iter-231 4-query matrix into 1 call. Returns caller_count + caller_file_count + callee_count + callee_file_count + categorical shape label per user-probe-056 Finding E + user-probe-057 refinement. Categories: `pure-leaf-factory` (≥5 callers in 1 file + 0 callees — e.g. tool_text_result), `wide-leaf-utility` (≥5 callers in ≥2 files + 0 callees — e.g. val_string per iter-235 refinement; widely-used but calls nothing), `thin-entry-point` (≥5 callers in ≥3 files + 1-2 callees — e.g. parse_doc; forwards traffic downstream), `subsystem-hub` (≥5 callers in 1 file + ≥1 callee — single-file utility with downstream), `extension-point` (≥5 callers in ≥2 files + ≥5 callees — many-in many-out), `peripheral` (otherwise — leaf functions / unused / corner cases). Per user-probe-057: thin-entry-point now requires callee_count >= 1 to distinguish forwarding from pure-leaf-utility.",
        params: &["symbol"],
        needs: &[],
    },
    // Per iter 235 (user-probe-057 Finding B follow-up):
    // INVERSE query for function-shape-summary. Given a
    // shape label, returns Functions of that shape ordered
    // by caller_count DESC. Pairs with shape-summary: agent
    // either asks "what shape is THIS function?" (shape-
    // summary) or "find me functions OF this shape"
    // (this query). Surfaces architectural-pattern
    // instances at corpus scale.
    SavedQuery {
        name: "functions-of-shape",
        description: "Inverse of function-shape-summary. Given `$shape_label` (one of `pure-leaf-factory` / `wide-leaf-utility` / `thin-entry-point` / `subsystem-hub` / `extension-point` / `peripheral`), returns PRODUCTION Functions matching that shape ordered by caller_count DESC. Per iter-237 (user-probe-058 Finding D): target-filter excludes test-fixture functions via `NOT target.file CONTAINS 'tests/' AND NOT target.symbol CONTAINS 'tests/'` per iter-218 + iter-225 conventions. Pre-iter-237 top-10s were 30-50% test-fixture-dominated; post-fix surfaces production architecture only. Use to find ALL instances of an architectural pattern at corpus scale — e.g. all pure-leaf-factories signal candidate code-quality targets (each is a sibling-pattern dispatch site). LIMIT 25.",
        params: &["shape_label"],
        needs: &["Function"],
    },
    // Per iter 237 (user-probe-058 Finding D follow-up):
    // corpus-wide function-shape distribution. Each row =
    // shape category + count of Functions matching. Lets
    // agent see "this corpus has N pure-leaf-factories +
    // M wide-leaf-utilities + ..." at a glance — the
    // architectural-overview view. Test-fixtures filtered
    // per iter-237 functions-of-shape pattern. Empty result
    // = no production Functions cross the ≥5 caller hub
    // floor (the corpus is small or function-axis ingest
    // didn't run).
    SavedQuery {
        name: "function-shape-distribution",
        description: "Corpus-wide categorical distribution of production Function shapes. Each row: shape label + count of Functions matching that shape (≥5 callers + per-shape thresholds per [[function-shape-summary]]). Test-fixtures filtered per iter-237 (`NOT target.file CONTAINS 'tests/'` + symbol-CONTAINS). Lets agent see the architectural-pattern density at a glance — e.g. 'doc-linter has 5 thin-entry-points + 4 wide-leaf-utilities + 3 pure-leaf-factories'. Mirrors iter-209 tag-axis-coverage-summary at the function-axis. Use for cold-start architectural orientation; compose with functions-of-shape($shape) to drill into specific category.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 239 (sibling to iter-237 function-shape-
    // distribution at File granularity): per-file shape
    // distribution. Aggregates production Function shapes
    // grouped by file.path so an agent investigating a
    // specific area sees "in src/parser.rs there are 3
    // thin-entry-points + 1 wide-leaf-utility" without
    // hand-joining functions-of-shape * file. Test-fixtures
    // filtered per iter-237 convention.
    SavedQuery {
        name: "function-shape-by-file",
        description: "Per-file production function-shape distribution. Returns file + shape label + count of Functions matching that shape in that file. Multi-row per file (one row per non-empty shape). Sibling to iter-237 [[function-shape-distribution]] at File granularity instead of corpus axis. Test-fixtures filtered per iter-237 (`NOT target.file CONTAINS 'tests/'` + symbol-CONTAINS). Use to surface the architectural-shape composition of each file in 1 call — agent investigating src/foo.rs sees its hub / utility / entry-point breakdown without iterating functions-of-shape per shape. Ordered by file ASC, function_count DESC.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 239 (sibling to iter-235 functions-of-shape
    // inverse — by-file projection): given a file-path
    // substring, list all production Functions in matching
    // files with their shape labels. Agent investigating
    // src/parser.rs uses this in 1 call instead of
    // iterating functions-of-shape per shape and grepping
    // for file.
    SavedQuery {
        name: "file-shape-profile",
        description: "Given `$file_path` (substring match via CONTAINS), returns all PRODUCTION Functions in matching files with their architectural-shape labels. Multi-row per file ordered by caller_count DESC. Pairs with iter-237 [[functions-of-shape]] — instead of 'find all functions of shape X corpus-wide' use 'show shapes of all hub Functions in file Y'. Same-family probe pattern as iter-218 untested-by-file-aggregated + iter-231 function-callees-by-file at File granularity. Test-fixtures filtered per iter-237 convention. LIMIT 50.",
        params: &["file_path"],
        needs: &["Function"],
    },
    // Per iter 220 (user-probe-053 Finding A + memory
    // [[feedback_test_for_sparse_on_python]] promotion):
    // calibration baseline for the test-coverage signal.
    // Reports TEST_FOR edge count + Function count + rate +
    // categorical label. Agent uses this BEFORE trusting
    // untested-functions / untested-by-file / callers-
    // density-hub-test-coverage outputs to gauge whether
    // the 0-tests signal carries information OR is noise.
    // Per the memory: Python corpora typically fall in
    // `coverage-sparse` / `coverage-empty`; Rust corpora
    // typically fall in `coverage-rich` if `#[cfg(test)]
    // mod tests` convention is used.
    SavedQuery {
        name: "test-coverage-baseline",
        description: "Corpus-wide TEST_FOR coverage rate + categorical trust-label per [[feedback_test_for_sparse_on_python]]. Returns 1 row: tested_function_count + total_function_count + coverage_rate (as percentage) + label. Categories: `coverage-rich` (≥10% — trust the 0-tests signal as untested), `coverage-sparse` (1-10% — interpret 0-tests as 'heuristic didn't match', not 'untested'), `coverage-empty` (<1% or 0 — 0-tests is noise; rely on CALLS-density alone). Run BEFORE callers-density-hub-test-coverage / untested-by-file-aggregated / untested-functions to calibrate trust in their `untested` classifications. Per the memory: Python typically lands in coverage-sparse/empty; Rust in coverage-rich.",
        params: &[],
        needs: &[],
    },
    SavedQuery {
        name: "findings-by-file",
        description: "Outstanding TODO / FIXME / HACK markers grouped by file, ranked by count. Returns file.path + total findings count + a DISTINCT list of finding kinds for that file (typically 1-3 distinct values).",
        params: &[],
        needs: &["File", "Finding"],
    },
    SavedQuery {
        name: "language-distribution",
        description: "File rows grouped by language — cold-start repo composition view",
        params: &[],
        needs: &["File"],
    },
    // Per iter 119 (user-probe-022 Finding C): when an agent
    // asks "how do I add a new MCP tool to this codebase?",
    // hub-functions (FUNCTION_MENTIONS-ranked) misses the
    // tool_X handlers — they have few entity mentions.
    // ENDPOINT_HANDLED_BY directly maps Endpoint to its
    // Function; filtering kind='mcp' gives the canonical
    // worked-example surface. Returns empty when the corpus
    // hasn't been re-ingested after iter 62's
    // gap-endpoint-extractor-misses-mcp-tools closure (commit
    // cd255ff) — that's diagnostic of the build-queue lag
    // user-probe-022 named, not a bug here.
    SavedQuery {
        name: "mcp-handlers",
        description: "MCP-tool Endpoints and their backing Function handlers — canonical 'show me an existing handler' query for contributors adding a new MCP tool",
        params: &[],
        needs: &["Endpoint", "Function"],
    },
    // Per iter 129 (interrogation-026 Finding C — 3rd reproduction
    // of the tag-axis loose-end pattern named in iter 111 +
    // 024 + 026): corpus-wide counterpart of iter 112's per-doc
    // suggest_tags_for_doc tool. Scans every Doc against the
    // catalogue of known tags; flags pairs where the Doc's
    // SUMMARY contains a tag-name the Doc doesn't carry. Coarse
    // (exact-substring only, no space-form vs kebab-form
    // normalisation; the per-doc tool handles that) but
    // mechanically detects the systematic loose-end the loop
    // has documented three times so the user can batch-fix
    // them with a single saved-query call.
    SavedQuery {
        name: "tag-axis-loose-ends",
        description: "Docs whose `summary` contains a known tag-name that they don't carry — corpus-wide tag-axis loose-end detector. Returns one row per (doc, suggested_tag) pair; doc-level dedup is the agent's job. Coarse substring match (exact kebab-form only); the per-doc `suggest_tags_for_doc` MCP tool handles space-form vs kebab-form normalisation.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 129 (interrogation-026 Finding D — loop-meta
    // authoring practice): the OTHER side of the tag-axis
    // loose-end pattern. When a Type R author chooses a new
    // family-tag for an absorption without checking siblings,
    // the result is often a SINGLETON tag — used by only one
    // doc, sometimes a slight variant of an existing
    // family-tag (e.g. `structural-similarity` vs the
    // established `graph-similarity`). Surfacing singletons
    // lets the agent ask "should this be folded into a
    // sibling family-tag?" before publishing the new tag as
    // a corpus-wide convention.
    SavedQuery {
        name: "singleton-tags",
        description: "Tags carried by exactly ONE Doc — candidates for either unification with a sibling family-tag OR retirement. Surfaces the tag-axis-loose-end pattern from the OPPOSITE side: when a new absorption uses a new tag-name without checking siblings, this query lists what got created. Compose with `research-by-tag(<candidate>)` to look up sibling family-tags before unifying.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 247 (user-probe-060 Finding B —
    // production-code-context-engines tag-axis-loose-end):
    // dual of singleton-tags at the Doc axis. Surfaces
    // Docs whose tags array contains NO discriminative
    // non-stopword tag — i.e., docs filed under generic
    // bucket tags only (`tool`, `production`, `research`,
    // `paper`, ...). The 7 production code-context-engine
    // docs (Sourcegraph + Cursor + Aider + Augment +
    // Continue.dev + Cline + DeepCode) before iter-247's
    // retag exemplified this: all 7 carried only
    // `tool, production, code-rag` — the third of which is
    // load-bearing but too broad. After the retag they
    // share `code-context-engine`. This query flags the
    // remaining loose-end docs at corpus scale.
    SavedQuery {
        name: "docs-without-family-tag",
        description: "Docs (role=doc) whose `tags` array contains ZERO non-stopword tags — buried under generic bucket-tags (`tool`, `production`, `research`, `paper`, `framework`, `survey`, `design`, `system`) without a discriminative family-tag. Surfaces the [[user-probe-060]] Finding B pattern: the production-code-context-engines family was 7 docs with no shared discriminative tag at iter 246. Companion to singleton-tags (node-axis from the OTHER side — singleton tags vs no-discriminative-tag docs). Use after a Type-R bundle to verify the absorbed docs carry a family tag, or as a corpus-wide retag audit. Stopwords identical to tag-axis-coverage-summary's filter set; also drops `audit` + `iter-*` per-probe-metadata tags. Empty-tags docs surface too (the most extreme form of the loose-end). Per iter-248 [[interrogation-052]] Finding C: excludes `audit-run-*` + `feature-*` HISTORICAL records (DO-NOT-TOUCH per loop spec) — the loop's prior self-referential sets legitimately carry only the `audit` / `feature` bucket-tag respectively and aren't authored against the current tagging convention.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 251 (iter-249 [[research-rocchio-relevance-
    // feedback]] + iter-243 Carpineto LOCAL-AQE sub-class
    // operationalisation at the entity-axis): given a seed
    // Doc, find OTHER Docs covering the most overlapping
    // Entities via COVERS edges. The Rocchio local-AQE
    // signal at doc-linter's entity-coverage axis — if user
    // is reading Doc X (which COVERS entities {a, b, c}),
    // Docs covering {a, b} or {a, c} or {b, c} are the
    // pseudo-relevance-feedback candidates Rocchio would
    // expand the query with. Sibling to iter-245 entity-
    // cooccurrence (which is pair-axis without a seed
    // anchor) at the seed-anchored single-doc variant.
    SavedQuery {
        name: "docs-via-shared-entity-coverage",
        description: "Given a seed Doc `$doc_id`, returns OTHER Docs sharing ≥1 DISCRIMINATIVE covered Entity (via COVERS edges), ordered by shared-entity count DESC. Operationalises [[research-rocchio-relevance-feedback]]'s local-AQE signal at the entity-coverage axis with [[research-voorhees-wordnet-ir]]'s external-AQE entity-discriminativeness refinement: only entities covered by ≤ 7 docs corpus-wide are counted (IDF-style — common entities like the 3 canonical hub entities cover-everything and add noise, not signal). Per iter-252 [[user-probe-061]] Finding A: the unfiltered iter-251 variant returned 15 docs tied at the max shared_count because every research doc covers the same 3 canonical entities; this refinement makes the signal informative. Companion to [[storyline-siblings]] (tag-axis) + [[entity-cooccurrence]] (pair-axis) + [[discriminative-entities]] (the entities themselves at corpus scale). Self-excludes the seed. LIMIT 25.",
        params: &["doc_id"],
        needs: &["Doc", "Entity"],
    },
    // Per iter 253 (operationalises [[research-voorhees-
    // wordnet-ir]]'s external-AQE entity-discriminativeness
    // signal at corpus axis): the entities themselves ranked
    // by INVERSE coverer count (rarer = more discriminative,
    // IDF-style). Companion to docs-via-shared-entity-coverage
    // — that one uses the discriminative set to find
    // neighbors of a seed doc; THIS one lists the
    // discriminative entities the corpus owns. Combine to ask
    // "which entities give my retrieval primitive purchase?".
    SavedQuery {
        name: "discriminative-entities",
        description: "Entities ranked by INVERSE doc-coverage count (rarer = more discriminative, IDF-style). Returns the top 25 entities whose corpus-wide doc-cover count is ≤ 7 (matching the iter-209 tag-axis load-bearing threshold). Operationalises [[research-voorhees-wordnet-ir]]'s external-AQE: the entities that DISCRIMINATE retrieval. Use to audit the corpus's entity-coverage distribution + as the entity-axis surface that powers [[docs-via-shared-entity-coverage]]'s ≤ 7 filter. Identifies which entities deserve more specific Doc.covers entries; an entity with cover_count = 1 is a near-singleton — either retag adjacent docs (per [[feedback_retag_adjacent_paper_recipe]]) or accept the entity is genuinely narrow. Ordered cover_count ASC, entity_id ASC.",
        params: &[],
        needs: &["Doc", "Entity"],
    },
    // Per iter 257 (entity-axis sibling of iter-251 docs-
    // via-shared-entity-coverage + extends iter-249
    // [[research-rocchio-relevance-feedback]] to the
    // entity-axis): given a seed Entity, return OTHER
    // Entities co-covered by ≥1 of the same docs. Rocchio's
    // local-AQE pseudo-relevance-feedback at the entity
    // axis. Where docs-via-shared-entity-coverage answers
    // "given a doc, which other docs share its entity
    // coverage?", this answers "given an entity, which
    // other entities show up in the same docs?".
    SavedQuery {
        name: "entities-via-shared-doc-coverage",
        description: "Given a seed Entity `$entity_id`, returns OTHER Entities co-covered by the same Docs, ordered by shared-doc count DESC. Entity-axis sibling of [[docs-via-shared-entity-coverage]]; operationalises [[research-rocchio-relevance-feedback]]'s local-AQE signal at the entity axis. Where docs-via-shared-entity-coverage asks 'given a doc, which docs are nearby in entity space?', this asks 'given an entity, which entities show up together with it?'. Each row carries the explicit shared-docs count for interpretable assignment. Self-excludes the seed. Use for entity-axis traversal: given the agent has Entity X in mind, this surfaces concept neighbors via doc co-cover evidence. LIMIT 25.",
        params: &["entity_id"],
        needs: &["Doc", "Entity"],
    },
    // Per iter 257 (cross-axis tag → entity expansion
    // candidate enumeration; operationalises
    // [[research-voorhees-wordnet-ir]]'s external-AQE at
    // the agent-query-rewrite boundary): given a tag, find
    // Entities that frequently appear (via COVERS) on Docs
    // carrying that tag. Surfaces tag→entity expansion
    // candidates — if the agent is querying for 'agent-
    // adaptive-rag', this returns the entities that
    // adaptive-RAG-tagged docs collectively cover (e.g.,
    // entity-retrieval-primitive at high count, entity-
    // pattern-transfer at high count). The agent can then
    // re-rank including those entity terms.
    SavedQuery {
        name: "entities-coupled-to-tag",
        description: "Given a tag `$tag`, returns the Entities most frequently covered by Docs carrying that tag, ordered by coverer-doc count DESC. Cross-axis bridge: tag → entity expansion candidates. Operationalises [[research-voorhees-wordnet-ir]]'s external-AQE at the agent-query-rewrite boundary: if a user-style question lands on a tag, the entities this query returns are the natural query-expansion candidates. Composes with [[docs-via-shared-entity-coverage]] (use the top entity returned as the next seed) + [[research-by-tag]] (use the same tag to expand the doc list). LIMIT 25.",
        params: &["tag"],
        needs: &["Doc", "Entity"],
    },
    // Per iter 196 (interrogation-039 Finding D — 4th
    // sub-pattern of feedback_tag_axis_loose_end_pattern):
    // sibling research papers absorbed in one Type-R
    // iteration tend to land with paper-SPECIFIC tags but
    // ZERO shared family tag, fragmenting tag-axis retrieval
    // even though they cite each other and form a wikilink-
    // traversable cluster. This pair-axis diagnostic surfaces
    // wikilink-connected research-Doc pairs whose tag
    // intersection contains NO meaningful (non-stopword)
    // tag — pairs the loop should retag with a shared
    // family tag. Companion to singleton-tags (node-axis
    // diagnostic for the same authoring drift).
    SavedQuery {
        name: "research-pairs-without-shared-family-tag",
        description: "Pairs of `research`-tagged Docs connected by a WIKILINK edge whose tag intersection contains NO meaningful (non-stopword) shared tag. Detects the iter-196 4th-sub-pattern of the tag-axis-loose-end family: sibling papers absorbed in one Type-R iter that received paper-specific tags but no shared family tag — tag-axis retrieval fragments them despite the wikilink-cluster being a coherent family. Stopwords (filtered): `research, paper, tool, production, framework, survey, design, system`. Excludes `index`-tagged hub-docs per user-probe-047 Finding B (they have wikilinks to every paper for navigation but no paper-specific tags — STRUCTURAL false positive). Each row is a (doc_a, doc_b) candidate for shared-family-tag addition. Companion to singleton-tags (the node-axis view of the same drift) and established-research-families (the positive view of what to fold sibling-loose-ends INTO).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 196 (interrogation-039 Finding A — node-axis
    // companion to research-pairs-without-shared-family-tag).
    // For each `research`-tagged Doc, count its
    // meaningful (non-stopword) tags and count how many of
    // those tags are ALSO carried by ANY other research
    // Doc — a doc whose all-meaningful-tags are corpus-
    // singletons among the research-* set is FULLY
    // tag-isolated from siblings. Surfaces the same
    // authoring drift from the node side: not just
    // wikilink-paired-but-disjoint (the pair-axis view) but
    // wholly-orphaned-among-the-family-cohort.
    SavedQuery {
        name: "research-docs-with-all-singleton-meaningful-tags",
        description: "Research-tagged Docs where EVERY meaningful (non-stopword) tag appears on NO other research-tagged Doc — fully tag-isolated from the research family cohort. Node-axis companion to research-pairs-without-shared-family-tag. Surfaces the iter-196 4th-sub-pattern from the per-doc view: a research doc with all-singleton tags is invisible to family-tag retrieval queries even when it cites siblings. Stopwords filtered (same set as pair-axis variant). Excludes `index`-tagged hub-docs per user-probe-047 Finding B.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 199 (user-probe-047 Finding C — positive
    // companion to research-pairs-without-shared-family-tag
    // and research-docs-with-all-singleton-meaningful-tags).
    // The two anti-pattern queries surface WHAT IS MISSING;
    // this query surfaces WHAT IS ESTABLISHED. For each
    // non-stopword tag carried by ≥3 `research`-tagged
    // Docs, return the tag + member doc-ids. The agent uses
    // this to decide which established family a loose-end
    // pair should be folded into (e.g. "deepsim + fixr lack
    // a shared tag — should they get `clone-detection` or
    // `bug-fix-mining`?" — established-research-families
    // shows the existing members of each candidate).
    SavedQuery {
        name: "established-research-families",
        description: "Non-stopword tags carried by ≥3 `research`-tagged Docs — the ESTABLISHED family tags an agent can fold sibling-loose-ends into. Positive-axis companion to research-pairs-without-shared-family-tag (anti-pattern) and research-docs-with-all-singleton-meaningful-tags (orphan). Each row: tag + member-count + member doc-ids (capped at 10 per family). Surfaces the corpus's current family taxonomy in 1 call so the agent can pick the right family for any loose-end candidate. Stopwords filtered. Excludes `index` to drop hub-doc noise.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 286 ([[interrogation-061]] Finding B
    // operationalisation): cross-family tag detector.
    // iter-285 surfaced that some tags span MULTIPLE
    // topical families when the underlying concept is
    // shared (e.g., `embedding-model` spans code-
    // embedding-lineage + code-rag + dense-retrieval).
    // This query identifies these cross-family concept
    // tags by computing, per family-tag (≥3 carriers),
    // how many DISTINCT sibling tags appear across its
    // member docs. High sibling_tag_count = cross-
    // family concept. Different from [[established-
    // research-families]] which returns family
    // membership; different from [[tag-cooccurrence]]
    // which returns per-pair counts. This is the
    // SPREAD signal at the family-tag axis.
    SavedQuery {
        name: "cross-family-concept-tags",
        description: "Cross-family concept-tag detector per [[interrogation-061]] Finding B. For each non-stopword tag carried by ≥3 `research`-tagged Docs (a 'family-tag'), counts how many distinct sibling tags appear across its member docs. High sibling_tag_count = CROSS-FAMILY CONCEPT (the tag spans multiple topical families). Example on design corpus: `embedding-model` is expected high since its 9 carriers span code-embedding-lineage + code-rag + dense-retrieval. Sibling to [[established-research-families]] (membership) and [[tag-cooccurrence]] (per-pair counts) at the SPREAD axis. Stopwords filtered (research/paper/tool/production/framework/survey/design/system + substantial-bundle + iter-NNN-*). Returns top 25 tags by sibling_tag_count DESC + tag name.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 286 ([[interrogation-061]] Finding E
    // operationalisation): absorption-depth ranking.
    // iter-285 surfaced the heuristic: family-tag
    // count + cohort-span = how deeply explored the
    // family is. On the design corpus the cohort-span
    // signal is degenerate per [[feedback_uniformly_
    // fresh_corpus_pattern]] (uniform updated dates);
    // this query SUBSTITUTES sibling-tag-spread as the
    // 2nd-axis depth signal: member_count × sibling_tag_count
    // gives a composite "exploration breadth" rank.
    // Composes with [[cross-family-concept-tags]]: that
    // detects cross-family concepts; this RANKS families
    // by combined depth (member-count) + breadth (sibling-
    // spread).
    SavedQuery {
        name: "family-tag-absorption-depth",
        description: "Family-tags ranked by absorption depth per [[interrogation-061]] Finding E. Composite signal: member_count × sibling_tag_count = exploration breadth. Substitutes sibling-tag-spread for the degenerate cohort-span signal on continuously-edited corpora ([[feedback_uniformly_fresh_corpus_pattern]]). Operationalises iter-285's heuristic: 'family-tag count + cohort span = how deeply explored the family is'. Companion to [[cross-family-concept-tags]] (which detects cross-family concepts at the same axis). Stopwords filtered (research/paper/tool/production/framework/survey/design/system + iter-NNN-*). Returns top 25 family-tags by depth_score DESC.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 292 ([[user-probe-071]] Finding D
    // operationalisation): WISDOM-LAYER query. iter-291
    // showed that for a given family-tag, the loop has
    // accumulated META-DOC LESSONS-LEARNED
    // (interrogation-* + user-probe-*) that document
    // what worked + what failed in retrieval / absorption
    // for that family. The agent doesn't see these via
    // research-by-tag (which returns only research-*).
    // This query bridges: given a family-tag, surface
    // the audits that cite (via WIKILINK) any member of
    // the family. Ranks by how many family papers each
    // audit cites — most relevant audits at top.
    SavedQuery {
        name: "lessons-for-research-family",
        description: "Wisdom-layer query: for a given family-tag, returns interrogation-* + user-probe-* meta-docs that WIKILINK to any research paper in the family — the LOOP'S LESSONS-LEARNED for the family. Per [[user-probe-071]] Finding D: the iter-291 cookbook probe surfaced 3 meta-docs (interrogation-024 + interrogation-025 + user-probe-023) as adjacent wisdom layer when BM25 happened to match. This query DELIBERATELY surfaces the meta-doc layer in 1 call for any family-tag. Each row: audit_id + audit_title + cites_count + cites_in_family (member ids cited) + audit_updated. Ranked by cites_count DESC then audit_updated DESC (most-recent + most-relevant first). Compose with [[research-by-tag]] to get research papers + this query to get loop lessons-learned — together the COMPLETE family knowledge package.",
        params: &["tag"],
        needs: &["Doc"],
    },
    // Per iter 292 ([[user-probe-071]] Finding B
    // operationalisation): code-domain-vs-generic
    // family-tag classifier. iter-291 surfaced that
    // BM25 queries with code-specific vocabulary route
    // to CODE-RAG family (jina/voyage), while generic
    // queries route to deployment-model family (BGE/
    // E5/nomic-embed). The agent needs to know WHICH
    // families are code-domain so it can specialize
    // its phrasing accordingly. This query classifies
    // each family-tag by whether its members
    // predominantly carry code-domain markers
    // (code-rag / code-embedding-lineage / ast-paths /
    // ...). Categorical label per family-tag.
    SavedQuery {
        name: "family-tags-by-code-domain",
        description: "Code-domain vs generic family-tag classifier per [[user-probe-071]] Finding B. For each family-tag (≥3 carriers), counts member docs that ALSO carry code-domain markers ('code-rag', 'code-embedding-lineage', 'code-context-engine', 'code-comprehension', 'static-analysis', 'code-indexer-architecture', 'code-similarity', 'code-embedding', 'ast-paths', 'ast-structural-features'). Categorical label: 'code-domain' if >50% of members are code-marked, 'generic' if <20%, 'mixed' otherwise. Agent uses this to specialize phrasing by domain — code-domain families surface via code-vocab BM25 queries (jina/voyage); generic families surface via generic-vocab BM25 queries (BGE/E5/nomic-embed). Composes with [[research-by-tag]] for domain-routed family lookup.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 294 ([[user-probe-072]] Finding D
    // operationalisation): WIKILINK-axis FALLBACK.
    // iter-293 surfaced that [[lessons-for-research-
    // family]] underdelivers on the design corpus
    // (1-2 rows per family) because audit-to-research
    // WIKILINK edges are sparse. The audit body text
    // mentions the family-tag (e.g., 'dense-retrieval'
    // appears in interrogation-060 + 062's titles)
    // but the WIKILINK edge isn't populated densely.
    // This query is the CONTENT-AXIS fallback: find
    // audits whose title or summary CONTAINS the
    // family-tag literal. Works on any corpus where
    // audit titles encode the topic.
    SavedQuery {
        name: "audits-mentioning-family-tag",
        description: "Content-axis FALLBACK to [[lessons-for-research-family]]. Per [[user-probe-072]] Finding D: WIKILINK-axis from audits to research papers is SPARSE on the design corpus (1-2 rows per family). This query bypasses WIKILINK by searching audit title + summary for the family-tag literal. Use when the lessons-for-research-family query returns near-empty results. Composes after the WIKILINK-based query as the 2nd-pass wisdom-layer lookup. Returns interrogation-* + user-probe-* docs whose title OR summary CONTAINS the family-tag, ordered by updated DESC then id. LIMIT 25.",
        params: &["tag"],
        needs: &["Doc"],
    },
    // Per iter 294 (sibling of audits-mentioning-
    // family-tag): iter-cohort-based audit retrieval.
    // The loop's audit-tag convention is `iter-NNN-
    // <topic>` (e.g., 'iter-283-trio-retrievability-
    // validated', 'iter-289-deployment-model'). Given
    // an iter-cohort tag, this query returns all audits
    // carrying it. Lets the agent answer "what did the
    // loop discover in iter-NNN?" in 1 call.
    SavedQuery {
        name: "audits-by-iter-tag",
        description: "Iter-cohort audit retrieval: given an iter-NNN-* tag (e.g., 'iter-283-trio-retrievability-validated'), returns all audits (interrogation-* + user-probe-*) carrying it. Operationalises the audit-tag convention where each iter cohort's discoveries get tagged with its iter-NNN slug. Lets the agent answer 'what did the loop discover in iter-NNN?' in 1 call. Composes with [[audits-mentioning-family-tag]] for content-axis wisdom-layer lookup at the iter-cohort axis.",
        params: &["iter_tag"],
        needs: &["Doc"],
    },
    // Per iter 297 ([[interrogation-063]] Finding B
    // operationalisation): for each family-tag (≥3
    // carriers), counts audits whose title or summary
    // CONTAINS the family-tag literal. iter-296
    // surfaced that audit-count = LOOP-EXPLORATION-
    // DURATION signal — graph-similarity (oldest) at
    // 14 audits vs ir-foundations (recent umbrella)
    // at 4. This is the cheap 1-axis primitive
    // delivering that signal directly.
    SavedQuery {
        name: "family-tag-audit-counts-summary",
        description: "For each family-tag (≥3 carriers), counts audits mentioning the tag via title/summary CONTAINS. Operationalises iter-296 [[interrogation-063]] Finding B: audit-count tracks LOOP-EXPLORATION-DURATION, not umbrella-status. Use as a cheap 1-axis cold-start signal of which families the loop has spent the most iters revisiting. Stopwords filtered same as [[established-research-families]] (research/paper/tool/production/framework/survey/design/system/ingest-pass-storyline). Returns top 25 families by audit_count DESC; families with 0 audits don't surface (effectively no exploration yet). Compose with [[family-tag-absorption-depth]] for the 2-axis maturity view in 2 calls — or use the iter-297 1-call composite [[family-maturity-2d]].",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 297 ([[interrogation-063]] Finding C
    // operationalisation): the 2-AXIS family-maturity
    // model. iter-296 surfaced that depth_score (iter-
    // 286 concept breadth) and audit-count (iter-296
    // exploration history) are COMPLEMENTARY axes
    // characterising family maturity. This query
    // unifies them into a 1-call ranking with
    // categorical maturity labels:
    //   high depth + high audits → 'deeply-explored-mature'
    //   high depth + low audits → 'recently-absorbed-umbrella'
    //   low depth + high audits → 'narrow-but-explored'
    //   low depth + low audits → 'narrow-recent'
    // Thresholds: depth_score >= 100 and audit_count
    // >= 7, empirically chosen per iter-296 data.
    SavedQuery {
        name: "family-maturity-2d",
        description: "2-axis family maturity ranking per iter-296 [[interrogation-063]] Finding C. Combines [[family-tag-absorption-depth]] (concept-breadth axis: member_count × sibling_tag_count) with iter-297 [[family-tag-audit-counts-summary]] (exploration-history axis: audit_count via content-axis). Returns family_tag + 4 metrics + categorical maturity_label. Thresholds (empirical from iter-296): depth_score ≥ 100 = HIGH BREADTH; audit_count ≥ 7 = HIGH EXPLORATION. The 4 cells map to: 'deeply-explored-mature' (graph-similarity 54+14 — wait, depth 54 BELOW 100, so actually 'narrow-but-explored'; code-rag 90+9 same band), 'recently-absorbed-umbrella' (ir-foundations 240+4, embedding-model 180+7), and 'narrow-recent' for the rest. Use this 1-call for cold-start family-maturity orientation. Stopwords filtered consistently.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 303 ([[user-probe-075]] Finding B
    // operationalisation): connectivity-axis ranking
    // of Docs by their wikilink-edge count. iter-302
    // surfaced that feature-anthropic-judge-backend
    // has 15 wikilink connections (4 inbound + 11
    // outbound) — far more than any research doc's
    // wisdom layer. NEW HEURISTIC: feature docs (and
    // possibly other hub-doc roles) are LOAD-BEARING
    // architectural cores. This query ranks all Docs
    // by total WIKILINK connectivity (undirected edge
    // count), surfacing the corpus's hub structure
    // in 1 call. Compose with [[node-context-doc]]
    // for deep-dive on each hub.
    SavedQuery {
        name: "doc-connectivity-ranking",
        description: "All Docs ranked by total WIKILINK connectivity (undirected — sum of inbound + outbound edges). Operationalises iter-302 [[user-probe-075]] Finding B: feature docs are LOAD-BEARING hubs (Judge backend has 15 connections); architectural cores have far more wikilinks than research docs (typically 0-5). This 1-call diagnostic surfaces the corpus's HUB structure across all roles. Returns id + title + role + total_connections, threshold ≥ 3 to filter leaf nodes. Composes with [[feature-doc-connectivity]] (iter-303 sibling, feature-only variant) and [[node-context-doc]] (deep-dive on a chosen hub).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 303 (sibling to doc-connectivity-
    // ranking): role-filtered variant for feature-*
    // docs only. iter-302 surfaced that doc-linter
    // has ONLY 3 features (Judge backend / external-
    // corpus-probe / region-granularity) but each is
    // a heavily-wikilinked architectural core. This
    // narrow variant lets the agent enumerate THE
    // SMALL FEATURE SURFACE and see each one's
    // connectivity in 1 call.
    SavedQuery {
        name: "feature-doc-connectivity",
        description: "Feature-role Docs ranked by total WIKILINK connectivity. Per [[user-probe-075]] Finding F: doc-linter's design surface is SMALL (3 features) but each is heavily-wikilinked. This query enumerates the feature surface + per-feature connectivity in 1 call — the agent's entry point for engineering-task context. Companion to [[doc-connectivity-ranking]] at the role-filtered axis. Use as the FIRST call when answering 'what does doc-linter ship?' or 'where would I add X?' style engineering questions. Returns id + title + total_connections, no threshold (every feature surfaces).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 305 ([[user-probe-076]] Finding D
    // operationalisation): the 4-TIER HUB TAXONOMY
    // iter-304 surfaced as an empirical
    // characterisation of corpus shape. Extends
    // iter-303 [[doc-connectivity-ranking]] with a
    // categorical maturity label per tier.
    // Thresholds (empirical from iter-304 design-
    // corpus data):
    //   tier-1-architectural-narrative: ≥ 40 conns
    //     (research-index 80, technical-design-v1
    //     66, design docs, entity-retrieval-
    //     primitive — the corpus's load-bearing
    //     narrative).
    //   tier-2-substantial: 25-39 conns (gaps +
    //     audit-runs + research peaks).
    //   tier-3-active-participation: 15-24 conns
    //     (research + interrogations + user-probes
    //     long tail).
    //   tier-4-leaf-or-feature: 3-14 conns (3
    //     features + most research + ontology-
    //     values).
    SavedQuery {
        name: "doc-hub-tier-classifier",
        description: "Docs ranked by WIKILINK connectivity + categorical TIER LABEL per iter-304 [[user-probe-076]] Finding D 4-tier hub taxonomy. Extends [[doc-connectivity-ranking]] with maturity buckets: tier-1-architectural-narrative (≥40 conns: research-index + design docs + entity-retrieval-primitive — load-bearing); tier-2-substantial (25-39: gaps + audits + research peaks); tier-3-active-participation (15-24: research + interrogations + user-probes); tier-4-leaf-or-feature (3-14: features + leaves). Use as a cold-start corpus-shape diagnostic — agent sees both the ranking AND which tier each doc occupies. Threshold ≥ 3 to surface; LIMIT 25.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 305 (sibling — tier-1 cold-start
    // reading list): just the tier-1 architectural-
    // narrative hubs. iter-304 surfaced that the
    // corpus's load-bearing layer is the 8-doc tier
    // (research-index + 5 design docs + entity-
    // retrieval-primitive + audit-run-004). An
    // agent landing on the corpus and asking 'what
    // are the architectural foundations to read
    // first?' gets these in 1 call. Compose with
    // [[node-context-doc]] for deep-dive on each.
    SavedQuery {
        name: "tier1-architectural-hubs",
        description: "Tier-1 architectural-narrative hubs only (≥40 WIKILINK connections) per iter-304 [[user-probe-076]] Finding D. The corpus's load-bearing reading list: research-index + design docs + entity-retrieval-primitive + audit-run-004 etc. on the design corpus. Returns id + title + role + total_connections, ordered by connectivity DESC. Agent's 1-call cold-start question 'what's the architectural foundation to read first?' Compose with [[node-context-doc]] for deep-dive on each tier-1 hub.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 312 ([[feedback_cross_axis_bridge_pattern]]
    // operationalisation, 3-instance robust): given a
    // research family-tag, surface research docs in that
    // family that WIKILINK to shipped feature-* docs.
    // iter-301 (rag-eval → Judge), iter-310 (vector-index →
    // embedding-model), iter-311 (vector-index → Cargo-
    // features) — bridges from absorbed research to
    // shipped infrastructure are LOAD-BEARING. This query
    // surfaces those bridges per family in 1 call.
    SavedQuery {
        name: "family-to-feature-bridges",
        description: "For given family-tag, lists research-tagged Docs in the family + their WIKILINK targets among feature-* role Docs. Operationalises [[feedback_cross_axis_bridge_pattern]] (3-instance robust): bridges from absorbed research to shipped infrastructure are load-bearing. Returns (research_id, research_title, feature_id, feature_title) pairs. Empty result means: family has no explicit wikilink bridges to features (bridge may still exist via BM25 body-vocab discoverability — see [[feedback_cross_axis_bridge_pattern]]). Use during 'add feature X' cookbook: research-by-tag + family-to-feature-bridges + query_doc(feature-existing) for full mirror-the-pattern context.",
        params: &["tag"],
        needs: &["Doc"],
    },
    // Per iter 312 (sibling — feature-side view of the
    // bridge): for each feature-* doc, count incoming
    // wikilinks from research-tagged Docs. Reveals which
    // shipped features are MOST REFERENCED by absorbed
    // research — the canonical bridges of the corpus.
    SavedQuery {
        name: "feature-incoming-from-research",
        description: "For each feature-* role Doc, counts incoming WIKILINKs from research-tagged Docs. Feature-side view of [[family-to-feature-bridges]]. Reveals which shipped features the loop's absorbed research most-often references — the corpus's canonical bridges. On design corpus expect feature-anthropic-judge-backend at top (research-rankzephyr from iter-306 + cross-axis bridge pattern). 0-incoming-research features are leaf-infrastructure not yet referenced. Use as a 1-call diagnostic of bridge-status across the feature surface.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 317 ([[user-probe-080]] PIPELINE
    // COMPLETENESS MILESTONE operationalisation):
    // 5-stage retrieval pipeline cookbook in 1 call.
    // iter-316 surfaced that the loop's 9-month
    // absorption produced complete coverage of the
    // canonical retrieval-pipeline-design textbook
    // across 5 family-tags (llm-era-query-rewriting
    // / chunking / deployment-model / reranking /
    // rag-evaluation). This query stage-labels each
    // member doc so the agent sees the pipeline
    // structure in 1 call.
    SavedQuery {
        name: "retrieval-pipeline-cookbook",
        description: "1-call retrieval-pipeline cookbook per iter-316 [[user-probe-080]] PIPELINE COMPLETENESS MILESTONE. Returns research papers across the 5 retrieval-pipeline stages with a stage_label: 1-query-rewriting / 2-chunking / 3-embedding / 4-reranking / 5-evaluation. Each stage has the canonical 3-paper trio per iter-316 SYMMETRIC architecture. Use as the cold-start textbook for an agent answering 'design my retrieval pipeline'. Composes with [[research-by-tag]] for per-stage deep-dive. Returns 15 papers in stage order.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 317 (sibling — pipeline coverage
    // diagnostic): for each pipeline stage, counts
    // members + emits categorical coverage_label.
    // iter-316 SYMMETRIC ARCHITECTURE noted each
    // stage at exactly 3 members. This query
    // diagnoses whether a corpus matches that
    // architecture or has gaps.
    SavedQuery {
        name: "pipeline-stage-coverage",
        description: "Per pipeline stage, counts research members + emits coverage_label ('trio-coverage' if ≥3 / 'partial-coverage' if 1-2 / 'gap' if 0). Operationalises iter-316 [[user-probe-080]] Finding B SYMMETRIC ARCHITECTURE observation: design corpus has trio-coverage at all 5 stages. Use as a 1-call corpus-shape diagnostic: 'does this corpus have complete retrieval-pipeline coverage?'. Empty result rows for stages with 0 members are NOT emitted by this aggregate; use [[retrieval-pipeline-cookbook]] for member-level enumeration.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 319 (extends iter-317 cookbook
    // pattern to graph-similarity family per
    // [[user-probe-081]] Finding G — cookbook
    // complexity 4→5→1 calls convergence; 2nd
    // mature topic gets its own 1-call cookbook):
    // graph-similarity is the 2nd most-developed
    // family after the retrieval pipeline (6
    // members per iter-285 + iter-298 family
    // maturity portrait). Pattern-transfer is
    // doc-linter's stated goal per
    // [[doc-design-pattern-transfer-goal]] —
    // this cookbook answers 'what graph-similarity
    // approaches exist for matching code against
    // trusted-repo patterns?'.
    SavedQuery {
        name: "graph-similarity-cookbook",
        description: "1-call graph-similarity research cookbook per iter-319 [[user-probe-081]] Finding G cookbook-convergence pattern. Returns research members in the graph-similarity family with a subfamily_label categorising design approach: 'graph-kernel' / 'graph-embedding' / 'neural-graph-matching' / 'cross-attention' / 'bipartite-matching' / 'other'. Use as the cold-start textbook for an agent answering 'what graph-similarity approaches exist for matching code against trusted-repo patterns?' (per [[doc-design-pattern-transfer-goal]]). Mirrors [[retrieval-pipeline-cookbook]] structure on a different mature topic.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 319 (sibling): per-subfamily member-
    // count + coverage_label diagnostic for graph-
    // similarity. Lets agent see which design
    // approaches are well-covered vs gaps.
    SavedQuery {
        name: "graph-similarity-subfamily-coverage",
        description: "Per-subfamily member-count + coverage_label for the graph-similarity family. Sibling to [[graph-similarity-cookbook]]; mirrors iter-317 [[pipeline-stage-coverage]] structure. Returns subfamily_label + member_count + coverage_label ('rich' if ≥2 / 'singleton' if 1 / 'gap' if 0). Empty rows for 0-member subfamilies are NOT emitted by this aggregate. Use as a 1-call diagnostic of which graph-similarity DESIGN APPROACHES the corpus covers.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 321 (3rd-mature-topic cookbook
    // extending iter-317 + iter-319 pattern; tests
    // iter-320 Finding F deep-vs-broad heuristic at
    // 2nd instance): query-expansion family has 7
    // members spanning Carpineto's 3 AQE sub-classes
    // (local Rocchio / global LSI / external Voorhees)
    // + 3-paper LLM-era trio (HyDE / Query2doc /
    // Step-back) + 1 survey (Carpineto 2012). MIXED
    // sub-structure: 4 categories with 1 paper each
    // (BROAD) + 1 category (llm-era) with 3 papers
    // (DEEP). The cookbook will reveal whether iter-
    // 320's deep-vs-broad framing holds OR if a 3rd
    // 'mixed-mature' shape emerges.
    SavedQuery {
        name: "query-expansion-cookbook",
        description: "1-call query-expansion research cookbook per iter-321 (3rd-mature-topic extending iter-317 + iter-319 pattern). Returns query-expansion family members with subfamily_label categorising Carpineto's AQE taxonomy: 'aqe-local' (Rocchio relevance feedback) / 'aqe-global' (LSI latent semantic) / 'aqe-external' (Voorhees WordNet) / 'llm-era' (HyDE / Query2doc / Step-back) / 'vocabulary-mismatch-survey' (Carpineto 2012) / 'other'. Tests iter-320 [[user-probe-082]] Finding F deep-vs-broad mature-shape heuristic at 2nd-instance. Use as cold-start textbook for 'what query-expansion techniques exist?' agent question.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 321 (sibling): per-subfamily count +
    // coverage_label for query-expansion family.
    // Mirrors iter-319 graph-similarity-subfamily-
    // coverage structure. Per iter-320 Finding F:
    // 'rich' coverage if ≥2 / 'singleton' if 1.
    // Expected: 1 rich (llm-era at 3) + 4 singletons
    // (aqe-local / aqe-global / aqe-external /
    // vocabulary-mismatch-survey).
    SavedQuery {
        name: "query-expansion-subfamily-coverage",
        description: "Per-subfamily member-count + coverage_label for query-expansion family per iter-321. Tests iter-320 [[user-probe-082]] Finding F deep-vs-broad heuristic — expected mixed-mature shape (4 singletons + 1 rich at llm-era trio). Sibling to [[query-expansion-cookbook]] mirroring [[pipeline-stage-coverage]] and [[graph-similarity-subfamily-coverage]] structure. Returns subfamily_label + member_count + coverage_label ('rich' if ≥2 / 'singleton' if 1 / 'gap' if 0; gap rows NOT emitted).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 323 (user-probe-083 Finding D +
    // feedback_cross_axis_bridge_pattern lineage): the
    // 3-shape framework (DEEP / BROAD / MIXED) had been
    // shipped 3 times as per-topic CASE-WHEN cookbooks
    // (iter-317 pipeline-stage-coverage / iter-319
    // graph-similarity-subfamily-coverage / iter-321
    // query-expansion-subfamily-coverage). This query
    // is the GENERIC version — parameterized by
    // $family_tag, classifies ANY family-tag's co-tag
    // distribution without per-topic CASE-WHEN
    // authoring. Cookbook-convergence at its limit:
    // 4→5→1 calls per-topic → 0-per-topic generic.
    SavedQuery {
        name: "mature-topic-shape-distribution",
        description: "Generic per-family co-tag distribution for ANY $family_tag (replaces the iter-317/iter-319/iter-321 per-topic CASE-WHEN siblings). For research Docs with $family_tag, emits each OTHER tag's member_count + coverage_label ('rich' if ≥3 / 'couple' if 2 / 'singleton' if 1). Filters meta-tags ('research', 'paper', short tags <6 chars, '*storyline*'). The 0-per-topic-authoring endpoint of the cookbook-convergence pattern: pipeline-cookbook + graph-similarity-cookbook + query-expansion-cookbook can each be replaced by 1 call to this saved query. Sibling: [[mature-topic-shape-census]] for the no-param corpus-wide version.",
        params: &["family_tag"],
        needs: &["Doc"],
    },
    // Per iter 323 (sibling to mature-topic-shape-
    // distribution): the CORPUS-WIDE no-param census.
    // For every family-tag appearing on ≥5 research
    // Docs, computes the SHAPE_LABEL (DEEP / BROAD /
    // MIXED / emergent) using the same coverage-tier
    // heuristic. One MCP call → entire corpus shape
    // census. The 0-authoring endpoint: an agent
    // discovers which mature topics the corpus
    // contains AND their shape in one go, without
    // knowing the family-tag names in advance.
    SavedQuery {
        name: "mature-topic-shape-census",
        description: "Corpus-wide mature-topic shape census per iter-323 user-probe-083 3-shape framework. For every family-tag appearing on ≥5 research Docs, emits total_members + rich_subfamilies (co-tag count ≥3) + couple_subfamilies (count=2) + singleton_cotags (count=1) + shape_label (DEEP-mature if rich≥2 + singleton≤2; BROAD-mature if rich=0 + couple≤1 + singleton≥4; MIXED-mature if rich≥1 + singleton≥2; else emergent). Sibling: [[mature-topic-shape-distribution]] for the per-family parameterized version. Cold-start endpoint: an agent gets the entire corpus's mature-topic shape census in 1 call, without prior knowledge of family-tag names.",
        params: &[],
        needs: &["Doc"],
    },
    SavedQuery {
        name: "cross-family-bridge-papers",
        description: "Research Docs at the intersection of 2 family tags ($family_a + $family_b). Operationalises the cross-family-bridge pattern per iter-325 interrogation-068 Finding C: bug-fix-mining ∩ pattern-mining returned Coming + Fixr + HAGGIS via 4 separate cypher calls; this query returns the same in 1 call. Sibling to [[cross-family-bridge-density]] (the no-param corpus-wide version). Use case: agent investigating 'which papers span topics X and Y?' for cross-axis-bridge-pattern reasoning per [[feedback_cross_axis_bridge_pattern]].",
        params: &["family_a", "family_b"],
        needs: &["Doc"],
    },
    // Per iter 326 (sibling to cross-family-bridge-
    // papers): corpus-wide bridge density. For every
    // pair of family-tags sharing ≥3 research Docs,
    // emit the pair + bridge_count. The 1-call cold-
    // start primitive for cross-family-bridge
    // landscape analysis. iter-324 user-probe-084
    // Finding F (ir-foundations at 27/27) predicts
    // this query surfaces ir-foundations bridging
    // to many other families at bridge_count ≥3.
    SavedQuery {
        name: "cross-family-bridge-density",
        description: "Corpus-wide cross-family-bridge density per iter-326 sibling to cross-family-bridge-papers. For every pair of family-tags sharing ≥3 research Docs, emits (family_a, family_b, bridge_count). Filters short tags (size<6) + 'research'/'paper' meta-tags + '*storyline*'. Ordered by bridge_count DESC. 1-call corpus-wide bridge landscape per [[feedback_cross_axis_bridge_pattern]]. The agent reading this sees umbrella-bridges (e.g., ir-foundations bridging to 8+ families per iter-324 user-probe-084) + topical-bridges (e.g., bug-fix-mining ∩ pattern-mining per iter-325 interrogation-068) in 1 call.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 328 (user-probe-085 Finding F + iter-327
    // feedback_clone_vs_edit_recipe_decomposition + the
    // 2-instance feedback_add_feature_x_agent_recipe):
    // identifies the EDIT-target file set for "add
    // feature X" agent recipes — files whose basename
    // matches a canonical layer-noun (models / crud /
    // schema / main / router / index / layout /
    // __init__). These are the 1-per-layer SHARED
    // FILES whose content must be APPENDED to (vs
    // 1-per-resource files which are CLONED). On
    // trusted FastAPI surfaces models.py + crud.py +
    // main.py + 4 __init__.py + frontend index/layout
    // files.
    SavedQuery {
        name: "shared-layer-source-files",
        description: "Canonical layer-shared source files (the EDIT-target set for 'add feature X' agent recipes per iter-327 [[user-probe-085]] Finding F + [[feedback_clone_vs_edit_recipe_decomposition]]). Filters by basename: models.py / crud.py / schema.py / main.py / router.py / routes.py / index.{ts,tsx} / layout.tsx / __init__.py. Returns path + language + loc + layer_role (models-shared / crud-shared / schema-shared / app-entry / router-aggregator / frontend-index / frontend-layout / python-package-init). Excludes test files + generated code. Pairs with [[resource-file-cluster]] (the CLONE-target sibling).",
        params: &[],
        needs: &["File"],
    },
    // Per iter 328 (sibling to shared-layer-source-
    // files): given a resource-stem substring (e.g.,
    // 'items' or 'users'), return the per-resource
    // file CLUSTER (the CLONE-target set) for "add
    // feature X" agent recipes. Classifies each file
    // by layer (route / test / frontend-component /
    // frontend-spec / frontend-tsx / backend-py).
    SavedQuery {
        name: "resource-file-cluster",
        description: "Per-resource file cluster for the CLONE-target set in 'add feature X' agent recipes per iter-327 [[user-probe-085]] Finding A + [[feedback_add_feature_x_agent_recipe]] 2-instance memory. Given $resource_stem (e.g., 'items'), returns all files whose path CONTAINS that substring + classified layer (test / route / backend-api / frontend-component / frontend-spec / frontend-tsx / frontend-ts / backend-py / other). On trusted FastAPI with stem='items' returns 4 files: items.py route + test_items.py + items.tsx + items.spec.ts. Sibling: [[shared-layer-source-files]] (the EDIT-target set).",
        params: &["resource_stem"],
        needs: &["File"],
    },
    SavedQuery {
        name: "resource-shape-classifier",
        description: "Classifies a resource's CLONE-target shape per iter-329 [[interrogation-069]] Finding E + iter-328 [[resource-file-cluster]] sibling. Given $resource_stem, emits backend_src + backend_test + frontend_src + frontend_test + shape_label (symmetric / backend-only / frontend-only / empty). On trusted FastAPI: items → symmetric (1/1/1/1); users → backend-only (1/1/0/0); admin → frontend-only (0/0/1/1). Use case: agent picks the matching clone sub-pattern from [[feedback_clone_vs_edit_recipe_decomposition]]'s 3-sub-shape table. Filters /__init__ + .gen.* + /test/ etc. — counts only canonical source + test files.",
        params: &["resource_stem"],
        needs: &["File"],
    },
    // Per iter 330 (sibling to resource-shape-
    // classifier): enumerates non-scaffold backend +
    // frontend route files. The cold-start primitive
    // for agent resource discovery in "add feature X"
    // recipes — agent calls this to list resources,
    // then calls resource-shape-classifier per stem.
    // Filters scaffold files: _layout.tsx / __root.tsx
    // / __init__.py / /test/ + /tests/.
    SavedQuery {
        name: "app-route-files",
        description: "Non-scaffold backend + frontend route files per iter-330. Cold-start primitive for agent resource discovery in 'add feature X' recipes per [[feedback_add_feature_x_agent_recipe]]. Filters out scaffold/system files (_layout.tsx, __root.tsx, __init__.py, /test/, /tests/). Returns path + loc + layer (backend-route / frontend-route). On trusted FastAPI returns ~13 rows: 5 backend routes (items / login / private / users / utils) + 8 frontend routes (admin / index / items / settings / login / signup / recover-password / reset-password). Sibling to [[resource-shape-classifier]] (per-resource shape detector).",
        params: &[],
        needs: &["File"],
    },
    // Per iter 332 (operationalises iter-326 cross-
    // family-bridge-density at per-family axis):
    // given $family_tag, returns OTHER family-tags it
    // bridges to with bridge_count. Replaces the
    // 2-call (mature-topic-shape-distribution +
    // cross-family-bridge-density-then-filter) probe
    // with 1 call.
    SavedQuery {
        name: "family-top-bridges",
        description: "Given $family_tag, returns OTHER family-tags it bridges to (research Docs in intersection) with bridge_count. Operationalises iter-326 [[cross-family-bridge-density]] at per-family axis per [[feedback_cross_axis_bridge_pattern]]. On design corpus with family_tag='query-expansion' returns 2 rows: (ir-foundations, 7) + (llm-era-query-rewriting, 3) — confirms cross-axis bridge pattern. Use case: agent asks 'what other research families does FAMILY_X span?' Sibling to [[family-rich-cotags]] (the archetype-cluster query). Threshold: bridge_count >= 2.",
        params: &["family_tag"],
        needs: &["Doc"],
    },
    // Per iter 332 (operationalises iter-325 [[interrogation-068]]
    // Finding B canonical-archetype interpretation):
    // given $family_tag, returns ONLY the rich-cluster
    // co-tags (member_count >= 3). For INTROSPECTIVE
    // families (workflow-companion etc.), the rich-
    // cluster intersection NAMES the canonical archetype.
    SavedQuery {
        name: "family-rich-cotags",
        description: "Given $family_tag, returns only the RICH (member_count >= 3) co-tags from the family's distribution per iter-325 [[interrogation-068]] Finding B + [[feedback_introspective_vs_topical_family_shape]]. For INTROSPECTIVE families (like workflow-companion), the rich-cluster intersection names the CANONICAL ARCHETYPE. On design corpus with family_tag='workflow-companion' returns 3 rows: production=4 + agent-shell=3 + code-context-engine=3 — EXACTLY iter-325 Finding B's canonical-archetype tags. Sibling to [[family-top-bridges]] (the cross-family axis). Filters meta-tags + short tags + storyline-* like other family queries.",
        params: &["family_tag"],
        needs: &["Doc"],
    },
    // Per iter 153 (user-probe-032 Finding D +
    // feedback_generated_code_visibility memory):
    // doc-linter's File table exposes auto-generated code
    // alongside hand-written source without semantic
    // distinction. This query filters by common
    // generated-code conventions (.gen.* extension,
    // /generated/ or __generated__/ path segments, .pb.*
    // for protobuf) so the agent can DISTINGUISH
    // regenerate-after-changes targets from edit-source
    // targets. Sibling: hand-written-source-files.
    SavedQuery {
        name: "generated-files",
        description: "Files matching auto-generated code conventions (.gen.* / /generated/ / .pb.* / __generated__/). The 'regenerate after backend changes — do not edit manually' set. Composes with files-by-path-substring($pattern) for filtering: an agent recommending file edits should EXCLUDE these from the to-edit list. Closes the source-vs-generated visibility gap surfaced in user-probe-032.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 153 (sibling — the COMPLEMENT for
    // edit-source-files): files that are NOT auto-
    // generated AND have a language set (i.e., are
    // source-language files the file scanner ingested).
    // The 'to-edit manually' set. Composes with
    // generated-files for the source/generated dichotomy.
    SavedQuery {
        name: "hand-written-source-files",
        description: "Source-language files that are NOT auto-generated — the 'to-edit manually' set. Filters by language IS NOT NULL AND excludes the four generated-code path conventions. Capped at 200 rows; use files-by-path-substring($pattern) for narrower scoping. Composes with generated-files (the complement).",
        params: &[],
        needs: &["File"],
    },
    // Per iter 173 (user-probe-039 Finding B —
    // dependency-analysis workflow IMPORTS-edge fallback):
    // when the IMPORTS edge table is empty for a corpus
    // (Py+TS per gap-imports-edge-py-ts-empty), the
    // architectural-layer view via path-CONTAINS is the
    // fallback. Hardcodes 8 common conventions (routes,
    // components, client, api, core, crud, migrations,
    // scripts) + fallback `other`. Returns count + total
    // LOC per layer so the agent gets the architectural
    // weighting at a glance.
    SavedQuery {
        name: "architectural-layers-by-path",
        description: "Files grouped by architectural-layer-by-path-substring (routes / components / client / api / core / crud / migrations / scripts / other), with count + total LOC. Fallback when the IMPORTS edge table is empty (per gap-imports-edge-py-ts-empty surfaced by user-probe-039). On the trusted corpus surfaces frontend-component (51/5153) as the dominant layer, backend-core (4/188), etc. Excludes test + generated files. Composes with files-coupled-to for per-layer blast-radius analysis.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 173 (user-probe-039 Finding A — 3rd
    // corpus-empty-primitive instance: IMPORTS edge table
    // empty on Py+TS): extends the iter-159 corpus-health-
    // diagnostic trio with the EDGE-axis primitive for
    // IMPORTS. The iter-159 trio (function-count + function-
    // mentions-by-language) covers Function/Type axes;
    // this query covers the IMPORTS edge so an agent can
    // tell "import-fan-out / dead-files / impact-analysis
    // will all be empty for THIS language" before calling
    // them.
    SavedQuery {
        name: "imports-edge-count-by-language",
        description: "IMPORTS edges grouped by the source File's language — diagnostic for `import-fan-out / dead-files / query_impact return empty` detection. When count is 0 for a language, every IMPORTS-edge-walking saved query AND the query_impact MCP tool will be empty for that language (per gap-imports-edge-py-ts-empty surfaced by user-probe-039). Sibling of function-count-by-language + function-mentions-by-language (iter 159) — together they form the corpus-health-diagnostic edge-axis trio.",
        params: &[],
        needs: &["File"],
    },
    SavedQuery {
        name: "pytest-prod-test-pairs",
        description: "Production-file ↔ pytest-test-file pairs via the `test_X.py` convention. Strips the `test_` prefix to recover the pair the basename-only match misses (per user-probe-038 Finding B: regexp_extract basename catches only 2/5 substantive pairs on the trusted corpus's pytest suite). Returns prod path + test path + each LOC. Sibling to same-name-cross-dir-clones (which handles `.spec.ts ↔ .tsx`-style same-name conventions) and a future spec-prod-test-pairs (deferred — Kuzu has no `replace` function so the .spec.ts stripping needs a different idiom).",
        params: &[],
        needs: &["File"],
    },
    // Per iter 170 (user-probe-038 gap-side — the
    // complement of pytest-prod-test-pairs): production
    // Python files ≥ 20 LOC that have NO test sibling via
    // the `test_X.py` convention. The OPTIONAL MATCH +
    // WHERE NULL anti-join idiom surfaces the gap rows.
    // 20-LOC floor skips trivial files (__init__.py,
    // minimal entry-points). On the trusted corpus surfaces
    // backend/app/models.py (129), backend/app/utils.py
    // (123), backend/app/core/config.py (119), etc.
    SavedQuery {
        name: "python-untested-files",
        description: "Production Python files ≥ 20 LOC that have NO test sibling via the pytest `test_X.py` convention — the GAP-side complement of pytest-prod-test-pairs. Excludes tests/, __pycache__/, and trivial files. OPTIONAL MATCH + WHERE NULL anti-join idiom. Caveat: doesn't catch test files that match via different conventions (e.g., the basename-equivalence case the iter-161 same-name-cross-dir-clones query handles). Compose with the existing iter-161 query to refine.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 187 (codifying the 4-regime taxonomy
    // surfaced by user-probes 037/042/043/044): the loop has
    // documented 4 distinct corpus-shape regimes — uniformly-
    // fresh (the loop's design corpus, ≤3-day span across
    // ≥50 narrative docs); doc-sparse (doc-linter source, <10
    // narrative); starter-ontology-noise (trusted corpus, mix
    // of ontology + narrative); doc-empty-ontology-only
    // (spacy + example-app, 0 narrative). The agent can
    // classify the current corpus in 1 call.
    SavedQuery {
        name: "corpus-shape-classifier",
        description: "Categorical corpus-regime classification in 1 call. Returns one of: 'doc-empty-ontology-only' (0 narrative docs; user-probes 043/044 spacy/example-app pattern), 'doc-sparse' (<10 narrative; user-probe-037 doc-linter source pattern), 'uniformly-fresh' (≥50 narrative + ≤3 day span; user-probe-037 design corpus pattern), 'mixed' (everything else). Use at cold-start to pick the right workflow (freshness probes on uniformly-fresh, structural cypher on doc-empty, etc.). Sibling to corpus-update-span (the timestamp-axis primitive iter 168 shipped).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 187 (sibling to corpus-shape-classifier): the
    // narrative-doc count is the discriminating signal at the
    // regime boundary; surfacing it directly makes it
    // composable with the existing iter-168 corpus-update-
    // span query (their JOIN tells the full uniformly-fresh
    // story: narrative-doc-count ≥ 50 AND span ≤ 3 days).
    SavedQuery {
        name: "narrative-doc-count",
        description: "Total Doc rows excluding ontology-* roles — the narrative-doc count. Sibling of corpus-shape-classifier + corpus-update-span: the 3 together let the agent classify the corpus regime + freshness in 3 1-call probes. Excludes ontology-value / ontology-axis / ontology-entity per feedback_starter_ontology_false_positives.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 203 (interrogation-040 Finding A — 3-state
    // ingest taxonomy across MCP corpora); extended iter 205
    // (user-probe-049 Finding D) to expose 2 additional
    // sub-pass-presence counters (coupled_with_count +
    // endpoint_count). The categorical `full-scip` label
    // collapses two distinct states per user-probe-049:
    // doc-linter (full-scip WITH coupling) vs spacy/example-app
    // (full-scip WITHOUT coupling). Surfacing both raw counts
    // alongside the label lets agents see per-pass presence
    // without expanding the categorical label space — per
    // [[feedback_ingest_pass_partial_pattern]] 4-pass
    // independence model.
    SavedQuery {
        name: "corpus-ingest-state-classifier",
        description: "Per-pass ingest-state diagnostic + categorical label in 1 call. Returns fn_count + file_count + doc_count + coupled_with_count + endpoint_count alongside ingest_state (one of: `full-scip` / `file-only-no-scip` / `doc-only-by-design` / `empty`). Per iter 205, the raw counts let agents distinguish full-scip-WITH-coupling (doc-linter) from full-scip-NO-coupling (spacy/example-app per user-probe-049) without expanding the label space. Per [[feedback_ingest_pass_partial_pattern]]: any of 4 ingest sub-passes can be missing independently; the counts surface that directly. Sibling of iter-187 corpus-shape-classifier (Doc-axis); together a 4×N routing matrix for cold-start workflow selection.",
        params: &[],
        needs: &[],
    },
    // Per iter 205 (user-probe-049 Finding D — row-per-pass
    // companion view to the extended corpus-ingest-state-
    // classifier). Same data, different shape: one row per
    // ingest sub-pass with name + populated flag + row count.
    // Lets agents iterate per-pass without parsing 5 columns
    // out of a single row, and surfaces the 4-pass model
    // from [[feedback_ingest_pass_partial_pattern]] as the
    // PRIMARY structure of the result rather than a derived
    // categorical label.
    SavedQuery {
        name: "ingest-pass-status",
        description: "Row-per-pass status table for the 4-sub-pass ingest pipeline. Each row: pass_name (file_walker / scip / git_coupling / endpoint_extract) + row_count + populated boolean. Surfaces the [[feedback_ingest_pass_partial_pattern]] model directly so the agent can see which sub-passes are missing on the current corpus. Companion to the iter-205-extended corpus-ingest-state-classifier (same data, condensed 1-row view). Use this when you want per-pass iteration; use the classifier when you want the categorical state label.",
        params: &[],
        needs: &[],
    },
    // Per iter 203 (interrogation-040 Finding D — tight-vs-
    // loose-cluster distinction on the feature-evolution-
    // coupling-hubs axis): same neighbor_count carries
    // different action guidance depending on avg_jaccard.
    // High (≥0.7): tight cluster, touching the hub cascades.
    // Low (<0.5): loose orbit, hub is extension point, no
    // cascade. Mid: mixed. Companion to iter-201's
    // feature-evolution-coupling-hubs (which exposes the raw
    // metric without category labels).
    SavedQuery {
        name: "tight-vs-loose-coupling-hubs",
        description: "Files coupled with ≥2 OTHER files at commits ≥5 + categorical cluster-type label per file. Categories: `tight-cluster` (avg_jaccard ≥0.7 — touching cascades), `loose-orbit` (avg_jaccard <0.5 — extension point, no cascade), `mixed` (in between). Companion to iter-201 feature-evolution-coupling-hubs; this surfaces the same raw rows + adds the action-guidance category. Use when an agent needs to predict 'should I expect cascading edits if I touch X?' rather than just 'how many neighbors does X have?'.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 211 (user-probe-050 Finding E — composite
    // cold-start probe): a single saved query emitting all
    // 3 categorical workflow-routing labels (corpus-shape /
    // ingest-state / coupling-axis) alongside the underlying
    // 6 counts. Closes the natural agent cold-start workflow
    // identified in iter-210 (the 4-classifier composition
    // is the typical end-user opening probe). 1-row result
    // analogous to iter-187 and iter-203 single-row
    // classifiers but JOINING them on the corpus axis. Use
    // when an agent first encounters a new corpus; subsequent
    // workflow choice routes on the categorical labels.
    SavedQuery {
        name: "cold-start-overview",
        description: "Composite cold-start probe — emits all 3 categorical workflow-routing labels (corpus-shape regime + ingest-state + coupling-axis) alongside the underlying 6 counts (fn / file / doc / narrative_doc / coupled_with / endpoint) in 1 row. Composes iter-187 corpus-shape-classifier + iter-203/205 corpus-ingest-state-classifier + a new coupling-axis classification (`coupling-rich` >50 edges, `coupling-sparse` 1-50, `coupling-empty` 0). Per user-probe-050 Finding E this is the natural cold-start probe shape; running 1 saved query gives the agent enough to route the next workflow choice.",
        params: &[],
        needs: &[],
    },
    // Per iter 261 (user-probe-062 Finding D + user-probe-
    // 063 Finding C — operationalises the 3-tier corpus-
    // routing recipe at the agent boundary): COMPOSES iter-
    // 211 cold-start-overview's 3 categorical labels
    // (ingest_state + doc_axis_regime + coupling_axis) into
    // a single workflow-routing recommendation string. The
    // agent runs this 1 query and gets a categorical
    // recommendation + a named primitive class to reach
    // for next. Closes the iter-260 Finding C 3-tier
    // refinement that user-probe-062/063 surfaced across
    // trusted (doc-sparse + coupling-rich) and spacy
    // (doc-moderate + coupling-empty + function-rich).
    SavedQuery {
        name: "corpus-routing-recommendation",
        description: "1-row workflow-routing recommendation per [[user-probe-063]] Finding C 3-tier classification. Returns the underlying ingest_state + doc_axis_regime + coupling_axis labels from [[cold-start-overview]] PLUS a derived `recommended_primitives` string naming the saved-query class the agent should reach for first. The 4 routes: (1) `doc-rich + function-rich` → query_similar (Doc + Function axes) + research-by-tag; (2) `doc-rich + function-sparse` → query_similar Doc-axis + research-by-tag; (3) `doc-sparse + coupling-rich` → file-coupling-degree + coupled-files + import-fan-out (trusted FastAPI case); (4) `doc-sparse + coupling-empty + function-rich` → language-loc-distribution + Function-axis BM25 (spacy case). Empty corpus = `unknown`. Use as the FIRST saved query an agent runs on any unfamiliar corpus.",
        params: &[],
        needs: &[],
    },
    // Per iter 261 (user-probe-063 Finding D — per-corpus-
    // axis-availability matters): corpus-wide metadata-
    // saturation diagnostic. Reports per-axis row count +
    // the fraction with key metadata populated. Sibling to
    // corpus-routing-recommendation at the FILL-RATE axis:
    // routing recommendation tells which axis to use; this
    // tells how COMPLETE each axis's data is.
    SavedQuery {
        name: "corpus-axis-saturation",
        description: "1-row corpus-wide metadata saturation diagnostic. Reports per-axis row count + the fraction of each row population with KEY METADATA populated: Docs with non-empty summary, Files with language tagged, Functions with signature populated. Per [[user-probe-063]] Finding D (per-corpus-axis-availability matters), this tells the agent NOT JUST whether an axis has rows but whether its data is mature enough to query. A Function table with 4000 rows but signatures all empty (per [[feedback_function_signature_column_empty_py]]) reports function_count=4000 but signature_populated_fraction=0.0 — the agent learns to use body_excerpt not signature for Python.",
        params: &[],
        needs: &[],
    },
    // Per iter 168 (interrogation-033 Finding A +
    // user-probe-037 Findings B+C — operationalises the
    // iter-165 `feedback_uniformly_fresh_corpus_pattern`
    // memory at cold-start): returns the min + max Doc.updated
    // timestamps + the Doc count, so the agent can compute
    // the span and decide whether freshness probes will be
    // productive. ≤ 3 days = uniformly-fresh regime (loop's
    // own corpus); ≥ 30 days = wide-span regime (real
    // staleness signal available).
    SavedQuery {
        name: "corpus-update-span",
        description: "Min + max Doc.updated timestamps + Doc count — cold-start diagnostic for the corpus-regime classification. Use BEFORE running stale-narrative-docs / docs-by-update-recency to decide whether the freshness signal is meaningful (wide-span corpus) or degenerate (uniformly-fresh per feedback_uniformly_fresh_corpus_pattern). Pair with files-by-recency-desc/asc for the file-axis equivalent.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 366 (user-probe-100 Finding C — domain-
    // mismatch silent-failure on example-app). The user
    // asked a clinical-NLP question against example-app
    // (actually a dev-workflow-diagnostic tool); BM25
    // returned highest-keyword-match (entity-privacy =
    // LLM provider data-residency) with NO signal that the
    // corpus's domain was different. Cold-start orientation
    // is the missing primitive — the agent should be able
    // to ask "what is this corpus ABOUT?" in 1 call BEFORE
    // issuing domain-specific BM25. Top entities by
    // mention_count answer that question: a corpus whose
    // top entities are `snapshot / capture / session /
    // jupyter / ollama / groq` is unambiguously NOT
    // clinical NLP; a corpus whose top entities are
    // `tokenizer / token / vocab / language` is
    // unambiguously NLP-tokenization. Pair with
    // corpus-top-doc-tags (sibling) for a full identity
    // card.
    SavedQuery {
        name: "corpus-top-entities-by-mention",
        description: "Top 15 Entity rows ordered by mention_count DESC — cold-start domain-orientation primitive per user-probe-100. The agent calls this BEFORE issuing domain-specific BM25 against an unfamiliar external corpus. The top-entity profile reveals the corpus's actual domain: a corpus topped by `snapshot/capture/session/jupyter` is a dev-workflow tool; one topped by `tokenizer/token/vocab` is NLP-tokenization; one topped by `patient/clinical/phi` is clinical NLP. Sibling to corpus-top-doc-tags. Together they form the corpus-identity-card 2-query pattern per feedback_domain_mismatch_silent_failure.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 366 (user-probe-100 Finding C). Sibling of
    // corpus-top-entities-by-mention; together they form
    // the corpus-identity-card 2-query pattern. Tags are
    // the Doc-level vocabulary axis; the most frequent
    // tags reveal what the corpus's narrative content is
    // ABOUT at the family-axis (vs entity-axis). On a
    // mature design corpus the top tags surface the
    // intellectual roof (per
    // feedback_umbrella_tag_pattern); on a dev-workflow
    // tool corpus they surface deployment-axis themes; on
    // a doc-null code-repo they may be empty / starter-
    // ontology only.
    SavedQuery {
        name: "corpus-top-doc-tags",
        description: "Top 15 most frequent tags across all Doc.tags arrays — the family-axis sibling of corpus-top-entities-by-mention. Reveals what the corpus's NARRATIVE content is about (vs entity-axis: what the CODE is about). Use as the 2nd half of the corpus-identity-card cold-start orientation (per feedback_domain_mismatch_silent_failure). On doc-null corpora returns near-empty (signal: this corpus has no narrative; route to file-axis per feedback_doc_sparse_code_repo_recipe).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 371 [[user-probe-103]] Finding G + iter 368
    // + iter 370 (3rd-instance cumulative): tag-axis
    // returns 3 tags freq=1 on doc-linter-adoption-branch
    // corpora (spacy/spacy-llm/example-app all 'docs',
    // 'navigation', 'ontology' — the starter
    // ontology/README.md template's tag set). Agents who
    // call corpus-top-doc-tags and see this thin result
    // don't know whether it's structural (adoption-branch
    // = empty tag-axis) or a query failure. This shape-
    // classifier disambiguates in 1 call: total tag count
    // + distinct tag count + categorical
    // tag_axis_shape ('tag-axis-empty' / 'tag-axis-thin' /
    // 'tag-axis-rich'). Threshold calibration: ≤5 distinct
    // tags = empty (the 3-tag starter scaffold), ≤30
    // distinct = thin (some authoring), > 30 = rich
    // (mature tag taxonomy like the design corpus's 100+).
    SavedQuery {
        name: "corpus-tag-axis-shape",
        description: "1-row shape diagnostic for the Doc.tags axis — returns total_tag_mentions + distinct_tag_count + max_tag_frequency + categorical tag_axis_shape ('tag-axis-empty' / 'tag-axis-thin' / 'tag-axis-rich'). Per iter-371 [[user-probe-103]] Finding G: 3 adoption-branch corpora (spacy, spacy-llm, example-app) all return 3 tags freq=1 each from the starter ontology/README.md template. This query lets the agent know in 1 call whether tag-axis has usable signal (rich) or whether to route via entity-axis only (empty / thin). Sibling of [[corpus-doc-starter-ratio]] (Doc-axis) + [[corpus-ingest-state-classifier]] (per-pass).",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling at entity-axis. Per iter-371 + iter-368 +
    // iter-374: identity-card recipe relies on entity-axis
    // when tag-axis is empty (graceful degradation per
    // [[feedback_doc_sparse_code_repo_recipe]] 3rd-
    // instance). A shape diagnostic here lets the agent
    // verify entity-axis has the signal density needed
    // BEFORE running domain-specific queries.
    // Per iter 376 [[user-probe-106]] Finding B: entity
    // descriptions split into 2 styles by promotion status.
    // Cluster-derived entities have descriptions starting
    // 'Cluster anchored on X (N functions across M
    // file(s), density D). Surfaces around F1, F2, F3, F4'
    // — they support the iter-373 4th recipe component
    // (entity-description-as-nav-index). Promoted entities
    // have human-curated semantic narrative — they don't.
    // Per iter-376 the 4th recipe component is CORPUS-
    // SHAPE-SPECIFIC; this classifier lets agents route
    // before applying it. Calibration: spacy/spacy-llm
    // adoption-branch corpora = mostly-cluster-derived;
    // example-app = mostly-promoted (per iter-376 tier +
    // snapshot promoted descriptions); design corpus =
    // mostly-promoted (per the project's mature ontology).
    SavedQuery {
        name: "corpus-entity-promotion-state",
        description: "1-row classifier for the Entity-description style — counts cluster_derived (description starts with 'Cluster anchored on') vs promoted (everything else) entities + emits categorical promotion_state_label ('no-entities' / 'fully-promoted' / 'fully-cluster-derived' / 'mostly-cluster-derived' / 'mostly-promoted'). Per iter-376 [[user-probe-106]] Finding B: the iter-373 4th recipe component (entity-description-as-nav-index) applies ONLY to cluster-derived entities. Agents call this classifier BEFORE choosing whether to dereference 'Surfaces around X' pointers in entity descriptions. Sibling diagnostic to corpus-tag-axis-shape + corpus-entity-axis-shape + corpus-doc-starter-ratio. See [[feedback_promoted_vs_cluster_entity_descriptions]] for routing recipe.",
        params: &[],
        needs: &["Entity"],
    },
    // Sibling per iter-376: agents can enumerate the actual
    // cluster-derived entities (the ones for which the 4th
    // recipe component applies) to apply the 'Surfaces
    // around X' dereference recipe.
    SavedQuery {
        name: "cluster-derived-entities",
        description: "Entities whose description starts with 'Cluster anchored on' — the explicit list of cluster-derived (auto-generated stub) entities for which the iter-373 4th recipe component (entity-description-as-nav-index) applies. Per iter-376 [[user-probe-106]] Finding B + [[feedback_promoted_vs_cluster_entity_descriptions]]: agents read each description's 'Surfaces around F1, F2, F3, F4' line to find scattered implementations via fn.symbol CONTAINS. LIMIT 30 for cold-start surveys.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 378 [[user-probe-107]] + iter 379 [[user-
    // probe-108]] recipe-stack workflow: agents who get
    // multiple BM25 hits often need to bulk-fetch entity
    // descriptions to dereference 'Surfaces around X'
    // pointers across all hits. Currently they'd need
    // N round-trips of query_entity (1 per hit). This
    // primitive fetches all matching entities in 1 call.
    SavedQuery {
        name: "entities-by-id-pattern",
        description: "Entities whose id CONTAINS the given `$pattern` substring — returns id + description + mention_count for batch fetch. Per iter-378 [[user-probe-107]] + iter-379 [[user-probe-108]] 4-component recipe stack workflow: agents who get multiple BM25 hits read each entity's description to dereference 'Surfaces around X, Y, Z' pointers (per iter-373 4th recipe component). This primitive lets them bulk-fetch in 1 call instead of N. ORDER BY mention_count DESC so the most-important entity surfaces first. LIMIT 20.",
        params: &["pattern"],
        needs: &["Entity"],
    },
    // Per iter 385 [[user-probe-112]] + iter 387 [[user-
    // probe-113]] [[feedback_bm25_silent_miss_high_mention_
    // entity]] 2-instance: high-mention entities (rule=97,
    // serialize=191) silently miss BM25 ranking for domain-
    // matched queries. Mitigation: agent queries by entity
    // DESCRIPTION substring in parallel with BM25 + entity-
    // id-pattern. Sibling primitive: entities whose
    // DESCRIPTION (not id) contains the keyword.
    SavedQuery {
        name: "entities-by-description-substring",
        description: "Entities whose description CONTAINS the given `$pattern` substring — sibling of [[entities-by-id-pattern]] (iter-380). Per iter-385 + iter-387 + [[feedback_bm25_silent_miss_high_mention_entity]] 2-instance: high-mention entities can be invisible to BM25 for domain-matched queries; one mitigation route is to search entity DESCRIPTIONS directly. Captures entities where the keyword appears in semantic narrative or 'Surfaces around X' cluster pointers but the entity ID itself doesn't contain the keyword (e.g., rule entity description mentions 'run_diagnostics' → searching pattern='run_diagnostics' surfaces entity-rule). ORDER BY mention_count DESC. LIMIT 20.",
        params: &["pattern"],
        needs: &["Entity"],
    },
    // Composite for the silent-miss mitigation recipe: 1
    // call returns entities matching $pattern in EITHER
    // id OR description. Saves agents 2 separate calls.
    SavedQuery {
        name: "entities-by-id-or-description-substring",
        description: "Entities whose id OR description CONTAINS the given `$pattern` substring — composite of [[entities-by-id-pattern]] (iter-380) + [[entities-by-description-substring]] (iter-389). Per [[feedback_bm25_silent_miss_high_mention_entity]] 2-instance mitigation recipe: agents who want maximum recall for a keyword can use this single call instead of running 2 separate queries. Returns the UNION ranked by mention_count DESC. LIMIT 20.",
        params: &["pattern"],
        needs: &["Entity"],
    },
    // Per iter 393 [[user-probe-117]] Finding A: spacy-llm
    // has BOTH entity-example (singular, 103 mentions,
    // PROMOTED placeholder) AND entity-examples (plural,
    // CLUSTER-DERIVED, actual few-shot-examples entity).
    // Both rank similarly in BM25; agent must disambiguate.
    // This primitive detects all singular/plural entity-id
    // pairs in the corpus so agents flag the ambiguity
    // when issuing a BM25 query whose target keyword is
    // morphologically ambiguous.
    SavedQuery {
        name: "entity-singular-plural-pairs",
        description: "Detects pairs of entities where one id is the other's `+s` plural form (e.g., entity-example + entity-examples). Per iter-393 [[user-probe-117]] Finding A: doc-linter init's PROMOTED placeholder can co-exist with a CLUSTER-DERIVED domain entity at the morphologically-related id, creating BM25 ambiguity. Agents call this query to enumerate all such pairs in the corpus before issuing a domain query. Returns singular_id + plural_id + their combined mention_count. ORDER BY combined_mention_count DESC. LIMIT 20. CAVEAT: only detects the simple +s plural form (not -ies / -es / irregular).",
        params: &[],
        needs: &["Entity"],
    },
    // Sibling: 1-row corpus-level summary so agents can
    // quickly check 'does this corpus have any singular/
    // plural entity-id ambiguity to worry about?' without
    // enumerating the pairs.
    SavedQuery {
        name: "corpus-singular-plural-pair-summary",
        description: "1-row summary of singular/plural entity-id pairs in the corpus — returns pair_count + total_entities_in_pairs + categorical singular_plural_ambiguity_label ('no-pairs' / 'sparse-ambiguity' / 'rich-ambiguity'). Per iter-393 [[user-probe-117]] Finding A morphological ambiguity. Use as a 1-call corpus diagnostic; if pair_count > 0, agent enumerates pairs via [[entity-singular-plural-pairs]] (iter-395) before issuing potentially-ambiguous BM25 queries.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 390 [[user-probe-115]] 4-step recognition
    // chain: agent had to compose BM25 → entities-by-
    // description-substring → entity description → grep
    // file contents to find rule_signals.py from
    // entity-cuda-error. Step 3 ('which file mentions
    // this entity?') is the missing primitive that this
    // query closes. Uses FUNCTION_MENTIONS + TYPE_MENTIONS
    // edges to traverse Entity → Function/Type → File via
    // fn.file. Reduces the 4-step chain to 2 calls
    // (description-search + this primitive).
    SavedQuery {
        name: "entity-to-implementation-files",
        description: "Given an entity `$entity_id`, returns DISTINCT files containing Function or Type rows that MENTION (FUNCTION_MENTIONS or TYPE_MENTIONS) the entity — plus the mention count per file. Per iter-390 [[user-probe-115]] 4-step recognition chain: closes the 'which files implement / mention this entity?' gap. Useful when the entity description names a CONCEPT but the implementation uses the keyword only in string literals or via cluster pointers that the cluster description didn't surface. Excludes test files. ORDER BY mention_count DESC. LIMIT 20. Sibling: [[entities-by-id-or-description-substring]] (iter-389) for the keyword → entity step; this query handles entity → file.",
        params: &["entity_id"],
        needs: &["Entity"],
    },
    // Sibling per iter-390: summary diagnostic that returns
    // mention totals without enumerating files. Use for the
    // 'does this entity have ANY implementation?' check.
    SavedQuery {
        name: "entity-implementation-summary",
        description: "Given an entity `$entity_id`, returns 1-row summary: function_mentions_count + type_mentions_count + function_belongs_count (FUNCTION_BELONGS_TO edges = entity-anchored functions) + distinct_implementation_files. Use as a 1-call diagnostic when the agent wants to know whether an entity has implementation coverage BEFORE enumerating files via [[entity-to-implementation-files]]. Per iter-390 [[user-probe-115]]: cuda-error had high entity_mention_count from text matching but 0 FUNCTION_MENTIONS — signal that the entity is data-pattern-recognized (string literals) rather than code-anchored.",
        params: &["entity_id"],
        needs: &["Entity"],
    },
    // Per iter 402 [[user-probe-123]] Finding B: noop in
    // spacy-llm appears as BOTH a task (spacy_llm/tasks/
    // noop.py) AND a model (spacy_llm/models/rest/noop/
    // folder). NEW organizational pattern: DUAL-ROLE
    // entity instantiated in multiple folder structures
    // simultaneously (testing/baseline scaffold use
    // case). This primitive detects per-entity-id by
    // returning files whose path contains the entity id
    // — agents inspect the returned paths to recognize
    // dual-role / multi-role placement.
    SavedQuery {
        name: "files-containing-entity-id-in-path",
        description: "Given an `$entity_id`, returns files whose path CONTAINS the id substring (excluding test paths). Per iter-402 [[user-probe-123]] Finding B: detects DUAL-ROLE entities — entities instantiated in MULTIPLE folder patterns (e.g., noop as both tasks/noop.py AND models/rest/noop/). Distinct from [[entity-to-implementation-files]] (iter-392) which uses FUNCTION_MENTIONS / TYPE_MENTIONS edges; this primitive uses path-substring directly so it catches folder placements that may NOT have function mentions yet. Agents inspect returned paths to recognize multi-role / dual-role / multi-language layouts. ORDER BY path. LIMIT 30.",
        params: &["entity_id"],
        needs: &["File"],
    },
    SavedQuery {
        name: "entity-organizational-shape",
        description: "1-row categorical diagnostic of an entity's organizational placement by file-path-substring count. Given `$entity_id`, returns total_file_matches + categorical organizational_shape ('absent' if 0 / 'single-file' if 1 / 'multi-file-or-folder' if more). Per iter-402 [[user-probe-123]] Finding B: noop on spacy-llm has multi-file-or-folder (DUAL-ROLE — tasks/noop.py + models/rest/noop/*). Agents inspect the actual file paths via [[files-containing-entity-id-in-path]] (iter-404 sibling) to distinguish dual-role from single-folder-multi-file. Folder-distinctness via SPLIT not available in Kuzu cypher; agent does it client-side.",
        params: &["entity_id"],
        needs: &["File"],
    },
    // Per iter 393 [[user-probe-117]] singular/plural
    // ambiguity + iter 411 (etc): agents often need to
    // compare TWO entities side-by-side (e.g., entity-
    // example placeholder vs entity-examples domain
    // entity). Currently requires 2 query_entity calls.
    // This primitive returns both in 1 call.
    SavedQuery {
        name: "entity-pair-comparison",
        description: "Returns 2 entities side-by-side for comparison: given `$entity_a` and `$entity_b`, returns id + display + description + mention_count for both. Per iter-393 [[user-probe-117]] singular/plural ambiguity (entity-example vs entity-examples) + general disambiguation workflows: agents call this in 1 round-trip instead of 2 separate query_entity calls. Use after [[entity-singular-plural-pairs]] (iter-395) or [[entities-by-id-or-description-substring]] (iter-389) surfaces multiple candidates.",
        params: &["entity_a", "entity_b"],
        needs: &["Entity"],
    },
    SavedQuery {
        name: "entities-by-display-substring",
        description: "Entities whose display (e.display human-readable name) CONTAINS the given `$pattern` substring. Completes the entity-search trio with [[entities-by-id-pattern]] (iter-380) + [[entities-by-description-substring]] (iter-389). Per iter-401 BM25 display-prepend refinement: queries often match the human-readable form ('CUDA Error', 'Response Normalizer') better than the id-form. Agents who need explicit display-substring search at the cypher layer call this primitive. Returns id + display + mention_count + description. ORDER BY mention_count DESC. LIMIT 20.",
        params: &["pattern"],
        needs: &["Entity"],
    },
    // Per iter-410 (loop-maturity composite per iter-400+
    // reflection): the cumulative recipe stack has grown
    // to 5+ components (identity-card + axis-fingerprint +
    // 4th-recipe + promotion-classifier + Function-axis
    // fallback). Common workflow: agent identifies a
    // candidate entity, then needs id + display +
    // description + mention_count + promotion classification
    // (cluster-derived / promoted-with-pointer / promoted-
    // pure-semantic) all at once. This single-call composite
    // returns all 5 fields. Reduces 2-3 round-trips to 1.
    SavedQuery {
        name: "entity-card-summary",
        description: "1-row composite card for an entity. Given `$entity_id`, returns id + display + description + mention_count + categorical promotion_label ('cluster-derived' / 'promoted-with-pointer' / 'promoted-pure-semantic'). Per iter-410 loop-maturity composite of iter-377 corpus-entity-promotion-state + iter-386 promoted-with-pointer detection per-entity. Use as 1-call cold-start view of an entity instead of composing 3 separate queries (query_entity + classifier + pointer-check). Sibling of [[entity-pair-comparison]] (iter-407) for 2-entity comparisons; this is the single-entity equivalent.",
        params: &["entity_id"],
        needs: &["Entity"],
    },
    // Sibling to entity-card-summary per iter-410: extends
    // with file-axis count via files-containing-entity-id-
    // in-path (iter-404) result. Single-call cold-start
    // view combining promotion classification + file-axis
    // presence.
    SavedQuery {
        name: "entity-card-with-file-axis",
        description: "1-row composite extending [[entity-card-summary]] (iter-410) with file-axis presence. Given `$entity_id`, returns the card fields PLUS file_axis_count (files whose path CONTAINS the id substring, excluding tests). Combines iter-377 promotion-classifier + iter-386 promoted-with-pointer + iter-404 path-substring counts in 1 call. Use as the deepest cold-start view of an entity when the agent wants categorical (promotion + organizational-shape signal) + numerical (mention + file counts) in 1 round-trip. Builds on [[entity-card-summary]] and [[files-containing-entity-id-in-path]].",
        params: &["entity_id"],
        needs: &["Entity"],
    },
    // Per iter-413 (closing iter-412's refutation finding):
    // when synthesizing "how to add a custom <thing>"
    // guidance, the agent must FIRST detect which pattern
    // the corpus uses — Protocol/Impl 2-class
    // (spacy-llm idiom: typed Protocol module + concrete
    // impl module) vs flat-dispatch (example-app idiom:
    // mirrored module-level private fns + dispatcher).
    // These 2 sibling queries fingerprint the two idioms
    // directly. Recipe: call BOTH; if (1) returns hits +
    // (2) empty → Protocol/Impl idiom; if (1) empty + (2)
    // hits → flat-dispatch idiom; if both → mixed.
    SavedQuery {
        name: "mirrored-private-functions-by-suffix",
        description: "Flat-dispatch pattern detector (iter-413 closing iter-412 user-probe-130 refutation). Given `$suffix` (e.g. '_generate' or '_available'), returns module-level private functions whose symbol ends with `_<X>$suffix` — the mirrored-fn fingerprint of flat-dispatch architecture. Excludes class methods (filters out `#`). Multiple hits with distinct prefixes (e.g. `_ollama_generate` + `_groq_generate`) signal a flat-dispatch extension point. Sibling of [[protocol-module-functions]] for the opposite idiom (Protocol/Impl). Per iter-412 example-app Ollama probe: detect pattern BEFORE synthesizing 'how to add a provider' recipe.",
        params: &["suffix"],
        needs: &["Function"],
    },
    // Sibling to mirrored-private-functions-by-suffix per
    // iter-413: the Protocol/Impl idiom fingerprint.
    // spacy-llm-style typed-Protocol modules
    // (spacy_llm.ty.Cache, spacy_llm.ty.LLM,
    // spacy_llm.ty.Task) expose Protocol interfaces;
    // concrete impls live in dedicated modules. Given a
    // `$protocol_module_substring` (e.g. '.ty`' for
    // spacy-llm, '.types`' for other styles), this query
    // returns the protocol-module function symbols.
    SavedQuery {
        name: "protocol-module-functions",
        description: "Protocol/Impl pattern detector (iter-413 closing iter-412 user-probe-130 refutation). Given `$protocol_module_substring` (e.g. '.ty`' for spacy-llm-style, '.types`' for other typed-module conventions), returns Functions in the protocol module — the Protocol/Impl fingerprint. Multiple Function symbols from a single typed module signal a Protocol-extension idiom (subclass + register pattern). Sibling of [[mirrored-private-functions-by-suffix]] for the opposite idiom (flat-dispatch). Per iter-412 spacy-llm Cache probe: detect Protocol layer BEFORE synthesizing 'how to add a custom X' recipe — the answer is corpus-specific (Protocol subclass vs mirrored-fn add).",
        params: &["protocol_module_substring"],
        needs: &["Function"],
    },
    // Per iter-416 (closing iter-415's 3-layer refinement
    // gap): iter-415 user-probe-132 surfaced that the
    // Protocol/Impl pattern is sometimes 3-layer (Protocol
    // → AbstractBase → Concrete) with a shared template-
    // method base. spacy_llm.ty exposes 27 functions
    // across 10+ Protocols — too many for the agent to
    // navigate without filtering. These 2 sibling queries
    // refine iter-413 pattern-detectors:
    //
    // 1. protocol-module-class-methods: stricter Protocol
    //    detector that surfaces only class-method symbols
    //    (CONTAINS '#'). Drops module-level helpers.
    //
    // 2. abstract-base-private-hooks: 3-layer middle-layer
    //    detector. Given a candidate AbstractBase class
    //    path substring (e.g. 'tasks.builtin_task'),
    //    returns private-hook methods (CONTAINS '#_') —
    //    the template-method signature.
    //
    // Together with iter-413: agent runs protocol-module-
    // class-methods to enumerate Protocols cleanly, then
    // for each candidate impl tree runs abstract-base-
    // private-hooks to detect the optional middle layer.
    SavedQuery {
        name: "protocol-module-class-methods",
        description: "3-layer-aware Protocol/Impl detector refinement (iter-416 closing iter-415 user-probe-132). Stricter sibling of [[protocol-module-functions]] (iter-413): given `$protocol_module_substring` (e.g. '.ty`'), returns ONLY class-method symbols (CONTAINS '#') from the Protocol module — drops module-level helpers. Use when the Protocol module is rich (per iter-415 spacy_llm.ty exposes 27 fns across 10+ Protocols) and the agent needs to enumerate the typed contracts cleanly. Group client-side by class name (extract between '/' and '#') to get the Protocol catalog.",
        params: &["protocol_module_substring"],
        needs: &["Function"],
    },
    // Sibling per iter-416: middle-layer detector for the
    // 3-layer pattern. Per iter-415: when an abstract base
    // exists (e.g. BuiltinTask at spacy_llm.tasks.
    // builtin_task), it follows the template-method idiom
    // — public methods delegate to private `_<hook>`
    // methods that concrete subclasses override.
    SavedQuery {
        name: "abstract-base-private-hooks",
        description: "3-layer Protocol/Impl middle-layer detector (iter-416 closing iter-415 user-probe-132 3-layer refinement). Given `$class_path_substring` (e.g. 'tasks.builtin_task' or 'BuiltinTask'), returns private-hook methods (CONTAINS '#_') — the template-method signature that identifies an AbstractBase class. Multiple private-hook methods on a single class signal middle-layer template-method pattern (Protocol → AbstractBase → Concrete). Per iter-415 spacy-llm BuiltinTask: `_get_prompt_data` + `_preprocess_docs_for_prompt` are the override points concrete tasks (`tasks/rel/task.py`, `tasks/textcat/task.py`) implement. Recipe: subclass AbstractBase, NOT Protocol directly; override these private hooks.",
        params: &["class_path_substring"],
        needs: &["Function"],
    },
    // Per iter-419 (closing iter-417 + iter-418 findings):
    // (1) iter-417 surfaced that abstract-base-private-
    //     hooks can hit test methods (e.g.
    //     TestPositionDetectorLive#_assert_meaningful_
    //     result), causing false-positive 3-layer
    //     detection on app corpora. Ship a non-test
    //     variant.
    // (2) iter-418 surfaced FACT vs PATTERN entity
    //     distinction — density × function-count
    //     discriminates, not mention count alone. Ship a
    //     classifier so agents can decide whether an
    //     entity is an extension-target candidate
    //     (PATTERN) or a usage target (FACT) BEFORE
    //     applying any "add X" recipe.
    SavedQuery {
        name: "abstract-base-private-hooks-non-test",
        description: "Test-filtered variant of [[abstract-base-private-hooks]] (iter-419 closing iter-417 user-probe-133 edge-case). Same Protocol/Impl 3-layer middle-layer detector but excludes test methods (filters out 'tests' + 'test_' patterns in symbol). Per iter-417: on example-app abstract-base-private-hooks on 'position_detector' hit only TestPositionDetectorLive#_assert_meaningful_result — a TEST method, not a production hook. Use this query when the candidate class might co-locate test helpers. Cross-corpus: on spacy-llm BuiltinTask should still return the real _get_prompt_data + _preprocess_docs_for_prompt hooks; on example-app position_detector should return EMPTY (correctly identifying NO 3-layer pattern).",
        params: &["class_path_substring"],
        needs: &["Function"],
    },
    // Sibling per iter-419: FACT vs PATTERN entity-role
    // classifier per iter-418 [[feedback_fact_vs_pattern_
    // entity]]. Density × function-count discriminates:
    // FACT (data structure used everywhere — low density
    // × huge fn count); PATTERN (extension point — high
    // density × small fn count). The classifier counts
    // distinct functions mentioning the entity and
    // applies thresholds. Agents check this BEFORE
    // synthesizing any "how do I add / extend X" recipe.
    SavedQuery {
        name: "entity-fact-vs-pattern-classifier",
        description: "FACT vs PATTERN entity-role classifier (iter-419 closing iter-418 user-probe-134 hypothesis). Given `$entity_id`, counts distinct Functions mentioning the entity and labels: FACT (fn_count > 50 — data structure threaded through the codebase, e.g. iter-418 spacy 'language' 419 fns); PATTERN (fn_count 1-30 — focused extension point, e.g. iter-411 spacy-llm 'cache' 3 fns); MID (31-50 — likely large extension point or transitional). Per iter-418 finding: only PATTERN entities are 'add X' recipe targets; FACT entities require file-axis recipes instead. Use as cold-start guard against treating a high-BM25 FACT entity as an extension target. Sibling of [[entity-card-summary]] (iter-410) — adds role classification dimension.",
        params: &["entity_id"],
        needs: &["Entity"],
    },
    SavedQuery {
        name: "entity-fact-vs-pattern-classifier-v2",
        description: "Density-text heuristic FACT vs PATTERN classifier (iter-421 closing iter-420 user-probe-135 finding that iter-419 broken). Per iter-420 5-instance calibration table: density 0.0X = FACT (data structure spread across many files like spacy 'language' 0.01 / 419 fns); density 0.1X = MID; density 0.2X+ = PATTERN (focused extension point like spacy-llm 'cache' 0.67 / 3 fns or 'task' 0.31 / 9 fns). Uses CONTAINS substring match on e.description (Kuzu lacks SPLIT/regex). PROMOTED entities (no cluster description) return 'unclassified-promoted'. Sibling of [[entity-card-summary]] (iter-410) — adds verified-working role classification. SUPERSEDES iter-419 entity-fact-vs-pattern-classifier (broken signal).",
        params: &["entity_id"],
        needs: &["Entity"],
    },
    // Sibling per iter-421: inventory of FACT-likely
    // entities (low cluster density). Same heuristic as
    // entity-fact-vs-pattern-classifier-v2 but enumerates
    // all matches. Agent uses this for corpus-wide FACT
    // inventory — e.g. "which entities should I NOT treat
    // as extension targets in this corpus?"
    SavedQuery {
        name: "low-density-entities-as-likely-fact",
        description: "Inventory of FACT-likely entities (iter-421 sibling of [[entity-fact-vs-pattern-classifier-v2]]). Returns all cluster-derived entities (description STARTS WITH 'Cluster anchored on') whose description CONTAINS 'density 0.0X' substring (X = 0-9). Per iter-420 5-instance calibration: low cluster density = entity spread across many files = FACT (data structure, not extension target). Agents use this to inventory the corpus's FACT entities BEFORE picking a high-BM25 hit as a recipe target. Cross-corpus expected: spacy will return many (language, doc, token, span, vocab, ...); spacy-llm should return few; example-app should return few.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter-424 (closing iter-423 user-probe-137
    // findings): workflow probes hit entity-axis gap
    // (workflow nouns like 'train' / 'ner' aren't
    // ontology entities → BM25 routes to adjacent CLI-
    // subcommand entities with vocab overlap), AND
    // iter-421 v2 PATTERN bucket conflates extension-
    // target Protocols with non-extension CLI subcommands
    // + utility clusters. Two sibling refinement queries:
    //
    // 1. axis-routing-card-for-vocabulary: given a
    //    candidate term, returns entity-axis count +
    //    file-axis count in one call so agent can
    //    classify probe shape (architecture / workflow /
    //    identifier / absent) BEFORE BM25 retrieval.
    //
    // 2. cli-subcommand-pattern-entities: corpus-wide
    //    inventory of likely CLI-subcommand entities
    //    (id starts with known CLI-verb prefix AND has
    //    cluster description). Lets agent post-filter
    //    iter-421 v2 PATTERN results that are actually
    //    CLI-subcommand non-extension entities.
    SavedQuery {
        name: "axis-routing-card-for-vocabulary",
        description: "Probe-shape pre-classifier (iter-424 closing iter-423 user-probe-137 workflow-probe-entity-axis-gap finding). Given `$term`, returns 2 rows: ('entity', count of Entities whose id CONTAINS term) + ('file', count of Files whose path CONTAINS term). Agent applies 4-axis routing rule per [[feedback_probe_shape_4_axis_routing]]: both > 0 → ARCHITECTURE/IDENTIFIER probe (proceed both axes); entity=0 + file>0 → WORKFLOW probe (route to file-axis only); entity>0 + file=0 → metadata-only (rare); both=0 → ABSENT (entity does not cover this concept). Call BEFORE BM25 to avoid the iter-423 silent-mis-route (workflow query returning CLI-subcommand entities at high score because the workflow noun isn't in the ontology).",
        params: &["term"],
        needs: &["Entity", "File"],
    },
    // Sibling per iter-424: corpus-wide inventory of
    // CLI-subcommand entities (PATTERN-bucket false-
    // positive sub-classifier per iter-423 findings).
    SavedQuery {
        name: "cli-subcommand-pattern-entities",
        description: "PATTERN-bucket sub-classifier (iter-424 closing iter-423 user-probe-137 PATTERN-conflation finding). Returns cluster-derived entities whose id starts with a known CLI-verb prefix — `debug-` / `init-` / `train-` / `validate-` / `convert-` / `apply-` / `package-` / `assemble-` / `evaluate-` / `run-` / `build-` / `deploy-`. Per iter-423: iter-421 v2 PATTERN label conflates extension-target Protocols (Cache, Task) with CLI subcommands (debug-data, init-config). Agent calls this query after iter-421 v2 says PATTERN; if candidate entity_id appears in this inventory, it's a CLI subcommand (NOT an extension target). Sibling of [[entity-fact-vs-pattern-classifier-v2]] (iter-421) — adds PATTERN-bucket role refinement. Cross-corpus expected: spacy returns many (init-config, debug-data, ...); spacy-llm / example-app few.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter-426 (closing iter-425 user-probe-138 BUG
    // + 5th-axis findings):
    //
    // (1) iter-424 axis-routing-card BUG: empty-axis
    //     branches collapse to 0 rows under UNION ALL.
    //     iter-426 v2 uses UNWIND [1] anchor +
    //     OPTIONAL MATCH so ABSENT correctly returns
    //     (entity_count=0, file_count=0).
    //
    // (2) iter-425 surfaced 5th probe axis FUNCTION-ONLY
    //     (scoring is method-level not file-level —
    //     ScorableTask Protocol + per-task scorer()
    //     methods + make_<task>_scorer factories). iter-
    //     426 v2 ALSO returns function_count.
    //
    // (3) Direct Function-axis search primitive for
    //     FUNCTION-ONLY-route probes.
    //
    // SUPERSEDES iter-424 axis-routing-card-for-vocabulary
    // (broken on ABSENT case).
    SavedQuery {
        name: "axis-routing-card-for-vocabulary-v2",
        description: "Probe-shape pre-classifier v2 (iter-426 closing iter-425 user-probe-138 BUG + 5th-axis findings). Given `$term`, returns 1 row with entity_count + file_count + function_count via UNWIND [1] anchor + OPTIONAL MATCH (correctly returns 0+0+0 for ABSENT). Agent applies 5-axis routing per [[feedback_probe_shape_4_axis_routing]] PROMOTED 2-instance: entity>0 + file>0 → ARCHITECTURE/IDENTIFIER (all axes); entity=0 + file>0 + function>0 → WORKFLOW (file-axis primary); entity=0 + file=0 + function>0 → FUNCTION-ONLY (Function-axis direct, e.g. iter-425 spacy-llm score/scorer); all 0 → ABSENT. SUPERSEDES iter-424 axis-routing-card-for-vocabulary (broken on ABSENT — UNION ALL of empty MATCH branches returns 0 rows total).",
        params: &["term"],
        needs: &[],
    },
    // Sibling per iter-426: FUNCTION-ONLY axis primitive
    // for the 5th probe shape (iter-425 surfaced). Given
    // $term, returns Function symbols containing it
    // (non-test). Used when iter-426 v2 axis-routing-card
    // shows entity_count=0 AND file_count=0 AND
    // function_count>0.
    SavedQuery {
        name: "function-axis-symbol-search",
        description: "FUNCTION-ONLY axis primitive (iter-426 closing iter-425 user-probe-138 5th-axis finding). Given `$term`, returns Function symbols whose symbol CONTAINS the term (excluding tests via 'tests' / 'test_' filters). Use after [[axis-routing-card-for-vocabulary-v2]] returns FUNCTION-ONLY route (entity_count=0 + file_count=0 + function_count>0). Example: iter-425 spacy-llm 'score' returns ScorableTask Protocol + LLMWrapper#score + per-task scorer() methods + make_<task>_scorer factories — all method-level, no dedicated entity or file. Sibling of [[mirrored-private-functions-by-suffix]] and [[protocol-module-class-methods]] but with non-test filter and broader CONTAINS match.",
        params: &["term"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "utility-cluster-pattern-entities",
        description: "Utility-cluster sub-classifier (iter-428 closing iter-423 + iter-427 PATTERN-bucket conflation). Returns cluster-derived entities whose description marks them as 1-2 file utility clusters (CONTAINS 'across 1 file(s)' OR 'across 2 file(s)'). Per iter-422 + iter-417 findings: these are likely simple-module-public-API or pipeline-of-private-fns app idioms (e.g. example-app capture, position_detector) — NOT real extension targets. Sibling of [[cli-subcommand-pattern-entities]] (iter-424) and [[multi-file-extension-target-candidates]] (iter-428). Agent calls all 3 to triage iter-421 v2 PATTERN bucket into 3 sub-classes before applying recipe.",
        params: &[],
        needs: &["Entity"],
    },
    // Sibling per iter-428: multi-file extension-target
    // candidates. Real extension-target Protocols span
    // multiple impl files (iter-415 spacy-llm tasks/<X>/
    // folders; iter-414 spacy.ty.TrainableComponent
    // across pipeline component classes).
    SavedQuery {
        name: "multi-file-extension-target-candidates",
        description: "Multi-file extension-target sub-classifier (iter-428 closing iter-423 + iter-427 PATTERN-bucket conflation). Returns cluster-derived entities whose description marks them as multi-file clusters (CONTAINS 'across 3 file(s)' through 'across 9 file(s)' — the upper PATTERN bucket per iter-421 v2). Per iter-415 spacy-llm task (9 files across tasks/<X>/) + iter-414 spacy TrainableComponent (multi-file pipeline components): real extension-target Protocols span multiple impl files. Sibling of [[utility-cluster-pattern-entities]] (iter-428) and [[cli-subcommand-pattern-entities]] (iter-424). Agent uses 3-way triage: CLI-subcommand intersection → CLI verb idiom; utility-cluster intersection → simple-module app idiom; multi-file intersection → real extension-target candidate; remainder → unclassified PATTERN.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter-431 (closing iter-429 + iter-430 DEBUGGING-
    // NL-MISMATCH 6th-axis findings): when user phrases
    // an error message in natural language, BM25 returns
    // ZERO hits because error vocab doesn't match codebase
    // vocab. iter-426 v2 axis-routing-card-v2 routes the
    // extracted lexical keyword, but the DEBUGGING use
    // case has a richer signal: the corpus typically has
    // dedicated Error/Exception Type classes that name
    // the failure mode. These 2 sibling Type-axis
    // primitives close the loop for debugging probes:
    //
    // 1. error-type-catalog: corpus-wide inventory of
    //    Error/Exception Type classes (symbol ENDS WITH
    //    'Error#' or 'Exception#'). Per iter-429 spacy
    //    has spacy.errors.Errors / ErrorsWithCodes /
    //    MatchPatternError; agent uses to find the
    //    failure-mode class to start debugging.
    //
    // 2. types-by-name-substring: given $term (a class
    //    name fragment user might mention), return Type
    //    rows. Sibling of [[entities-by-id-pattern]]
    //    (iter-380) at the Type axis.
    SavedQuery {
        name: "error-type-catalog",
        description: "Corpus-wide Error/Exception Type catalog (iter-431 closing iter-429 + iter-430 6th-axis DEBUGGING-NL-MISMATCH findings). Returns Type rows whose symbol ENDS WITH 'Error#' or 'Exception#' — the inventory of failure-mode classes. Per iter-429 spacy: spacy.errors module has Errors + ErrorsWithCodes + MatchPatternError. Per iter-430 spacy-llm: tasks raise KeyError + parse-failures. Agent uses this BEFORE BM25 on debugging probes — directly enumerates the corpus's error vocabulary. Sibling of [[entities-by-id-pattern]] (iter-380) at the Type axis. Returns symbol + file + line + body_excerpt for each error/exception class.",
        params: &[],
        needs: &["Type"],
    },
    // Sibling per iter-431: Type-axis substring search
    // primitive. Sibling of iter-380 entities-by-id-
    // pattern at the Type axis.
    SavedQuery {
        name: "types-by-name-substring",
        description: "Type-axis substring search primitive (iter-431). Given `$term`, returns Type rows whose symbol CONTAINS the term. Sibling of [[entities-by-id-pattern]] (iter-380) at the Type axis. Useful for finding 'where is class X defined' when the user mentions a class name in a debugging or refactor probe. Excludes test files (CONTAINS 'tests' / 'test_'). Returns symbol + file + line + body_excerpt + kind + signature. Complements [[error-type-catalog]] (iter-431) for the DEBUGGING shape: error-type-catalog enumerates ALL error classes; types-by-name-substring finds a specific class by partial match.",
        params: &["term"],
        needs: &["Type"],
    },
    // Per iter-433 (closing iter-432 user-probe-142 signal-
    // class-form finding): iter-431 error-type-catalog
    // only catches ENDS WITH 'Error#' / 'Exception#'.
    // iter-432 surfaced 2 misses:
    //
    // (1) NAMING-CONVENTION extensions: production
    //     codebases also use 'Failure', 'Issue',
    //     'Diagnosis', 'Fault' suffixes for failure-mode
    //     classes. SUPERSEDED by failure-mode-class-
    //     catalog with broader endings.
    //
    // (2) SIGNAL-CLASS form: data classes for observation
    //     events (example-app ErrorSignal). These name a
    //     failure mode but ARE NOT exceptions — they're
    //     records of observed failures. signal-class-by-
    //     substring captures them via CONTAINS 'Signal'.
    SavedQuery {
        name: "failure-mode-class-catalog",
        description: "Corpus-wide failure-mode Type catalog (iter-433 closing iter-432 user-probe-142 signal-class-form finding; SUPERSEDES [[error-type-catalog]] iter-431 with broader naming-convention coverage). Returns Type rows whose symbol ENDS WITH 'Error#' / 'Exception#' / 'Failure#' / 'Issue#' / 'Fault#' / 'Diagnosis#'. Captures broader failure-mode class catalog than iter-431. Per iter-429 spacy MatchPatternError + iter-432 example-app ErrorSignal (which fails iter-431 ENDS WITH but matches sibling [[signal-class-by-substring]]). Excludes tests. Use BEFORE BM25 on debugging probes to enumerate corpus's failure vocabulary. Returns symbol + file + line + body_excerpt.",
        params: &[],
        needs: &["Type"],
    },
    // Sibling per iter-433: signal-class-form detector.
    // example-app ErrorSignal is a data class recording
    // failure observations — not an Exception, but still
    // a failure-mode class. Captured via 'Signal' OR
    // 'Signature' OR 'Detector' substrings.
    SavedQuery {
        name: "signal-class-by-substring",
        description: "Failure-mode signal/observation data-class detector (iter-433 closing iter-432 SIGNAL-CLASS-FORM 1-instance hypothesis). Given `$term`, returns Type rows whose symbol CONTAINS the term AND CONTAINS 'Signal' / 'Signature' / 'Detector' / 'Record' / 'Observation'. Captures data classes that name a failure mode WITHOUT being exception classes (e.g. example-app `ErrorSignal` records observed errors as data, not raised exceptions). Sibling of [[failure-mode-class-catalog]] (iter-433) which catches exception forms; this catches observation forms. Per iter-432 user-probe-142 example-app ErrorSignal: iter-431 ENDS WITH filter missed it but its production purpose is failure-mode tracking. Excludes tests.",
        params: &["term"],
        needs: &["Type"],
    },
    // Per iter-436 (closing iter-434 + iter-435 7th-axis
    // REFACTOR-IMPACT 2-instance findings): iter-434 spacy
    // Doc + iter-435 spacy-llm LLMWrapper both confirm
    // god_node markers as blast-radius signal. Two sibling
    // primitives for the REFACTOR-IMPACT recipe:
    //
    // 1. god-node-entities: corpus-wide inventory of
    //    Entity rows where is_god_node = true. The
    //    "blast-radius pivot" set — entities whose
    //    rename/refactor would cascade widely. Agent uses
    //    BEFORE proposing any rename to gauge whether the
    //    target is a god_node-adjacent concept.
    //
    // 2. function-entity-touch-card: given $function_
    //    symbol, return the entities the function touches
    //    via FUNCTION_MENTIONS with god_node status. The
    //    saved-query analog of `query impact`'s entities-
    //    list; lets agents pull blast-radius from
    //    query_saved without depending on the impact
    //    CLI subcommand.
    SavedQuery {
        name: "god-node-entities",
        description: "Corpus-wide blast-radius-pivot inventory (iter-436 closing iter-434/435 7th-axis REFACTOR-IMPACT findings). Returns Entity rows where is_god_node = true — the entities whose rename/refactor would cascade through the corpus. Per iter-434 spacy: doc/span/token are god_nodes (3 markers on Doc refactor = entire data model); per iter-435 spacy-llm: llm/task are god_nodes. Agent calls BEFORE proposing any rename/refactor to gauge whether the target is god_node-adjacent. Sibling of [[function-entity-touch-card]] (iter-436) for per-function blast radius. Returns id + display + mention_count + description.",
        params: &[],
        needs: &["Entity"],
    },
    // Sibling per iter-436: per-function blast-radius
    // saved query. Equivalent to the entities-list
    // returned by `query impact` CLI but exposed as a
    // saved query for agents that compose via
    // query_saved.
    SavedQuery {
        name: "function-entity-touch-card",
        description: "Per-function blast-radius primitive (iter-436 closing iter-434/435 7th-axis findings). Given `$function_symbol` (full SCIP symbol), returns entities the function touches via FUNCTION_MENTIONS with their is_god_node status + mention_count. Equivalent to query_impact's `entities:` list but exposed as saved query. Per iter-434 spacy Doc#__init__: returns 6 entities (doc + span + token god_node + character-embed + strings + vocab). Per iter-435 spacy-llm LLMWrapper#__init__: returns 6 entities (llm + task god_node + cache + model + noop + ty). Agent calls AFTER axis-routing identifies a Function symbol; the god_node count = blast-radius estimate.",
        params: &["function_symbol"],
        needs: &["Entity", "Function"],
    },
    // Per iter-438 (closing iter-437 SCIP-coverage-
    // completeness 1-instance hypothesis): iter-437
    // surfaced that CALLS-empty pattern (iter-434/435)
    // is Cython-specific, not universal Python.
    // example-app has rich CALLS edges; spacy/spacy-llm
    // don't. Before committing to a refactor-impact
    // route (depths-based vs entities-based), agent
    // should test corpus SCIP coverage. Two sibling
    // primitives:
    //
    // 1. corpus-calls-coverage-ratio: corpus-wide
    //    CALLS coverage scalar. Returns total Functions,
    //    Functions-with-inbound-CALLS, and ratio. Low
    //    ratio (e.g. <10%) signals SCIP gap; agent
    //    routes to entities-based blast radius. High
    //    ratio signals CALLS-rich; agent uses depths.
    //
    // 2. function-direct-callers-count: scalar variant
    //    of query_impact's depths-list-length. Given
    //    $function_symbol, returns count of direct
    //    callers via inbound CALLS. Used as a trial
    //    query before committing to depths-based
    //    refactor recipe.
    SavedQuery {
        name: "corpus-calls-coverage-ratio",
        description: "Corpus SCIP-coverage scalar (iter-438 closing iter-437 user-probe-145 SCIP-coverage-completeness 1-instance hypothesis). Returns total_functions + functions_with_inbound_calls + coverage_ratio. Low ratio (e.g. <10%) signals Cython/SCIP-gap pattern (per iter-434/435 spacy + spacy-llm: CALLS sparse despite many Functions); agent routes refactor-impact to ENTITIES-based recipe. High ratio (e.g. >50%) signals SCIP-rich (per iter-437 example-app: CALLS populated); agent uses DEPTHS-based recipe. Sibling of [[function-direct-callers-count]] (iter-438) for per-function trial probe. Use BEFORE committing to a refactor-impact route.",
        params: &[],
        needs: &["Function"],
    },
    // Sibling per iter-438: per-function callers-count
    // primitive for the SCIP-coverage trial probe.
    SavedQuery {
        name: "function-direct-callers-count",
        description: "Per-function CALLS coverage trial (iter-438). Given `$function_symbol`, returns count of direct callers via inbound :CALLS relationship. Used as quick trial BEFORE running full query_impact: if 0 → corpus's SCIP coverage may be sparse for this Function (per iter-434/435 Cython gap); use [[function-entity-touch-card]] (iter-436) entities-based recipe instead. If >0 → CALLS-rich; full query_impact will return useful depths tree (per iter-437 example-app capture_event 3 direct callers).",
        params: &["function_symbol"],
        needs: &["Function"],
    },
    // Per iter-441 (closing iter-439 + iter-440 8th-axis
    // ONBOARDING-EMPTY 2-instance findings): adoption-
    // branch corpora are 100% ontology-meta docs (spacy
    // 91/91 + spacy-llm 45/45 = 136/136); example-app is
    // mixed content+ontology. The ontology-role ratio
    // discriminates corpus purpose. Two sibling
    // primitives:
    //
    // 1. corpus-purpose-classifier: corpus-wide
    //    ontology-role ratio + categorical label
    //    (ADOPTION-BRANCH / CONTENT-RICH / MIXED).
    //    Agent calls BEFORE ONBOARDING queries; if
    //    ADOPTION-BRANCH → route to file-axis
    //    (README.md / external sources). If CONTENT-RICH
    //    → Doc-axis BM25 will surface real content.
    //
    // 2. non-ontology-docs: returns Doc rows that are
    //    NOT ontology-* roles — the actual content-doc
    //    subset (if any) on a mixed corpus.
    SavedQuery {
        name: "corpus-purpose-classifier",
        description: "Corpus-purpose classifier (iter-441 closing iter-439 + iter-440 8th-axis ONBOARDING-EMPTY 2-instance findings). Returns total_docs + ontology_meta_docs (roles in {ontology-entity, ontology-value, ontology-axis, ontology-migration, index}) + ontology_ratio + corpus_purpose label. Per iter-439/440: spacy 100% + spacy-llm 100% adoption-branch → ADOPTION-BRANCH label; example-app mixed → CONTENT-RICH or MIXED. Agent calls BEFORE ONBOARDING / CONTENT queries: if ADOPTION-BRANCH → route to file-axis (README.md) or external sources; if CONTENT-RICH → Doc-axis BM25 surfaces actual content. Cold-start corpus-shape primitive.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-441: filter for actual content
    // docs (NOT ontology-*). On adoption-branch corpora
    // this returns empty or near-empty (per iter-439/440
    // 100% meta-taxonomy); on content-rich example-app
    // returns ~50% of docs.
    SavedQuery {
        name: "non-ontology-docs",
        description: "Content-doc filter (iter-441 sibling of [[corpus-purpose-classifier]]). Returns Doc rows whose role is NOT in {ontology-entity, ontology-value, ontology-axis, ontology-migration, index} — the actual content-doc subset. Per iter-439/440: empty on adoption-branch corpora (spacy + spacy-llm); populated on content-rich corpora (example-app). Agent uses this AFTER corpus-purpose-classifier returns CONTENT-RICH or MIXED to enumerate the actual onboarding/tutorial/how-to/reference content. Returns id + role + kind + title + summary.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter-443 (closing iter-442 user-probe-148
    // MIXED-corpus content-thin finding): example-app has
    // 10/89 non-ontology docs; among those the
    // onboarding-shape ones are kind in {explanation,
    // how-to, tutorial}. Two sibling primitives:
    //
    // 1. onboarding-doc-shortlist: focused subset of
    //    non-ontology-docs (iter-441) — only kind in
    //    {explanation, how-to, tutorial}. Sharp filter
    //    for "give me 3-5 starting points" use case.
    //
    // 2. session-context-docs: subset of non-ontology
    //    docs whose id CONTAINS session/handoff/next.
    //    Session-continuity docs are a recurring shape
    //    (per iter-442 example-app handoff-to-original-
    //    session + next-session). Useful when user is
    //    resuming work in a Claude Code session.
    SavedQuery {
        name: "onboarding-doc-shortlist",
        description: "Sharp onboarding-doc filter (iter-443 closing iter-442 user-probe-148 MIXED-corpus content-thin finding). Returns Doc rows where role='doc' AND kind in {explanation, how-to, tutorial} — the diátaxis quadrants that suit onboarding. Subset of [[non-ontology-docs]] (iter-441). Per iter-442 example-app: returns example-app-readme (explanation) + contributing (how-to) + ~3 more — actionable 3-5 doc starting set for the agent's recipe. Use AFTER corpus-purpose-classifier returns MIXED or CONTENT-RICH.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-443: session-context docs filter.
    SavedQuery {
        name: "session-context-docs",
        description: "Session-continuity doc filter (iter-443 sibling of [[onboarding-doc-shortlist]]). Returns Doc rows whose id contains 'session' / 'handoff' / 'next' / 'resume' — the cross-session context docs that AI agents (esp. Claude Code) reach for when resuming work. Per iter-442 example-app: handoff-to-original-session + next-session both surface high in BM25 onboarding queries because they describe the latest session state. Agent calls when user signals 'resuming previous work' OR 'where did we leave off'.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter-479 D (closing iter-478 Finding C entity-
    // def-vs-workflow gap): on ADOPTION-BRANCH corpora
    // with rich ontology (≥0.5 entities/doc), Doc-axis
    // BM25 surfaces entity-* / value-* / axis-* DOCS but
    // not narrative how-to docs (cf. spacy-llm iter-478
    // probe: entity-llm 4.13 + entity-task 3.11 + entity-
    // builtin-task 3.00 at top-3). Agent needs explicit
    // primitives to (1) LIST the ontology-meta docs +
    // (2) classify a corpus by ontology-vs-narrative
    // split.
    SavedQuery {
        name: "ontology-doc-shortlist",
        description: "Ontology-meta-doc lister (iter-479 closing iter-478 Finding C entity-def-vs-workflow gap). Returns Doc rows whose role is ontology-entity / ontology-axis / ontology-value — the entity-definition / value-definition / axis-definition docs. These are the docs Doc-axis BM25 tends to surface on rich-ontology adoption corpora (per iter-478 spacy-llm probe top-3 = entity-llm + entity-task + entity-builtin-task). Sibling of [[onboarding-doc-shortlist]] (narrative-side) for direct routing of entity-def-vs-workflow user questions. Use AFTER Doc-axis BM25 surfaces entity-* hits to CONFIRM which docs are definitions vs which are how-to.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-479: ontology-vs-narrative ratio
    // classifier. Quick 1-call probe answering "is this
    // a rich-ontology corpus?" — the threshold for the
    // iter-478 entity-def-vs-workflow recipe.
    SavedQuery {
        name: "ontology-vs-narrative-doc-ratio",
        description: "Ontology-vs-narrative doc ratio classifier (iter-479 sibling of [[ontology-doc-shortlist]], iter-481 widened filter). Returns ontology_doc_count + narrative_doc_count + ontology_pct + ontology_routing_label. Ontology bucket includes ontology-entity / ontology-axis / ontology-value / ontology-migration / index (iter-481 widened per [[user-probe-175]] under-count finding — initial iter-479 filter missed migration + index meta-docs). Label thresholds: ≥70% ontology → 'rich-ontology-doc-axis-finds-defs'; 30-70% → 'mixed'; <30% → 'narrative-doc-axis-finds-howtos'. Per iter-480 cross-corpus probe: spacy 100% + spacy-llm 100% + example-app 88.8% all collapse to 'rich-ontology' on the 3-trusted set. Use [[narrative-doc-list]] (iter-481) to enumerate the TRUE narrative subset.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-481 D closing iter-480 V under-count
    // finding: the TRUE narrative subset is role='doc' only.
    // Per cross-corpus probe: spacy 0 + spacy-llm 0 +
    // example-app 10. Sharper than the inverse of [[ontology-
    // doc-shortlist]] which leaves ambiguity on edge roles
    // (index/migration/feature/gap).
    SavedQuery {
        name: "narrative-doc-list",
        description: "True narrative-doc lister (iter-481 sibling of [[ontology-doc-shortlist]] + [[ontology-vs-narrative-doc-ratio]]). Returns Doc rows where role='doc' — the SHARP narrative subset (excluding all ontology-* + index + ontology-migration meta-docs). Per [[user-probe-175]] cross-corpus probe: spacy 0 + spacy-llm 0 + example-app 10. Useful when [[ontology-vs-narrative-doc-ratio]] returns 'rich-ontology' label and agent wants to know whether there are ANY narrative docs at all to fall back on (spacy/spacy-llm = NONE → routes to Function-axis exclusively).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter-445 (closing iter-444 user-probe-149
    // detector-gap finding): iter-424 cli-subcommand-
    // pattern-entities only catches 12 verb prefixes
    // (debug-/init-/train-/validate-/convert-/apply-/
    // package-/assemble-/evaluate-/run-/build-/deploy-).
    // iter-444 surfaced benchmark- + profile- as missed
    // CLI verbs. Two sibling refinements:
    //
    // 1. cli-subcommand-pattern-entities-v2: extended
    //    prefix list adding benchmark-/profile-/serve-/
    //    show-/list-/check-/info-/find-/clean-/upload-/
    //    download-/install-/update-. SUPERSEDES iter-424.
    //
    // 2. functions-in-cli-module: Function-axis CLI
    //    detector. Returns Functions whose symbol
    //    contains '.cli.' or '/cli/' — broader signal
    //    that doesn't depend on entity-id-prefix
    //    matching.
    SavedQuery {
        name: "cli-subcommand-pattern-entities-v2",
        description: "Extended CLI-subcommand sub-classifier (iter-445 closing iter-444 user-probe-149 detector-gap finding; SUPERSEDES [[cli-subcommand-pattern-entities]] iter-424). Returns cluster-derived entities whose id starts with broader CLI-verb prefix list: debug-/init-/train-/validate-/convert-/apply-/package-/assemble-/evaluate-/run-/build-/deploy-/benchmark-/profile-/serve-/show-/list-/check-/info-/find-/clean-/install-/update-. Per iter-444 spacy: benchmark-speed + profile both surface in this v2 list. Sibling of [[functions-in-cli-module]] (iter-445) for broader CLI detection via Function-axis.",
        params: &[],
        needs: &["Entity"],
    },
    // Sibling per iter-445: Function-axis CLI detector.
    // Path-based signal catches CLI subcommands regardless
    // of entity-id-prefix matching (per iter-444:
    // 'profile' single-word noun misses iter-424
    // prefix-only detection; but profile() function
    // lives in spacy.cli.profile module).
    SavedQuery {
        name: "functions-in-cli-module",
        description: "Function-axis CLI detector (iter-445 sibling of [[cli-subcommand-pattern-entities-v2]]). Returns Functions whose symbol CONTAINS '.cli.' (Python module convention) or '/cli/' (file-path convention). Broader signal than entity-id-prefix matching — catches CLI subcommands like 'profile' (single-word noun) that iter-424 prefix-list misses. Per iter-444 spacy: profile() lives in spacy.cli.profile module. Use COMBINED with iter-421 v2 PATTERN classifier: if PATTERN + matched in this list → CLI subcommand sub-class; if PATTERN + not in this list → real extension target or utility cluster.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter-448 (loop-maturity composite per iter-410
    // pattern): the loop now exposes 9-axis probe routing
    // + corpus-purpose classifier + extensive sub-
    // classifiers. Agents need a cold-start summary
    // primitive that orients them to a corpus in a single
    // call. Two sibling cold-start composites:
    //
    // 1. corpus-cold-start-summary: 1-row corpus shape
    //    diagnostic. Returns total_docs +
    //    total_entities + total_files + total_functions
    //    + corpus_purpose label. Single comprehensive
    //    primitive for the agent's first call on a new
    //    corpus.
    //
    // 2. corpus-entity-shape-summary: counts cluster-
    //    derived vs PROMOTED entities + count by density
    //    bucket. Sibling diagnostic for entity-axis shape.
    SavedQuery {
        name: "corpus-cold-start-summary",
        description: "Single-call cold-start corpus diagnostic (iter-448 loop-maturity composite). Returns total_docs + total_entities + total_functions + total_files + corpus_purpose label (ADOPTION-BRANCH / CONTENT-RICH / MIXED). Agent's first call on a new corpus — orients to corpus shape + scale BEFORE issuing probe-shape queries. Composes [[corpus-purpose-classifier]] (iter-441) + 4 scalar counts in 1 row. Per iter-444/446/447 9-axis routing recipe: cold-start summary → then axis-routing-card-for-vocabulary-v2 with the probe term. SUPERSEDES manual orientation via separate count queries.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-448: entity-shape diagnostic.
    SavedQuery {
        name: "corpus-entity-shape-summary",
        description: "Entity-shape diagnostic (iter-448 sibling of [[corpus-cold-start-summary]]). Returns total_entities + cluster_derived_count + promoted_count + density-bucket distribution (FACT 0.0X + MID 0.1X + PATTERN 0.2X+). Per iter-421 v2 + iter-420 5-instance FACT/PATTERN: characterizes the entity axis's role distribution. Per iter-444 spacy: PATTERN density 0.67 → entity-benchmark-speed CLI subcommand; iter-411 spacy-llm Cache PATTERN density 0.67; iter-418 spacy language FACT density 0.01. Agent uses to gauge how many extension-target entities exist vs background data structures.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter-451 (closing iter-449/450 META-PROBE
    // findings): the cold-start recipe collapses search
    // space to 3-4 files + ~20 fns via the per-provider
    // folder convention + Function-axis class scope. Two
    // sibling primitives that complete the agent's
    // per-folder + per-class navigation:
    //
    // 1. files-under-folder-path: given a $folder_path
    //    substring (e.g. 'models/rest/anthropic'),
    //    returns all matching files. Per iter-450:
    //    surfaces 4-file per-provider folder cleanly.
    //
    // 2. methods-on-class-by-substring: given a
    //    $class_name (e.g. 'Anthropic' or 'LLMWrapper'),
    //    returns Function symbols containing
    //    `<ClassName>#` substring — class-method
    //    inventory. Sibling of [[types-by-name-substring]]
    //    (iter-431) at the method-axis. Per iter-450:
    //    surfaces Anthropic#__init__ + #_request + etc.
    SavedQuery {
        name: "files-under-folder-path",
        description: "Files under a folder path (iter-451 closing iter-450 META-PROBE per-provider folder convention 5th-instance). Given `$folder_path` substring (e.g. 'models/rest/anthropic'), returns matching files with LOC. Per iter-450 spacy-llm Anthropic: 4 files in dedicated per-provider folder. Per iter-415 task / iter-418 lang / iter-420 model: same per-folder convention. Use to confirm per-provider/per-task folder structure after axis-routing-card returns WORKFLOW route. Excludes tests via NOT CONTAINS 'test'.",
        params: &["folder_path"],
        needs: &["File"],
    },
    // Sibling per iter-451: per-class method inventory.
    SavedQuery {
        name: "methods-on-class-by-substring",
        description: "Class-method inventory (iter-451 sibling of [[files-under-folder-path]]; method-axis equivalent of [[types-by-name-substring]] iter-431). Given `$class_name` (e.g. 'Anthropic' / 'LLMWrapper' / 'Doc'), returns Function symbols containing `<class>#` substring — the class's method surface. Per iter-450 spacy-llm Anthropic: surfaces Anthropic#__init__ + #_request + per-version factories. Per iter-435 spacy-llm LLMWrapper: similar pattern. Excludes tests. Use AFTER files-under-folder-path identifies the class location; this enumerates its methods.",
        params: &["class_name"],
        needs: &["Function"],
    },
    // Per iter-454 (closing iter-453 user-probe-155
    // TEST-WRITING 10th-shape candidate finding): test-*
    // entity prefix is a meaningful corpus signal. spacy
    // has 9 test-* entity clusters indicating per-domain
    // test pattern groupings. Two sibling primitives:
    //
    // 1. test-entity-anchors: corpus-wide test-* entity
    //    inventory. Lets agent see all test pattern
    //    groupings available to mirror.
    //
    // 2. test-files-for-domain: given a $domain
    //    substring, return test files matching that
    //    domain. Bridges the test-entity to actual
    //    test-file location.
    SavedQuery {
        name: "test-entity-anchors",
        description: "Test pattern grouping inventory (iter-454 closing iter-453 user-probe-155 TEST-WRITING 10th-shape finding). Returns Entity rows with id STARTS WITH 'test-' — the corpus's test pattern clusters. Per iter-453 spacy: 9 test-* entities = per-domain test groupings (test-language, test-models, test-parse, test-rehearse, ...). Agent uses to enumerate available patterns to mirror when writing new tests. Each test-* entity's cluster anchor function is a canonical test example.",
        params: &[],
        needs: &["Entity"],
    },
    // Sibling per iter-454: test-file locator. Given a
    // $domain substring (e.g. 'pipeline' / 'tokens' /
    // 'cli'), returns test files under tests/<domain>/
    // path. Bridges test-entity to actual file location.
    SavedQuery {
        name: "test-files-for-domain",
        description: "Test-file locator (iter-454 sibling of [[test-entity-anchors]]). Given a `$domain` substring (e.g. 'pipeline' / 'tokens' / 'cli' / 'training'), returns test files under tests/<domain>/ or matching the domain substring. Per iter-453 spacy: spacy/tests/pipeline/test_ner.py + spacy/tests/tokens/test_doc.py etc. follow per-domain directory convention. Agent uses to find canonical test examples to mirror when writing new tests. Returns path + LOC.",
        params: &["domain"],
        needs: &["File"],
    },
    // Per iter-457 (closing iter-454-456 TEST-WRITING
    // STABLE 3-instance findings): TEST-WRITING agent
    // recipe needs to bridge test-* entity → canonical
    // test function/file. Two sibling navigation
    // primitives:
    //
    // 1. test-files-matching-test-entity: given a $test_
    //    entity_id (e.g. 'test-causal-chain'), find test
    //    files matching the analogous production name.
    //    example-app: test-causal-chain → tests/test_
    //    causal_chain.py.
    //
    // 2. functions-mentioning-test-entity: given a $test_
    //    entity_id, return Functions mentioning that
    //    entity via FUNCTION_MENTIONS — surfaces the
    //    canonical test pattern function as the agent's
    //    mirroring template.
    // Sibling per iter-457: function-axis bridge from
    // test-* entity to canonical test pattern function.
    SavedQuery {
        name: "functions-mentioning-test-entity",
        description: "Test-pattern function locator (iter-457). Given `$test_entity_id` (e.g. 'test-causal-chain'), returns Function symbols mentioning that test entity via FUNCTION_MENTIONS. The mentioning Functions are the canonical test examples to mirror. Per iter-454 + iter-456 TEST-WRITING recipe: pick test-* entity → run this query → read top Function as template → mirror structure for new test.",
        params: &["test_entity_id"],
        needs: &["Entity", "Function"],
    },
    // Per iter-459 (closing iter-458 user-probe-158
    // BROKEN-AT-SIGNAL finding): iter-457 functions-
    // mentioning-test-entity + test-files-matching-test-
    // entity both broken — test cluster entities have 0
    // FUNCTION_MENTIONS edges + the file query ignored
    // its param. Path-based fixes:
    //
    // 1. test-functions-by-stem: given $stem (without
    //    'test-' prefix), returns Function symbols
    //    containing 'test_<stem>' substring. Per iter-
    //    458 spacy: 'language' returns evil_component +
    //    nlp + assert_sents_error + warn_error + perhaps_
    //    set_sentences. SUPERSEDES iter-457 functions-
    //    mentioning-test-entity.
    //
    // 2. test-files-by-stem: given $stem, returns test
    //    files whose path contains 'test_<stem>'.
    //    SUPERSEDES iter-457 test-files-matching-test-
    //    entity (which ignored its param).
    SavedQuery {
        name: "test-functions-by-stem",
        description: "Path-based test-function locator (iter-459 closing iter-458 BROKEN-AT-SIGNAL finding; SUPERSEDES [[functions-mentioning-test-entity]] iter-457). Given `$stem` (the test-* entity's id without 'test-' prefix, e.g. 'language' for test-language entity), returns Function symbols containing 'test_<stem>' substring. Per iter-458 spacy: 'language' returns evil_component + nlp + assert_sents_error + warn_error + perhaps_set_sentences from spacy.tests.test_language. iter-457 functions-mentioning-test-entity was BROKEN at signal level — test cluster entities have 0 FUNCTION_MENTIONS edges by ingest design. This query bridges via file-path naming convention instead.",
        params: &["stem"],
        needs: &["Function"],
    },
    // Sibling per iter-459: path-based test-file locator.
    SavedQuery {
        name: "test-files-by-stem",
        description: "Path-based test-file locator (iter-459 sibling of [[test-functions-by-stem]]; supersedes the removed iter-457 `test-files-matching-test-entity`, which ignored its param). Given `$stem` (the test-* entity's id without 'test-' prefix, e.g. 'causal_chain' for test-causal-chain), returns test files whose path contains 'test_<stem>' substring. Per iter-456 example-app: 'causal_chain' returns tests/test_causal_chain.py. iter-457 v1 had a Cypher bug where the param was unused. Returns path + LOC ordered DESC.",
        params: &["stem"],
        needs: &["File"],
    },
    // Per iter-462 (loop-maturity composite per iter-410
    // pattern): TEST-WRITING recipe (iter-454-461) has
    // converged. Cold-start composite for the recipe:
    //
    // 1. test-writing-card: given $stem (e.g. 'cache'),
    //    returns axis-routing-style summary — count of
    //    test functions matching stem + count of test
    //    files. Single-call orientation for TEST-WRITING
    //    probes.
    //
    // 2. test-fixture-helpers-by-stem: given $stem,
    //    return Function symbols matching stem + '#_'
    //    pattern (private/underscore-prefixed) OR '/_'
    //    pattern — surfaces fixture helpers commonly
    //    named _snap / _init_nlp / _sc / _err per iter-
    //    461 example-app causal_chain test pattern.
    SavedQuery {
        name: "test-writing-card",
        description: "Cold-start composite for TEST-WRITING probes (iter-462 loop-maturity composite). Given `$stem` (test-* entity id minus prefix, e.g. 'cache'), returns scalar counts: test_function_count + test_file_count. Per iter-460 spacy-llm cache: 5 fns + 1 file. Per iter-461 example-app causal_chain: 5 fns + 1 file. Single-call orientation before calling [[test-functions-by-stem]] (iter-459) + [[test-files-by-stem]] (iter-459). Returns ABSENT-like (0+0) if test pattern doesn't exist.",
        params: &["stem"],
        needs: &[],
    },
    // Sibling per iter-462: fixture-helper detector.
    SavedQuery {
        name: "test-fixture-helpers-by-stem",
        description: "Test fixture-helper detector (iter-462 sibling of [[test-writing-card]]). Given `$stem`, returns test Function symbols matching stem AND containing underscore-prefix marker (`/_` or `#_`) — fixture helpers commonly named _snap / _init_nlp / _sc / _err / _cmd_co. Per iter-461 example-app causal_chain: 4 fixture helpers (_snap + _sc + _err + _cmd_co). Per iter-460 spacy-llm cache: _init_nlp helper. Agent reads fixture helpers FIRST when mirroring tests since they define the test data shape.",
        params: &["stem"],
        needs: &["Function"],
    },
    // Per iter-464 (closing iter-463 over-broad finding):
    // iter-462 test-fixture-helpers-by-stem catches BOTH
    // /_underscore module helpers AND #__dunder class
    // methods. Split into 2 narrower siblings:
    //
    // 1. test-private-function-helpers-by-stem: only
    //    `/_<name>()` module-level helpers. Per iter-461
    //    example-app pattern (_snap, _sc, _err, _cmd_co).
    //
    // 2. test-inner-class-helpers-by-stem: only `#__`
    //    dunder methods on inner test classes. Per iter-
    //    463 spacy pattern (WhitespaceTokenizer#__init__,
    //    CustomTokenizer#__call__).
    SavedQuery {
        name: "test-private-function-helpers-by-stem",
        description: "Narrower fixture-helper detector for module-level helpers (iter-464 closing iter-463 over-broad iter-462 finding). Given `$stem`, returns test Functions matching stem AND containing `/_` (module-level private helper) but NOT `#__` (excludes inner-class dunder methods). Per iter-461 example-app causal_chain pattern: _snap + _sc + _err + _cmd_co. Per iter-460 spacy-llm cache: _init_nlp. Sibling of [[test-inner-class-helpers-by-stem]] (iter-464) for the class-method variant.",
        params: &["stem"],
        needs: &["Function"],
    },
    // Sibling per iter-464: inner-class dunder method
    // detector.
    SavedQuery {
        name: "test-inner-class-helpers-by-stem",
        description: "Narrower fixture-helper detector for inner-class dunder methods (iter-464 sibling of [[test-private-function-helpers-by-stem]]). Given `$stem`, returns test Functions matching stem AND containing `#__` (dunder method on inner test class). Per iter-463 spacy 'language' pattern: WhitespaceTokenizer#__init__ + WhitespaceTokenizer#__call__ + CustomTokenizer#__init__ + CustomTokenizer#__call__ (inner test-only tokenizer classes). Excludes module-level `/_` private helpers.",
        params: &["stem"],
        needs: &["Function"],
    },
    // Per iter-467 (closing iter-466 framework-vs-app
    // fixture idiom 1-instance hypothesis): corpus-wide
    // diagnostics consolidating the TEST-WRITING recipe
    // recent findings.
    //
    // 1. test-fixture-idiom-classifier: corpus-wide
    //    classifier returning module_helper_count +
    //    inner_class_helper_count + idiom label
    //    (FRAMEWORK-MIXED / APP-MODULE-ONLY / EMPTY).
    //    Per iter-466: spacy + spacy-llm = FRAMEWORK-
    //    MIXED; example-app = APP-MODULE-ONLY.
    //
    // 2. corpus-test-shape-summary: comprehensive cold-
    //    start summary for the TEST-WRITING shape.
    SavedQuery {
        name: "test-fixture-idiom-classifier",
        description: "Corpus-wide test fixture idiom classifier (iter-467 closing iter-466 framework-vs-app fixture idiom hypothesis). Returns module_helper_count (Functions matching 'test_' + '/_' but NOT '#__') + inner_class_helper_count (Functions matching 'test_' + '#__' but NOT 'tests.test_') + idiom label. Per iter-466 3-corpus validation: spacy + spacy-llm = FRAMEWORK-MIXED; example-app = APP-MODULE-ONLY. Framework corpora demonstrate extension subclasses in tests via inner-class fixtures; app corpora use only module-level helpers. Combine with [[corpus-cold-start-summary]] (iter-448) for full corpus orientation.",
        params: &[],
        needs: &[],
    },
    // Sibling per iter-467: comprehensive test-shape
    // diagnostic.
    SavedQuery {
        name: "corpus-test-shape-summary",
        description: "Comprehensive TEST-WRITING shape diagnostic (iter-467 sibling of [[test-fixture-idiom-classifier]]). Returns 1 row with total_test_files + total_test_functions + module_helper_count + inner_class_helper_count + test_entity_count + test_fixture_idiom label. Single-call orientation for TEST-WRITING workflow. Per iter-453 spacy: 9 test entities + 352 test files. Per iter-455 spacy-llm: 4 + 40. Per iter-456 example-app: 13 + 21.",
        params: &[],
        needs: &[],
    },
    // Per iter-470 (TEST-WRITING recipe convergence
    // milestone per iter-469): consolidates the 2 cold-
    // start composites (iter-448 + iter-467) into a
    // single unified diagnostic. Agent's first call on
    // a new corpus gets BOTH corpus-shape AND test-shape
    // in 1 row.
    //
    // 1. unified-corpus-diagnostic: comprehensive cold-
    //    start with corpus_purpose + 4 main counts +
    //    test_fixture_idiom + 4 test counts. Combines
    //    corpus-cold-start-summary + corpus-test-shape-
    //    summary.
    //
    // 2. function-axis-token-classifier: given $term,
    //    classify Function symbols matching it by shape
    //    (class-method / module-level / private). Useful
    //    for understanding Function-axis composition.
    SavedQuery {
        name: "unified-corpus-diagnostic",
        description: "Unified cold-start diagnostic (iter-470 TEST-WRITING recipe convergence milestone). Returns 10 fields in 1 row: corpus_purpose label + total_docs + total_entities + total_functions + total_files + test_fixture_idiom + total_test_files + total_test_functions + module_helper_count + inner_class_helper_count. Combines [[corpus-cold-start-summary]] (iter-448) + [[corpus-test-shape-summary]] (iter-467) into a single comprehensive cold-start primitive. Agent's first call on a new corpus gets BOTH corpus shape + test shape in 1 query.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-470: Function-axis composition
    // classifier.
    SavedQuery {
        name: "function-axis-token-distribution-for-stem",
        description: "Function-axis composition diagnostic (iter-470 sibling of [[unified-corpus-diagnostic]]). Given `$stem`, returns counts of Function symbols matching the stem by shape: class_method_count (CONTAINS '#'), private_count (CONTAINS '/_'), test_count (CONTAINS 'test_'). Useful for understanding Function-axis result composition before drilling into specific symbols. Per iter-469 spacy-llm 'cache': mix of class methods + private + test fns.",
        params: &["stem"],
        needs: &[],
    },
    // Per iter-473 (loop-convergence cold-start ratios):
    // ship 2 corpus-shape ratio primitives that surface
    // testing density + documentation density. Useful
    // health-check signals beyond the 4-way signature.
    SavedQuery {
        name: "corpus-test-coverage-ratio",
        description: "Test coverage ratio diagnostic (iter-473 loop-convergence cold-start). Returns total_functions + test_function_count + test_function_pct (test_function_count * 100 / total_functions). Per iter-468/469/471 actual data: spacy 2196/3890=56%; spacy-llm 320/778=41%; example-app 673/904=74%. App corpora (example-app) tend toward higher ratios (test-heavy); framework corpora tend toward lower ratios (prod-heavy). Useful health-check signal for spotting under-tested corpora.",
        params: &[],
        needs: &["Function"],
    },
    // Iter-476 completes the 4-ratio cold-start stack
    // (joining iter-473's 2 ratios). Per iter-475 cross-
    // corpus probe: entities/doc reveals an ORTHOGONAL
    // axis (ontology depth, NOT app-shape) — spacy 0.76
    // tops the list despite being framework-shaped.
    SavedQuery {
        name: "corpus-entities-per-doc-ratio",
        description: "Ontology-depth ratio diagnostic (iter-476 4-ratio cold-start sibling). Returns total_docs + total_entities + entities_per_doc_ratio. Per iter-475 actual data: spacy 0.76 (69/91); spacy-llm 0.51 (23/45); example-app 0.58 (52/89). ORTHOGONAL axis to test/doc/fns app-vs-framework signals — measures ONTOLOGY DEPTH not app-shape. spacy tops despite being framework (rich domain via init). Sibling to [[corpus-test-coverage-ratio]] + [[corpus-doc-coverage-ratio]] + [[corpus-functions-per-file-ratio]] forming 4-ratio cold-start composite.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling per iter-476: functions-per-file ratio
    // diagnostic. Per iter-475 STRONGEST single-ratio
    // app-vs-framework signal (example-app 16.74 outlier).
    SavedQuery {
        name: "corpus-functions-per-file-ratio",
        description: "Function-density ratio diagnostic (iter-476 4-ratio cold-start sibling). Returns total_functions + total_files + functions_per_file_ratio. Per iter-475 actual data: spacy 4.38 (3890/889); spacy-llm 4.60 (778/169); example-app 16.74 (904/54). STRONGEST single-ratio app-vs-framework signal — apps concentrate logic per file (example-app 16.74 outlier); frameworks spread it across many small files (spacy/spacy-llm clustered 4.4-4.6). Sibling to [[corpus-test-coverage-ratio]] + [[corpus-doc-coverage-ratio]] + [[corpus-entities-per-doc-ratio]] forming 4-ratio cold-start composite.",
        params: &[],
        needs: &["Function"],
    },
    // Sibling per iter-473: doc coverage ratio diagnostic.
    SavedQuery {
        name: "corpus-doc-coverage-ratio",
        description: "Doc coverage ratio diagnostic (iter-473 sibling of [[corpus-test-coverage-ratio]]). Returns total_docs + total_files + docs_per_file_ratio (total_docs / total_files). Per iter-470 actual: spacy 91/889=0.10; spacy-llm 45/169=0.27; example-app 89/54=1.65. App corpora (example-app) tend toward higher ratios (doc-heavy); framework adoption corpora tend toward lower ratios (code-heavy). Useful for spotting under-documented corpora.",
        params: &[],
        needs: &["Doc"],
    },
    // Sibling to cluster-derived-entities (iter-377): the
    // explicit list of PROMOTED entities (manually-curated
    // descriptions). Per iter-376 [[user-probe-106]] +
    // iter-378 [[feedback_promoted_vs_cluster_entity_
    // descriptions]] 2-instance: promoted entities don't
    // support the 4th recipe component (no 'Surfaces
    // around X' pointers). Agents who need the PROMOTED
    // subset for semantic-bridge route use this query.
    SavedQuery {
        name: "promoted-entities",
        description: "Entities whose description does NOT start with 'Cluster anchored on' — the explicit list of PROMOTED (manually-curated) entities. Sibling of [[cluster-derived-entities]] (iter-377). Per iter-376 + iter-378 [[feedback_promoted_vs_cluster_entity_descriptions]] 2-instance: PROMOTED descriptions are semantic narrative, not 'Surfaces around X' pointers. The iter-373 4th recipe component does NOT apply to promoted entities. Agents on mostly-promoted corpora (example-app, design corpus) iterate this list to get the semantic disambiguation content for their query terms. ORDER BY mention_count DESC. LIMIT 30.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 385 [[user-probe-112]] Finding A + E:
    // PROMOTED entities split into 2 sub-styles by whether
    // their description contains a navigational pointer
    // (function/identifier name in backticks). Heuristic
    // detection: backtick character presence in the
    // description.
    //
    // - PROMOTED-with-pointer: description contains
    //   backtick (e.g., rule entity 'emitted as a flag
    //   by `run_diagnostics`'). iter-373 4th recipe
    //   component APPLIES — agent dereferences the
    //   identifier to find the implementation.
    // - PROMOTED-pure-semantic: description has no
    //   backtick (e.g., tier entity 'Pricing band of
    //   hosted offering'). 4th recipe DOES NOT apply
    //   — agent uses semantic-bridge.
    //
    // The corpus-level promoted-with-pointer ratio tells
    // agents whether to default to 4th recipe or
    // semantic-bridge for promoted entities on this
    // corpus.
    SavedQuery {
        name: "promoted-with-pointer-entities",
        description: "PROMOTED entities (description does NOT start with 'Cluster anchored on') whose description ALSO contains a backtick character — a heuristic for the PROMOTED-with-pointer sub-class per iter-385 [[user-probe-112]] Finding A. Backtick presence indicates the curator included an inline-code identifier (e.g., rule entity's 'emitted as a flag by `run_diagnostics`') that the iter-373 4th recipe component can dereference. Sibling to [[promoted-entities]] (iter-380) + [[cluster-derived-entities]] (iter-377). Agent recipe: enumerate this list to find PROMOTED entities where 4th recipe still applies; the COMPLEMENT (promoted without backtick) is PROMOTED-pure-semantic where semantic-bridge is the right route. ORDER BY mention_count DESC. LIMIT 30. CAVEAT: backtick heuristic has false positives (backticks can wrap non-identifiers like config keys) and false negatives (curators may include identifiers without backticks).",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 385 corpus-level diagnostic: refines iter-
    // 377 corpus-entity-promotion-state with a sub-
    // breakdown of promoted entities. Agents pick recipe
    // routing per corpus shape:
    // - mostly-promoted-with-pointers → default to 4th
    //   recipe component for promoted entities too.
    // - mostly-promoted-pure-semantic → default to
    //   semantic-bridge for promoted entities.
    // - mostly-cluster-derived → 4th recipe primarily;
    //   sub-breakdown still informs the promoted minority.
    SavedQuery {
        name: "promoted-entity-pointer-breakdown",
        description: "1-row sub-breakdown of PROMOTED entities by pointer-presence heuristic — refines iter-377 [[corpus-entity-promotion-state]] with promoted_with_pointer_count + promoted_pure_semantic_count + categorical promoted_sub_label ('promoted-mostly-with-pointers' / 'promoted-mostly-pure-semantic' / 'promoted-mixed' / 'no-promoted'). Per iter-385 [[user-probe-112]] Findings A + E: 4th recipe component applicability isn't binary on cluster-vs-promoted — PROMOTED-with-pointer (backtick-containing description) also supports it. This breakdown tells agents whether to default to 4th recipe or semantic-bridge for the promoted subset of a corpus. Composes with corpus-entity-promotion-state for full routing diagnostic.",
        params: &[],
        needs: &["Entity"],
    },
    SavedQuery {
        name: "corpus-entity-axis-shape",
        description: "1-row shape diagnostic for the Entity-axis — returns total_entity_count + max_mention_count + median_mention_count proxy (sum/count) + categorical entity_axis_shape ('entity-axis-empty' / 'entity-axis-sparse' / 'entity-axis-rich'). Per iter-371 [[user-probe-103]]: identity-card recipe relies on entity-axis when tag-axis is empty (3rd-instance adoption-branch finding). This query confirms entity-axis density BEFORE the agent runs domain-specific BM25. Threshold calibration: ≤10 entities = empty (starter scaffold only); ≤50 entities = sparse (single-axis-narrative corpora); > 50 = rich (mature ontology). Sibling: corpus-tag-axis-shape (Doc.tags) + corpus-doc-starter-ratio (Doc-axis).",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 168 (interrogation-033 Finding A —
    // corpus-content lag detection primitive): return the
    // top-10 research-* Docs ordered by `updated` DESC.
    // Companion to recent-research (existing corpus-local
    // query, role-specific) but portable and explicitly
    // tied to the absorption-recency view. The agent can
    // detect "is the latest research-doc I expect actually
    // ingested?" in one call.
    SavedQuery {
        name: "recent-research-by-update",
        description: "Top 10 research-* Docs ordered by updated DESC. Use to detect corpus-content lag (per interrogation-033 Finding A — the 3rd axis of build-queue-lag): compare the latest absorbed research-doc on disk against the top of this list; if the disk-newest doesn't appear, the corpus is mid-ingest. Sibling: docs-by-update-recency (all-Docs version) + files-by-recency-desc (file-axis).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 180 (user-probe-042 Finding D — NEW
    // stale-doc-low-inbound prioritization pattern): JOIN
    // staleness ASC with inbound WIKILINK count so the agent
    // can distinguish HIGH-PRIORITY-LOW-IMPACT refreshes
    // (stale + low inbound — fix one doc, doesn't ripple)
    // from HIGH-IMPACT-COORDINATED-REWRITE candidates (stale
    // + high inbound — many references will need updating).
    // user-probe-042 surfaced doc-linter-usage at 21 days
    // stale + 1 inbound = the canonical low-impact refresh.
    SavedQuery {
        name: "stale-docs-with-impact",
        description: "Narrative Docs ordered by `updated` ASC, with inbound WIKILINK count co-projected for impact assessment. Combines iter-165's stale-narrative-docs filter (exclude ontology-* roles) with iter-149's doc-fanin (inbound-WIKILINK count). Surfaces both axes in 1 call so the agent can prioritize: stale + LOW inbound = quick-win refresh; stale + HIGH inbound = wait-for-coordinated-rewrite. Per the user-probe-042 NEW pattern. EXCLUDES `kind = ''` per interrogation-036 Finding A — empty-kind ontology-meta docs (ontology-mig-* / ontology-index) slipped through the role-based filter and surfaced as the 'stalest'. LIMIT 20.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 180 (sibling to stale-docs-with-impact —
    // freshness aggregated by Doc.kind so the agent can spot
    // which CATEGORY of doc is systematically stale): if
    // `reference` docs are ASC older than `how-to` docs the
    // reference layer is rotting; if `explanation` is
    // newest the loop is currently in deep-dive mode. The
    // aggregated min/max/count surface lets the agent draw
    // cross-kind conclusions in 1 call.
    SavedQuery {
        name: "freshness-by-kind",
        description: "Narrative Doc freshness aggregated by `kind` — per-kind min/max/count of `updated` timestamps. Use when asking 'which CATEGORY of doc is most stale?' (e.g., reference vs how-to). Excludes ontology-* roles per feedback_starter_ontology_false_positives AND `kind = ''` per interrogation-036 Finding B — empty-string kinds are NOT NULL but ARE meaningless aggregation buckets (typically ontology-meta docs). Composes with stale-docs-with-impact for the per-doc drill-down view.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 165 (user-probe-037 Findings A + D —
    // documentation-freshness workflow): rank narrative
    // Docs by `updated` ASC so the agent surfaces the
    // STALEST docs first. EXCLUDES `role` IN
    // ['ontology-value', 'ontology-axis', 'ontology-entity']
    // because starter-ontology values legitimately don't
    // need re-dating when code changes — per
    // feedback_starter_ontology_false_positives. Filters
    // for `updated IS NOT NULL` because Doc.updated is
    // optional on some pipelines.
    SavedQuery {
        name: "stale-narrative-docs",
        description: "Narrative Docs ranked by `updated` ASC — stalest first. EXCLUDES ontology-* roles (their staleness is structurally OK; they don't track code timestamps per feedback_starter_ontology_false_positives). Pre-filter for the documentation-freshness workflow surfaced in user-probe-037. Pair with files-by-recency-desc to see whether the staleness is real (compare doc.updated vs the most-recent code touch in the same window). Empty result IS architectural signal per feedback_uniformly_fresh_corpus_pattern — the loop's own design corpus has every doc updated within a 2-day window because of continuous editing.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 165 (sibling to stale-narrative-docs and
    // files-by-recency-{desc,asc} from iter 147 —
    // doc-axis recency primitive): cold-start "show me
    // the most-recently updated Docs" view, mirror of the
    // file-axis version. Useful when the agent needs to
    // know "what docs has the user touched recently?" —
    // e.g., for a freshness comparison or to surface
    // current focus areas. NO ontology-* filter here
    // because the desc-order view IS used for "current
    // activity" probes where ontology updates ARE
    // signal.
    SavedQuery {
        name: "docs-by-update-recency",
        description: "All Docs (role-agnostic) ordered by `updated` DESC — the most-recently touched first. Doc-axis analog of files-by-recency-desc (iter 147). Use for current-focus / activity probes; pair with files-by-recency-desc to see whether doc + code updates align. Does NOT filter ontology-* roles because for current-activity probes those updates ARE signal.",
        params: &[],
        needs: &["Doc"],
    },
    SavedQuery {
        name: "loc-pair-clone-candidates",
        description: "File pairs with similar LOC (±20), same language, both hand-written, both ≥ 100 LOC — the LOC-pair clone-detection fallback when Function-signature dedup is empty (Py+TS case per user-probe-035). Excludes .gen.* / test paths per the generated-code-visibility framework. Composes with `files-coupled-to` (git-coupling, ORTHOGONAL signal per feedback_orthogonal_clone_signals): same-LOC pairs surface independent-but-similar clones (write-once-then-copy); coupled-files surface co-evolving clones. Both for comprehensive clone detection.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 161 (sibling of loc-pair-clone-candidates —
    // same clone-detection-fallback shape, different axis):
    // when two files share a basename across directories,
    // they're often related clones (template + customization,
    // generic + specialised, mirror packages across mono-repo
    // halves). On full-stack-fastapi-template this surfaces
    // routes/_layout/items.tsx ↔ routes/items.tsx style pairs.
    // Pure path-axis clone signal, orthogonal to both
    // LOC-pair (size-similarity) and git-coupling
    // (co-evolution).
    SavedQuery {
        name: "same-name-cross-dir-clones",
        description: "File pairs that share the same basename across different directories — the path-axis clone-detection fallback. Surfaces template+customization, generic+specialised, and mirror-package clones that LOC-pair and git-coupling axes both miss. Third axis in the orthogonal-clone-signals framework (per feedback_orthogonal_clone_signals): LOC-pair finds size-similar clones, coupled-files finds co-evolving clones, this finds name-shared clones. Excludes generated and test paths.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 159 (user-probe-035 Findings A + B —
    // gap-function-mentions-py-ts-empty +
    // gap-fastapi-endpoint-detection-empty): the natural
    // perf-workflow primitives (hub-functions /
    // query_endpoints) returned 0 corpus-wide on the
    // trusted FastAPI corpus. The agent had NO way to
    // detect the empty-backing-table failure mode until
    // it called the dependent saved query and got 0 rows
    // back — at which point it can't distinguish
    // "no hot functions" from "Function table empty for
    // this language". This diagnostic primitive surfaces
    // the Function-row count broken down by language so
    // the agent can sanity-check BEFORE calling
    // hub-functions / functions-in-file etc. Sibling of
    // language-distribution (which aggregates File rows
    // by language) but on the Function axis.
    SavedQuery {
        name: "function-count-by-language",
        description: "Function rows grouped by language — diagnostic for empty-backing-table detection per user-probe-035. When a language's count is 0, hub-functions / functions-in-file / tests-in-file will be empty corpus-wide for that language regardless of the saved query's filter. Use this as a 1-call cold-start sanity check before assuming hub-functions has data. Sibling of language-distribution (File axis); both compose to a per-language ingest-coverage view.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 193 (user-probe-045 Finding C — 17th
    // workflow shape API surface analysis): operationalises
    // the CONTAINS-based public-API filter that worked on
    // spacy. Excludes test scaffolding + constructors +
    // underscore-prefixed (Python's private convention). On
    // spacy this surfaces spacy/util.py (107 public funcs),
    // spacy/language.py (73), token.pyi / doc.pyi type stubs.
    // For non-Python corpora the underscore-prefix
    // heuristic is a no-op; the test-exclude is still
    // useful. Caveat: doesn't exclude generated code per
    // feedback_generated_code_visibility — compose with
    // hand-written-source-files (iter 153) for that filter.
    SavedQuery {
        name: "public-api-functions",
        description: "Functions filtered to the public API surface — excludes test files (path CONTAINS '/tests/' or 'test_'), constructors (symbol CONTAINS '__init__'), and underscore-prefixed names (Python's private convention). Returns top-25 files by public-function count. On spacy surfaces spacy/util.py (107) + language.py (73) + type stubs. Caveat: doesn't exclude generated code — compose with the iter-153 generated-files exclusion. Sibling of iter-184's architectural-layers-by-path for the per-layer view.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 193 (sibling to public-api-functions — the
    // intersection of hub-functions density with public-API
    // surface, the architectural-backbone primitive). On
    // spacy returns SpanRuler.__init__ + EntityRuler.__init__
    // + SpanCategorizer.__init__ at the top — but those ARE
    // __init__'s, which the public-api-functions query
    // excluded. Per user-probe-045 Finding F: in OO-Python,
    // the constructor IS the public API, so this query
    // KEEPS __init__ (counter-direction to public-api-
    // functions). The two queries serve different shapes of
    // the public-API question.
    SavedQuery {
        name: "public-api-hub-functions",
        description: "Hub-functions intersected with public-API filter — surfaces the architectural backbone (most-mentioned non-private non-test Functions, INCLUDING constructors). Counter-direction to public-api-functions: KEEPS __init__ because in OO-Python the constructor IS the public API. On spacy returns SpanRuler.__init__ (11 mentions) + EntityRuler (10) + SpanCategorizer (7) + CLI entries. Compose with public-api-functions for the per-file vs per-symbol views.",
        params: &[],
        needs: &["Entity", "Function"],
    },
    // Per iter 189 (interrogation-037 NEW 3rd diagnostic
    // — Function.signature COLUMN population on Python
    // ingest is empty even when rows ARE populated; 2-
    // instance confirmed on spacy + example-app per the
    // iter-185 caveat memory): extends the iter-159
    // function-count + function-mentions-by-language
    // diagnostic trio to a quartet. The agent can now
    // distinguish 3 sub-causes of Function-axis
    // emptiness:
    //   1. Walker missed → language-distribution = 0
    //   2. Rows ingested but mentions absent →
    //      function-count > 0 but function-mentions = 0
    //   3. Rows + mentions populated but signature
    //      column empty (THIS query) →
    //      function-count > 0 AND function-mentions > 0
    //      but function-signature-population = 0%
    SavedQuery {
        name: "function-signature-population-by-language",
        description: "Per-language Function row count + populated-signature count + percent — diagnoses Function.signature COLUMN population gap (interrogation-037 finding: 100% empty on spacy+example-app's Python ingest). Sibling to iter-159's function-count-by-language + function-mentions-by-language; together the 3 queries form a quartet for the 3-sub-cause taxonomy of feedback_natural_primitives_can_be_corpus_empty. Use when Function-signature dedup returns 0 rows to distinguish 'empty by ingest design' (THIS query returns 0% across the board) from 'empty by data shape' (returns some > 0 entries).",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 159 (sibling of function-count-by-language
    // on the edge axis — same root surfacing pattern as
    // user-probe-035 Finding A): hub-functions ranks
    // Functions by FUNCTION_MENTIONS edge count. If the
    // edge table is empty for a language (Py / TS in the
    // trusted corpus), hub-functions returns 0 even when
    // Function rows ARE populated. This diagnostic groups
    // FUNCTION_MENTIONS edges by the source Function's
    // language so the agent can detect "Functions
    // ingested but mentions-edges absent" — the specific
    // failure mode user-probe-035 quantified.
    SavedQuery {
        name: "function-mentions-by-language",
        description: "FUNCTION_MENTIONS edges grouped by the source Function's language — diagnostic for `hub-functions returns empty` detection. When count is 0 for a language, hub-functions will be empty for that language even if Function rows exist (the edges are populated by a separate ingest pass that's currently Rust-only). Sibling of function-count-by-language; together they distinguish 'Function table empty' (use language-distribution to sanity-check) from 'Functions present, mentions absent' (the user-probe-035 Py+TS case).",
        params: &[],
        needs: &["Entity", "Function"],
    },
    // Per iter 151 (interrogation-030 Finding C —
    // gap-workflow-validations-umbrella, symmetric to
    // iter-141's architectural-patterns gap that surfaced
    // on a different content axis): the loop has validated
    // 7+ distinct workflow shapes via V probes 019-031,
    // each tagged with a `*-workflow` tag. Individual
    // workflow-tags exist; no umbrella. This saved query
    // mirrors iter-141's architectural-patterns: returns
    // docs tagged with ANY of the named workflow-shape
    // tags as a single corpus-wide call.
    SavedQuery {
        name: "workflow-validations",
        description: "Catalog of loop-validated workflow shapes across V probes — docs tagged with any of: install-setup-workflow, churn-analysis-workflow, pre-commit-workflow, security-review-workflow, review-workflow, refactor-workflow. Each row is a user-probe documenting the workflow's primitive set + synthesis. Mirror of architectural-patterns (positive patterns) + retrieval-failure-modes (failure modes) + retrieval-mitigation-modes (mitigations) — together they cover the loop's accumulated content axes.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 151 (sibling — META-VALIDATION axis):
    // patterns whose validity has been confirmed across
    // MULTIPLE corpora (not just one). Surfaces probes
    // tagged with `cross-corpus-validation` — the empirical
    // evidence stratum that distinguishes "interesting
    // observation" from "validated principle" (per
    // feedback_absence_as_signal's 5-instance threshold).
    SavedQuery {
        name: "cross-corpus-validated-patterns",
        description: "Docs documenting patterns confirmed across MULTIPLE corpora — tagged with `cross-corpus-validation`. The meta-validation axis: patterns surface here only after a 2nd or 3rd corpus reproduction (per the loop's evidence-strength threshold). Distinguishes 'interesting observation' from 'validated principle'. Cf. feedback_absence_as_signal (5-instance cross-corpus validation) + interrogation-029 (16-iteration durability).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 149 (user-probe-031 Finding F + memory
    // feedback_hub_doc_fallback): operationalises the
    // hub-doc-fallback mitigation pattern (4th mitigation
    // after the iter-125 three-mitigation framework) as a
    // queryable primitive. Promotes the design-corpus-local
    // doc-fanin.toml to compile-time portable so every
    // doc-linter install has the hub-doc discovery primitive.
    // Ranks Docs by WIKILINK inbound count — hubs sit at the
    // top.
    SavedQuery {
        name: "doc-fanin",
        description: "Docs ranked by WIKILINK inbound count — the hub-doc discovery primitive. Top of the ranking is the architecturally central content an end-user agent should read first. Operationalises the hub-doc-fallback mitigation: when BM25 retrieval fails, query the corpus's hub docs directly via this query, then query_doc the top entry. Sibling to leaf-docs (zero-inbound).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 149 (sibling — leaf docs as the inverse of
    // hub docs): Docs with ZERO inbound WIKILINK + MD_LINK +
    // INFORMED_BY edges are orphaned. May be authored-but-
    // forgotten OR genuinely intended as standalone reference.
    // Useful for curation audits: leaf docs may need
    // explicit cross-references added OR retirement.
    SavedQuery {
        name: "leaf-docs",
        description: "Docs with ZERO inbound doc-to-doc edges (no WIKILINK, MD_LINK, or INFORMED_BY references) — the leaf / orphaned doc detector. Useful for ontology-curation audits: leaf docs may need cross-references added (recover discoverability), tag fixes (per tag-axis-loose-ends), or retirement. Sibling to doc-fanin (the hub side).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 222 (interrogation-045 Finding C +
    // [[feedback_authoring_time_backlink_discipline]] memory
    // promotion): storyline-specific variant of leaf-docs.
    // Surfaces Docs carrying any registered storyline-family
    // tag AND having zero inbound WIKILINK edges. The
    // recurring pattern (iter-206 + iter-208 + iter-221) is
    // that newly-authored storyline docs commit FORWARD
    // links to predecessors but rarely include the BACKWARD
    // update on the predecessor doc. This query catches the
    // missed backlinks at corpus scale.
    SavedQuery {
        name: "storyline-leaf-detector",
        description: "Docs carrying a known storyline-family tag (currently `ingest-pass-storyline` — extend the WHERE clause to add new families) AND with ZERO inbound WIKILINK edges. Per iter-222 [[feedback_authoring_time_backlink_discipline]] memory the recurring 3-instance pattern is that newly-authored storyline docs miss backlinks from predecessors. This query surfaces the missed-backlink cases at corpus scale. Composes with `leaf-docs` (the generic version — this is the role-aware storyline-specific variant). Empty result = all storyline docs have inbound backlinks (healthy). Non-empty = candidates for predecessor-doc updates.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 222 (interrogation-045 Finding D — taxonomy
    // maturity check companion): generic recent-leaf detector.
    // Returns Docs with ZERO inbound WIKILINK ordered by
    // updated DESC. Surfaces the most-recently-authored
    // unreferenced docs FIRST — the typical case where the
    // authoring-time backlink discipline missed an update.
    // Companion to leaf-docs (which orders by doc-id and
    // checks 3 edge types); this orders by recency to
    // prioritize fresh authoring drift over historical
    // orphans.
    SavedQuery {
        name: "recent-leaf-docs",
        description: "Docs with ZERO inbound WIKILINK ordered by `updated` DESC (most recent first). Surfaces newly-authored docs whose predecessor wasn't updated with a backlink per [[feedback_authoring_time_backlink_discipline]]. Use this AFTER a Type R / I / V iteration to catch the missed-backlink case before it accumulates. Companion to leaf-docs (which checks 3 edge types + orders by doc-id) and storyline-leaf-detector (the family-tag-filtered variant). LIMIT 15.",
        params: &[],
        needs: &["Doc"],
    },
    SavedQuery {
        name: "files-by-recency-desc",
        description: "Most-recently-changed files (top 10) — the churn-analysis hot-side primitive. Returns path + last_touched + loc, ordered by last_touched DESC. Useful for 'where is the team investing effort right now?' / new-contributor orientation / picking up an active thread. Compose with functions-in-file($path) on a hot file to drill into the API surface that was touched. Sibling: files-by-recency-asc for stale-file detection.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 147 (sibling — stale-file detector):
    // the time-axis primitive for "which files have
    // STOPPED moving?" Useful for spotting either
    // (a) feature-complete stable abstractions OR
    // (b) neglected/parked code. The same primitive
    // gives both answers; the agent disambiguates via
    // doc/coupling context.
    SavedQuery {
        name: "files-by-recency-asc",
        description: "Stalest files (top 10) — the churn-analysis stale-side primitive. Returns path + last_touched + loc, ordered by last_touched ASC. Useful for spotting feature-complete-stable-abstractions OR neglected/parked code. The empty-coupled-files signal (per feedback_absence_as_signal) helps disambiguate: stale + zero-coupling = well-encapsulated leaf; stale + heavy-coupling = neglected hotspot. Sibling: files-by-recency-desc for hot-file detection.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 139 (user-probe-027 Finding C —
    // NEW test-co-location pattern named): the reviewer
    // workflow's "show me the tests for this file" question
    // misses Rust's `#[cfg(test)] mod tests` idiom because
    // tests live INSIDE the production .rs file. The right
    // primitive is function-symbol filtering on the
    // `tests/` SCIP-symbol segment. Closes the
    // discoverability gap user-probe-027 quantified
    // (10 in-file tests at lines 497-709 evaded a
    // path-CONTAINS test query). Sibling to
    // functions-in-file (which returns ALL functions); the
    // subset projection is useful as ACI — clearer agent
    // intent when the question is specifically about tests.
    SavedQuery {
        name: "tests-in-file",
        description: "Test functions defined within the given `$path` — filters functions-in-file by SCIP symbol CONTAINS 'tests/'. Catches Rust's #[cfg(test)] mod tests idiom (tests co-located in production .rs files), Go's TestX functions, similar in-file test conventions. Returns symbol + line + doc_comment ordered by line. Pairs with functions-in-file (returns all) and `untested-functions` (the complement at corpus scale). Empty result IS architectural signal per feedback_test_co_location: either no tests, or test functions live in separate _test.rs files outside this path — try files-coupled-to or path-substring search instead.",
        params: &["path"],
        needs: &["Function"],
    },
    // Per iter 139 (sibling to tests-in-file): for
    // module-level overviews the agent often wants ALL
    // functions in a subtree, not just one file. This
    // query operates on a directory prefix
    // (e.g. `src/cmd/`) and returns every Function whose
    // file path starts with the prefix. Useful for the
    // "what's in this module?" cold-start question.
    SavedQuery {
        name: "functions-in-directory",
        description: "Functions defined in any file under the given `$prefix` directory path — module-level overview. Projects symbol + file + line + signature so the agent sees the module's complete function surface, grouped by file. Composes with files-by-path-substring($pattern) for keyword-scoped discovery + functions-in-file($path) for single-file drill-down.",
        params: &["prefix"],
        needs: &["Function"],
    },
    // Per iter 137 (user-probe-026 Finding C —
    // navigation-budget gap for file→function drill-down):
    // file-level queries (files-by-path-substring +
    // files-coupled-to from iter 135) give the agent the WHAT
    // but not the WHERE-WITHIN. With 25 functions in a
    // 1021-LOC file like src/embeddings.rs, the agent needs
    // a per-file enumeration primitive. This query closes the
    // navigation gap.
    SavedQuery {
        name: "functions-in-file",
        description: "Functions defined in the given `$path` — projects symbol + line + doc_comment + signature so the agent can navigate WITHIN a file the file-axis primitives surfaced. Compose: files-by-path-substring($pattern) → pick a file → functions-in-file(that path). Useful for refactor planning (how many functions are in this module? what's the API surface?). Sibling to types-in-file.",
        params: &["path"],
        needs: &["Function"],
    },
    // Per iter 137 (user-probe-026 sibling): the Type-axis
    // counterpart to functions-in-file. Refactor planning
    // often wants both — for a Rust module file we want to
    // see fn signatures AND the struct/enum/trait
    // definitions. Same shape, different node table.
    SavedQuery {
        name: "types-in-file",
        description: "Types (structs / enums / traits / classes) defined in the given `$path` — projects symbol + kind + line + doc_comment + signature so the agent sees the type surface alongside functions-in-file. Compose: files-by-path-substring($pattern) → pick a file → types-in-file(that path). The type kind axis (struct vs enum vs trait) is in the `kind` column, useful for filtering.",
        params: &["path"],
        needs: &["Type"],
    },
    SavedQuery {
        name: "files-by-path-substring",
        description: "Files whose path CONTAINS the given `$pattern` substring — the keyword-vocabulary file-discovery primitive. Useful when a refactor target maps to path tokens (auth, payments, login, etc.). Composes with `files-coupled-to($path)` to surface the blast radius around each hit. Returns one row per file with path, language, loc.",
        params: &["pattern"],
        needs: &["File"],
    },
    // Per iter 371 + 370 + 368 (inverse-axis-asymmetry
    // 3-shape pattern from [[feedback_inverse_axis_asymmetry]]):
    // file-axis path queries silently UNDERFLOW on .pyi
    // (iter-368) and OVERFLOW on .py with sparse Function
    // ingest (iter-370). Agents that trust only file-axis
    // miss implementation files. Sibling primitive
    // function-files-by-pattern surfaces DISTINCT fn.file
    // matches from the Function-axis — the orthogonal
    // fallback. Together with files-by-path-substring,
    // agents take UNION client-side for a complete
    // pattern hit-set.
    SavedQuery {
        name: "function-files-by-pattern",
        description: "DISTINCT fn.file values where fn.file CONTAINS the given `$pattern` substring — Function-axis sibling of files-by-path-substring. Use when file-axis underflows for a pattern (the .pyi case from iter-368 user-probe-101) OR when file-axis is rich but you want the function-count-per-file as a richness signal. Returns one row per file with fn_count. Per [[feedback_inverse_axis_asymmetry]] 3-shape pattern: union with files-by-path-substring for a complete hit-set.",
        params: &["pattern"],
        needs: &["Function"],
    },
    // Per iter 371 [[user-probe-103]] Finding C + iter 370
    // Finding C + iter 368 Finding D: corpus-axis-
    // completeness varies by (file-extension × SCIP-
    // language-coverage). 3 sub-shapes observed across the
    // external trusted corpora. Agents need a 1-call
    // fingerprint that exposes both axis counts AND a
    // categorical label so they pick the right routing
    // immediately. Threshold: 4 files = 'rich' (matches
    // example-app detector ecosystem cardinality; spacy-llm
    // sharding had 4 file-axis matches; spacy matcher 1
    // file-axis but 6 fn-axis files). Sibling primitive to
    // corpus-shape-classifier (Doc-axis) +
    // corpus-ingest-state-classifier (per-pass).
    SavedQuery {
        name: "path-axis-completeness-fingerprint",
        description: "For a path-substring `$pattern`, returns the file-axis count + Function-axis distinct-file count + total Function count + a categorical axis_completeness_label (one of: 'both-rich' / 'file-rich-fn-sparse' / 'file-sparse-fn-rich' / 'both-sparse'). Per [[feedback_inverse_axis_asymmetry]] 3-shape pattern (iter-368 .pyi sparse-rich / iter-370 .py rich-sparse / iter-371 example-app BOTH-rich): 1-call diagnostic for picking the right axis. Agents use the label to decide whether file-axis OR Function-axis OR BOTH route should drive their probe. Threshold: 4 files = 'rich' (calibrated to example-app detector ecosystem at iter-371).",
        params: &["pattern"],
        needs: &["File"],
    },
    // Per iter 382 [[user-probe-110]] Finding A + iter-369
    // walker fix half-closes Cython invisibility: when a
    // pattern's `path-axis-completeness-fingerprint` returns
    // 'both-sparse', the agent can't tell whether the keyword
    // is a NARROW concept (iter-373 factory case) or a
    // CYTHON-IMPLEMENTATION-INVISIBLE case (iter-382 parser
    // case, where _parser_internals/*.pyx is invisible to
    // both axes). This Cython-aware sibling fingerprint
    // adds the .pyx/.pxd file count + a refined label
    // ('cython-implementation-detected') so the agent
    // recognizes the Cython case and falls back to reading
    // .pyx files directly via Bash. Calibration: iter-382
    // spacy 'parser' → file_axis=2 + fn_axis_distinct=2 +
    // cython_count=N (post-iter-369-rebuild) →
    // 'cython-implementation-detected'.
    SavedQuery {
        name: "path-axis-fingerprint-cython-aware",
        description: "Cython-aware sibling of [[path-axis-completeness-fingerprint]]. For a path-substring `$pattern`, returns the file-axis count + Function-axis distinct-file count + total Function count + cython_file_count (.pyx + .pxd matches) + a categorical cython_aware_label. The label is 'cython-implementation-detected' when cython_file_count >= 1 (regardless of other axis counts — Cython invisibility dominates the agent's recipe). Otherwise falls through to the standard 4-shape label ('both-rich' / 'file-rich-fn-sparse' / 'file-sparse-fn-rich' / 'both-sparse'). Per iter-382 [[user-probe-110]] Finding A: agent recipe for Cython-implementation-detected cases is to read .pyx files directly via Bash/Read since Function-axis is structurally blind to Cython (SCIP-Python limitation, not doc-linter limitation). Sibling: [[cython-files-by-pattern]] (iter-271) enumerates the .pyx/.pxd files when cython_file_count >= 1.",
        params: &["pattern"],
        needs: &["File"],
    },
    // Per iter 382 [[user-probe-110]] Finding D — clusters
    // UNDER-COUNT Cython-heavy entities because the cluster
    // pass aggregates only SCIP-visible functions. Agents
    // who see an entity description claiming 'density > 0.5'
    // but only 2-4 functions should check whether the actual
    // implementation is in Cython (invisible). This boolean
    // check answers that in 1 call.
    SavedQuery {
        name: "cython-presence-by-pattern",
        description: "1-row boolean-ish check: does the `$pattern` path-substring have ANY .pyx or .pxd file matches? Returns cython_file_count + has_cython ('yes' / 'no'). Per iter-382 [[user-probe-110]] Finding D: agents who see a cluster entity description claiming 'density > 0.5' but only 2-4 functions can use this query to detect whether the actual implementation is in Cython (where SCIP-Python is structurally blind). If 'yes', the entity's true function count is HIGHER than the cluster reports — Cython functions don't show in Function-axis but the .pyx files DO show in file-axis (post-iter-369 walker fix). Simpler diagnostic than [[path-axis-fingerprint-cython-aware]] when only the yes/no signal is needed.",
        params: &["pattern"],
        needs: &["File"],
    },
    // Per iter 269 (operationalises [[user-probe-064]]
    // Finding E recipe at the COMPOSED-DIAGNOSTIC layer):
    // pattern-matching files + their coupling-degree in 1
    // call. The pre-iter-269 workflow required N+1 round
    // trips: run files-by-path-substring → for each result
    // run files-coupled-to. This composite returns both
    // surfaces atomically. Per user-probe-064 the auth flow
    // probe needed paths AND degrees to identify login.tsx
    // as the hub.
    SavedQuery {
        name: "files-by-path-pattern-with-coupling",
        description: "Files matching `$pattern` PLUS their COUPLED_WITH neighbor count (degree) + total co-change commits — 1-call composite of [[files-by-path-substring]] + [[file-coupling-degree]]. Per [[user-probe-064]] Finding E: on doc-sparse code-repo corpora the agent's workflow is 'find files matching keyword + identify which is the coupling hub'. This composite returns BOTH in 1 query. Files with no coupling edges surface with degree=0 (preserved via OPTIONAL MATCH). LIMIT 50.",
        params: &["pattern"],
        needs: &["File"],
    },
    // Per iter 269 (companion at the per-language split
    // axis): pattern-matching files grouped by language with
    // file counts + LOC totals. Per [[user-probe-064]] the
    // auth-flow probe surfaced a FRONTEND-HEAVY pattern (7
    // tsx files at 13-189 LOC + 1 backend py file at 123 LOC);
    // this query exposes the per-language split in 1 row
    // per language so the agent sees the structural balance
    // without manually grouping files-by-path-substring
    // results.
    SavedQuery {
        name: "files-by-pattern-language-split",
        description: "Per-language file count + total LOC + max LOC for files matching `$pattern`. Per [[user-probe-064]] Finding E: on doc-sparse code-repo corpora the agent's structural question 'where does X live, frontend vs backend?' gets answered by this split. On a FastAPI+React template's auth question this query would emit (tsx, 7 files, 819 LOC) + (typescript, 3 files, 255 LOC) + (python, 2 files, 314 LOC), revealing the frontend-vs-backend balance immediately. NULL/0-LOC files excluded.",
        params: &["pattern"],
        needs: &["File"],
    },
    SavedQuery {
        name: "functions-by-symbol-pattern",
        description: "Functions whose `symbol` CONTAINS the given `$pattern`, with test-path filtering. Per [[user-probe-065]] Finding D: Function-axis BM25 on test-rich corpora (spacy: 3890 functions, top-10 mostly test_tokenizer_*) is biased toward fixture vocabulary. This query gives the agent an IMPLEMENTATION-ONLY view via cypher-side substring filter + `NOT f.file CONTAINS 'tests/'` + `NOT f.symbol CONTAINS 'tests/'` per iter-225 [[feedback_test_for_sparse_universal]] convention. On spacy with `$pattern='tokenizer'` should surface spacy.tokenizer.Tokenizer methods + Vocab-related functions instead of test_tokenizer_*. LIMIT 50.",
        params: &["pattern"],
        needs: &["Function"],
    },
    // Per iter-487 D closing iter-486 V Finding A
    // (cohesive-module 2-instance PROMOTED) + Finding B
    // (stub-only-Cython 1-instance regime): composite
    // classifier returning impl_count + internal/external
    // callee split + cohesion_label. Routes the iter-486
    // 3-regime classification (cohesive / dependent /
    // stub-only-cython) in 1 call.
    SavedQuery {
        name: "module-cohesion-classifier",
        description: "Module cohesion classifier (iter-487 → iter-490 7-label refinement per [[user-probe-184]] sub-classification 2-instance + 'leaf-with-deps' 1-instance hypothesis). Given `$pattern`, returns impl_count + internal_count + external_count + external_pct (0-100 share) + cohesion_label (7-way). Labels: 'empty' (no impls), 'stub-only-cython' (impls but 0 callees — Cython invisibility), 'cohesive' (0 external — example-app causal_chain, spacy.language.Language), 'leaf-with-deps' (impl_count≤2 AND internal=0 — example-app.report 1/0/3 + example-app.session 2/0/1), 'mostly-cohesive' (external_pct ≤ 10 — spacy_llm.cache 13/13/1=7%), 'balanced-dependent' (external_pct 11-67), 'heavily-dependent' (external_pct > 67 — spacy_llm.tasks.ner 13/3/19=86%; spacy_llm.pipeline.llm 15/3/12=80%). Division-by-zero protected via internal+external=0 short-circuit. Test-filtered on targets per iter-271. Companion to [[functions-by-symbol-pattern]] + [[function-callees-of-symbol-pattern]] (iter-484) + [[module-external-callees-by-pattern]] (iter-487).",
        params: &["pattern"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "file-language-by-path",
        description: "Returns the indexed language for a specific `$file_path` (iter-524 closing [[user-probe-218]] iter-523 V mixed-visibility finding). On freshly-ingested spacy with `$file_path='spacy/tokenizer.pyx'` should return language='cython'. With `$file_path='spacy/vocab.pyx'` also 'cython'. With pure-Python `$file_path='spacy/language.py'` returns 'python'. Agent uses this to PER-QUESTION route between cypher iter-484 (succeeds on python) and filesystem-find (required for cython). Complements [[corpus-workflow-viability-classifier]] (iter-518) which is corpus-level.",
        params: &["file_path"],
        needs: &["File"],
    },
    SavedQuery {
        name: "function-call-count-between-files",
        description: "Scalar count of CALLS edges from functions in `$caller_file` to functions in `$callee_file` (iter-520 closing [[user-probe-214]] iter-519 V cypher-composition-blind-spot hypothesis). On spacy-llm with `$caller_file='spacy_llm/pipeline/llm.py'` + `$callee_file='spacy_llm/cache.py'` should return call_count=0 (composition relationship per iter-519). On example-app with `$caller_file='example_app/report.py'` + `$callee_file='example_app/causal_chain.py'` likely returns >0 (call-based relationship). 1-row scalar pre-check for composition-vs-CALLS diagnosis.",
        params: &["caller_file", "callee_file"],
        needs: &["Function"],
    },
    // Per iter-518 D closing iter-517 V hypothesis 2-
    // instance promotion. Composite classifier returning
    // narrative_doc_count + cython_file_count +
    // viability_label. Routes the agent's pre-probe
    // decisions per [[feedback_workflow_success_predicted_
    // by_narrative_doc_count]] STABLE 2-instance memory.
    SavedQuery {
        name: "corpus-workflow-viability-classifier",
        description: "Pre-probe corpus classifier (iter-518 closing [[user-probe-212]] iter-517 V hypothesis 2-instance promotion). Returns narrative_doc_count + cython_file_count + viability_label (1 row): 'architecture-viable' if narrative >= 10; 'pure-python-narrative-sparse' if narrative < 10 + cython = 0 (cypher fallback works); 'cython-blocked' if narrative < 10 + cython > 0 (DOUBLE FAILURE per [[user-probe-212]] iter-517). On example-app (narrative=10 + cython=0) → 'architecture-viable'. On spacy-llm (0 + 0) → 'pure-python-narrative-sparse'. On spacy STALE (0 + 0 in graph but 47 .pyx on FS) → 'pure-python-narrative-sparse' per graph BUT actual is 'cython-blocked' — see [[user-probe-209]] for FS cross-check.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter-516 D closing iter-515 V workflow-success
    // observation: BM25 query_similar ranks by relevance
    // but doesn't EXACT-MATCH phrases. Sometimes agent
    // needs to find docs containing a specific phrase like
    // 'deprecated' or 'detected by' — exact substring on
    // Doc.summary is the right tool. Per [[feedback_
    // workflow_success_predicted_by_narrative_doc_count]]
    // 1-instance hypothesis: narrative-rich corpora benefit
    // most from this primitive.
    SavedQuery {
        name: "docs-by-summary-substring",
        description: "Docs whose `summary` CONTAINS the given `$substring` (iter-516 closing [[user-probe-210]] iter-515 V workflow-success observation). Exact-substring match — complements BM25 query_similar which RANKS by relevance but doesn't filter to exact-phrase containment. On example-app with `$substring='detected by'` should return entity-cuda-error + other entity-defs whose summary contains the phrase (per iter-515 V finding). Useful for relationship-document lookups (\"which entities are 'detected by' X?\"). Returns id + title + summary. Test-filtered. LIMIT 30.",
        params: &["substring"],
        needs: &["Doc"],
    },
    // Per iter-513 D closing iter-512 V stale-graph
    // detection gap. Without these, the agent cannot
    // diagnose whether stub-only-Cython routing recipe
    // (per iter-486 V + [[feedback_function_signature_
    // column_empty_py]]) applies to the current corpus.
    SavedQuery {
        name: "file-language-distribution",
        description: "Per-language File count distribution (iter-513 closing [[user-probe-207]] stale-graph detection gap). Returns rows of (language, file_count) sorted by count DESC. On spacy (post-reingest with walker iter-369 .pyx support) should return python ~700 + cython ~50 + maybe yaml/toml. ON STALE spacy graph (per iter-512 V): only python rows; cython MISSING despite Cython usage in actual codebase. Lets agent DIAGNOSE stub-only-Cython recipe applicability — if cython count is 0 but Python files exist for a known Cython codebase, graph is stale. Test-filtered. LIMIT 25.",
        params: &[],
        needs: &["File"],
    },
    // Sibling per iter-513: cython-specific count for
    // direct stub-only-Cython recipe pre-check.
    SavedQuery {
        name: "cython-file-count",
        description: "Scalar count of .pyx + .pxd Cython files in File table (iter-513 sibling of [[file-language-distribution]]). Per [[user-probe-207]] iter-512 V finding: spacy graph has 0 .pyx files despite spacy using Cython extensively for tokenizer/matcher/etc. If count is 0 on a known-Cython codebase, graph is STALE per pre-iter-369 walker; agent should fall back to filesystem `find <root> -name '*.pyx'`. Cheap 1-row probe.",
        params: &[],
        needs: &["File"],
    },
    // Per iter-510 D: file-size sort variants of iter-497
    // [[files-by-path-prefix]]. Returns (path, impl_count)
    // sorted by impl_count DESC (biggest) or ASC (smallest
    // non-empty). Directly enables the "pick a mirror
    // target" external-corpus user-probe pattern used in
    // iter-503/509: when there are multiple plugin-
    // template instances of varying sizes (openai
    // registry.py 25 impls vs cohere registry.py 2),
    // agent picks the canonical (largest) OR minimal
    // (smallest) instance to copy.
    SavedQuery {
        name: "files-by-path-prefix-impl-count-desc",
        description: "Files under `$path_prefix` sorted by impl_count DESC (iter-510 sibling of [[files-by-path-prefix]] iter-497). Surfaces the BIGGEST modules first — canonical mirror targets for new plugins. On spacy-llm with `$path_prefix='spacy_llm/models/rest/'` should return openai/registry.py at top (25 impls per iter-503). On spacy with `$path_prefix='spacy/lang/en/'` should return lemmatizer.py / lex_attrs.py / syntax_iterators.py at top (each 1 impl) — the small per-language adapter shape. Test-filtered. LIMIT 30.",
        params: &["path_prefix"],
        needs: &["File"],
    },
    // Sibling per iter-510: ASC variant for minimal mirror
    // targets. Surfaces files with smallest non-zero
    // impl_count first; data-only files (impl_count=0)
    // bubble up first which may or may not be useful —
    // filter applied post-fetch by agent if needed.
    SavedQuery {
        name: "files-by-path-prefix-impl-count-asc",
        description: "Files under `$path_prefix` sorted by impl_count ASC (iter-510 sibling of [[files-by-path-prefix-impl-count-desc]]). Surfaces SMALLEST modules first — minimal mirror targets. On spacy-llm with `$path_prefix='spacy_llm/models/rest/cohere/'` should return cohere/registry.py with impl_count 2 (per iter-503 cohere is the minimal provider). Data-only files (impl_count=0) appear first; agent can filter post-fetch. Test-filtered. LIMIT 30.",
        params: &["path_prefix"],
        needs: &["File"],
    },
    // Per iter-508 D: scalar count companion to iter-506
    // [[files-by-basename-and-path-prefix]]. Cheap pre-
    // check before LIMIT-50 enumeration.
    SavedQuery {
        name: "files-by-basename-and-path-prefix-count",
        description: "Scalar count of files matching BOTH `$basename` (ENDS WITH) AND `$path_prefix` (STARTS WITH) (iter-508 sibling of [[files-by-basename-and-path-prefix]]). Per [[feedback_corpus_one_shape_preference]] STABLE 3-instance: agent uses this to confirm VERTICAL-template instances scoped to a specific subtree (e.g. basename='lemmatizer.py' + path_prefix='spacy/lang/' → 6+) BEFORE running the LIMIT-50 list query. Cheap 1-row probe.",
        params: &["basename", "path_prefix"],
        needs: &["File"],
    },
    SavedQuery {
        name: "files-by-basename-and-path-prefix",
        description: "Files matching BOTH `$basename` (ENDS WITH) AND `$path_prefix` (STARTS WITH) (iter-506 closing [[user-probe-200]] 2-shape taxonomy). Useful for confirming plugin-template instances are scoped to a specific subtree. On spacy with `$basename='lemmatizer.py'` + `$path_prefix='spacy/lang/'` returns 6+ per-language adapters; with `$path_prefix='spacy/pipeline/'` returns the single pipeline lemmatizer.py base class. On example-app with `$basename='detector.py'` + `$path_prefix='example_app/'` returns the 3 horizontal-variant detector files. Companion to [[files-by-basename]] (horizontal — any subtree) + [[files-by-path-prefix]] (any basename in subtree). Test-filtered.",
        params: &["basename", "path_prefix"],
        needs: &["File"],
    },
    SavedQuery {
        name: "files-by-basename",
        description: "All files whose path ENDS WITH `$basename` regardless of cohesion shape (iter-504 closing [[user-probe-198]] Finding C — leaf-with-deps filter misses LARGE plugin templates). On spacy with `$basename='lemmatizer.py'` should return 6+ spacy/lang/<lang>/lemmatizer.py rows including .py files of any size. On spacy-llm with `$basename='registry.py'` should return 4+ spacy_llm/models/<provider>/registry.py rows (azure/openai/cohere/langchain — openai has 25 impls so doesn't satisfy [[leaf-with-deps-files-by-basename]] filter). Complements iter-493 leaf-with-deps-files-by-basename which filters by cohesion shape; this enumerates ALL repeated-basename instances. Test-filtered. LIMIT 50.",
        params: &["basename"],
        needs: &["File"],
    },
    // Sibling per iter-504: scalar count companion to
    // [[files-by-basename]]. Cheap pre-check.
    SavedQuery {
        name: "files-by-basename-count",
        description: "Scalar count of files whose path ENDS WITH `$basename` (iter-504 sibling of [[files-by-basename]]). Per [[feedback_plugin_template_requires_framework_shape]] 3-instance memory: count >= 3 indicates plugin-template pattern regardless of cohesion shape (catches LARGE templates like openai/registry.py that fail leaf-with-deps filter). Cheap 1-row probe used BEFORE the LIMIT-50 list query.",
        params: &["basename"],
        needs: &["File"],
    },
    // Per iter-502 D closing iter-501 V Findings B+C
    // (within-label continuum on tight-family modules +
    // external_pct > label as routing field). Per-file
    // cohesion classification sorted by external_pct
    // DESC — surfaces the most-dependent → least-dependent
    // spectrum within a family at a glance.
    SavedQuery {
        name: "per-file-cohesion-by-path-prefix",
        description: "Per-file cohesion classification under `$path_prefix` (iter-502 closing [[user-probe-196]] Finding C — external_pct ranking is the right routing field within tight-family modules). Returns one row per file with impl_count + internal_count + external_count + external_pct + cohesion_label, sorted by external_pct DESC. On spacy with `$path_prefix='spacy/pipeline/'` should return all Python pipeline modules ranked from most-dependent (lemmatizer ~68%) → mid (span_ruler ~52%) → least-dependent (spancat ~20%). Test-filtered. LIMIT 30. Companion to [[module-cohesion-classifier]] (iter-490, single-module focus) — this query surfaces the WITHIN-FAMILY spectrum.",
        params: &["path_prefix"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "per-file-cohesion-by-path-prefix-asc",
        description: "Per-file cohesion classification under `$path_prefix` sorted by external_pct ASC (iter-502 sibling of [[per-file-cohesion-by-path-prefix]] which sorts DESC). Surfaces the LEAST-dependent module at top — useful when agent wants the most self-contained shape to mirror for a new component. On spacy with `$path_prefix='spacy/pipeline/'` should return spancat (~20% external) first then climb to lemmatizer (~68%). Returns same fields as DESC variant but flipped ordering. Test-filtered. LIMIT 30.",
        params: &["path_prefix"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "files-by-path-prefix",
        description: "Files whose path STARTS WITH `$path_prefix`, with Function impl_count per file (iter-497 closing [[user-probe-191]] Finding A — plugin-template has 4 sub-shapes, iter-490 leaf-with-deps catches only 1). On spacy with `$path_prefix='spacy/lang/en/'` should return all 8 template files including data-only (examples/punctuation/stop_words/tokenizer_exceptions.py — impl_count=0) AND leaf-with-deps (lemmatizer/syntax_iterators — impl_count=1). Companion to [[leaf-with-deps-files-by-path-prefix]]: this returns the FULL directory enumeration; that returns only the leaf-with-deps slice. Agent calls BOTH to mirror the complete template. Test-filtered. LIMIT 50.",
        params: &["path_prefix"],
        needs: &["File"],
    },
    // Sibling per iter-497: corpus-wide template-directory
    // survey. For each directory containing >=1 leaf-with-
    // deps file, count total files + leaf-with-deps files
    // + impl_count sum. Surfaces 'template directories'
    // distinctly.
    SavedQuery {
        name: "template-directory-survey",
        description: "Per-directory aggregation: total files + leaf-with-deps files (iter-497 sibling of [[files-by-path-prefix]]). Returns directories under `$path_prefix` containing at least 1 leaf-with-deps file, with both counts. Useful 1-call corpus-wide template enumeration. On spacy with `$path_prefix='spacy/lang/'` should return many <lang> subdirs each with total_files=6-9 + leaf_with_deps_files=2 (lemmatizer + syntax_iterators). Per [[user-probe-191]]: total_files > leaf_with_deps_files indicates the rest are data-only or no-call sub-shapes. Returns parent_dir (substring up to last '/'), but Kuzu lacks reverse-find so we group by FULL file_path then aggregate client-side — actually simpler: returns per-FILE rows with the template flag, agent groups. KEY: provides leaf_with_deps_marker boolean per file so agent can identify which subset to clone.",
        params: &["path_prefix"],
        needs: &["File"],
    },
    // Per iter-495 D companion to memory promotion
    // [[feedback_plugin_template_requires_framework_shape]]
    // (3-instance): scalar 1-row count companion to
    // [[leaf-with-deps-files-by-basename]] for cheap
    // "is this basename a plugin-template?" check. If
    // count >= 3 → likely plugin-template.
    SavedQuery {
        name: "leaf-with-deps-count-by-basename",
        description: "Scalar count of leaf-with-deps files matching `$basename` (iter-495 sibling of [[leaf-with-deps-files-by-basename]]). Per [[feedback_plugin_template_requires_framework_shape]] 3-instance memory: count >= 3 indicates plugin-template pattern. On spacy with `$basename='lemmatizer.py'` should return 6-12. On spacy-llm with `$basename='run_pipeline.py'` should return 8. On example-app with any basename should return 0-2 (NEGATIVE case). Cheap 1-row probe used BEFORE the LIMIT-50 list query to predict whether enumeration is worth running.",
        params: &["basename"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "leaf-with-deps-files-by-basename",
        description: "Leaf-with-deps files whose path ENDS WITH the given `$basename` (iter-493 sibling of [[leaf-with-deps-files]]). Per [[user-probe-187]] plugin-template 2-instance: on spacy with `$basename='lemmatizer.py'` should return 8-12 spacy/lang/<lang>/lemmatizer.py rows; with `$basename='syntax_iterators.py'` should return 10+ rows. On spacy-llm with `$basename='run_pipeline.py'` should return 8 usage_examples/<task_provider>/run_pipeline.py rows. Confirms plugin-template hypothesis on a SPECIFIC basename. Test-filtered.",
        params: &["basename"],
        needs: &["Function"],
    },
    // Sibling per iter-493: parent-prefix variant.
    SavedQuery {
        name: "leaf-with-deps-files-by-path-prefix",
        description: "Leaf-with-deps files whose path STARTS WITH the given `$path_prefix` (iter-493 sibling of [[leaf-with-deps-files]] + [[leaf-with-deps-files-by-basename]]). Per [[user-probe-186]] + [[user-probe-187]]: on spacy with `$path_prefix='spacy/lang/'` should return all per-language adapter files (15+ lemmatizer/syntax_iterators files). On spacy-llm with `$path_prefix='usage_examples/'` should return 8 run_pipeline.py demo scripts. Confirms plugin-template hypothesis on a SPECIFIC subtree. Test-filtered. Useful when agent suspects a directory contains a plugin template.",
        params: &["path_prefix"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "leaf-with-deps-files",
        description: "Per-file leaf-with-deps detector (iter-490 sibling of [[module-cohesion-classifier]]). For each file, returns impl_count + internal_count + external_count + the 'leaf-with-deps' filter: impl_count≤2 AND internal_count=0 AND external_count>0. Per [[user-probe-184]] example-app surface: example_app/report.py (1/0/3) + example_app/session.py (2/0/1) — pure entry-point modules to external services. Corpus-wide architectural overview: shows agents which files are 'thin wrappers' vs 'cohesive modules' vs 'dependent orchestrators'. Test-filtered. ORDER BY external_count DESC then impl_count ASC. LIMIT 50.",
        params: &[],
        needs: &["Function"],
    },
    // Sibling per iter-487: list the actual external
    // callees (the cross-file dependencies) for inspection
    // when classifier reports 'dependent'.
    SavedQuery {
        name: "module-external-callees-by-pattern",
        description: "External callees of Functions whose symbol matches `$pattern` — sibling of [[module-cohesion-classifier]]. Returns (target_symbol, callee_symbol, callee_file) for callees in DIFFERENT files than their targets (the cross-file dependency footprint). Useful after classifier reports 'dependent' to inspect what external modules the target depends on. Test-filtered on targets. LIMIT 100.",
        params: &["pattern"],
        needs: &["Function"],
    },
    SavedQuery {
        name: "function-callers-of-symbol-pattern",
        description: "Direct callers of Functions whose symbol matches `$pattern`, with test-path filtering on TARGETS (per iter-271 test-bias mitigation). Per iter-484 D closing [[user-probe-178]] Finding B (BM25/cypher complementarity): after [[functions-by-symbol-pattern]] surfaces implementation set, this query surfaces the CONSUMERS in 1 call. On spacy-llm with `$pattern='cache'`: targets = BatchCache# methods; callers should include LLMWrapper#__init__, set/get/persist consumers, test fixtures NOT filtered (callers may legitimately be tests). Returns (target_symbol, caller_symbol, caller_file). ORDER BY target + caller. LIMIT 100.",
        params: &["pattern"],
        needs: &["Function"],
    },
    // Sibling per iter-484: forward (callee) direction —
    // what do the implementations DEPEND on?
    SavedQuery {
        name: "function-callees-of-symbol-pattern",
        description: "Direct callees of Functions whose symbol matches `$pattern`, with test-path filtering on TARGETS. Sibling of [[function-callers-of-symbol-pattern]] (iter-484). Per iter-483 Finding B: after [[functions-by-symbol-pattern]] surfaces implementation set, this surfaces the DEPENDENCIES. On spacy-llm with `$pattern='cache'`: targets = BatchCache# methods; callees should include shelve operations, hashlib, pathlib utilities — the dependency footprint. Returns (target_symbol, callee_symbol, callee_file). ORDER BY target + callee. LIMIT 100.",
        params: &["pattern"],
        needs: &["Function"],
    },
    // Per iter 271 (closes [[user-probe-065]] Finding E —
    // Cython implementation invisible to Python SCIP ingest):
    // surfaces .pyx and .pxd Cython source files matching
    // the pattern. Per [[feedback_function_signature_column_empty_py]]
    // + the iter-270 tokenizer finding: Python SCIP ingest
    // doesn't analyse Cython, so the tokenizer engine in
    // spacy/tokenizer.pyx is invisible to Function-axis
    // queries. This query informs the agent that Cython code
    // exists by surfacing it via path-based File lookup.
    SavedQuery {
        name: "cython-files-by-pattern",
        description: "Cython source files (.pyx / .pxd) whose path CONTAINS the given `$pattern`. Per [[user-probe-065]] Finding E: Python SCIP ingest doesn't analyse Cython sources, so Cython implementation is INVISIBLE to Function-axis queries on Python corpora. This query informs the agent via File-axis lookup that Cython code exists for a given keyword — the agent learns to read the .pyx files directly rather than searching for them via query_similar type=function. On spacy with $pattern='tokenizer' should surface spacy/tokenizer.pyx + spacy/tokenizer.pxd (the actual tokenizer engine + its Cython header).",
        params: &["pattern"],
        needs: &["File"],
    },
    // Per iter 135 (user-probe-025 Finding B — targeted
    // blast-radius primitive): the corpus-wide
    // `coupled-files` query returns the ENTIRE jaccard
    // ranking. For a refactor against a SPECIFIC file the
    // agent wants ONLY that file's COUPLED_WITH neighbors —
    // not 29 rows over the whole corpus. This parameterised
    // query gives the targeted answer. Sibling to
    // files-by-path-substring (the file-discovery primitive
    // it composes with).
    SavedQuery {
        name: "files-coupled-to",
        description: "Files that historically co-change with the given `$path` (its COUPLED_WITH neighbors) — the targeted refactor-blast-radius primitive. Returns one row per coupled file with its path, jaccard score, commit count, and last co-change time. Composes with `files-by-path-substring($pattern)`: discover files by path, then compute each one's blast radius. Empty result IS architectural signal per the absence-as-signal pattern (well-encapsulated leaf module).",
        params: &["path"],
        needs: &["File"],
    },
    // Per iter 133 (interrogation-027 Finding A —
    // gap-research-by-tag-silent-empty): the corpus-local
    // `research-by-tag` filters to research-* docs only.
    // Agents asking the natural first-instinct question
    // ("research-by-tag(vocabulary-mismatch)") silently get
    // 0 rows even though the tag exists on docs of other
    // roles. This compile-time portable query is the
    // role-agnostic counterpart: return EVERY Doc carrying
    // `$tag`, regardless of role. Sibling to `roles-using-tag`
    // (the diagnostic primitive). Together they close the
    // discoverability gap interrogation-027 quantified.
    SavedQuery {
        name: "docs-by-tag",
        description: "Return EVERY Doc carrying the given `$tag`, regardless of role. Role-agnostic counterpart to research-by-tag (which filters to research-* docs) and audits-by-tag (which filters to audit/user-probe docs). Use this when the role-scoped queries return 0 — the tag may live on a different role than expected. Compose with `roles-using-tag` to discover which role(s) carry a given tag before calling docs-by-tag.",
        params: &["tag"],
        needs: &["Doc"],
    },
    // Per iter 133 (interrogation-027 Finding A — diagnostic
    // companion to docs-by-tag): when an agent's research-by-tag
    // or audits-by-tag returns 0 unexpectedly, this query
    // answers "which roles ACTUALLY carry this tag?" — the
    // missing diagnostic that would let the agent route to
    // the right role-scoped query (or fall back to docs-by-tag).
    SavedQuery {
        name: "roles-using-tag",
        description: "Diagnostic: which Doc roles carry the given `$tag`? Returns one row per (role, count) pair. Use when research-by-tag or audits-by-tag returns 0 unexpectedly — the result names which role-scoped query would have hit. Composes with docs-by-tag (the role-agnostic alternative).",
        params: &["tag"],
        // Use the projected alias `role` in ORDER BY because
        // `d` is out of scope after the aggregating RETURN
        // (per iter 143's syntax-validation test catching
        // the iter-133 binder bug).
        needs: &["Doc"],
    },
    // Per iter 209 (interrogation-042 + iter-204 cross-role-
    // storyline observation): corpus-wide tag-axis health
    // diagnostic. Emits a categorical label per tag based on
    // use count — same workflow-routing shape as iter-203
    // corpus-ingest-state-classifier + iter-187 corpus-shape-
    // classifier (1-row condensed + per-row categorical view).
    // Buckets: singleton (1 use), small-family (2-3),
    // mid-family (4-6), load-bearing (7+). Stopwords excluded
    // (research, paper, tool, ...). Lets agent see corpus
    // tag-axis health distribution in 1 call rather than
    // per-tag inspection.
    SavedQuery {
        name: "tag-axis-coverage-summary",
        description: "Corpus-wide tag distribution + categorical health label per tag. Categories: `singleton` (1 use), `small-family` (2-3), `mid-family` (4-6), `load-bearing` (7+). Stopwords excluded (research, paper, tool, production, framework, survey, design, system, substantial-bundle, user-probe, interrogation). Mirrors the iter-187/203 categorical-classifier shape but per-tag rather than per-corpus. Use to audit tag-axis health: many singletons = fragmented; many load-bearing = mature taxonomy. Composes with singleton-tags (which shows ONLY the 1-use rows) + tag-axis-loose-ends (cross-reference summary mentions).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 209 (interrogation-042 Finding B — unified
    // storyline traversal across roles): given a starting
    // Doc id, return other Docs that share ≥1 non-stopword
    // tag with it, ordered by tag-overlap count DESC. The
    // per-doc cross-role storyline-discovery view. Where
    // docs-by-tag answers "what docs have THIS tag?", this
    // answers "what docs share THIS doc's tags?" — different
    // shape, sibling intent. Closes iter-208's Finding B
    // observation that no single saved query returns all 7
    // ingest-pass-storyline members: this one returns all
    // siblings of any one member.
    SavedQuery {
        name: "storyline-siblings",
        description: "Given a Doc `$doc_id`, return OTHER Docs sharing ≥1 non-stopword tag with it, ordered by shared-tag count DESC. Cross-role storyline-discovery view. From any storyline member the agent can find all siblings in 1 call (vs research-by-tag + audits-by-tag separately). Per interrogation-042 Finding B: 7-member ingest-pass-storyline reaches across research + user-probe + interrogation roles; this query unifies discovery. Stopwords filtered (same set as tag-axis-coverage-summary).",
        params: &["doc_id"],
        needs: &["Doc"],
    },
    // Per iter 245 (interrogation-051 Finding C + iter-243
    // [[research-carpineto-query-expansion-survey]]): tag
    // pair co-occurrence diagnostic. Operationalises
    // Carpineto's "global AQE" signal class — pairs of tags
    // that travel together on the same doc are candidates
    // for query-expansion when the agent searches for one.
    // Sibling to iter-209 tag-axis-coverage-summary (per-tag
    // density) at the tag-PAIR axis instead of the per-tag
    // axis. Stopwords + iter-NNN-* per-probe-metadata tags
    // filtered same as tag-axis-coverage-summary.
    SavedQuery {
        name: "tag-cooccurrence",
        description: "Pairs of tags that co-occur on the same Doc, ordered by co-occurrence count DESC. Operationalises [[research-carpineto-query-expansion-survey]]'s global-AQE signal: tags travelling together are candidates for query-side expansion when the agent searches for one. Sibling to iter-209 tag-axis-coverage-summary (per-tag density) at the tag-PAIR axis. Stopwords + iter-NNN-* per-probe-metadata tags filtered. Use to discover family adjacency that BM25 free-text misses ([[feedback_meta_doc_displaces_research]]); compose with iter-242 with_tag retrieval primitive to ask 'find docs with the expansion-friend tag'. Returns top 25 pairs at cooccur_count >= 2.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 245 (Carpineto global-AQE companion at the
    // entity axis): pairs of Entities co-covered by the same
    // Doc, ordered by co-cover count DESC. Sibling to iter-
    // 100 entities-by-coverage-density (per-entity density)
    // at the entity-PAIR axis. Surfaces conceptual
    // proximity that the explicit RELATES_TO graph may not
    // capture (entity-pattern-transfer + entity-retrieval-
    // primitive co-cover the entire research-paper set,
    // exposing the project's two-core-axes structure).
    SavedQuery {
        name: "entity-cooccurrence",
        description: "Pairs of Entities co-covered by the same Doc (via COVERS edges), ordered by co-cover count DESC. Sibling to iter-100 entities-by-coverage-density (per-entity density) at the entity-PAIR axis. Operationalises the entity-side of [[research-carpineto-query-expansion-survey]]'s external-AQE signal — entities co-covered by the same docs are conceptually proximate even when no RELATES_TO edge connects them directly. Use to detect the corpus's design axes (e.g., entity-pattern-transfer + entity-retrieval-primitive co-cover the research-paper set = the project's two intellectual threads). Returns top 25 pairs at cooccur_count >= 2.",
        params: &[],
        needs: &["Doc", "Entity"],
    },
    // Per iter 141 (interrogation-028 Finding C —
    // symmetric gap to user-probe-024's failure-mode-
    // catalog gap on the POSITIVE-pattern axis): the loop
    // has discovered architectural patterns documented
    // across user-probe-021/023/024/025/026/027 with
    // pattern-tags (absence-as-signal, test-co-location,
    // query-reformulation, cross-corpus-validation,
    // cross-workflow-comparison). Individual pattern-tags
    // exist; no umbrella. This saved query unifies the
    // catalog by enumerating docs that carry ANY of the
    // named pattern-tags. Mirror image of iter-131's
    // retrieval-failure-modes; together the two saved
    // queries cover the failure/positive pattern duality.
    SavedQuery {
        name: "architectural-patterns",
        description: "Catalog of loop-discovered POSITIVE architectural patterns across the corpus — docs tagged with any of: absence-as-signal (well-encapsulated leaf modules), test-co-location (in-file tests via #[cfg(test)]), query-reformulation (user-vocab → doc-vocab as vocabulary-mismatch mitigation), cross-corpus-validation (pattern confirmed on 2+ corpora), cross-workflow-comparison (same primitives, different synthesis). Single-call corpus-wide architectural-pattern survey. Mirror of retrieval-failure-modes — together they cover failure + positive pattern duality.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 141 (sibling — portable Diataxis kind
    // survey): the design corpus has a corpus-local
    // `docs-by-kind` saved query but compile-time
    // doc-linter installs don't get it. Promoting to
    // compile-time so EVERY install has the Diataxis
    // kind survey primitive. The corpus-local override
    // path (iter 114) keeps backward compatibility —
    // the design corpus's existing docs-by-kind.toml
    // shadows this for that one corpus.
    SavedQuery {
        name: "docs-by-kind",
        description: "Docs filtered by Diataxis `$kind` (tutorial / how-to / reference / explanation). The Diataxis kind axis answers what the reader needs from the doc — to learn (tutorial), to do (how-to), to look up (reference), or to understand (explanation). Portable across every doc-linter install. Compose with docs-by-tag for tag-axis filtering, OR list-without-param to see the full corpus kind distribution.",
        params: &["kind"],
        needs: &["Doc"],
    },
    // Per iter 264 ([[interrogation-056]] Finding B —
    // narrative-vs-ontology distinction clarified
    // iter-258 'doc-sparse' caveat). The agent reading
    // doc_count sees a misleading total — the design
    // corpus has 264 Docs but only 222 are narrative
    // (role=doc); the rest are ontology entities + values
    // + axes + features + index hubs. The agent should
    // SEE this distribution before assuming "doc-rich"
    // means "narrative-rich". Companion to docs-by-kind
    // (Diataxis-axis) at the doc-ROLE axis.
    SavedQuery {
        name: "doc-role-distribution",
        description: "Corpus-wide Doc.role distribution — count of Docs per `role` value (doc / ontology-entity / ontology-axis / ontology-value / ontology-migration / feature / gap / index / etc.). Per [[interrogation-056]] Finding B: the iter-261 corpus-routing-recommendation router uses narrative_doc_count (role='doc' excluding ontology-*); the agent benefits from seeing the FULL role split to understand which docs are narrative vs ontology vs feature scaffolding. Sibling to docs-by-kind (Diataxis-axis) at the doc-ROLE axis. Use for cold-start corpus shape inspection alongside [[cold-start-overview]] + [[corpus-routing-recommendation]]. Returns the top 25 most-populated roles.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 264 ([[feedback_meta_doc_displaces_research]] +
    // iter-263 12-iter QA cadence finding): the agent reading
    // a corpus needs to distinguish RESEARCH absorptions
    // (research-*) from META auditing (interrogation-* /
    // user-probe-* / audit-run-*). The meta-doc fraction
    // reveals corpus epistemic state: high = self-
    // referential loop; low = newly absorbed research.
    // Companion to docs-by-kind + doc-role-distribution at
    // the loop-process axis.
    SavedQuery {
        name: "loop-process-doc-distribution",
        description: "Doc count partitioned by loop-process axis: research-absorptions / 3 META sub-classes (interrogation + user-probe + audit-run, separately) / other narrative / ontology / feature / gap. Operationalises [[feedback_meta_doc_displaces_research]] at the authoring-level. Returns 1 row with 8 counts + 2 ratios (meta_to_research + interrogation_to_user_probe). Per iter-266 [[interrogation-057]] Finding refinement: the 3 meta sub-classes are now separated since they reflect distinct loop processes — interrogations measure CORPUS retrievability, user-probes measure EXTERNAL-corpus surfaces, audit-runs are previous loop's historical records. Uses id-prefix matching since the loop's authoring convention encodes the doc category in the id.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 267 (sibling to refined loop-process-doc-
    // distribution): cold-start orientation query returning
    // the MOST RECENT doc per loop-process class. Lets an
    // agent answer "what is the loop most recently doing?"
    // in 1 call without scanning the whole corpus. Composes
    // with loop-process-doc-distribution: that gives counts;
    // this gives the latest-doc-per-class for narrative
    // jump-in.
    SavedQuery {
        name: "latest-by-loop-process",
        description: "Most-recent Doc per loop-process class (research / interrogation / user-probe / audit-run) — cold-start orientation for an agent landing on a corpus mid-stream. Returns 1 row per process-class with the latest doc's id + updated timestamp. Companion to [[loop-process-doc-distribution]]: that returns COUNTS; this returns the LATEST-DOC-PER-CLASS so the agent can read what the loop most recently authored in each process and rebuild context quickly. Per iter-267 (initial ship) + iter-274 [[interrogation-058]] Finding B fix: tie-handling now uses 3-pass max(updated) → max(id-at-max-updated) → re-match to correctly return EXACTLY 1 row per class when many docs share the iteration's date (the loop's authoring discipline produces uniform timestamps).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 274 ([[interrogation-058]] Finding C
    // operationalisation): the loop's epistemic-balance
    // metric. user-probe count vs interrogation count tells
    // the agent (and the loop itself) whether the
    // recent-iters cadence has been Type-V-heavy or
    // Type-I-heavy. Per the discovered design-corpus
    // metric: 63 user-probes / 56 interrogations = ratio
    // 1.13 — slight Type-V emphasis. Categorical label
    // lets the agent route decisions on whether to
    // recommend more V or I in the next iter.
    SavedQuery {
        name: "loop-process-balance-metric",
        description: "Loop epistemic balance metric: user-probe / interrogation count + a categorical label (V-heavy / I-heavy / balanced). Per [[interrogation-058]] Finding C: the design corpus has more user-probes than interrogations (63 vs 56 = 1.13), revealing the recent-iters cadence has been Type-V-heavy. Categorical thresholds: `V-heavy` ratio > 1.2, `I-heavy` ratio < 0.83, `balanced` between. Use as a 1-call diagnostic for the loop's iteration-type balance + to decide whether to recommend more external-validation or corpus-interrogation in next iters. On non-loop corpora returns 0/0 and 'balanced' (degenerate but harmless).",
        params: &[],
        needs: &["Doc"],
    },
    SavedQuery {
        name: "corpus-doc-starter-ratio",
        description: "1-call doc-null detector. Returns starter-ontology Doc count + authored Doc count + total + ratio + categorical label. iter-336 5-label refinement (was 4-label): 'doc-empty' if total=0 / 'doc-null' if ratio=1.0 / 'mostly-starter' if 0.8<=ratio<1.0 / 'authored-mixed' if 0.3<=ratio<0.8 / 'mostly-authored' if ratio<0.3. Per [[user-probe-068]] Finding B: identifies the 'doc-null code-repo' regime. Per iter-331 [[user-probe-086]] Finding G: 'authored-mixed' distinguishes corpora with BOTH substantial ontology AND substantial narrative (e.g. doc-linter source corpus_size=43 mix of ontology + how-to/reference/roadmap) from 'mostly-authored' corpora (design corpus where narrative dominates). Starter detection: role IN [ontology-value / -axis / -entity / -migration] OR (role='index' AND id STARTS WITH 'ontology'). Compose with [[authored-docs-sample]] + [[corpus-narrative-doc-roles-distribution]] for content orientation.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 281 (sibling to corpus-doc-starter-
    // ratio): when the ratio query indicates 'mostly-
    // authored' or 'mostly-starter', the agent
    // benefits from SEEING what the authored content
    // actually is. Returns up to 10 non-starter Docs
    // with id + role + title + summary. On doc-null
    // corpora returns 0 rows (the signal is "no
    // content to read"). Composes with corpus-doc-
    // starter-ratio for a 2-call corpus-shape +
    // content-orientation diagnostic.
    SavedQuery {
        name: "authored-docs-sample",
        description: "Up to 10 NON-starter-ontology Docs with id + role + title + summary + updated — composes with [[corpus-doc-starter-ratio]] (iter-281 sibling) to give the agent a 2-call corpus-shape + content-orientation diagnostic. Returns 0 rows when the corpus is 'doc-null' ([[user-probe-068]] Finding B); returns up to 10 rows on mixed/authored corpora ordered by updated DESC then id. Use after corpus-doc-starter-ratio's label is 'mostly-authored' or 'mostly-starter'. Starter exclusion mirrors corpus-doc-starter-ratio: role NOT IN [ontology-value/-axis/-entity/-migration] AND NOT (role='index' AND id STARTS WITH 'ontology').",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 336 (sibling to corpus-doc-starter-
    // ratio + authored-docs-sample): role-axis
    // breakdown of non-starter Docs. Tells the agent
    // what KINDS of narrative content the corpus has
    // (how-to / reference / research / interrogation
    // / user-probe / feature / etc.). Operationalises
    // iter-331 [[user-probe-086]] Finding G: doc-
    // linter source 'authored-mixed' corpora benefit
    // from role-axis content-orientation in 1 call.
    SavedQuery {
        name: "corpus-narrative-doc-roles-distribution",
        description: "Role-axis distribution of NON-starter-ontology Docs per iter-336. Returns 1 row per role with narrative_doc_count, ordered DESC. Composes with [[corpus-doc-starter-ratio]] (regime classifier) + [[authored-docs-sample]] (content-sample) for a 3-call corpus-orientation diagnostic. On 'doc-null' corpora returns 0 rows; on 'authored-mixed' corpora (e.g., doc-linter source per [[user-probe-086]]) surfaces both ontology-derived roles AND authored narrative roles; on 'mostly-authored' corpora (e.g., design corpus) surfaces research / interrogation / user-probe / reference / feature dominantly. Starter-doc exclusion mirrors corpus-doc-starter-ratio.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 338 (sibling to corpus-narrative-doc-
    // roles-distribution + iter-337 [[user-probe-088]]
    // Finding C): role-axis is COARSE on
    // documentation-rich corpora (270/273 authored
    // docs on design corpus use role='doc'). KIND
    // axis is the GRANULAR layer — reference / how-to
    // / explanation / tutorial per Diataxis. This query
    // surfaces the kind distribution for the agent's
    // content orientation. Sibling: [[corpus-narrative-doc-roles-distribution]]
    // for role-axis (1-dimensional on most corpora).
    SavedQuery {
        name: "corpus-narrative-doc-kinds-distribution",
        description: "Kind-axis (Diataxis) distribution of NON-starter-ontology Docs per iter-338 [[user-probe-088]] Finding C, refined iter-340 [[user-probe-089]] Finding H. Returns 1 row per kind with narrative_doc_count, ordered DESC. iter-340: empty-string kind values are RELABELED as 'kind-unset' so the column is readable + surfaces corpus-frontmatter gap as a named category. KIND axis is the GRANULAR layer — design corpus surfaces reference=254 + explanation=17 + how-to=2 (3 kinds) vs role-axis's 270 (1 role=doc). doc-linter source surfaces kind-unset=5 + reference=3 + explanation=2 + how-to=1 (the kind-unset rows are roadmap-entry role docs). Composes with [[corpus-doc-starter-ratio]] (regime classifier) + [[corpus-narrative-doc-roles-distribution]] (role-axis) for a 3-axis corpus content fingerprint. On doc-null corpora returns 0 rows. Sibling: [[authored-docs-by-kind]] for per-kind sample + [[corpus-narrative-doc-lifecycles-distribution]] for lifecycle-axis.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 338 (sibling parameterized): given a
    // $kind value, returns up to 10 authored Docs of
    // that kind. Lets the agent drill into a specific
    // Diataxis category to see actual content.
    SavedQuery {
        name: "authored-docs-by-kind",
        description: "Up to 10 NON-starter-ontology Docs of the specified $kind per iter-338. Parameterized drill-down companion to [[corpus-narrative-doc-kinds-distribution]] (the kind-axis distribution). Returns id + role + title + summary + updated, ordered by updated DESC. On design corpus with kind='reference' returns the 10 most-recent reference docs (research-*, interrogation-*, user-probe-*); with kind='how-to' returns the how-to docs. On doc-null corpora returns 0 rows. Starter-doc exclusion mirrors [[corpus-doc-starter-ratio]].",
        params: &["kind"],
        needs: &["Doc"],
    },
    // Per iter 340 (sibling to corpus-narrative-doc-
    // kinds-distribution at lifecycle-axis):
    // distribution of authored Docs by lifecycle value
    // (stable / planning / draft / superseded /
    // decided). Empty-string lifecycle (the
    // corpus-frontmatter unset case) relabeled as
    // 'lifecycle-unset' for readability per iter-339
    // [[user-probe-089]] Finding H.
    SavedQuery {
        name: "corpus-narrative-doc-lifecycles-distribution",
        description: "Lifecycle-axis distribution of NON-starter-ontology Docs per iter-340 sibling to [[corpus-narrative-doc-kinds-distribution]]. Returns 1 row per lifecycle value (stable / planning / draft / superseded / decided + 'lifecycle-unset' for empty-string) with narrative_doc_count, ordered DESC. On design corpus surfaces lifecycle-unset=270 + planning=2 + stable=1 — the unset is the dominant 'most authored docs don't specify lifecycle' pattern. doc-linter source surfaces planning + stable mix per its roadmap-entry + reference docs. Together with kinds-distribution + roles-distribution provides 4-axis corpus content fingerprint (regime / role / kind / lifecycle).",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 342 (corrective response to iter-341
    // [[interrogation-071]] Finding G + new analysis):
    // architectural hub-functions ranked by CALLER
    // BREADTH (distinct callers + distinct caller files)
    // — orthogonal axis to existing iter-218 callers-
    // density-hub-test-coverage which ranks by CALL-
    // EDGE VOLUME. Distinguishes API-tier hubs (callers
    // spread across many files) from internal-cluster
    // hubs (callers within same file).
    SavedQuery {
        name: "hub-functions-by-caller-breadth",
        description: "Hub functions ranked by CALLER BREADTH (distinct callers + caller_files), with categorical spread_label ('api-tier-hub' if caller_files >= 5, 'mid-tier-hub' if 2-4, 'internal-cluster-hub' if 1). Orthogonal axis to [[callers-density-hub-test-coverage]] which ranks by call-edge volume. On doc-linter source: store/connect (40 callers / 18 files / api-tier) + projections/val_string (34/15/api-tier) + cmd/mcp/handle_line (21/1/internal-cluster) + cmd/mcp/tool_text_result (17/1/internal-cluster) + parse_scip (16/5/api-tier) — the 'internal-cluster' label surfaces same-file-clustered hubs that the call-volume ranking conflates with cross-codebase utilities. Use to distinguish API-tier blast-radius from internal-module clustering. Test-fixture filter per iter-225 (file + symbol both checked). LIMIT 25.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 342 (sibling): corpus-wide spread-class
    // distribution. Composes with hub-functions-by-
    // caller-breadth to give the agent a 1-call
    // diagnostic of how many hubs fall in each spread
    // tier.
    SavedQuery {
        name: "hub-spread-distribution",
        description: "Corpus-wide distribution of hub functions per spread class per iter-342. Returns 1 row per spread_label ('api-tier-hub', 'mid-tier-hub', 'internal-cluster-hub') with hub_count. Sibling of [[hub-functions-by-caller-breadth]] at the corpus-distribution axis. On doc-linter source surfaces relative balance: many api-tier-hubs = cross-codebase coupling; many internal-cluster-hubs = strong per-module encapsulation. Mirrors iter-237 function-shape-distribution at the spread-axis.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 131 (user-probe-024 Finding C —
    // gap-missing-failure-mode-umbrella-tag): the loop has
    // documented 9 instances of vocabulary-mismatch + 3 of
    // tag-axis-loose-end + 1 of negation-failure + 2 of
    // adjacent-family-displacement + 1 of
    // embedding-backend-not-compiled scattered across audit/
    // and research/. Individual pattern-tags exist; no
    // umbrella. Rather than retroactively re-tag ~10 docs
    // (drift per ontology-organic memory), this saved query
    // unifies the catalogue by enumerating docs that carry
    // ANY of the named pattern-tags. Closes the
    // discoverability gap at the compile-time catalog layer.
    SavedQuery {
        name: "retrieval-failure-modes",
        description: "Catalog of documented BM25 / retrieval failure modes across the corpus — docs tagged with any of: vocabulary-mismatch, tag-axis-loose-end, negation-failure, adjacent-family-displacement, embedding-backend-not-compiled, vocabulary-asymmetry. Single-call corpus-wide failure-mode survey for an agent hitting a BM25 issue and wanting the checklist.",
        params: &[],
        needs: &["Doc"],
    },
    // Per iter 131 (user-probe-024 Finding C — companion to
    // retrieval-failure-modes): the mitigation side of the
    // catalogue. user-probe-023's three-mitigation framing
    // (query reformulation, audit_doc_region full-body,
    // embedding backend) is documented across multiple docs;
    // this query unifies their catalog so an agent who
    // already knows the failure mode can jump directly to
    // the documented mitigation surface.
    SavedQuery {
        name: "retrieval-mitigation-modes",
        description: "Catalog of documented mitigations for BM25 / retrieval failure modes — docs tagged with any of: vocabulary-mismatch-mitigation, audit-as-surface, query-reformulation. Composes with `retrieval-failure-modes` (failure-mode catalog): agent identifies the failure mode, then looks up the mitigation catalog for actionable workarounds.",
        params: &[],
        needs: &["Doc"],
    },
    // Roadmap issue #19: with FastAPI / Flask / Express landed,
    // a repo may have endpoints from multiple frameworks. This
    // breakdown surfaces the mix at a glance — useful when an
    // agent is asked "what API surface does this project have?"
    SavedQuery {
        name: "endpoint-by-kind",
        description:
            "Endpoint count grouped by kind (axum / clap / mcp / fastapi / flask / express)",
        params: &[],
        needs: &["Endpoint"],
    },
    // Roadmap issue #9: CALLS edges enable "who's at the top of
    // the call graph?" queries. Functions with no inbound CALLS
    // are entry points — `main()`, request handlers, test
    // bodies, etc. Pairing with `dead-files` gives a "ranked
    // attack surface" view.
    SavedQuery {
        name: "callgraph-roots",
        description:
            "Functions with no inbound CALLS — likely entry points (main, handlers, tests)",
        params: &[],
        needs: &["Function"],
    },
    // Roadmap issue #16: IMPORTS edges expose the file-level
    // dependency graph. Files at the top of the fan-out are
    // popular dependencies — touching them ripples through the
    // codebase. Aggregating by importer count rather than
    // individual edges gives "popularity" in the agent sense.
    SavedQuery {
        name: "import-fan-out",
        description: "Files imported by the most other files — popular dependencies",
        params: &[],
        needs: &["File"],
    },
    // Roadmap issue #38: COUPLED_WITH edges aggregate git
    // co-change history into File→File. Pairs with high
    // jaccard are likely conceptually coupled — touching one
    // typically implies touching the other. Useful "what
    // ELSE should I look at?" view next to a known file.
    SavedQuery {
        name: "coupled-files",
        description:
            "File pairs with the highest git co-change jaccard — likely conceptually coupled",
        params: &[],
        needs: &["File"],
    },
    // Per iter 345 (user-probe-090 Finding G —
    // scaffold-clique filter recipe): filters out
    // scaffold-clique pairs (jaccard = 1.0) that arise
    // from initial-commit co-creation. On trusted these
    // dominate the top of coupled-files at jaccard 1.0
    // / 0.75 / 0.6 (28 frontend pairs) and hide the
    // real items↔users CRUD-route coupling at
    // jaccard=0.5. iter-201's feature-evolution-coupling
    // uses commits>=5 threshold which misses trusted's
    // commits=3 jaccard=0.5 real signal; this query
    // uses the jaccard<1.0 filter which catches it.
    SavedQuery {
        name: "coupled-files-without-scaffold",
        description: "File pairs with COUPLED_WITH edge ranked by jaccard DESC, EXCLUDING scaffold-clique pairs (jaccard = 1.0) per iter-344 [[user-probe-090]] Finding G. On trusted FastAPI corpus this filters out 28 frontend scaffold-clique pairs and surfaces the 1 REAL coupling: backend/app/api/routes/items.py ↔ users.py at jaccard=0.5. Complementary to iter-201 [[feature-evolution-coupling]] which uses commits>=5 threshold (misses commits=3 jaccard=0.5 trusted signal) and iter-211 [[feature-evolution-coupling-by-commits]]. Sibling: [[coupled-files-by-jaccard-range]] parameterized version.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 345 (parameterized sibling): given
    // $min_jaccard + $max_jaccard, return coupled
    // pairs in that range. Use to drill into specific
    // jaccard bands (e.g., 0.3-0.8 for real feature
    // couplings vs 0.0-0.3 for weak coupling).
    SavedQuery {
        name: "coupled-files-by-jaccard-range",
        description: "Parameterized coupled-files filter by jaccard range ($min_jaccard <= jaccard <= $max_jaccard) per iter-345. Use to drill into specific coupling bands: jaccard 0.5-0.9 = strong feature-evolution co-evolution; 0.3-0.5 = moderate; 0.0-0.3 = weak. On trusted FastAPI with min=0.4, max=0.9 should surface only the items↔users pair (jaccard=0.5) excluding scaffold-clique (jaccard=1.0). Both bounds are STRING parameters (Kuzu param substitution); pass as float-like strings ('0.5', '0.9'). Sibling: [[coupled-files-without-scaffold]] for the no-param common case.",
        params: &["min_jaccard", "max_jaccard"],
        needs: &["File"],
    },
    // Per iter 347 (corrective response to iter-346
    // [[user-probe-091]] material correction): the
    // iter-345 jaccard<1.0 filter was PARTIALLY
    // effective — it dropped 15 of 28 scaffold-clique
    // pairs on trusted but preserved 13 more at
    // jaccard=0.75 + 0.6. iter-346 surfaced the
    // refined heuristic: commits>=5 OR jaccard<=0.55.
    // Hybrid catches: (a) doc-linter-source-style real
    // couplings (commits>=5 path); (b) trusted-style
    // real couplings buried under scaffold-clique
    // (jaccard<=0.55 path). Verified empirically:
    // trusted returns 1 row (items↔users); doc-linter
    // source returns 405 of 494 total (18% scaffold).
    SavedQuery {
        name: "feature-evolution-coupling-hybrid",
        description: "File pairs in real feature co-evolution per iter-347 hybrid heuristic: commits>=5 OR jaccard<=0.55. Catches BOTH doc-linter-source-style real couplings (commits>=5 = many co-commits over time) AND trusted-style real couplings buried under scaffold-clique noise (jaccard<=0.55 = low overlap with rest of corpus). Empirically validated: on trusted surfaces 1 row (items↔users at jaccard=0.5, commits=3); on doc-linter source surfaces ~405 of 494 total COUPLED_WITH edges. Replaces iter-201 feature-evolution-coupling (commits>=5 only, missed trusted's commits=3 signal) and iter-345 coupled-files-without-scaffold (jaccard<1.0, kept 13 scaffold pairs on trusted). Sibling: [[coupling-density-stats]] for corpus-shape understanding.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 347 (sibling): corpus-shape coupling
    // density diagnostic. 1-call understanding of
    // how much of the corpus's COUPLED_WITH edges
    // are scaffold-clique vs real co-evolution.
    SavedQuery {
        name: "coupling-density-stats",
        description: "Coupling-axis density diagnostic per iter-347 sibling of feature-evolution-coupling-hybrid. Returns 1 row with total_couplings + scaffold_count (commits<5 AND jaccard>0.55) + real_count (commits>=5 OR jaccard<=0.55). On trusted FastAPI: 29 total / 28 scaffold / 1 real (96.6% scaffold-noise). On doc-linter source: 494 total / 89 scaffold / 405 real (18% scaffold-noise). Lets the agent see corpus-shape coupling profile at a glance — high scaffold ratio = trusted-style template corpus; low = mature codebase. Composes with corpus-doc-starter-ratio (iter-336) for full corpus characterization.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 349 (user-probe-092 Finding F — file-
    // naming pairing as workable heuristic when
    // TEST_FOR is empty): Rust test files by
    // convention. Classifies into 'sibling' (X_tests.rs
    // in src/ paired with X.rs) and 'integration'
    // (tests/ directory files). Workable when iter-218
    // callers-density-hub-test-coverage is unreliable
    // due to empty TEST_FOR.
    SavedQuery {
        name: "rust-test-files-by-pattern",
        description: "Rust test files surfaced via file-naming convention per iter-349 (response to [[user-probe-092]] Finding F). Returns path + loc + test_kind ('sibling' if path ENDS WITH '_tests.rs' or '_test.rs' inside src/; 'integration' if path STARTS WITH 'tests/'; 'helper' if path CONTAINS 'tests/common'). On doc-linter source surfaces 2 sibling tests (path_tests.rs + scan_tests.rs in store/symbols/) + ~17 integration tests + 1 helper. Workable test-coverage map heuristic when [[feedback_test_for_sparse_universal]] applies (TEST_FOR edges empty). Sibling: [[rust-production-source-files]] for the to-pair-with set.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 349 (sibling): Rust production source
    // files in src/ — the to-pair-with set. Excludes
    // _tests.rs / _test.rs (caught by sibling query)
    // and mod.rs (commonly module declaration only).
    SavedQuery {
        name: "rust-production-source-files",
        description: "Rust production .rs files in src/ — the to-pair-with set per iter-349. Excludes _tests.rs / _test.rs (sibling test files) + mod.rs (module declaration only). Composes with [[rust-test-files-by-pattern]] for client-side file-naming test-coverage pairing per [[user-probe-092]] Finding E heuristic. Agent compares the two lists: a production foo.rs in src/X/ paired with sibling foo_tests.rs in same folder = tested via sibling-test convention; without = candidate untested file (potentially covered by tests/ integration tests).",
        params: &[],
        needs: &["File"],
    },
    // Per iter 351 (user-probe-093 Finding F + G):
    // Type-axis caller-breadth analog to iter-342
    // hub-functions-by-caller-breadth. Reveals
    // architecturally important TYPES (data shapes)
    // complementary to function-axis (call paths).
    // On doc-linter source surfaces LintConfig as
    // top api-tier-type (66 users / 36 files).
    SavedQuery {
        name: "hub-types-by-caller-breadth",
        description: "Hub TYPES ranked by USES_TYPE caller breadth (distinct user functions + user_files) with categorical spread_label per iter-351. Parallel to [[hub-functions-by-caller-breadth]] (iter-342) at the type-axis. Surfaces architecturally important DATA SHAPES complementary to function-axis CALL PATHS. spread_label: 'api-tier-type' if user_files >= 5; 'mid-tier-type' if 2-4; 'internal-cluster-type' if 1. On doc-linter source: LintConfig (66 users / 36 files = api-tier-type) + Ontology (29/20) + Issue enum (27/15) + Doc (19/15) + McpError (19/1 = internal-cluster) + ScipIngestStats (16/1 = internal-cluster) per iter-350 [[user-probe-093]] Findings A+B. Test-fixture filter per iter-225. Sibling: [[type-spread-distribution]].",
        params: &[],
        needs: &["Function", "Type"],
    },
    // Per iter 351 (sibling): corpus-wide type
    // spread-class distribution. 1-call diagnostic of
    // how many hub-types fall in each spread tier.
    SavedQuery {
        name: "type-spread-distribution",
        description: "Corpus-wide distribution of hub TYPES per spread class per iter-351. Returns 1 row per spread_label ('api-tier-type', 'mid-tier-type', 'internal-cluster-type') with hub_count. Sibling to [[hub-types-by-caller-breadth]] at the corpus-distribution axis. Mirrors [[hub-spread-distribution]] (iter-342) at function-axis. Together with iter-342's function-axis distribution gives the agent 2-axis architectural-tier balance for the corpus.",
        params: &[],
        needs: &["Function", "Type"],
    },
    SavedQuery {
        name: "function-hub-files",
        description: "Files containing at least one function-axis api-tier hub per iter-353. Aggregation of [[hub-functions-by-caller-breadth]] at file granularity — distinct_callers >= 5 AND caller_files >= 5. On doc-linter source returns 6 files: src/config/mod.rs + src/disambiguation.rs + src/store/mod.rs + src/store/projections.rs + src/parser.rs + src/scip_ingest.rs. Sibling: [[type-hub-files]] — agent client-side intersection yields full-anchor files (parser.rs + scip_ingest.rs + disambiguation.rs + config/mod.rs per iter-353 cypher result). Test-fixture filter per iter-225.",
        params: &[],
        needs: &["Function"],
    },
    // Per iter 353 (sibling): file-level aggregation
    // of type-axis api-tier hubs. Composes with
    // [[function-hub-files]] for agent-side cross-axis
    // intersection. On doc-linter source returns 9
    // files.
    SavedQuery {
        name: "type-hub-files",
        description: "Files containing at least one type-axis api-tier hub per iter-353. Aggregation of [[hub-types-by-caller-breadth]] at file granularity — distinct_users >= 5 AND user_files >= 5. On doc-linter source returns 9 files: src/config/mod.rs + src/disambiguation.rs + src/endpoint_extract.rs + src/graph.rs + src/ids.rs + src/ontology.rs + src/parser.rs + src/scip_ingest.rs + src/validator/issue.rs. Sibling: [[function-hub-files]] for cross-axis intersection (full-anchor / pure-utility / pure-schema taxonomy per iter-352 [[user-probe-094]] Finding D). Test-fixture filter per iter-225.",
        params: &[],
        needs: &["Function", "Type"],
    },
    // Per iter 355 (user-probe-095 Finding F —
    // bimodal Finding-axis attachment): Finding ingest
    // produces 2 distinct attachment modes by kind.
    // 'unstubbed-concept' findings carry Doc IDs in
    // their file field with no File-side HAS_FINDING
    // edge; {todo / fixme / xxx / hack} findings carry
    // source-code paths + HAS_FINDING edges. This
    // query is the 1-call diagnostic showing both
    // counts.
    SavedQuery {
        name: "findings-by-attachment-mode",
        description: "Finding-axis bimodal distribution per iter-355 (response to [[user-probe-095]] Finding F). Returns 2 rows: ('source-code-marker', N) + ('doc-unstubbed-concept', M) where N = TODO/FIXME/XXX/HACK markers attached to source files and M = unstubbed-concept findings attached to narrative Docs. On doc-linter source returns (source-code-marker, 30) + (doc-unstubbed-concept, 70). Helps the agent understand the bimodal Finding ingest pipeline before interpreting list_findings results. Sibling: [[unstubbed-concepts-by-doc]] for drill-down into the doc-attached mode.",
        params: &[],
        needs: &["Finding"],
    },
    // Per iter 355 (sibling): unstubbed-concept
    // findings per Doc. The 70 (per doc-linter source)
    // unstubbed-concept findings represent ontology
    // gaps in narrative documentation — terms used in
    // doc body that lack entity definitions. This
    // query surfaces which docs have the most
    // unstubbed concepts, so authoring effort can be
    // prioritized.
    SavedQuery {
        name: "unstubbed-concepts-by-doc",
        description: "Docs with the most unstubbed-concept findings per iter-355. Joins Finding (kind='unstubbed-concept') to Doc via implicit file=Doc.id pattern produced by the loop's ontology-gap detector. Each row: doc_id + finding_count. On doc-linter source surfaces doc-linter-usage + crate-doc-linter as the highest-finding docs (concept references in narrative without entity definitions). Closing these via entity additions reduces lint backlog substantially. Sibling: [[findings-by-attachment-mode]] for the mode aggregate.",
        params: &[],
        needs: &["Finding"],
    },
    SavedQuery {
        name: "entity-mention-ratio",
        description: "Per-entity FUNCTION_MENTIONS / TYPE_MENTIONS ratio + categorical flavor_label per iter-357. Operationalises iter-356 [[interrogation-073]] Finding G: cross-axis mention ratio reveals entity FLAVOR. flavor_label: 'function-heavy' if type_mentions=0 OR ratio >= 2.0 (entity is dealt with primarily by handler/processor functions); 'type-heavy' if ratio <= 1.5 (entity has many data-shape definitions); 'balanced' otherwise. On doc-linter source returns 8 rows: mcp (2.92 function-heavy) + scip (2.26) + endpoint (2.21) > onnx (1.75 balanced) + lsp (1.69) > roadmap (1.41 type-heavy) + cypher (1.31) + doc-graph (1.29). Use case: agent asks 'what kind of code references entity X?' Test-fixture filter per iter-225.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 357 (sibling): corpus-wide flavor
    // distribution. 1-call diagnostic of how many
    // entities fall in each flavor class.
    SavedQuery {
        name: "entity-flavor-distribution",
        description: "Corpus-wide distribution of entity flavor classes per iter-357. Returns 1 row per flavor_label ('function-heavy', 'balanced', 'type-heavy') with entity_count. Sibling to [[entity-mention-ratio]] at corpus aggregate axis. On doc-linter source returns 3 rows: function-heavy=3 + type-heavy=3 + balanced=2 (mostly-balanced distribution; corpus has architectural balance across function and type orientation per entity).",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 359 (interrogation-074 Finding F —
    // BELONGS/MENTIONS ratio reveals entity LIFECYCLE):
    // computes FUNCTION_BELONGS_TO / FUNCTION_MENTIONS
    // ratio per entity and emits categorical
    // lifecycle_label. Orthogonal axis to iter-357
    // entity-mention-ratio (entity FLAVOR).
    SavedQuery {
        name: "entity-belongs-vs-mentions-ratio",
        description: "Per-entity FUNCTION_BELONGS_TO / FUNCTION_MENTIONS ratio + categorical lifecycle_label per iter-359. Operationalises iter-358 [[interrogation-074]] Finding F: BELONGS/MENTIONS ratio reveals entity LIFECYCLE. lifecycle_label: 'implementation-heavy' if belongs >= 1 AND ratio >= 2.0 (matured beyond docs); 'balanced' (ratio 0.7-2.0); 'discussion-heavy' (ratio < 0.7, talked-about > scoped); 'pure-discussion' if belongs = 0 (entities like roadmap+lsp with only doc-comment mentions); 'pure-structural' if mentions = 0. Orthogonal axis to [[entity-mention-ratio]] (function/type for FLAVOR). On doc-linter source returns 8 rows: onnx (68/7/9.71/implementation-heavy) + doc-graph (578/176/3.28) + cypher (43/17/2.53) + mcp (74/38/1.95/balanced) + scip (48/79/0.61/discussion-heavy) + endpoint (49/86/0.57) + lsp (0/22/pure-discussion) + roadmap (0/62/pure-discussion).",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 359 (sibling): corpus-wide lifecycle
    // class distribution. 1-call diagnostic of how
    // many entities fall in each lifecycle stage.
    SavedQuery {
        name: "entity-lifecycle-distribution",
        description: "Corpus-wide distribution of entity lifecycle classes per iter-359. Returns 1 row per lifecycle_label with entity_count. Sibling to [[entity-belongs-vs-mentions-ratio]] at corpus aggregate axis. On doc-linter source returns 4 rows: implementation-heavy=3 + discussion-heavy=2 + pure-discussion=2 + balanced=1 (corpus has diverse entity-state distribution). Together with [[entity-flavor-distribution]] (iter-357 FLAVOR axis) provides 2-axis entity-state characterization.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 361 (user-probe-096 cluster taxonomy):
    // unified 2D entity-state grid combining FLAVOR
    // (iter-357) + LIFECYCLE (iter-359) into 1 row per
    // entity with cluster_label. 5 cluster labels per
    // iter-360 [[user-probe-096]] findings + 1
    // unclassified catch-all.
    SavedQuery {
        name: "entity-state-grid",
        description: "Unified 2D entity-state grid per iter-361. Combines iter-357 [[entity-mention-ratio]] FLAVOR axis + iter-359 [[entity-belongs-vs-mentions-ratio]] LIFECYCLE axis into 1 row per entity with cluster_label classification. 5 cluster labels per iter-360 [[user-probe-096]] taxonomy: 'implemented-infrastructure' (type-heavy + implementation-heavy = data shapes matured beyond docs); 'discussed-handlers' (function-heavy + discussion-heavy = handlers talked-about > scoped); 'architectural-center' (function-heavy + balanced = project's I/O surface); 'future-work' (pure-discussion = planned features); 'implementation-anomaly' (balanced + implementation-heavy = implemented but doc-sparse); 'unclassified' (other). On doc-linter source returns 8 entities: cypher + doc-graph (implemented-infrastructure) + endpoint + scip (discussed-handlers) + mcp (architectural-center) + onnx (implementation-anomaly) + lsp + roadmap (future-work). 1-call diagnostic for 'what stage is entity X at?' agent question. Sibling: [[entity-cluster-distribution]].",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 361 (sibling): corpus-wide cluster
    // distribution. 1-call diagnostic of how entities
    // fall across the 6 cluster classes.
    SavedQuery {
        name: "entity-cluster-distribution",
        description: "Corpus-wide distribution of entity cluster classes per iter-361. Returns 1 row per cluster_label with entity_count. Sibling to [[entity-state-grid]] at corpus aggregate axis. On doc-linter source returns 5 rows: implemented-infrastructure=2 + discussed-handlers=2 + future-work=2 + architectural-center=1 + implementation-anomaly=1.",
        params: &[],
        needs: &["Entity"],
    },
    // Per iter 259 (user-probe-062 Finding C+D — doc-
    // sparse corpora need file-axis diagnostics): the
    // DEGREE-axis sibling of coupled-files. coupled-files
    // surfaces pair-strength via jaccard; THIS surfaces
    // hub-files via neighbor count. user-probe-062 found
    // admin.tsx in trusted FastAPI template couples with
    // 7+ frontend routes; this query exposes the hub
    // pattern as a 1-call diagnostic. The agent's natural
    // question "which files are coupling hubs?" gets a
    // direct answer instead of needing manual neighbor
    // aggregation over coupled-files rows.
    SavedQuery {
        name: "file-coupling-degree",
        description: "Files ranked by COUPLED_WITH neighbor count (DEGREE axis, not jaccard). Returns each file's degree (distinct neighbor count) + sum of co-change commits across all edges + the most-coupled file's path. Sibling to [[coupled-files]] (pair-strength view) at the per-file hub-detection axis. Per [[user-probe-062]] Finding C+D: on doc-sparse corpora the FILE-AXIS is where the actionable agent signal lives; admin.tsx in the trusted FastAPI template couples with 7+ frontend routes — this query exposes that pattern directly. LIMIT 50.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 259 (user-probe-062 Finding C — file-axis
    // signal richness on doc-sparse corpora): the LOC-aware
    // sibling of language-distribution. The existing
    // language-distribution returns file counts per
    // language; this query adds total LOC, average LOC, and
    // max LOC per language. Captures the "what's the actual
    // size weight of each language?" question that pure
    // file-count under-answers (e.g., a few large Python
    // backend files matter more than 60 tiny CSS files).
    SavedQuery {
        name: "language-loc-distribution",
        description: "Per-language file count + total LOC + average LOC + max LOC. LOC-aware extension of [[language-distribution]] giving the agent SIZE-WEIGHTED corpus composition. Per [[user-probe-062]] Finding C: on doc-sparse corpora the file-axis is the actionable agent surface; LOC-weighting answers 'which language dominates by code volume?' rather than 'which by file count?'. Use as the cold-start sizing diagnostic. NULL-LOC files are excluded from average + max calculations to avoid skewing.",
        params: &[],
        needs: &["File"],
    },
    SavedQuery {
        name: "feature-evolution-coupling",
        description: "File pairs at high jaccard whose COUPLED_WITH commit count is ≥5 — meaningful CO-EVOLUTION coupling that's NOT scaffold-noise. Filters out the user-probe-048-surfaced scaffold-coupling pattern (template repos accumulate jaccard=1.0 pairs at commits=3 from setup commits, not feature evolution). Use this on corpora with short commit history (templates, recent forks, freshly-initialized repos) where coupled-files is dominated by scaffold pairs. Composes with coupling-hub-files-feature-evolution-only for the hub-axis view.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 201 (user-probe-048 Finding A companion):
    // hub-axis view of the same filter. Identifies files that
    // co-evolve with ≥2 OTHER files at the meaningful-feature
    // threshold. Excludes the scaffold-clique that dominates
    // the unfiltered coupling-hub view (on trusted: admin.tsx
    // appeared at 7 neighbors but ALL at commits=3, making
    // it not a real architectural hub — just the second-wave
    // scaffold file).
    SavedQuery {
        name: "feature-evolution-coupling-hubs",
        description: "Files coupled with ≥2 OTHER files at commits ≥5 — the FEATURE-EVOLUTION hub-axis view that excludes scaffold-clique noise. Per file: neighbor count + average jaccard among the meaningful-coupling subset. Companion to feature-evolution-coupling (the pair-axis view). On scaffold-template corpora (user-probe-048 pattern), this returns 0 rows — accurately surfacing that the corpus has no feature-evolution-driven hub yet.",
        params: &[],
        needs: &["File"],
    },
    // Per iter 211 (user-probe-050 Finding B — sibling
    // to feature-evolution-coupling). On corpora with rich
    // commit history (e.g. doc-linter source: scip_pipeline.rs
    // ↔ schema.rs at commits=15), the iter-201 query's
    // ORDER BY jaccard DESC promotes high-jaccard-low-commit
    // test-scaffold pairs ABOVE the architecturally
    // meaningful low-jaccard-high-commit src pairs. This
    // sibling orders by commits DESC FIRST, surfacing the
    // depth of co-evolution rather than its tightness. Both
    // orderings are useful for different corpus shapes.
    SavedQuery {
        name: "feature-evolution-coupling-by-commits",
        description: "Sibling to feature-evolution-coupling ordered by COMMIT COUNT descending instead of jaccard descending. On rich-history corpora (doc-linter source, mature open-source repos) this surfaces the architectural-backbone pairs (commits=15 jaccard=0.5) above the test-scaffold pairs (commits=5 jaccard=1.0). Use this when feature-evolution-coupling's jaccard-DESC ordering surfaces test-suite scaffolding above source-tree co-evolution. Companion to feature-evolution-coupling (jaccard-tight pairs) — together the 2 orderings cover different cuts of co-evolution.",
        params: &[],
        needs: &["File"],
    },
    // Roadmap issue #29 follow-up (gap-006): proper-noun candidates
    // not yet stubbed as Entity. Groups Finding(kind="unstubbed-
    // concept") rows by message to give a ranked candidate list with
    // mention counts and source docs.
    SavedQuery {
        name: "unstubbed-concepts",
        description:
            "Capitalised / snake_case proper-nouns mentioned in Doc summaries but not in the entity TermIndex — gap-006",
        params: &[],
        needs: &["Finding"],
    },
    // Roadmap issue #29 follow-up (gap-004): roadmap features with
    // unbuilt dependencies. Requires the design corpus to have at
    // least one Doc with role='feature' (per value-role-feature)
    // and `depends_on:` frontmatter that gets ingested as
    // DEPENDS_ON edges. Returns the blocked feature, its unmet dep,
    // and the dep's lifecycle so an agent can ask "what's blocking
    // each feature?" in one MCP call.
    SavedQuery {
        name: "roadmap-unmet-deps",
        description:
            "role=feature docs whose DEPENDS_ON target is still planning/implementing — gap-004",
        params: &[],
        needs: &["Doc"],
    },
    // Roadmap issue #29 follow-up (gap-001): surface competitor-tool
    // feature attributes. Returns every Entity that has at least one
    // `attributes` row, so an author can ask "which tools have we
    // evaluated and what features does each have?" in one MCP call.
    SavedQuery {
        name: "competitive-tools",
        description:
            "Entities with non-empty attributes (key=value rows) — gap-001 competitor-tool table",
        params: &[],
        needs: &["Entity"],
    },
    // Roadmap issue #29 follow-up (gap-003): surface roadmap-phase
    // membership. Returns every Doc grouped by phase so an agent
    // can ask "what's in v1 vs deferred?" in one MCP call. Docs
    // without a `phase:` frontmatter value group under the empty
    // bucket, surfacing the "unphased" docs as a side-effect.
    SavedQuery {
        name: "phasing",
        description:
            "Docs grouped by frontmatter phase (v1/v2/mvp/deferred/...) — gap-003",
        params: &[],
        needs: &["Doc"],
    },
    SavedQuery {
        name: "node-context-function",
        description:
            "Function row + linked docs/entities/type/CALLS (in+out) in one call",
        params: &["symbol"],
        needs: &["Function"],
    },
    // Roadmap issue #29 follow-up (gap-007 stopgap, doc sibling):
    // Doc-centred companion to `node-context-entity`. Returns the Doc
    // frontmatter plus both inbound and outbound doc-graph edges in
    // one call, each neighbour carrying its summary so an agent does
    // not need a follow-up `query_doc` per neighbour. Walks WIKILINK,
    // COVERS, INFORMED_BY, MD_LINK, DEPENDS_ON, SUPERSEDES — the same
    // typed-edge set the validator surfaces.
    SavedQuery {
        name: "node-context-doc",
        description:
            "Doc frontmatter + inbound + outbound (id/title/summary/path/kind/line/edge) in one call",
        params: &["doc"],
        needs: &["Doc"],
    },
    SavedQuery {
        name: "node-context-entity",
        description:
            "Entity row + every covering Doc (id/title/summary/path/kind/line) in one call",
        params: &["entity"],
        needs: &["Entity"],
    },
    // Roadmap issue #29 follow-up (gap-002): surface design-decision
    // provenance. Falls back to WIKILINK when INFORMED_BY is sparse,
    // so it returns useful rows even before authors backfill the
    // typed-edge frontmatter. Edge column distinguishes the two.
    SavedQuery {
        name: "decisions-to-prior-art",
        description:
            "design docs paired with prior-art they cite (INFORMED_BY where set, WIKILINK otherwise)",
        params: &[],
        needs: &["Doc"],
    },
    // First-run "work list": concepts ranked by how badly served
    // they are. Narrative chapters only (ontology-* docs excluded,
    // so the starter scaffold never counts as coverage or noise).
    SavedQuery {
        name: "concept-work-list",
        description: "Work list: entities ranked by how badly served — functions mentioning the entity vs narrative docs covering it ('N functions, 0 chapters' first). Ontology-* docs are not counted as chapters. LIMIT 25.",
        params: &[],
        needs: &["Entity"],
    },
];

/// Owned-string variant of [`SavedQuery`] for runtime queries
/// loaded from `<root>/saved-queries/*.toml` files. The TOML
/// loader returns these; the listing/run paths merge them with
/// the compile-time `SAVED_QUERIES` on every call so authors
/// can add a new saved query by writing a TOML file — no
/// rebuild, no `/mcp` reconnect.
///
/// `origin` is computed (not TOML-sourced) — see [`merged_catalog`].
/// Closes gap-saved-query-catalog-portability (user-probe-021):
/// the agent's "is this query portable across corpora?"
/// question is answered by this field. `compile-time` means the
/// query is part of every doc-linter install's universal catalog
/// and is safe to recommend cross-corpus. `corpus-local` means
/// the query came from `<root>/saved-queries/*.toml` and only
/// makes sense in this corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSavedQuery {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub params: Vec<String>,
    /// SQL (`$name` placeholders). Built-ins carry theirs in the embedded
    /// `saved_sql` table instead, so this is `None` for them. A legacy
    /// TOML with only a `cypher` field parses (the key is ignored) but has
    /// no SQL and fails to run with a message saying so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default = "default_origin")]
    pub origin: String,
    /// Node tables the query needs rows in (built-ins only).
    #[serde(default, skip_deserializing, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
}

/// Default `origin` for TOML-deserialized entries before
/// [`merged_catalog`] overrides them to `corpus-local`. The
/// override happens unconditionally, so this value only matters
/// for callers using the raw TOML loader directly — currently
/// no production path does that, but keeping the default safe
/// avoids surprises.
fn default_origin() -> String {
    "compile-time".to_string()
}

impl From<&SavedQuery> for RuntimeSavedQuery {
    fn from(q: &SavedQuery) -> Self {
        RuntimeSavedQuery {
            name: q.name.to_string(),
            description: q.description.to_string(),
            params: q.params.iter().map(|p| (*p).to_string()).collect(),
            sql: None,
            origin: "compile-time".to_string(),
            needs: q.needs.iter().map(|n| (*n).to_string()).collect(),
        }
    }
}

/// Stable list payload for `query saved --list`. Mirrors the
/// catalog directly so consumers can serialise it as JSON.
#[derive(Debug, Clone, Serialize)]
pub struct SavedQueryListing {
    pub count: usize,
    pub queries: Vec<RuntimeSavedQuery>,
}

/// Build the listing payload from the compile-time catalog ONLY —
/// no disk read, no merge. Use [`list_saved_queries_with_root`]
/// when the caller wants the runtime `.doc-lint/saved/*.toml`
/// queries merged in.
pub fn list_saved_queries() -> SavedQueryListing {
    let queries: Vec<RuntimeSavedQuery> =
        SAVED_QUERIES.iter().map(RuntimeSavedQuery::from).collect();
    SavedQueryListing {
        count: queries.len(),
        queries,
    }
}

/// Build the listing merged with `<root>/saved-queries/*.toml`
/// files. Runtime queries override compile-time ones on name
/// collision so authors can patch a buggy builtin without
/// rebuilding.
pub fn list_saved_queries_with_root(root: &Path) -> SavedQueryListing {
    let merged = merged_catalog(root);
    SavedQueryListing {
        count: merged.len(),
        queries: merged,
    }
}

/// Merge compile-time + runtime saved queries, with runtime
/// overriding by `name` collision. Read-fresh on every call;
/// no caching. The TOML directory is `<root>/saved-queries/`.
pub fn merged_catalog(root: &Path) -> Vec<RuntimeSavedQuery> {
    let mut by_name: HashMap<String, RuntimeSavedQuery> = HashMap::new();
    for q in SAVED_QUERIES {
        let r = RuntimeSavedQuery::from(q);
        by_name.insert(r.name.clone(), r);
    }
    for mut r in load_runtime_queries(root) {
        // Tag corpus-local origin AFTER load — TOML files don't
        // set it (the serde default leaves it "compile-time"),
        // and a runtime override of a compile-time name is still
        // a corpus-local query semantically.
        r.origin = "corpus-local".to_string();
        by_name.insert(r.name.clone(), r);
    }
    let mut out: Vec<RuntimeSavedQuery> = by_name.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Read `<root>/saved-queries/*.toml` and parse each as a
/// `RuntimeSavedQuery`. Silently skips files that don't parse so
/// one broken file doesn't break the catalog. Logs parse errors
/// to stderr.
///
/// The directory is at repo root (not under `.doc-lint/`, which
/// is gitignored) so authored queries are checked into version
/// control alongside the docs they query.
pub fn load_runtime_queries(root: &Path) -> Vec<RuntimeSavedQuery> {
    let dir = root.join("saved-queries");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<RuntimeSavedQuery> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        match std::fs::read_to_string(&path)
            .with_context(|| format!("read saved-query TOML {}", path.display()))
            .and_then(|text| {
                toml::from_str::<RuntimeSavedQuery>(&text)
                    .with_context(|| format!("parse saved-query TOML {}", path.display()))
            }) {
            Ok(q) => out.push(q),
            Err(e) => eprintln!("doc-linter: saved-query TOML skipped: {e:#}"),
        }
    }
    out
}

/// Find `name` in the compile-time catalog, or the merged catalog when
/// `root` is given.
pub fn resolve_query(root: Option<&Path>, name: &str) -> Result<RuntimeSavedQuery> {
    let catalog: Vec<RuntimeSavedQuery> = match root {
        Some(r) => merged_catalog(r),
        None => SAVED_QUERIES.iter().map(RuntimeSavedQuery::from).collect(),
    };
    let Some(query) = catalog.iter().find(|q| q.name == name) else {
        let available: Vec<String> = catalog.iter().map(|q| q.name.clone()).collect();
        bail!(
            "query saved: unknown name `{name}`. Available: {}",
            available.join(", ")
        );
    };
    Ok(query.clone())
}

/// Check every declared parameter was supplied and fill `name=default`
/// ones. Order doesn't matter: `--param k=v` can appear in any order.
pub fn fill_params(
    query: &RuntimeSavedQuery,
    name: &str,
    params: &HashMap<String, String>,
) -> Result<HashMap<String, String>> {
    let mut params = params.clone();
    for param in &query.params {
        match param.split_once('=') {
            Some((key, default)) => {
                params
                    .entry(key.to_string())
                    .or_insert_with(|| default.to_string());
            }
            None if !params.contains_key(param) => bail!(
                "query saved {name}: missing required --param {param}=<value>. \
                 Declared params: {}",
                query.params.join(", ")
            ),
            None => {}
        }
    }
    Ok(params)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn catalog_names_are_unique() {
        let mut names: Vec<&str> = SAVED_QUERIES.iter().map(|q| q.name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "duplicate saved-query name");
    }

    #[test]
    fn catalog_names_are_kebab_case() {
        for q in SAVED_QUERIES {
            assert!(
                q.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "saved-query name `{}` must be lowercase kebab-case",
                q.name
            );
            assert!(!q.name.is_empty(), "empty saved-query name");
        }
    }

    #[test]
    fn compile_time_listing_marks_origin_compile_time() {
        // gap-saved-query-catalog-portability: every compile-time
        // entry must be marked `origin = "compile-time"` so an
        // agent listing the catalog can identify which queries
        // are universal across corpora.
        let listing = list_saved_queries();
        assert!(!listing.queries.is_empty());
        for q in &listing.queries {
            assert_eq!(
                q.origin, "compile-time",
                "compile-time entry `{}` carries wrong origin `{}`",
                q.name, q.origin
            );
        }
    }

    #[test]
    fn merged_catalog_marks_corpus_local_origin() {
        // Drop a TOML in a tempdir's saved-queries/ and check
        // merged_catalog tags it `corpus-local` (while leaving
        // every other entry as `compile-time`).
        let tmp = std::env::temp_dir().join(format!(
            "doc-linter-saved-origin-test-{}",
            std::process::id()
        ));
        let sq_dir = tmp.join("saved-queries");
        std::fs::create_dir_all(&sq_dir).unwrap();
        std::fs::write(
            sq_dir.join("project-specific.toml"),
            r#"name = "project-specific"
description = "fake test query"
params = []
sql = "SELECT id FROM Doc LIMIT 1"
"#,
        )
        .unwrap();

        let merged = merged_catalog(&tmp);
        let local = merged
            .iter()
            .find(|q| q.name == "project-specific")
            .expect("runtime TOML query missing from merged catalog");
        assert_eq!(local.origin, "corpus-local");

        for q in &merged {
            if q.name == "project-specific" {
                continue;
            }
            assert_eq!(
                q.origin, "compile-time",
                "non-runtime entry `{}` carries wrong origin `{}`",
                q.name, q.origin
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn corpus_local_override_of_compile_time_name_is_corpus_local() {
        // If a TOML re-uses a compile-time name (override path —
        // documented as supported), the merged entry's origin
        // must still be `corpus-local`, not silently stay
        // `compile-time`. Otherwise an agent recommending the
        // query cross-corpus would silently get the override's
        // SQL.
        let tmp = std::env::temp_dir().join(format!(
            "doc-linter-saved-override-test-{}",
            std::process::id()
        ));
        let sq_dir = tmp.join("saved-queries");
        std::fs::create_dir_all(&sq_dir).unwrap();
        std::fs::write(
            sq_dir.join("orphan-entities-override.toml"),
            r#"name = "orphan-entities"
description = "overridden"
params = []
sql = "SELECT id FROM Entity LIMIT 1"
"#,
        )
        .unwrap();

        let merged = merged_catalog(&tmp);
        let entry = merged
            .iter()
            .find(|q| q.name == "orphan-entities")
            .expect("orphan-entities present in catalog");
        assert_eq!(entry.origin, "corpus-local");
        assert_eq!(entry.description, "overridden");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
