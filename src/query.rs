//! Query operations over the [[entity-doc-graph]]. Each sub-command emits
//! JSON so LLMs and scripts can consume the output without regex.
//!
//! SQL-backed through [`GraphRead`]. The in-memory petgraph version still
//! serves the lint pass (orphan detection in `cmd_check`).

use crate::config::LintConfig;
use crate::coverage::{self, ReportFilters};
use crate::graph::EdgeKind;
use crate::graph_read::{doc_filter_sql, query_sql, GraphRead};
use crate::ids::{DocId, EntityId, FunctionSymbol};

/// Every subcommand reads through the trait object.
type Database = dyn GraphRead;
use crate::store::{self, DocRow, LinkRow};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;

/// Clap args for `query list` — frontmatter filters (role, kind,
/// lifecycle, covers, status, tag) over the [[entity-doc-graph]] doc set.
#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Filter by `role:` frontmatter field (e.g. doc, adr, roadmap-entry, research).
    /// `--type` is kept as a back-compat alias.
    #[arg(long = "role", alias = "type")]
    pub role: Option<String>,

    /// Filter by Diátaxis `kind:` (tutorial, how-to, reference, explanation).
    /// Only meaningful for docs with role=doc.
    #[arg(long)]
    pub kind: Option<String>,

    /// Filter by `lifecycle:` (planning, decided, implementing, stable, superseded).
    #[arg(long)]
    pub lifecycle: Option<String>,

    /// Filter by domain entity in `covers:` — matches if the doc's covers list
    /// contains this entity id (e.g. outlet, pricing-rule, salesman).
    #[arg(long)]
    pub covers: Option<String>,

    /// Filter by `status:` frontmatter field (draft, stable, archived, deprecated)
    #[arg(long)]
    pub status: Option<String>,

    /// Filter by tag — matches if the doc's `tags:` array contains this value
    #[arg(long)]
    pub tag: Option<String>,

    /// Return entities ranked by FUNCTION_MENTIONS edge count, with a
    /// god-node flag on the top-3. Overrides every doc-row filter — the
    /// output shape is `RankedEntityOutput` (entities + count), not
    /// `ListOutput`. Lets a downstream consumer consume the structural-importance
    /// signal without a separate SQL round-trip.
    #[arg(long)]
    pub ranked: bool,
}

/// Clap args for `query neighbors` — outbound-link traversal of one
/// doc id in the [[entity-doc-graph]], filterable by edge type.
#[derive(clap::Args, Debug)]
pub struct NeighborsArgs {
    /// Doc id to inspect
    pub id: String,

    /// Filter by edge type (wikilink, md-link, depends-on,
    /// informed-by, supersedes, crate-ref, covers).
    /// May be repeated to match any of several types.
    #[arg(long = "edge-type")]
    pub edge_type: Vec<String>,
}

/// Clap args for `query backlinks` — inbound-link traversal of one doc
/// id in the [[entity-doc-graph]], filterable by edge type.
#[derive(clap::Args, Debug)]
pub struct BacklinksArgs {
    /// Doc id to inspect
    pub id: String,

    /// Filter by edge type — see `neighbors --help` for valid values.
    #[arg(long = "edge-type")]
    pub edge_type: Vec<String>,
}

/// Clap args for `query path` — shortest path between two doc ids in
/// the [[entity-doc-graph]], optionally restricted to listed edge types.
#[derive(clap::Args, Debug)]
pub struct PathArgs {
    /// Start doc id
    pub from: String,
    /// Target doc id
    pub to: String,

    /// Restrict the search to edges of these types only.
    #[arg(long = "edge-type")]
    pub edge_type: Vec<String>,
}

/// Clap args for `query subgraph` — root-id + depth BFS over the
/// [[entity-doc-graph]], with optional edge-type restriction.
///
/// Roadmap issue #12 (v0.3.0): when `--entity` is set, the
/// positional `id` is ignored and the traversal walks
/// `RELATES_TO` from the named entity instead — yields the
/// entity ego graph the pattern matcher uses for bipartite
/// alignment + chain detection.
#[derive(clap::Args, Debug)]
pub struct SubgraphArgs {
    /// Root doc id (ignored when `--entity` is set).
    #[arg(default_value = "")]
    pub id: String,
    /// How many hops to follow (default 2)
    #[arg(long, default_value = "2")]
    pub depth: usize,

    /// Restrict edges followed during BFS to these types only.
    /// Doc-side only — ignored under `--entity`.
    #[arg(long = "edge-type")]
    pub edge_type: Vec<String>,

    /// Walk RELATES_TO around this entity id instead of the
    /// Doc graph. Returns `{centre, depth, nodes, edges}` where
    /// nodes carry `mention_count` + `is_god_node` and edges
    /// carry `type` + `weight` + `frequency` + `edge_source`.
    #[arg(long)]
    pub entity: Option<String>,
}

/// Clap args for `query context` — frontmatter + outbound + inbound
/// for one doc id in the [[entity-doc-graph]] in a single call.
#[derive(clap::Args, Debug)]
pub struct ContextArgs {
    /// Doc id to inspect
    pub id: String,
}

/// Clap args for `query sql` — a read-only SQL escape hatch over the graph.
#[derive(clap::Args, Debug)]
pub struct SqlArgs {
    /// A single read-only `SELECT` / `WITH` statement against the SQLite
    /// graph (one table per node and rel kind; rel tables are
    /// `(src, dst, props...)`; run `query schema` to see them). Anything
    /// that could write is refused. `REGEXP` matches the whole string.
    /// Output is `{columns, row_count, rows}`.
    pub query: String,

    /// Bind `$name` placeholders: `--param name=value` (text), repeatable.
    #[arg(long = "param", value_parser = parse_param)]
    pub params: Vec<(String, String)>,
}

/// Clap args for `query functions-mentioning` — all functions whose
/// doc-comment carries an entity mention in the [[entity-doc-graph]].
#[derive(clap::Args, Debug)]
pub struct FunctionsMentioningArgs {
    /// Ontology entity id (e.g. `pricing-rule`).
    pub entity: String,
}

/// Clap args for `query function-context` — full Function row plus
/// every Doc / Entity it links to in the [[entity-doc-graph]].
#[derive(clap::Args, Debug)]
pub struct FunctionContextArgs {
    /// Full SCIP symbol string. Use the same form rust-analyzer emits,
    /// e.g. `rust-analyzer cargo doc-linter 0.1.0 src/parser.rs/parse_doc().`
    pub symbol: String,
}

/// Clap args for `query at <file>:<line>` (roadmap issue #25) — an
/// inverse lookup from a source location to the surrounding graph
/// context. Pass the location as a single colon-delimited string,
/// e.g. `backend/server.py:142`. Empty or unmatched locations
/// return null fields rather than erroring — agents probing
/// uncovered code should still get a usable JSON shape.
#[derive(clap::Args, Debug)]
pub struct AtArgs {
    /// Source location as `<repo-relative-path>:<1-based-line>`.
    pub location: String,
}

/// Clap args for `query map` (roadmap issue #39) — render a slice
/// of the graph as a Mermaid (or future graphviz / SVG) diagram.
/// v1 only handles `--scope entity --name <id>`; the schema-side
/// data for module imports (#16) and bounded-context cross-
/// references aren't fully populated yet, so those scopes return
/// a clear error directing the agent to the entity scope.
#[derive(clap::Args, Debug)]
pub struct MapArgs {
    /// What to render. `entity` (default) walks the RELATES_TO
    /// neighbourhood around `--name`. `module` and
    /// `bounded-context` are reserved; they emit an error today
    /// pointing at the entity-scope fallback.
    #[arg(long, default_value = "entity")]
    pub scope: String,

    /// Entity id (for `--scope entity`) or module / bounded-
    /// context name (reserved). Required for every scope.
    #[arg(long)]
    pub name: String,

    /// How many RELATES_TO hops to walk for entity scope. Capped
    /// at 30 nodes by default (the issue's GitHub-markdown
    /// legibility floor); raising `--depth` past 2 on a dense
    /// graph blows past that.
    #[arg(long, default_value = "2")]
    pub depth: usize,
    // No `--format` here: mermaid is the only output, and a local
    // `format: String` collided with the global `--format`
    // (`OutputFormat`), panicking clap on every `query map` call.
}

/// Clap args for `query saved` (roadmap issue #29) — curated
/// catalog of named SQL views shipped with the binary. Two
/// modes: `--list` prints the catalog; otherwise pass `<name>`
/// to run that query, with `--param key=val` for each declared
/// parameter.
#[derive(clap::Args, Debug)]
pub struct SavedArgs {
    /// Name of the saved query to run (e.g. `orphan-entities`,
    /// `hub-functions`). Required unless `--list` is set.
    #[arg(default_value = "")]
    pub name: String,

    /// Enumerate every saved query in the catalog as JSON.
    #[arg(long)]
    pub list: bool,

    /// Per-parameter substitution. Repeat for each `$param`
    /// the query declares: `--param entity=auth --param depth=2`.
    #[arg(long = "param", value_parser = parse_param)]
    pub params: Vec<(String, String)>,
}

/// Parse `--param key=val` into a `(key, val)` tuple. Bare `key`
/// (no `=`) and trailing `=` (no value) both error so the user
/// gets a clear shape complaint instead of a silent empty string.
fn parse_param(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("--param must be `key=value`, got `{s}`"))?;
    if k.is_empty() {
        return Err(format!("--param has empty key: `{s}`"));
    }
    Ok((k.to_string(), v.to_string()))
}

/// Clap args for `query impact <symbol>` (roadmap issue #36) — the
/// transitive caller graph for a Function symbol. Default depth is
/// 3, matching the Axon convention so cross-tool comparisons line
/// up. Use the same full SCIP symbol string `query function-context`
/// expects.
#[derive(clap::Args, Debug)]
pub struct ImpactArgs {
    /// Full SCIP symbol of the Function to start from.
    pub symbol: String,
    /// Maximum caller-graph depth to walk. Each level produces a
    /// separate bucket in the `depths:` output array.
    #[arg(long, default_value = "3")]
    pub depth: u32,
}

/// Clap args for `query endpoints` — list every axum/clap/MCP endpoint
/// in the [[entity-doc-graph]], filterable by kind and darkness.
#[derive(clap::Args, Debug)]
pub struct EndpointsArgs {
    /// Optional: emit only endpoints of this kind (`axum`, `clap`, `mcp`).
    #[arg(long)]
    pub kind: Option<String>,

    /// Show only "dark" endpoints — those with no
    /// `ENDPOINT_TOUCHES_ENTITY` edge. Useful for "list every API
    /// surface that doesn't yet declare a domain entity".
    #[arg(long)]
    pub dark: bool,
}

#[derive(clap::Args, Debug)]
pub struct CoverageReportArgs {
    /// Optional: emit only the section for this crate. Filters the
    /// `by_crate` array; the `global` and `by_entity` sections are
    /// unaffected.
    #[arg(long)]
    pub crate_filter: Option<String>,

    /// Optional: emit only the section for this entity. Filters the
    /// `by_entity` array; the `global` and `by_crate` sections are
    /// unaffected.
    #[arg(long)]
    pub entity_filter: Option<String>,

    /// If set, also rewrite `<root>/docs/coverage-report.md` from the
    /// same data. Idempotent — only touches the file when the body
    /// (excluding the `updated:` frontmatter date) actually changed.
    #[arg(long)]
    pub write: bool,
}

/// Clap subcommand enum dispatching to one of the [[entity-doc-graph]]
/// `query` operations (list, neighbors, backlinks, path, subgraph,
/// context, sql, functions-mentioning, function-context, ...).
#[derive(clap::Subcommand, Debug)]
pub enum QueryKind {
    /// List all docs, optionally filtered by type/status/tag
    List(ListArgs),
    /// Show outbound wikilinks from a doc
    Neighbors(NeighborsArgs),
    /// Show inbound wikilinks to a doc (which docs reference it)
    Backlinks(BacklinksArgs),
    /// Find the shortest link-path between two docs (both directions)
    Path(PathArgs),
    /// Return a doc plus all docs within N hops (inbound+outbound)
    Subgraph(SubgraphArgs),
    /// Return frontmatter + outbound + inbound for a doc in one call
    Context(ContextArgs),
    /// Run a read-only SQL query against the graph DB
    Sql(SqlArgs),
    /// Round 3B: every function whose doc-comment mentions an ontology
    /// entity. Requires SCIP ingest to have run (i.e. `code.scip` exists
    /// at the conventional path).
    FunctionsMentioning(FunctionsMentioningArgs),
    /// Round 3B: full Function fact + every Doc / Entity it links to.
    FunctionContext(FunctionContextArgs),
    /// Phase 0 of roadmap-43: emit a code-doc reachability report. JSON
    /// to stdout by default; `--write` also regenerates
    /// `docs/coverage-report.md` from the same data.
    CoverageReport(CoverageReportArgs),
    /// Phase 2 of roadmap-43: list every Endpoint (axum routes, clap
    /// subcommands, MCP tools) with its entity coverage. Filterable by
    /// kind (`--kind axum|clap|mcp`) and by darkness (`--dark`).
    Endpoints(EndpointsArgs),
    /// Roadmap issue #25 (v0.3.0): inverse lookup from `<file>:<line>`
    /// to the surrounding graph context — function, file, module,
    /// entities, covering docs, nearby endpoints. One call, full
    /// context. Designed for "I'm looking at line N, what should I
    /// know?"-style agent queries.
    At(AtArgs),
    /// Roadmap issue #36 (v0.3.0): transitive blast-radius view from a
    /// Function symbol. Walks CALLS backwards up to `--depth`, groups
    /// callers by depth, and lists touched entities + endpoints. The
    /// "if I change this, what else moves?" query agents reach for
    /// when scoping refactors.
    Impact(ImpactArgs),
    /// Roadmap issue #37 (v0.3.0): Functions with no inbound CALLS
    /// edge and no Endpoint binding. Cleanup checklist for unused
    /// symbols — paste-friendly JSON the agent can iterate over.
    DeadCode,
    /// Roadmap issue #39 (v0.3.0): render a slice of the graph as
    /// a Mermaid diagram suitable for embedding in markdown docs.
    /// v1 supports `--scope entity --name <id>`; module +
    /// bounded-context scopes defer to a follow-up.
    Map(MapArgs),
    /// Roadmap issue #29 (v0.3.0): curated catalog of high-value
    /// SQL views (orphan-entities, hub-functions, endpoint-
    /// darkness, ...). Agents reach for these instead of composing
    /// `query sql` strings by hand. `--list` enumerates the
    /// catalog; pass a name to run one.
    Saved(SavedArgs),
    /// Roadmap issue #20 (v0.3.0): self-describing schema for cold-start
    /// AI agents. Returns every node table, every rel table, their
    /// columns (with type + primary-key flag), the live row counts,
    /// and a curated list of example SQL queries. Reuses the store's
    /// `CALL show_tables()` / `CALL TABLE_INFO(...)` introspection so
    /// schema drift is impossible — whatever the store reports is what the
    /// caller sees.
    Schema,
    /// Roadmap issue #28 (v0.3.0): natural-language search over docs,
    /// functions, or entities. v1 uses BM25 over the existing text
    /// columns (no embedding-model dependency); semantic/vector
    /// search lands in v0.4.0 once an ONNX runtime is wired.
    Similar(SimilarArgs),
    /// Roadmap issue #32 (v0.3.0): list Type rows from the
    /// dedicated Type table (struct / enum / trait / module /
    /// type_alias). Filters: `--kind`, `--substring` (matched
    /// against `symbol`), `--top` for truncation.
    Types(TypesArgs),
    /// One-shot graph size summary: per-node-table and per-edge-table
    /// row counts, plus totals. Apples-to-apples comparison surface
    /// for benchmarking against external graph builders that report a
    /// single `nodes / edges` headline.
    GraphSummary,
    /// Coverage history: one row per past `doc-linter check` (docs,
    /// entities, functions, reach ratios, finding counts), newest rows
    /// last in the table view. `--limit` caps rows, `--json` prints the
    /// raw `{columns,row_count,rows}` newest-first.
    History(HistoryArgs),
}

/// Clap args for `query history`.
#[derive(clap::Args, Debug)]
pub struct HistoryArgs {
    /// Show at most this many of the newest snapshots.
    #[arg(long, default_value_t = 20)]
    pub limit: usize,

    /// Print JSON instead of a table.
    #[arg(long)]
    pub json: bool,
}

/// Clap args for `query types` — filtered listing of the Type
/// node table.
#[derive(clap::Args, Debug)]
pub struct TypesArgs {
    /// Optional kind filter — `struct` / `enum` / `trait` /
    /// `module` / `type_alias`. When omitted, every row matches.
    #[arg(long)]
    pub kind: Option<String>,

    /// Roadmap issue #32 v3: optional crate filter — matches
    /// `Type.crate` exactly. Combines AND-style with the other
    /// filters when set.
    #[arg(long = "crate", value_name = "CRATE")]
    pub krate: Option<String>,

    /// Optional substring filter on the SCIP `symbol` column. Case-
    /// sensitive, plain `CONTAINS` — SQL escape hatch users
    /// can drop down to `query sql` for regex.
    #[arg(long)]
    pub substring: Option<String>,

    /// Maximum number of rows to return. Defaults to 100; pass 0
    /// for unlimited.
    #[arg(long, default_value_t = 100)]
    pub top: usize,
}

/// Clap args for `query similar` — natural-language ranking over
/// docs / functions / entities. v1 is BM25 over the text columns
/// `Doc.summary` + `Doc.title`, `Function.doc_comment` +
/// `Function.signature`, or `Entity.display` + `Entity.description`.
#[derive(clap::Args, Debug)]
pub struct SimilarArgs {
    /// Free-text query — the natural-language string to rank
    /// against. Tokenised by lowercase + non-alphanumeric split;
    /// stopwords ignored.
    pub text: String,

    /// Search corpus to rank over. `doc` (default; title + summary),
    /// `function`, `entity`, `type`, `section` (heading-level slices of
    /// doc bodies), or `all` (combined ranking with a `kind` tag on
    /// each hit).
    #[arg(long, value_name = "KIND", default_value = "doc")]
    pub r#type: String,

    /// Number of top hits to return. The output is JSON
    /// `{query, type, hits: [{id, score, text_excerpt}, ...]}`.
    #[arg(long, default_value_t = 5)]
    pub top: usize,

    /// Roadmap issue #28 v5: max chars in each hit's
    /// `text_excerpt`. 160 is the v1 default — enough to spot
    /// the matched phrase plus a sentence of context. Agents
    /// wanting fuller code preview can raise to 300-500.
    #[arg(long, default_value_t = 160)]
    pub snippet_chars: usize,

    /// Roadmap issue #28 v7: drop hits whose score is below this
    /// floor. 0.0 (default) keeps every positive-score hit. BM25
    /// scores are unbounded above (bump to ~1.0 to skip weak
    /// lexical matches); cosine scores from the `embedding`
    /// backend are in `[-1, 1]` (bump to ~0.5 to skip "loosely
    /// related" matches).
    #[arg(long, default_value_t = 0.0)]
    pub min_score: f64,

    /// Roadmap issue #28 v4 (v0.4.0): ranking backend. `bm25`
    /// (default) uses lexical scoring over the existing
    /// title/body text columns; `embedding` uses cosine similarity
    /// over the FLOAT[384] vectors populated by v3's ingest pass.
    /// The `embedding` backend requires `--features embeddings` at
    /// build time AND a successful ingest run that populated the
    /// embedding columns (otherwise every row's vector is NULL
    /// and the search returns no hits).
    #[arg(long, value_name = "BACKEND", default_value = "bm25")]
    pub backend: String,

    /// Gap-010 phase 3: restrict ranking to rows whose `repo_id`
    /// matches. Today only the `Doc` table carries `repo_id`
    /// (phase 2 of gap-010); Entity / Function searches silently
    /// return all rows regardless of this filter. Use `list_repos`
    /// to enumerate valid ids; pass `None` (the default) to
    /// rank across every ingested repo.
    #[arg(long, value_name = "REPO_ID")]
    pub source_repo: Option<String>,

    /// Per [[research-repoformer]] (iter 83) + iter 88: minimum
    /// confidence band the response must meet. "low" (default,
    /// equivalent to no gate), "medium", or "high". When the
    /// computed band falls below the threshold, the response
    /// carries `met_threshold: false` AND an empty hits list — the
    /// agent reads the signal and decides whether to reformulate
    /// (RepoCoder iterate) or fall back to deterministic surfaces
    /// (sql / saved queries). When the band meets or exceeds
    /// the threshold, hits ARE returned and `met_threshold: true`.
    /// Composes with iter 85's confidence field: iter 85 is the
    /// signal, iter 88 is the server-side gate.
    #[arg(long, value_name = "BAND", default_value = "low")]
    pub min_confidence: String,

    /// Per iter 241 ([[feedback_meta_doc_displaces_research]]):
    /// restrict Doc-axis ranking to rows whose `tags` array
    /// contains this value. Common values: "research" (only
    /// research papers), "interrogation" (only meta-discussions),
    /// "user-probe" (only Type V probes). Composes with
    /// `without_tag` for finer routing. Today only the Doc
    /// axis honours this filter; Function / Entity / Type
    /// axes silently return all rows (the tag column is Doc-
    /// only in the current schema).
    #[arg(long, value_name = "TAG")]
    pub with_tag: Option<String>,

    /// Per iter 241 ([[feedback_meta_doc_displaces_research]]):
    /// drop Doc-axis hits whose `tags` array contains this
    /// value. Common values: "interrogation" / "user-probe"
    /// (exclude meta-docs so BM25 surfaces research-axis
    /// hits cleanly). Sibling to `with_tag`. Doc-axis only.
    #[arg(long, value_name = "TAG")]
    pub without_tag: Option<String>,
}

/// Serde shape for the [[entity-doc-graph]] `query list` JSON output —
/// total count plus the list of doc rows.
#[derive(Serialize)]
struct ListOutput {
    count: usize,
    docs: Vec<DocRow>,
}

/// Serde shape for `query list --ranked` — entities ordered by
/// FUNCTION_MENTIONS count, with the top-3 flagged `god_node`. Distinct
/// from `ListOutput` so the JSON consumer can branch on the presence of
/// `entities` vs `docs` without inspecting the rest of the document.
#[derive(Serialize)]
struct RankedEntityOutput {
    count: usize,
    entities: Vec<store::RankedEntityRow>,
}

/// One outbound or inbound link entry in the [[entity-doc-graph]] query
/// JSON output — target id, title, role, source line, and edge type.
#[derive(Serialize)]
struct LinkEntry {
    id: String,
    title: String,
    role: String,
    line: usize,
    edge_type: String,
}

/// Serde shape for the [[entity-doc-graph]] `query neighbors` JSON
/// output — node row plus its outbound link list.
#[derive(Serialize)]
struct NeighborsOutput {
    node: DocRow,
    outbound: Vec<LinkEntry>,
}

/// Serde shape for the [[entity-doc-graph]] `query backlinks` JSON
/// output — node row plus its inbound link list.
#[derive(Serialize)]
struct BacklinksOutput {
    node: DocRow,
    inbound: Vec<LinkEntry>,
}

/// One hop in a [[entity-doc-graph]] `query path` result — node id,
/// title, and the source line + edge type that linked from the previous
/// hop.
#[derive(Serialize)]
struct PathHop {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    via_line: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    via_edge_type: Option<String>,
}

/// Serde shape for the [[entity-doc-graph]] `query path` JSON output —
/// from/to ids, found flag, and the hop list.
#[derive(Serialize)]
struct PathOutput {
    from: String,
    to: String,
    found: bool,
    hops: Vec<PathHop>,
}

/// Serde shape for the [[entity-doc-graph]] `query subgraph` JSON
/// output — root row, depth, and the node/edge sets reached by the BFS.
#[derive(Serialize)]
struct SubgraphOutput {
    root: DocRow,
    depth: usize,
    nodes: Vec<DocRow>,
    edges: Vec<SubgraphEdge>,
}

/// One edge entry in the [[entity-doc-graph]] `query subgraph` JSON
/// output — from/to ids, source line, and edge type.
#[derive(Serialize)]
struct SubgraphEdge {
    from: String,
    to: String,
    line: usize,
    edge_type: String,
}

/// Serde shape for the [[entity-doc-graph]] `query context` JSON
/// output — node row plus its outbound + inbound link lists.
#[derive(Serialize)]
struct ContextOutput {
    node: DocRow,
    outbound: Vec<LinkEntry>,
    inbound: Vec<LinkEntry>,
}

/// Dispatches a parsed `QueryKind` to the matching subcommand handler
/// and returns the rendered JSON for the [[entity-doc-graph]] query CLI.
pub fn run(db: &Database, root: &Path, config: &LintConfig, kind: QueryKind) -> Result<String> {
    match kind {
        QueryKind::List(args) => list(db, args),
        QueryKind::Neighbors(args) => neighbors(db, args),
        QueryKind::Backlinks(args) => backlinks(db, args),
        QueryKind::Path(args) => path(db, args),
        QueryKind::Subgraph(args) => subgraph(db, args),
        QueryKind::Context(args) => context(db, args),
        QueryKind::Sql(args) => sql(db, args),
        QueryKind::FunctionsMentioning(args) => functions_mentioning(db, args),
        QueryKind::FunctionContext(args) => function_context(db, args),
        QueryKind::CoverageReport(args) => coverage_report(db, root, config, args),
        QueryKind::Endpoints(args) => endpoints(db, args),
        QueryKind::At(args) => at(db, args),
        QueryKind::Impact(args) => impact(db, args),
        QueryKind::DeadCode => dead_code(db),
        QueryKind::Map(args) => map(db, args),
        QueryKind::Saved(args) => saved(db, args),
        QueryKind::Schema => schema(db),
        QueryKind::Similar(args) => similar(db, args),
        QueryKind::Types(args) => types(db, args),
        QueryKind::GraphSummary => graph_summary(db),
        QueryKind::History(args) => history(db, args),
    }
}

/// Validates a list of `--edge-type` CLI values into a set of
/// `EdgeKind`s used to gate [[entity-doc-graph]] traversals.
fn parse_edge_filter(values: &[String]) -> Result<Option<HashSet<EdgeKind>>> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut set = HashSet::new();
    for v in values {
        let Some(kind) = EdgeKind::parse(v) else {
            bail!(
                "unknown --edge-type '{v}' \
                 (valid: wikilink, md-link, depends-on, \
                 informed-by, supersedes, crate-ref, covers)"
            );
        };
        set.insert(kind);
    }
    Ok(Some(set))
}

/// True when a single link row passes the active edge-type filter for
/// the [[entity-doc-graph]] query traversal.
fn link_passes(filter: &Option<HashSet<EdgeKind>>, l: &LinkRow) -> bool {
    let Some(f) = filter else { return true };
    let Some(k) = EdgeKind::parse(&l.edge_type) else {
        return false;
    };
    f.contains(&k)
}

/// Implements `query list` — applies the [[entity-doc-graph]]
/// frontmatter filters and renders the result as JSON.
///
/// @endpoint CLI list
fn list(db: &Database, args: ListArgs) -> Result<String> {
    if args.ranked {
        // --ranked replaces the doc-row output with ranked entities. Doc
        // filters are silently ignored — they don't compose with the
        // entity-ranking shape, and erroring on them would punish users
        // running `query list --kind entity --ranked` the way the spec
        // describes it.
        let entities = db.ranked_entities()?;
        return Ok(serde_json::to_string_pretty(&RankedEntityOutput {
            count: entities.len(),
            entities,
        })?);
    }
    let docs = db.list_all_docs()?;
    let docs: Vec<DocRow> = docs
        .into_iter()
        .filter(|m| match &args.role {
            Some(r) => &m.role == r,
            None => true,
        })
        .filter(|m| match &args.kind {
            Some(k) => m.kind.as_deref() == Some(k.as_str()),
            None => true,
        })
        .filter(|m| match &args.lifecycle {
            Some(l) => m.lifecycle.as_deref() == Some(l.as_str()),
            None => true,
        })
        .filter(|m| match &args.covers {
            Some(c) => m.covers.iter().any(|x| x == c),
            None => true,
        })
        .filter(|m| match &args.status {
            Some(s) => &m.status == s,
            None => true,
        })
        .filter(|m| match &args.tag {
            Some(t) => m.tags.iter().any(|tag| tag == t),
            None => true,
        })
        .collect();
    let out = ListOutput {
        count: docs.len(),
        docs,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Implements `query neighbors` — returns one node's outbound
/// [[entity-doc-graph]] links as JSON.
///
/// @endpoint CLI neighbors
fn neighbors(db: &Database, args: NeighborsArgs) -> Result<String> {
    let id = DocId::from(args.id.as_str());
    let Some(node) = db.get_doc(&id)? else {
        bail!("unknown doc id: {}", args.id);
    };
    let filter = parse_edge_filter(&args.edge_type)?;
    let mut outbound: Vec<LinkRow> = db.outbound(&id)?;
    outbound.retain(|l| link_passes(&filter, l));
    sort_links(&mut outbound);
    Ok(serde_json::to_string_pretty(&NeighborsOutput {
        node,
        outbound: outbound.into_iter().map(into_entry).collect(),
    })?)
}

/// Implements `query backlinks` — returns one node's inbound
/// [[entity-doc-graph]] links as JSON.
///
/// @endpoint CLI backlinks
fn backlinks(db: &Database, args: BacklinksArgs) -> Result<String> {
    let id = DocId::from(args.id.as_str());
    let Some(node) = db.get_doc(&id)? else {
        bail!("unknown doc id: {}", args.id);
    };
    let filter = parse_edge_filter(&args.edge_type)?;
    let mut inbound: Vec<LinkRow> = db.inbound(&id)?;
    inbound.retain(|l| link_passes(&filter, l));
    sort_links(&mut inbound);
    Ok(serde_json::to_string_pretty(&BacklinksOutput {
        node,
        inbound: inbound.into_iter().map(into_entry).collect(),
    })?)
}

/// Lifts a the store `LinkRow` into the JSON-shaped `LinkEntry` returned by
/// the [[entity-doc-graph]] query CLI.
fn into_entry(l: LinkRow) -> LinkEntry {
    LinkEntry {
        id: l.id,
        title: l.title,
        role: l.role,
        line: l.line,
        edge_type: l.edge_type,
    }
}

/// Sorts a link-row slice deterministically (id then line) so the
/// [[entity-doc-graph]] query JSON output is byte-stable across runs.
fn sort_links(rows: &mut [LinkRow]) {
    // Stable sort by (id, edge_type, line) so JSON output is deterministic
    // across the store engine versions (the store makes no MATCH ordering guarantees).
    rows.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.edge_type.cmp(&b.edge_type))
            .then_with(|| a.line.cmp(&b.line))
    });
}

/// Implements `query context` — node + outbound + inbound links in one
/// JSON blob over the [[entity-doc-graph]].
///
/// @endpoint CLI context
fn context(db: &Database, args: ContextArgs) -> Result<String> {
    let id = DocId::from(args.id.as_str());
    let Some(node) = db.get_doc(&id)? else {
        bail!("unknown doc id: {}", args.id);
    };
    let mut outbound = db.outbound(&id)?;
    let mut inbound = db.inbound(&id)?;
    sort_links(&mut outbound);
    sort_links(&mut inbound);
    Ok(serde_json::to_string_pretty(&ContextOutput {
        node,
        outbound: outbound.into_iter().map(into_entry).collect(),
        inbound: inbound.into_iter().map(into_entry).collect(),
    })?)
}

/// Hop bound used for the SHORTEST-path search. the store requires an explicit
/// max on `[* SHORTEST 1..N]` (no unbounded form), and the previous Rust
/// BFS had no limit. 10 is well past the vault's diameter (≤4 between any
/// two well-connected docs at audit time) while still cheap to evaluate.
const PATH_MAX_HOPS: u32 = 10;

/// Implements `query path` — the store shortest-path between two doc ids in
/// the [[entity-doc-graph]], rendered as JSON hops.
///
/// @endpoint CLI path
fn path(db: &Database, args: PathArgs) -> Result<String> {
    let from = DocId::from(args.from.as_str());
    let to = DocId::from(args.to.as_str());
    let Some(_from_doc) = db.get_doc(&from)? else {
        bail!("unknown doc id: {}", args.from);
    };
    let Some(_to_doc) = db.get_doc(&to)? else {
        bail!("unknown doc id: {}", args.to);
    };
    let filter = parse_edge_filter(&args.edge_type)?;

    // Single-query the store SHORTEST. Replaces the per-hop Rust BFS that
    // issued `2 * visited` round-trips. SQL pattern matches the
    // legacy semantics: undirected, Doc-Doc edges only.
    let steps = db.shortest_path(&from, &to, filter.as_ref(), PATH_MAX_HOPS)?;

    let found = !steps.is_empty();
    let hops: Vec<PathHop> = steps
        .into_iter()
        .map(|s| PathHop {
            id: s.id,
            title: s.title,
            via_line: s.via_line,
            via_edge_type: s.via_edge_type,
        })
        .collect();

    Ok(serde_json::to_string_pretty(&PathOutput {
        from: args.from,
        to: args.to,
        found,
        hops,
    })?)
}

/// Implements `query subgraph` — bounded BFS from a root id over the
/// [[entity-doc-graph]] returning the reached node + edge sets as JSON.
///
/// Roadmap issue #12: when `--entity` is set, dispatches to the
/// Entity-side ego-graph walker instead of the Doc traversal.
///
/// @endpoint CLI subgraph
fn subgraph(db: &Database, args: SubgraphArgs) -> Result<String> {
    if let Some(ref entity_id) = args.entity {
        // `args.depth` is already a usize; the entity walker uses u32
        // (BFS counter never exceeds u32::MAX in practice). Saturating
        // cast handles the unrealistic overflow case cleanly.
        let depth = u32::try_from(args.depth).unwrap_or(u32::MAX);
        let Some(result) = db.query_entity_subgraph(entity_id, depth)? else {
            bail!(
                "query subgraph --entity: unknown entity id `{entity_id}`. \
                 Run `query list --ranked` to see available ids."
            );
        };
        return Ok(serde_json::to_string_pretty(&result)?);
    }
    if args.id.is_empty() {
        bail!(
            "query subgraph: positional <id> is required when `--entity` is not set. \
             Pass a doc id or use `--entity <entity-id>` for the ego-graph view."
        );
    }
    let root_id = DocId::from(args.id.as_str());
    let Some(root_doc) = db.get_doc(&root_id)? else {
        bail!("unknown doc id: {}", args.id);
    };
    let filter = parse_edge_filter(&args.edge_type)?;

    // Treat the graph as undirected for context-gathering (matches legacy).
    let mut visited_ids: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();
    queue.push_back((args.id.clone(), 0));
    visited_ids.insert(args.id.clone());

    let mut edges: Vec<SubgraphEdge> = Vec::new();
    let mut emitted_edges: HashSet<(String, String, usize, String)> = HashSet::new();

    while let Some((id, depth)) = queue.pop_front() {
        if depth >= args.depth {
            continue;
        }
        for link in db.outbound(&DocId::from(id.as_str()))? {
            if let Some(k) = EdgeKind::parse(&link.edge_type) {
                if filter.as_ref().is_none_or(|f| f.contains(&k)) {
                    if visited_ids.insert(link.id.clone()) {
                        queue.push_back((link.id.clone(), depth + 1));
                    }
                    let key = (
                        id.clone(),
                        link.id.clone(),
                        link.line,
                        link.edge_type.clone(),
                    );
                    if emitted_edges.insert(key) {
                        edges.push(SubgraphEdge {
                            from: id.clone(),
                            to: link.id.clone(),
                            line: link.line,
                            edge_type: link.edge_type.clone(),
                        });
                    }
                }
            }
        }
        for link in db.inbound(&DocId::from(id.as_str()))? {
            if let Some(k) = EdgeKind::parse(&link.edge_type) {
                if filter.as_ref().is_none_or(|f| f.contains(&k)) {
                    if visited_ids.insert(link.id.clone()) {
                        queue.push_back((link.id.clone(), depth + 1));
                    }
                    // Inbound — record as (link.id -> id) since that's the
                    // edge's actual direction in the graph.
                    let key = (
                        link.id.clone(),
                        id.clone(),
                        link.line,
                        link.edge_type.clone(),
                    );
                    if emitted_edges.insert(key) {
                        edges.push(SubgraphEdge {
                            from: link.id.clone(),
                            to: id.clone(),
                            line: link.line,
                            edge_type: link.edge_type.clone(),
                        });
                    }
                }
            }
        }
    }

    let mut nodes: Vec<DocRow> = Vec::new();
    let mut id_list: Vec<String> = visited_ids.into_iter().collect();
    id_list.sort();
    for id in &id_list {
        if let Some(d) = db.get_doc(&DocId::from(id.as_str()))? {
            nodes.push(d);
        }
    }
    edges.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.to.cmp(&b.to))
            .then_with(|| a.edge_type.cmp(&b.edge_type))
            .then_with(|| a.line.cmp(&b.line))
    });

    Ok(serde_json::to_string_pretty(&SubgraphOutput {
        root: root_doc,
        depth: args.depth,
        nodes,
        edges,
    })?)
}

/// Implements `query map` (roadmap issue #39) — render a slice of
/// the graph as a Mermaid diagram. v1 supports the entity scope
/// only (reuses `query_entity_subgraph` from #12); module and
/// bounded-context scopes return a clear error pointing at the
/// entity fallback.
///
/// @endpoint CLI map
fn map(db: &Database, args: MapArgs) -> Result<String> {
    match args.scope.as_str() {
        "entity" => {
            let depth = u32::try_from(args.depth).unwrap_or(u32::MAX);
            let Some(subgraph) = db.query_entity_subgraph(&args.name, depth)? else {
                bail!(
                    "query map --scope entity: unknown entity id `{}`. Run \
                     `query list --ranked` to see available ids.",
                    args.name
                );
            };
            Ok(store::render_entity_mermaid(&subgraph))
        }
        "module" | "bounded-context" => bail!(
            "query map --scope {}: not implemented in v1. Use --scope entity for \
             the RELATES_TO neighbourhood. Module / bounded-context scopes will \
             land alongside roadmap #16 (IMPORTS edges) + #12's follow-up.",
            args.scope
        ),
        other => bail!(
            "query map: unknown --scope `{other}`. Valid: entity (default) — \
             module / bounded-context reserved for follow-up."
        ),
    }
}

/// Implements `query saved` (roadmap issue #29) — curated SQL
/// catalog. With `--list`, emits the catalog. With a name, runs
/// the named query against the supplied `--param` values.
///
/// @endpoint CLI saved
fn saved(db: &Database, args: SavedArgs) -> Result<String> {
    if args.list {
        let listing = store::list_saved_queries();
        return Ok(serde_json::to_string_pretty(&listing)?);
    }
    if args.name.is_empty() {
        bail!(
            "query saved: pass `--list` to enumerate the catalog, or a name (e.g. `orphan-entities`) to run one"
        );
    }
    let mut params: std::collections::HashMap<String, String> =
        std::collections::HashMap::with_capacity(args.params.len());
    for (k, v) in args.params {
        params.insert(k, v);
    }
    let value = db.run_saved_query(None, &args.name, &params)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Serde shape for `query dead-code` JSON output — total count plus
/// the matching Function rows.
#[derive(Serialize)]
struct DeadCodeOutput {
    count: usize,
    functions: Vec<store::DeadCodeRow>,
}

/// Implements `query dead-code` (roadmap issue #37) — Functions
/// with no inbound CALLS and no Endpoint binding. JSON shape
/// mirrors the other list-style queries (`{count, functions}`)
/// so an agent can pipe it through the same templates.
///
/// @endpoint CLI dead-code
fn dead_code(db: &Database) -> Result<String> {
    let rows = db.list_dead_code()?;
    let out = DeadCodeOutput {
        count: rows.len(),
        functions: rows,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Implements `query impact <symbol>` (roadmap issue #36) — the
/// transitive caller blast-radius for a Function symbol. Returns
/// `Err` when the symbol isn't in the Function table; the
/// underlying `query_impact` distinguishes "unknown symbol"
/// (Option::None) from "no callers" (empty depths array) so the
/// agent can tell those cases apart.
///
/// @endpoint CLI impact
fn impact(db: &Database, args: ImpactArgs) -> Result<String> {
    let Some(result) = db.query_impact(&args.symbol, args.depth)? else {
        bail!(
            "query impact: unknown function symbol `{}`. Run `query function-context <symbol>` \
             first to confirm the symbol is in the Function table.",
            args.symbol
        );
    };
    Ok(serde_json::to_string_pretty(&result)?)
}

/// Implements `query history`: the coverage-history snapshots that
/// `check` appends to the [[entity-doc-graph]] store.
///
/// @endpoint CLI history
fn history(db: &Database, args: HistoryArgs) -> Result<String> {
    let v = crate::store_sqlite::history::read(db, args.limit)?;
    if args.json {
        Ok(serde_json::to_string_pretty(&v)?)
    } else {
        Ok(crate::store_sqlite::history::render_table(&v)
            .trim_end()
            .to_string())
    }
}

/// Implements `query sql` — a read-only `SELECT` / `WITH` against the
/// SQLite graph, rendered as `{columns, row_count, rows}` JSON.
///
/// @endpoint CLI sql
fn sql(db: &Database, args: SqlArgs) -> Result<String> {
    let params = args
        .params
        .into_iter()
        .map(|(k, v)| (k, store::Value::Str(v)))
        .collect::<Vec<_>>();
    let bound = params
        .iter()
        .map(|(k, v)| (k.as_str(), v.clone()))
        .collect();
    let value = db.run_sql(&args.query, bound)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

/// Implements `query at <file>:<line>` (roadmap issue #25) — inverse
/// lookup from a source location to the surrounding graph context.
/// Parses the location, calls `store::query_at`, renders the
/// resulting struct as JSON.
///
/// Returns an error only on a malformed `<location>` string (no colon
/// or non-numeric line). An unmatched file or line returns a stable
/// JSON shape with null fields — see the `AtQueryResult` doc.
///
/// @endpoint CLI at
fn at(db: &Database, args: AtArgs) -> Result<String> {
    let (file, line) = parse_location(&args.location)?;
    let result = db.query_at(&file, line)?;
    Ok(serde_json::to_string_pretty(&result)?)
}

/// Parse `<path>:<line>` (e.g. `backend/server.py:142`). Returns the
/// file path verbatim and the 1-based line as `u32`. Path may
/// contain forward slashes; only the *last* colon separates path
/// from line, so Windows-style paths with drive letters still
/// parse cleanly.
fn parse_location(s: &str) -> Result<(String, u32)> {
    let (file, line) = s.rsplit_once(':').ok_or_else(|| {
        anyhow::anyhow!(
            "query at: location `{s}` must be `<file>:<line>` (e.g. backend/server.py:142)"
        )
    })?;
    let line: u32 = line.parse().map_err(|_| {
        anyhow::anyhow!("query at: line `{line}` is not a non-negative integer (location: `{s}`)")
    })?;
    Ok((file.to_string(), line))
}

/// One column entry in the [[entity-doc-graph]] schema introspection.
#[derive(Serialize)]
struct SchemaColumn {
    name: String,
    #[serde(rename = "type")]
    ty: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pk: bool,
}

/// One node or rel table entry in the schema introspection.
#[derive(Serialize)]
struct SchemaTable {
    label: String,
    /// `"node"` or `"rel"`.
    kind: String,
    columns: Vec<SchemaColumn>,
    row_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<&'static str>,
}

/// One example query bundled with the schema response — gives an
/// AI agent a cold-start library of patterns it can adapt. The graph is
/// SQLite: one table per node and rel kind, rel tables are
/// `(src, dst, props...)`, list columns live in `doc_tags` / `doc_covers` /
/// ... side tables.
#[derive(Serialize)]
struct SchemaExample {
    name: &'static str,
    sql: &'static str,
}

/// Top-level shape for `query schema` JSON output.
#[derive(Serialize)]
struct SchemaOutput {
    nodes: Vec<SchemaTable>,
    edges: Vec<SchemaTable>,
    examples: &'static [SchemaExample],
}

/// Curated example queries the README catalogs — bundled into the
/// schema response so an agent can self-discover patterns without a
/// round-trip to the docs. Order intentionally orchestral: simplest
/// first, structural next, then code-graph + coverage-flavored.
const SCHEMA_EXAMPLES: &[SchemaExample] = &[
    SchemaExample {
        name: "list every Doc",
        sql: "SELECT id, title FROM Doc",
    },
    SchemaExample {
        name: "every doc that covers an entity",
        sql: "SELECT src AS doc, dst AS entity FROM COVERS",
    },
    SchemaExample {
        name: "every function whose doc-comment mentions an entity",
        sql: "SELECT src AS symbol FROM FUNCTION_MENTIONS WHERE dst = 'pricing-rule'",
    },
    SchemaExample {
        name: "Function → Function call edges from a given caller",
        sql: "SELECT dst AS symbol FROM CALLS WHERE src = $sym",
    },
    SchemaExample {
        name: "Entity-level call density (derived)",
        sql: "SELECT src AS caller, dst AS callee, frequency, weight FROM ENTITY_CALLS ORDER BY frequency DESC",
    },
    SchemaExample {
        name: "functions defined in a specific file",
        sql: "SELECT src AS symbol FROM DEFINED_IN_FILE WHERE dst = 'src/lib.rs'",
    },
    SchemaExample {
        name: "every endpoint with its handler symbol",
        sql: "SELECT src AS endpoint, dst AS handler FROM ENDPOINT_HANDLED_BY",
    },
];

/// Human-readable one-liners for the tables this schema knows about.
/// Falls back to no description for tables not in the map.
fn table_description(label: &str) -> Option<&'static str> {
    match label {
        "Doc" => Some("Every parsed markdown doc with frontmatter."),
        "Entity" => Some("Every domain entity from the ontology."),
        "Section" => Some("Heading-level slice of a doc body; id `<doc-id>#<anchor>`, `text` searchable via query_similar type=section."),
        "SECTION_OF" => Some("Section→Doc — the doc a heading-level section belongs to."),
        "Function" => Some(
            "Code symbols extracted from SCIP — functions, methods, structs, enums, traits, modules.",
        ),
        "Endpoint" => Some(
            "API surface points — axum HTTP routes, clap subcommands, MCP tools.",
        ),
        "File" => Some("First-class source file (Rust / Python / TS); replaces the legacy Function.file string column."),
        "Module" => Some("Coarse code grouping — Rust crates (Cargo.toml dir) and Python packages (__init__.py dir)."),
        "WIKILINK" | "MD_LINK" | "DEPENDS_ON" | "INFORMED_BY" | "SUPERSEDES" | "CRATE_REF" => {
            Some("Doc→Doc edge derived from wikilinks, markdown links, or frontmatter declarations.")
        }
        "COVERS" => Some("Doc→Entity — what concepts a doc explains."),
        "RELATES_TO" => Some("Author-declared Entity→Entity relationship from ontology frontmatter."),
        "FUNCTION_MENTIONS" => Some("Function→Entity — entity mentioned in a function's doc-comment or symbol path."),
        "FUNCTION_DEFINED_IN" => Some("Function→Doc — the crate README that describes a function's defining crate."),
        "FUNCTION_BELONGS_TO" => Some("Function→Entity — entity that owns a function via `source_modules:` glob match."),
        "CALLS" => Some("Function→Function — call graph derived from SCIP reference occurrences."),
        "ENTITY_CALLS" => Some("Entity→Entity — derived structural call density (caller-fn's entity → callee-fn's entity)."),
        "ENDPOINT_HANDLED_BY" => Some("Endpoint→Function — which function implements an endpoint."),
        "ENDPOINT_TOUCHES_ENTITY" => Some("Endpoint→Entity — which entities an endpoint exposes."),
        "DEFINED_IN_FILE" => Some("Function→File — first-class file membership."),
        "IN_MODULE" => Some("File→Module — file's enclosing crate / package."),
        "IMPORTS_MODULE" => Some("Module→Module — import-statement derived (population deferred to a follow-up)."),
        "DESCRIBED_BY" => Some("Module→Doc — module's README."),
        _ => None,
    }
}

/// Implements `query schema` — table and column introspection plus a
/// per-table row count, returned as JSON. Cold-start agents
/// use this in lieu of reading the source code.
///
/// @endpoint CLI schema
fn schema(db: &Database) -> Result<String> {
    let mut nodes: Vec<SchemaTable> = Vec::new();
    let mut edges: Vec<SchemaTable> = Vec::new();
    for t in db.schema_tables()? {
        let entry = SchemaTable {
            description: table_description(&t.name),
            label: t.name,
            kind: if t.is_rel { "rel" } else { "node" }.to_string(),
            columns: t
                .columns
                .into_iter()
                .map(|c| SchemaColumn {
                    name: c.name,
                    ty: c.ty,
                    pk: c.pk,
                })
                .collect(),
            row_count: t.row_count,
        };
        if t.is_rel {
            edges.push(entry);
        } else {
            nodes.push(entry);
        }
    }
    nodes.sort_by(|a, b| a.label.cmp(&b.label));
    edges.sort_by(|a, b| a.label.cmp(&b.label));

    let out = SchemaOutput {
        nodes,
        edges,
        examples: SCHEMA_EXAMPLES,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Implements `query graph-summary` — apples-to-apples graph size for
/// benchmarking. Returns per-node-table and per-edge-table row counts
/// plus totals, no column metadata or examples — that's `query
/// schema`'s job. The JSON shape is stable so a comparison script can
/// pipe two runs through `jq` and report deltas.
fn graph_summary(db: &Database) -> Result<String> {
    let mut nodes: Vec<GraphSummaryEntry> = Vec::new();
    let mut edges: Vec<GraphSummaryEntry> = Vec::new();
    for t in db.schema_tables()? {
        let entry = GraphSummaryEntry {
            label: t.name,
            count: t.row_count,
        };
        if t.is_rel {
            edges.push(entry);
        } else {
            nodes.push(entry);
        }
    }
    nodes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));
    edges.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.label.cmp(&b.label)));

    let total_nodes: u64 = nodes.iter().map(|e| e.count).sum();
    let total_edges: u64 = edges.iter().map(|e| e.count).sum();

    let out = GraphSummaryOutput {
        total_nodes,
        total_edges,
        nodes,
        edges,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

#[derive(Serialize)]
struct GraphSummaryEntry {
    label: String,
    count: u64,
}

#[derive(Serialize)]
struct GraphSummaryOutput {
    total_nodes: u64,
    total_edges: u64,
    nodes: Vec<GraphSummaryEntry>,
    edges: Vec<GraphSummaryEntry>,
}

/// Serde shape for the [[entity-doc-graph]] `query functions-mentioning`
/// JSON output — the entity id, count, and the matching function rows.
#[derive(Serialize)]
struct FunctionsMentioningOutput {
    entity: String,
    count: usize,
    functions: Vec<store::FunctionRow>,
}

/// Implements `query functions-mentioning` — every function whose
/// doc-comment carries an [[entity-doc-graph]] mention of the supplied
/// entity.
///
/// @endpoint CLI functions-mentioning
fn functions_mentioning(db: &Database, args: FunctionsMentioningArgs) -> Result<String> {
    let entity = EntityId::from(args.entity.as_str());
    let rows = db.functions_mentioning(&entity)?;
    let out = FunctionsMentioningOutput {
        entity: args.entity,
        count: rows.len(),
        functions: rows,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Implements `query function-context` — the full Function row plus
/// every Doc / Entity it links to in the [[entity-doc-graph]].
///
/// @endpoint CLI function-context
fn function_context(db: &Database, args: FunctionContextArgs) -> Result<String> {
    let symbol = FunctionSymbol::from(args.symbol.as_str());
    let Some(ctx) = db.function_context(&symbol)? else {
        bail!("unknown function symbol: {}", args.symbol);
    };
    Ok(serde_json::to_string_pretty(&ctx)?)
}

/// Phase 0 of roadmap-43. Builds the coverage report from the live the store
/// graph and emits it as JSON. With `--write`, also regenerates
/// `<root>/docs/coverage-report.md` so the human-readable status doc
/// lives in the vault.
///
/// @endpoint CLI coverage-report
/// Operates on [[entity-coverage]] and [[entity-doc-graph]].
fn coverage_report(
    db: &Database,
    root: &Path,
    config: &LintConfig,
    args: CoverageReportArgs,
) -> Result<String> {
    let filters = ReportFilters {
        crate_filter: args.crate_filter,
        entity_filter: args.entity_filter,
    };
    let report = coverage::build_report(db, config, &filters)?;

    if args.write {
        // Today's date is used only for the `updated:` frontmatter
        // field. The body itself is data-driven and should not vary
        // unless the underlying numbers change — `write_markdown_report`
        // honours that with a body-equality check before rewriting.
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let target = root.join("docs/coverage-report.md");
        let _changed = coverage::write_markdown_report(&target, &report, &today)?;
    }

    Ok(serde_json::to_string_pretty(&report)?)
}

/// Phase 2 of roadmap-43: list endpoints with entity coverage.
///
/// @endpoint CLI endpoints
fn endpoints(db: &Database, args: EndpointsArgs) -> Result<String> {
    if let Some(k) = &args.kind {
        match k.as_str() {
            "axum" | "clap" | "mcp" => {}
            _ => bail!("unknown --kind '{k}' (valid: axum, clap, mcp)"),
        }
    }
    let rows = db.list_endpoints(args.kind.as_deref(), args.dark)?;
    Ok(serde_json::to_string_pretty(&rows)?)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::float_cmp,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::{
        bm25_search, confidence_band, confidence_rank, score_signals, tokenize,
        type_axis_source_repo_message, unknown_type_message, ConfidenceSignals, SimilarHit,
    };

    #[test]
    fn confidence_rank_low_medium_high_ordering() {
        assert!(confidence_rank("low") < confidence_rank("medium"));
        assert!(confidence_rank("medium") < confidence_rank("high"));
        assert_eq!(confidence_rank("LOW"), confidence_rank("low"));
        assert_eq!(confidence_rank("High"), confidence_rank("high"));
    }

    #[test]
    fn confidence_rank_unknown_treated_as_low() {
        // Unknown input is permissive — same rank as the default ("low").
        assert_eq!(confidence_rank("nonsense"), confidence_rank("low"));
        assert_eq!(confidence_rank(""), confidence_rank("low"));
    }

    #[test]
    fn confidence_rank_high_threshold_requires_high_band() {
        // The agent-facing gate: high threshold requires high band.
        let high_threshold = confidence_rank("high");
        assert!(confidence_rank("high") >= high_threshold);
        assert!(confidence_rank("medium") < high_threshold);
        assert!(confidence_rank("low") < high_threshold);
    }

    #[test]
    fn confidence_rank_medium_threshold_passes_medium_and_high() {
        let med_threshold = confidence_rank("medium");
        assert!(confidence_rank("high") >= med_threshold);
        assert!(confidence_rank("medium") >= med_threshold);
        assert!(confidence_rank("low") < med_threshold);
    }

    fn hit(score: f64) -> SimilarHit {
        SimilarHit {
            id: "x".to_string(),
            score,
            text_excerpt: "x".to_string(),
            kind: None,
            role: None,
            doc_kind: None,
            lifecycle: None,
            tags: None,
            covers: None,
            doc_id: None,
        }
    }

    #[test]
    fn confidence_band_high_on_dominant_top_score_bm25() {
        // research-deepcode probe shape — top 24, second 14, spread 10.
        let s = ConfidenceSignals {
            top_score: 24.0,
            spread: 10.0,
        };
        assert_eq!(confidence_band(&s, "bm25"), "high");
    }

    #[test]
    fn confidence_band_high_on_strong_spread_even_with_moderate_top_bm25() {
        // Modest top but clear winner over the rest.
        let s = ConfidenceSignals {
            top_score: 10.0,
            spread: 6.0,
        };
        assert_eq!(confidence_band(&s, "bm25"), "high");
    }

    #[test]
    fn confidence_band_medium_on_workable_score_bm25() {
        // interrogation-016 Q3 zone — top 9.5, no real spread.
        let s = ConfidenceSignals {
            top_score: 9.5,
            spread: 0.5,
        };
        assert_eq!(confidence_band(&s, "bm25"), "medium");
    }

    #[test]
    fn confidence_band_low_when_top_score_zero() {
        // Empty hit list.
        let s = ConfidenceSignals {
            top_score: 0.0,
            spread: 0.0,
        };
        assert_eq!(confidence_band(&s, "bm25"), "low");
    }

    #[test]
    fn confidence_band_low_when_weak_top_and_no_spread_bm25() {
        let s = ConfidenceSignals {
            top_score: 3.0,
            spread: 0.2,
        };
        assert_eq!(confidence_band(&s, "bm25"), "low");
    }

    #[test]
    fn confidence_band_embedding_uses_scaled_thresholds() {
        // Cosine 0.7 with a clear lead is "high" even though 0.7 << BM25 20.
        let s = ConfidenceSignals {
            top_score: 0.7,
            spread: 0.09,
        };
        assert_eq!(confidence_band(&s, "embedding"), "high");
        // Handoff cpg-os-vault #5: 0.742 vs 0.697 (spread 0.045) — a wrong
        // top hit in a near-tie was rated "high". Now capped at medium.
        let s = ConfidenceSignals {
            top_score: 0.742,
            spread: 0.045,
        };
        assert_eq!(confidence_band(&s, "embedding"), "medium");
        // Cosine 0.5 is medium.
        let s = ConfidenceSignals {
            top_score: 0.5,
            spread: 0.02,
        };
        assert_eq!(confidence_band(&s, "embedding"), "medium");
        // Cosine 0.3 is low.
        let s = ConfidenceSignals {
            top_score: 0.3,
            spread: 0.01,
        };
        assert_eq!(confidence_band(&s, "embedding"), "low");
    }

    #[test]
    fn score_signals_extracts_top_and_spread() {
        let hits = vec![hit(20.0), hit(12.0), hit(5.0)];
        let s = score_signals(&hits);
        assert_eq!(s.top_score, 20.0);
        assert_eq!(s.spread, 8.0);
    }

    #[test]
    fn score_signals_empty_hits_zero() {
        let s = score_signals(&[]);
        assert_eq!(s.top_score, 0.0);
        assert_eq!(s.spread, 0.0);
    }

    #[test]
    fn score_signals_single_hit_zero_spread() {
        let hits = vec![hit(15.0)];
        let s = score_signals(&hits);
        assert_eq!(s.top_score, 15.0);
        assert_eq!(s.spread, 0.0);
    }

    #[test]
    fn unknown_type_message_names_sql_fallback_for_known_tables() {
        // iter 80: `type` is now a first-class axis (no longer in the
        // recipe-emission list). Endpoint / Module / File / Finding /
        // Migration remain unaddressable via query_similar.
        for axis in ["endpoint", "module", "file", "finding", "migration"] {
            let msg = unknown_type_message(axis);
            assert!(
                msg.contains("SELECT"),
                "missing sql recipe for `{axis}`: {msg}"
            );
            assert!(
                msg.contains("FROM Endpoint")
                    || msg.contains("FROM Module")
                    || msg.contains("FROM File")
                    || msg.contains("FROM Finding")
                    || msg.contains("FROM Migration"),
                "missing node label for `{axis}`: {msg}"
            );
            assert!(
                msg.contains("query_at") || msg.contains("function_context"),
                "missing alt-tool hint for `{axis}`: {msg}"
            );
        }
    }

    #[test]
    fn unknown_type_message_keeps_bare_form_for_truly_unknown() {
        let msg = unknown_type_message("nonsense");
        assert!(msg.contains("valid: doc, function, entity, type, section, all"));
        assert!(
            !msg.contains("SELECT"),
            "bare form should not embed a recipe: {msg}"
        );
    }

    #[test]
    fn type_axis_source_repo_message_names_constraint_and_recovery() {
        let msg = type_axis_source_repo_message();
        assert!(
            msg.contains("source_repo"),
            "msg should name the offending arg: {msg}"
        );
        assert!(msg.contains("repo_id"), "msg should explain why: {msg}");
        assert!(
            msg.contains("`sql`"),
            "msg should name the fallback tool: {msg}"
        );
        assert!(
            msg.contains("FROM Type"),
            "msg should embed the sql recipe: {msg}"
        );
        assert!(
            msg.contains("function") && msg.contains("entity") && msg.contains("doc"),
            "msg should name the axes that DO accept source_repo: {msg}"
        );
    }

    use super::parse_location;

    #[test]
    fn parse_location_handles_repo_relative_path_and_line() {
        let (f, l) = parse_location("backend/server.py:142").unwrap();
        assert_eq!(f, "backend/server.py");
        assert_eq!(l, 142);
    }

    #[test]
    fn parse_location_uses_rightmost_colon_for_split() {
        // Windows-style absolute paths with a drive letter contain an
        // earlier colon — `rsplit_once` picks the rightmost one so
        // these still parse.
        let (f, l) = parse_location("C:/code/lib.rs:55").unwrap();
        assert_eq!(f, "C:/code/lib.rs");
        assert_eq!(l, 55);
    }

    #[test]
    fn parse_location_rejects_missing_colon() {
        let err = parse_location("just-a-path").unwrap_err();
        assert!(err.to_string().contains("must be `<file>:<line>`"));
    }

    #[test]
    fn parse_location_rejects_non_numeric_line() {
        let err = parse_location("src/lib.rs:NaN").unwrap_err();
        assert!(err.to_string().contains("not a non-negative integer"));
    }

    // Roadmap issue #28 (v0.3.0): BM25 unit tests.

    #[test]
    fn tokenize_lowercases_and_drops_punct() {
        let toks = tokenize("Hello, World! How_are_you?");
        // hello, world, how, are, you — "are" is a stopword, dropped.
        assert!(toks.iter().any(|t| t == "hello"));
        assert!(toks.iter().any(|t| t == "world"));
        assert!(!toks.iter().any(|t| t == "are"));
    }

    #[test]
    fn bm25_ranks_exact_match_above_unrelated() {
        let docs = vec![
            (
                "auth".to_string(),
                "How authentication works".to_string(),
                "JWT auth pipeline".to_string(),
            ),
            (
                "billing".to_string(),
                "Billing reconciliation".to_string(),
                "Stripe webhook handler".to_string(),
            ),
        ];
        let hits = bm25_search("authentication", &docs, 2, 160);
        // Only the "auth" doc shares a token with the query — the
        // unrelated billing doc gets a 0 score and is filtered out.
        assert!(!hits.is_empty(), "auth doc should match");
        assert_eq!(hits[0].id, "auth", "auth doc should rank first");
        assert!(hits[0].score > 0.0);
    }

    #[test]
    fn bm25_empty_corpus_returns_empty() {
        let hits = bm25_search("anything", &[], 5, 160);
        assert!(hits.is_empty());
    }

    #[test]
    fn bm25_empty_query_returns_empty() {
        let docs = vec![("a".to_string(), "Foo".to_string(), "Bar".to_string())];
        let hits = bm25_search("", &docs, 5, 160);
        assert!(hits.is_empty());
    }

    // Per iter-398 BM25 silent-miss fix (iter-385/387/390/397
    // 4-instance): the iter-397 normalizer probe showed BM25
    // returning only entity-llm for query 'normalize LLM response'
    // because entity-normalizer's description didn't repeat the
    // word 'normalize'. With the iter-398 fix prepending entity ID
    // to body, queries matching the ID directly should rank the
    // target entity ABOVE generic entities whose description
    // happens to contain a query token. This test pins that
    // behavior by simulating the entity-corpus AFTER the
    // id-prepend step.
    #[test]
    fn bm25_entity_id_token_lifts_target_above_noise() {
        // After iter-398, fetch_entity_corpus_filtered prepends
        // id to body. Simulate that shape here:
        let entity_corpus = vec![
            (
                "cuda-error".to_string(),
                "CUDA Error".to_string(),
                // Body = id + ' ' + description (post-iter-398).
                // Note: original description doesn't mention 'cuda'.
                "cuda-error GPU error class; one of the detector signatures for stuck-loop diagnosis.".to_string(),
            ),
            (
                "llm".to_string(),
                "LLM".to_string(),
                // Generic entity that matches via 'LLM' but not 'cuda'.
                "llm Large language model behind the runtime endpoints.".to_string(),
            ),
        ];
        let hits = bm25_search("cuda errors during embedding", &entity_corpus, 5, 160);
        assert!(!hits.is_empty(), "fix must surface cuda-error entity");
        assert_eq!(
            hits[0].id, "cuda-error",
            "iter-398 id-prepend must lift cuda-error above llm for 'cuda' query"
        );
    }

    // Per iter-401 (refinement of iter-398): bm25 body also
    // includes entity DISPLAY tokens so natural-language queries
    // ('Response Normalizer' vs id-token 'normalizer') still
    // match.
    #[test]
    fn bm25_entity_display_tokens_lift_target_for_natural_language_query() {
        // After iter-401, fetch_entity_corpus_filtered prepends
        // id AND display to body. Simulate that shape here.
        let entity_corpus = vec![
            (
                "normalizer".to_string(),
                "Response Normalizer".to_string(),
                // Body = id + ' ' + display + ' ' + description.
                // Description doesn't repeat the term 'response'.
                "normalizer Response Normalizer Strip + lowercase utility for cleanup before parsing."
                    .to_string(),
            ),
            (
                "llm".to_string(),
                "LLM".to_string(),
                // Generic entity that matches 'LLM' but not 'response'.
                "llm LLM Large language model behind the runtime endpoints.".to_string(),
            ),
        ];
        let hits = bm25_search("response normalizer for LLM output", &entity_corpus, 5, 160);
        assert!(!hits.is_empty(), "fix must surface normalizer entity");
        assert_eq!(
            hits[0].id, "normalizer",
            "iter-401 display-prepend must lift normalizer above llm for 'response normalizer' query"
        );
    }

    #[test]
    fn excerpt_window_centers_on_first_match() {
        // 200-char body with the token "rule" in the middle.
        let body = format!(
            "{} the pricing rule applies {}",
            "a".repeat(80),
            "b".repeat(80)
        );
        let tokens = vec!["rule".to_string()];
        let out = super::excerpt_around_match(&body, &tokens, 60);
        assert!(
            out.contains("rule"),
            "excerpt must contain the match: {out}"
        );
        // Wider context — leading ellipsis since the window starts
        // past char 0.
        assert!(
            out.starts_with('…'),
            "should prefix with ellipsis when window starts past 0"
        );
    }

    #[test]
    fn excerpt_window_falls_back_to_head_when_no_match() {
        let body = "Plain prose with nothing matching.";
        let tokens = vec!["zebra".to_string()];
        let out = super::excerpt_around_match(body, &tokens, 60);
        // Falls back to head behaviour — no leading ellipsis.
        assert_eq!(out, body);
    }

    #[test]
    fn excerpt_window_handles_match_near_end() {
        let body = format!("{} pricing rule", "x".repeat(300));
        let tokens = vec!["rule".to_string()];
        let out = super::excerpt_around_match(&body, &tokens, 50);
        assert!(out.contains("rule"));
        assert!(
            out.starts_with('…'),
            "near-end match should prefix with ellipsis"
        );
        // No trailing ellipsis when the window reaches the end.
        assert!(out.ends_with("rule"), "got: {out}");
    }

    // Roadmap issue #28 v4 (v0.4.0): embedding backend unit tests.
    // The DB-touching round-trip is covered by tests/query_similar.rs;
    // here we exercise the pure functions (cosine ranker, snippet
    // helper, dispatcher input validation).

    #[test]
    fn snippet_excerpt_trims_at_word_boundary() {
        let body = "alpha beta gamma delta epsilon zeta eta theta";
        let out = super::snippet_excerpt(body, 20);
        // 20 chars lands inside "epsilon"; helper backs up to the
        // preceding space so the snippet doesn't cut a word.
        assert!(out.ends_with("…"), "long body should ellipsize: {out}");
        assert!(
            !out.contains("epsilo…") && !out.contains("psilo…"),
            "must not split a word: {out}"
        );
    }

    #[test]
    fn snippet_excerpt_returns_body_verbatim_when_short() {
        let body = "short";
        assert_eq!(super::snippet_excerpt(body, 100), "short");
    }
}

/// Serde shape for one `query similar` hit.
#[derive(Debug, Serialize)]
struct SimilarHit {
    id: String,
    score: f64,
    text_excerpt: String,
    /// Roadmap issue #28 v3: the corpus this hit came from
    /// (`doc` / `function` / `entity`). Only emitted when
    /// `--type all` mixes corpora; per-corpus searches omit it
    /// (back-compat with the v1 wire format).
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
    // interrogation-001 gap-B: fused hit envelope. When the hit
    // is a Doc, project frontmatter alongside score so callers
    // don't pay N+1 round-trips to assemble a CBR-shaped case
    // (problem-features → solution → outcome). All Option so the
    // wire format stays backward-compatible for non-doc hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lifecycle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    covers: Option<Vec<String>>,
    /// Parent Doc id for section hits (`<doc-id>#<anchor>`).
    #[serde(skip_serializing_if = "Option::is_none")]
    doc_id: Option<String>,
}

/// Top-level shape for the `query similar` JSON output.
#[derive(Serialize)]
struct SimilarOutput {
    query: String,
    r#type: String,
    /// "bm25" today; "embedding" once the v0.4.0 ONNX path lands.
    algorithm: String,
    /// Roadmap issue #28 v6: size of the searched corpus
    /// (rows considered, not edges). Lets an agent tell "no
    /// hits because empty corpus" from "no hits because no
    /// match". For `--type all`, this is the sum across
    /// docs + functions + entities.
    corpus_size: usize,
    hits: Vec<SimilarHit>,
    /// Per [[research-repoformer]] (iter 83) and gap-bm25-confidence-
    /// vocabulary-mismatch ([[interrogation-016]] iter 84): a per-
    /// query confidence signal so the agent can decide whether to
    /// trust the hits, iterate (RepoCoder-style reformulated query),
    /// or fall back to deterministic surfaces (e.g. sql or saved
    /// queries). Heuristic is corpus-agnostic — uses absolute top
    /// score AND the spread between top-1 and the second-best hit.
    /// `confidence: "low"` for an empty hit list.
    confidence: String,
    /// Raw signals the heuristic was derived from, so an agent can
    /// rebuild the decision with its own threshold if needed.
    confidence_signals: ConfidenceSignals,
    /// Per iter 88: did the response's confidence meet or exceed
    /// `args.min_confidence`? When false, `hits` is emptied to
    /// signal "don't trust this" — the agent reads `met_threshold`,
    /// `confidence`, and `confidence_signals` to decide whether to
    /// iterate (RepoCoder), reformulate the query, or fall back to
    /// deterministic surfaces.
    met_threshold: bool,
}

/// Per-query confidence signals exposed alongside the categorical
/// `confidence` band. `top_score` is the rank-1 BM25 (or cosine)
/// score; `spread` is `top_score - second_score` (0.0 when fewer
/// than two hits). Per [[research-repoformer]], these two together
/// approximate the "should I trust this?" question without paying
/// the training cost of a learned classifier.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
struct ConfidenceSignals {
    top_score: f64,
    spread: f64,
}

/// Implements `query similar` — BM25 ranking over Doc / Function /
/// Entity text columns. v1 has no embedding-model dependency; the
/// ONNX semantic-search path lands in v0.4.0 (issue #28).
///
/// @endpoint CLI similar
fn similar(db: &Database, args: SimilarArgs) -> Result<String> {
    let out = similar_for_mcp(db, args)?;
    Ok(serde_json::to_string_pretty(&out)?)
}

/// Same as [`similar`] but returns the structured [`serde_json::Value`]
/// instead of a serialized string. Used by the MCP server so it can
/// wrap the result in its own `content` envelope without re-parsing.
pub fn similar_for_mcp(db: &Database, args: SimilarArgs) -> Result<serde_json::Value> {
    let kind = args.r#type.to_ascii_lowercase();
    let snippet = args.snippet_chars.max(40);
    let backend = args.backend.to_ascii_lowercase();
    let source_repo = args.source_repo.as_deref();
    let with_tag = args.with_tag.as_deref();
    let without_tag = args.without_tag.as_deref();
    let (mut hits, corpus_size, algorithm) = match backend.as_str() {
        "bm25" => {
            let (h, n) = match kind.as_str() {
                "doc" => {
                    let corpus = fetch_doc_corpus_filtered(db, source_repo, with_tag, without_tag)?;
                    let size = corpus.len();
                    (bm25_search(&args.text, &corpus, args.top, snippet), size)
                }
                "function" => {
                    let corpus = fetch_function_corpus_filtered(db, source_repo)?;
                    let size = corpus.len();
                    (bm25_search(&args.text, &corpus, args.top, snippet), size)
                }
                "entity" => {
                    let corpus = fetch_entity_corpus_filtered(db, source_repo)?;
                    let size = corpus.len();
                    (bm25_search(&args.text, &corpus, args.top, snippet), size)
                }
                "section" => {
                    let corpus = fetch_section_corpus(db, source_repo, with_tag, without_tag)?;
                    let size = corpus.len();
                    (bm25_search(&args.text, &corpus, args.top, snippet), size)
                }
                "type" => {
                    if source_repo.is_some() {
                        bail!(type_axis_source_repo_message());
                    }
                    let corpus = fetch_type_corpus(db)?;
                    let size = corpus.len();
                    (bm25_search(&args.text, &corpus, args.top, snippet), size)
                }
                "all" => bm25_search_all_corpora(
                    db,
                    &args.text,
                    args.top,
                    snippet,
                    source_repo,
                    with_tag,
                    without_tag,
                )?,
                other => bail!(unknown_type_message(other)),
            };
            (h, n, "bm25")
        }
        "embedding" => {
            let (h, n) = embedding_search_dispatch(
                db,
                &kind,
                &args.text,
                args.top,
                snippet,
                DocFilters {
                    source_repo,
                    with_tag,
                    without_tag,
                },
            )?;
            (h, n, "embedding")
        }
        other => {
            // Per iter 124 (interrogation-025 Finding C + D
            // discoverability gap): list only the backends
            // available in THIS build/runtime, not the closed
            // enum the source declares. An embedding-less
            // build that mentions `embedding` as "valid" is
            // misleading — the agent's next call would still
            // fail. With dynamic enumeration the agent can
            // make a clean retry decision.
            let embedding_available = crate::embeddings::default_embedder().is_available();
            let avail = if embedding_available {
                "bm25, embedding"
            } else {
                "bm25"
            };
            let suffix = if embedding_available {
                String::new()
            } else {
                String::from(
                    " (embedding backend is not available in this build — \
                     to enable, rebuild with `--features embeddings` and set \
                     DOC_LINTER_EMBED_MODEL; check `query_schema` → \
                     `retrieval_backends` to confirm availability)",
                )
            };
            bail!("unknown --backend '{other}' (available: {avail}).{suffix}")
        }
    };
    // Roadmap issue #28 v7: drop hits below `min_score` floor.
    // Applied AFTER ranking + truncation so an agent requesting
    // `top=10` + `min_score=2.0` gets at most 10 hits all above
    // the floor. The floor's units depend on backend (BM25 is
    // unbounded; cosine is `[-1, 1]`).
    if args.min_score > 0.0 {
        hits.retain(|h| h.score >= args.min_score);
    }
    for h in &mut hits {
        if kind == "section" || h.kind.as_deref() == Some("section") {
            h.doc_id = h.id.split_once('#').map(|(d, _)| d.to_string());
        }
    }
    // interrogation-001 gap-B: fused hit envelope. For doc-type
    // searches, decorate each hit with its frontmatter (role,
    // kind, lifecycle, tags, covers) in one extra SQL call so
    // the calling agent can synthesize a CBR-shaped case without
    // an N+1 round-trip to query_doc per hit. Mixed-corpus
    // "all"-type queries get the same decoration for the Doc rows
    // they contain — non-Doc hits are left unchanged.
    if matches!(kind.as_str(), "doc" | "all") && !hits.is_empty() {
        enrich_doc_hits(db, &mut hits)?;
    }
    let confidence_signals = score_signals(&hits);
    let confidence = confidence_band(&confidence_signals, algorithm);
    let min_confidence_rank = confidence_rank(&args.min_confidence);
    let computed_rank = confidence_rank(&confidence);
    let met_threshold = computed_rank >= min_confidence_rank;
    // Per iter 88: when the response's confidence falls below the
    // agent's threshold, drop the hits — the agent reads the signal
    // and decides whether to iterate (RepoCoder) or fall back.
    if !met_threshold {
        hits.clear();
    }
    let out = SimilarOutput {
        query: args.text,
        r#type: kind,
        algorithm: algorithm.to_string(),
        corpus_size,
        hits,
        confidence,
        confidence_signals,
        met_threshold,
    };
    Ok(serde_json::to_value(out)?)
}

/// Roadmap issue #28 v3: BM25 ranking across all three corpora,
/// merged into a single ranked list. Each corpus is scored
/// independently (so IDF is per-corpus rather than pooled, which
/// matches what the agent expects — a doc with one rare term
/// beats a function with one rare term iff its raw BM25 score
/// is higher within its own corpus). Hits carry the `kind` field
/// so the caller can tell which corpus a row came from.
/// Extract the score signals (top score + spread) from a hit list.
/// Empty hit list → both zero. Single hit → spread = 0.0. The spread
/// is `top - second`, never negative — `bm25_search` and
/// `embedding_search` both pre-sort descending so rank invariants hold.
/// Map a confidence band string to a comparable rank. Unknown
/// inputs are treated as "low" — the most permissive interpretation.
/// Used by the min_confidence gate to compare the user's threshold
/// against the response's computed band.
fn confidence_rank(band: &str) -> u8 {
    match band.to_ascii_lowercase().as_str() {
        "high" => 2,
        "medium" => 1,
        _ => 0, // "low" or anything unrecognised
    }
}

fn score_signals(hits: &[SimilarHit]) -> ConfidenceSignals {
    let top_score = hits.first().map_or(0.0, |h| h.score);
    // Spread is only meaningful with at least two hits — with one hit
    // there's nothing to compare against, so we report 0.0 to avoid
    // overstating the top-1 dominance.
    let spread = match (hits.first(), hits.get(1)) {
        (Some(a), Some(b)) => (a.score - b.score).max(0.0),
        _ => 0.0,
    };
    ConfidenceSignals { top_score, spread }
}

/// Per [[research-repoformer]] / gap-bm25-confidence-vocabulary-mismatch
/// (iter 84): map raw score signals to a categorical confidence band
/// ("high" / "medium" / "low") the agent can act on without re-deriving
/// the threshold. The heuristic is corpus-agnostic (no per-corpus
/// tuning) and uses two corpus-relative axes:
///
///   * **top_score**: absolute strength of the top-1 hit.
///   * **spread**: rank-1 minus rank-2 — how cleanly the top hit
///     dominates. Large spread means the corpus has a clear winner;
///     small spread means several hits are equivalently relevant.
///
/// BM25 and embedding scores live on incompatible scales — BM25 is
/// unbounded but typically 5-40 for in-distribution hits; cosine is
/// [-1, 1]. The thresholds below are tuned for BM25; the embedding
/// path uses scaled thresholds. The "low"/"medium"/"high" categories
/// are deliberately coarse; agents that need finer-grained confidence
/// can read `confidence_signals` directly.
fn confidence_band(signals: &ConfidenceSignals, algorithm: &str) -> String {
    if signals.top_score <= 0.0 {
        return "low".to_string();
    }
    // `top_spread_floor`: minimum spread for a strong top score alone to
    // earn "high". Cosine scores bunch up (0.74 vs 0.70 is common), so a
    // high top cosine with a near-tie says nothing about which hit is right.
    let (high_top, med_top, high_spread, med_spread, top_spread_floor) = if algorithm == "embedding"
    {
        // Cosine in [0, 1] effective range after L2-normalisation.
        (0.65_f64, 0.45_f64, 0.10_f64, 0.05_f64, 0.08_f64)
    } else {
        // BM25 — empirical bands tuned against interrogation-014
        // (deepcode at 24.46, sourcegraph at 22.67) and interrogation-016
        // (repocoder at 15.94, repoformer at 15.41).
        (20.0_f64, 8.0_f64, 5.0_f64, 1.5_f64, 0.0_f64)
    };
    if (signals.top_score >= high_top && signals.spread >= top_spread_floor)
        || signals.spread >= high_spread
    {
        "high".to_string()
    } else if signals.top_score >= med_top || signals.spread >= med_spread {
        "medium".to_string()
    } else {
        "low".to_string()
    }
}

fn bm25_search_all_corpora(
    db: &Database,
    query: &str,
    top: usize,
    snippet_chars: usize,
    source_repo: Option<&str>,
    with_tag: Option<&str>,
    without_tag: Option<&str>,
) -> Result<(Vec<SimilarHit>, usize)> {
    let mut all: Vec<SimilarHit> = Vec::new();
    let mut total_size = 0usize;
    for (label, rows) in [
        (
            "doc",
            fetch_doc_corpus_filtered(db, source_repo, with_tag, without_tag)?,
        ),
        ("function", fetch_function_corpus_filtered(db, source_repo)?),
        ("entity", fetch_entity_corpus_filtered(db, source_repo)?),
        (
            "section",
            fetch_section_corpus(db, source_repo, with_tag, without_tag)?,
        ),
    ] {
        total_size += rows.len();
        // Pull top-`top` from each corpus, tag with kind, then
        // re-rank the union. Pulling more than `top` per corpus
        // would tail-pad the union without changing the top-`top`
        // result so we don't bother.
        for mut hit in bm25_search(query, &rows, top, snippet_chars) {
            hit.kind = Some(label.to_string());
            all.push(hit);
        }
    }
    all.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    all.truncate(top);
    Ok((all, total_size))
}

/// Roadmap issue #28 v4 (v0.4.0): dispatcher for the embedding
/// backend across per-corpus searches + the combined `all`
/// search. Returns `(hits, corpus_size)` in the same shape
/// `bm25_search_all_corpora` does so the surrounding
/// `similar_for_mcp` code is unchanged.
fn embedding_search_dispatch(
    db: &Database,
    kind: &str,
    query_text: &str,
    top: usize,
    snippet_chars: usize,
    filters: DocFilters<'_>,
) -> Result<(Vec<SimilarHit>, usize)> {
    let embedder = crate::embeddings::default_embedder();
    if !embedder.is_available() {
        bail!(
            "query similar --backend embedding requires an available embedder. \
             Build with `--features embeddings` and set DOC_LINTER_EMBED_MODEL + \
             DOC_LINTER_EMBED_TOKENIZER to a valid model/tokenizer pair, then \
             re-run `doc-linter check --embeddings` so the embedding columns \
             populate. (The `--embeddings` flag is opt-in because the populate \
             pass is the dominant cost on a full rebuild.)"
        );
    }
    let query_vec = embedder
        .embed(query_text)
        .map_err(|e| anyhow::anyhow!("embed query text for similarity search: {e:#}"))?;
    if query_vec.is_empty() {
        bail!(
            "embedder returned an empty vector for the query text. \
             This is the NoOp sentinel — confirm the backend reports `is_available() == true`."
        );
    }
    embedding_hits(db, kind, &query_vec, top, snippet_chars, filters)
}

/// Repo and tag filters shared by the Doc-axis searches.
#[derive(Clone, Copy, Default)]
pub struct DocFilters<'a> {
    pub source_repo: Option<&'a str>,
    pub with_tag: Option<&'a str>,
    pub without_tag: Option<&'a str>,
}

/// Rank one corpus (or all four for `all`) against an already-embedded
/// query. SQLite answers from its HNSW index through
/// [`GraphRead::embedding_nearest`]; the store has no index, so its corpus is
/// fetched with the vector column and ranked by dot product here.
pub fn embedding_rank(
    db: &Database,
    kind: &str,
    query_vec: &[f32],
    top: usize,
    f: DocFilters<'_>,
) -> Result<(Vec<(String, f64)>, usize)> {
    let (hits, n) = embedding_hits(db, kind, query_vec, top, 80, f)?;
    Ok((hits.into_iter().map(|h| (h.id, h.score)).collect(), n))
}

fn embedding_hits(
    db: &Database,
    kind: &str,
    query_vec: &[f32],
    top: usize,
    snippet_chars: usize,
    f: DocFilters<'_>,
) -> Result<(Vec<SimilarHit>, usize)> {
    if kind == "all" {
        let mut all: Vec<SimilarHit> = Vec::new();
        let mut total = 0_usize;
        for label in ["doc", "function", "entity", "section"] {
            let (mut per, n) =
                embedding_hits_one(db, label, query_vec, top, snippet_chars, f, Some(label))?;
            total += n;
            all.append(&mut per);
        }
        all.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all.truncate(top);
        return Ok((all, total));
    }
    embedding_hits_one(db, kind, query_vec, top, snippet_chars, f, None)
}

fn embedding_hits_one(
    db: &Database,
    kind: &str,
    query_vec: &[f32],
    top: usize,
    snippet_chars: usize,
    f: DocFilters<'_>,
    label: Option<&str>,
) -> Result<(Vec<SimilarHit>, usize)> {
    if kind == "type" && f.source_repo.is_some() {
        bail!(type_axis_source_repo_message());
    }
    if !matches!(kind, "doc" | "function" | "entity" | "section" | "type") {
        bail!(unknown_type_message(kind));
    }
    let req = crate::graph_read::NearestRequest {
        kind,
        query: query_vec,
        top,
        source_repo: f.source_repo,
        with_tag: f.with_tag,
        without_tag: f.without_tag,
    };
    let res = db.embedding_nearest(&req)?;
    let hits = res
        .hits
        .into_iter()
        .map(|(id, body, score)| SimilarHit {
            id,
            score,
            text_excerpt: snippet_excerpt(&body, snippet_chars),
            kind: label.map(str::to_string),
            role: None,
            doc_kind: None,
            lifecycle: None,
            tags: None,
            covers: None,
            doc_id: None,
        })
        .collect();
    Ok((hits, res.corpus_size))
}

/// Roadmap issue #28 v4 (v0.4.0): build a snippet excerpt from a
/// body text for the embedding backend. BM25's snippet picker
/// uses the matched-query span as the anchor, which doesn't
/// apply for vector search — there's no per-token signal at the
/// snippet level. We fall back to "leading N chars, trimmed at a
/// word boundary" which mirrors the legacy summary-card behaviour
/// agents see in `query list` output.
fn snippet_excerpt(body: &str, max_chars: usize) -> String {
    if body.len() <= max_chars {
        return body.to_string();
    }
    let mut cut = max_chars;
    while cut > 0 && !body.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &body[..cut];
    if let Some(idx) = head.rfind(char::is_whitespace) {
        format!("{}…", &head[..idx])
    } else {
        format!("{head}…")
    }
}

/// interrogation-001 gap-B: per-hit frontmatter decoration.
/// Looks up `role`, `kind`, `lifecycle`, `tags`, `covers` for
/// every hit whose `id` matches a `Doc` row and populates the
/// matching fields on `SimilarHit`. One SQL call total,
/// independent of `top` — issued only for hit ids the ranker
/// already returned so the cost stays bounded. Mixed-corpus
/// hits whose `id` doesn't resolve to a Doc are left untouched.
/// Per-doc metadata `enrich_doc_hits` joins onto hits.
type DocMeta = (String, String, String, Vec<String>, Vec<String>);

fn enrich_doc_hits(db: &Database, hits: &mut [SimilarHit]) -> Result<()> {
    if hits.is_empty() {
        return Ok(());
    }
    let mut meta: HashMap<String, DocMeta> = HashMap::with_capacity(hits.len());
    for h in hits.iter() {
        let res = query_sql(
            db,
            "SELECT d.role AS role, d.kind AS kind, d.lifecycle AS lifecycle, \
             (SELECT json_group_array(value) FROM \
                (SELECT value FROM doc_tags WHERE doc_id = d.id ORDER BY rowid)) AS tags, \
             (SELECT json_group_array(value) FROM \
                (SELECT value FROM doc_covers WHERE doc_id = d.id ORDER BY rowid)) AS covers \
             FROM Doc d WHERE d.id = $id LIMIT 1",
            vec![("id", crate::store::Value::Str(h.id.clone()))],
        )?;
        let rows = res
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let Some(row) = rows.into_iter().next() else {
            continue;
        };
        let Some(obj) = row.as_object() else { continue };
        let role = obj
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let kind = obj
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let lifecycle = obj
            .get("lifecycle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tags = obj
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let covers = obj
            .get("covers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        meta.insert(h.id.clone(), (role, kind, lifecycle, tags, covers));
    }
    for h in hits.iter_mut() {
        if let Some((role, kind, lifecycle, tags, covers)) = meta.remove(&h.id) {
            if !role.is_empty() {
                h.role = Some(role);
            }
            if !kind.is_empty() {
                h.doc_kind = Some(kind);
            }
            if !lifecycle.is_empty() {
                h.lifecycle = Some(lifecycle);
            }
            if !tags.is_empty() {
                h.tags = Some(tags);
            }
            if !covers.is_empty() {
                h.covers = Some(covers);
            }
        }
    }
    Ok(())
}

/// Gap-010 phase 2 (Function): fetch Function-corpus rows with
/// embeddings for cosine ranking, optionally filtered by
/// `Function.repo_id`. Pass `None` for the unfiltered (cross-repo)
/// ranking.
/// Recoverable error for the type-axis mismatch — per the ACI
/// principle ([[research-swe-agent]]), the rejection should name
/// the closest existing axis AND the sql fallback recipe so the
/// agent can recover in one step instead of asking for help. Used
/// by both the BM25 and embedding dispatchers, so the user sees the
/// same hint whichever backend they routed through. Surfaced by
/// [[user-probe-012]] which hit `--type type` and got only the bare
/// "valid: doc, function, entity, type, section, all" list.
/// Error emitted when an agent requests `--type type` with a
/// `source_repo` filter. Type nodes lack `repo_id` in the current
/// schema so the filter can't be applied; the message names the
/// constraint AND the recovery path (drop the filter or use the
/// `sql` tool for finer control).
fn type_axis_source_repo_message() -> String {
    "type: source_repo filter not yet supported (the Type node table \
     does not carry repo_id in the current schema). Re-run without \
     `source_repo`, or fall back to the `sql` MCP tool: \
     SELECT symbol, file, line FROM Type WHERE instr(symbol, '<term>') > 0. \
     The function / entity / doc axes accept source_repo normally."
        .to_string()
}

fn unknown_type_message(other: &str) -> String {
    let table = match other.to_ascii_lowercase().as_str() {
        "endpoint" | "endpoints" => Some("Endpoint"),
        "module" | "modules" => Some("Module"),
        "file" | "files" => Some("File"),
        "finding" | "findings" => Some("Finding"),
        "migration" | "migrations" => Some("Migration"),
        _ => None,
    };
    let mut msg =
        format!("unknown --type '{other}' (valid: doc, function, entity, type, section, all).");
    if let Some(t) = table {
        msg.push_str(&format!(
            " The `{t}` node table exists in the graph but is not directly \
             addressable via query_similar. Fall back to the `sql` MCP tool: \
             SELECT * FROM {t} LIMIT 20 \
             — or use `query_at` / `function_context` for line-anchored lookups."
        ));
    }
    msg
}

/// Implements `query types` — filtered listing of the Type node
/// table introduced in roadmap issue #32. The Type table is
/// dual-written from struct / enum / trait / module / type_alias
/// SCIP facts; this subcommand replaces the `query sql
/// "MATCH (t:Type {kind:'struct'}) ..."` ceremony.
///
/// @endpoint CLI types
fn types(db: &Database, args: TypesArgs) -> Result<String> {
    let allowed_kinds = ["struct", "enum", "trait", "module", "type_alias"];
    if let Some(kind) = &args.kind {
        if !allowed_kinds.contains(&kind.as_str()) {
            bail!(
                "unknown --kind '{kind}' (valid: {})",
                allowed_kinds.join(" / ")
            );
        }
    }

    // SQL is built from the filter combination — bind params for
    // the values to avoid quoting issues even though the columns are
    // free-text.
    let mut sql = String::from(
        "SELECT t.symbol AS symbol, t.kind AS kind, t.crate AS crate, \
         t.file AS file, t.line AS line, t.language AS language FROM Type t",
    );
    let mut conditions: Vec<&str> = Vec::new();
    let mut sql_conditions: Vec<&str> = Vec::new();
    if args.kind.is_some() {
        conditions.push("t.kind = $kind");
        sql_conditions.push("t.kind = $kind");
    }
    if args.krate.is_some() {
        conditions.push("t.crate = $crate");
        sql_conditions.push("t.crate = $crate");
    }
    if args.substring.is_some() {
        conditions.push("t.symbol CONTAINS $sub");
        // instr is case-sensitive like CONTAINS; LIKE is not.
        sql_conditions.push("instr(t.symbol, $sub) > 0");
    }
    if !conditions.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&sql_conditions.join(" AND "));
    }
    sql.push_str(" ORDER BY t.symbol");

    // Use raw sql for v1 — building a typed projection helper
    // here would duplicate scaffolding from store/query/. The
    // raw-SQL path emits the same {columns, row_count, rows}
    // shape every other subcommand returns.
    let mut wrapped = serde_json::Map::new();
    let mut bindings: Vec<(&str, crate::store::Value)> = Vec::new();
    if let Some(kind) = &args.kind {
        bindings.push(("kind", crate::store::Value::Str(kind.clone())));
    }
    if let Some(cr) = &args.krate {
        bindings.push(("crate", crate::store::Value::Str(cr.clone())));
    }
    if let Some(sub) = &args.substring {
        bindings.push(("sub", crate::store::Value::Str(sub.clone())));
    }
    let result = query_sql(db, &sql, bindings)?;
    let mut rows = result
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if args.top > 0 && rows.len() > args.top {
        rows.truncate(args.top);
    }
    wrapped.insert("count".into(), serde_json::Value::from(rows.len()));
    if let Some(k) = &args.kind {
        wrapped.insert("kind".into(), serde_json::Value::String(k.clone()));
    }
    if let Some(cr) = &args.krate {
        wrapped.insert("crate".into(), serde_json::Value::String(cr.clone()));
    }
    if let Some(s) = &args.substring {
        wrapped.insert("substring".into(), serde_json::Value::String(s.clone()));
    }
    wrapped.insert("types".into(), serde_json::Value::Array(rows));
    Ok(serde_json::to_string_pretty(&serde_json::Value::Object(
        wrapped,
    ))?)
}

/// Corpus row triple: `(id, title-or-display, body)`. Both
/// `title` and `body` are tokenised; the body is used for the
/// excerpt.
type CorpusRow = (String, String, String);

/// Gap-010 phase 3: fetch Doc-corpus rows for BM25 ranking,
/// optionally filtered by `Doc.repo_id`. Pass `None` for the
/// unfiltered (cross-repo) ranking.
fn fetch_doc_corpus_filtered(
    db: &Database,
    source_repo: Option<&str>,
    with_tag: Option<&str>,
    without_tag: Option<&str>,
) -> Result<Vec<CorpusRow>> {
    // ORDER BY id: BM25 ties keep corpus order, so make it deterministic.
    let (sql_where, sql_params) = doc_filter_sql("d", source_repo, with_tag, without_tag);
    let sql = format!(
        "SELECT d.id AS id, d.title AS title, d.summary AS body FROM Doc d {} ORDER BY d.id",
        if sql_where.is_empty() {
            String::new()
        } else {
            format!("WHERE {sql_where}")
        }
    );
    let res = query_sql(db, &sql, sql_params)?;
    Ok(rows_to_corpus(&res))
}

/// Heading-level Section rows for BM25, filtered through the parent
/// Doc's repo / tag filters: `(section id, heading, text)`.
fn fetch_section_corpus(
    db: &Database,
    source_repo: Option<&str>,
    with_tag: Option<&str>,
    without_tag: Option<&str>,
) -> Result<Vec<CorpusRow>> {
    let res = run_section_query(db, source_repo, with_tag, without_tag)?;
    Ok(rows_to_corpus(&res))
}

fn run_section_query(
    db: &Database,
    source_repo: Option<&str>,
    with_tag: Option<&str>,
    without_tag: Option<&str>,
) -> Result<serde_json::Value> {
    let (sql_where, sql_params) = doc_filter_sql("d", source_repo, with_tag, without_tag);
    let sql = format!(
        "SELECT s.id AS id, s.heading AS title, s.text AS body \
         FROM Section s JOIN \"SECTION_OF\" so ON so.src = s.id JOIN Doc d ON d.id = so.dst {} \
         ORDER BY s.id",
        if sql_where.is_empty() {
            String::new()
        } else {
            format!("WHERE {sql_where}")
        }
    );
    query_sql(db, &sql, sql_params)
}

/// Gap-010 phase 2 (Function): fetch Function-corpus rows for
/// BM25, optionally filtered by `Function.repo_id`. Pass `None`
/// for the unfiltered (cross-repo) ranking.
fn fetch_function_corpus_filtered(
    db: &Database,
    source_repo: Option<&str>,
) -> Result<Vec<CorpusRow>> {
    // Roadmap issue #28 v2: include `body_excerpt` alongside
    // `doc_comment`. Code search ("where is auth done") often
    // matches identifiers / inline comments that only live in the
    // body — not the doc-comment prose. We concatenate so a single
    // BM25 pass ranks against both texts; the excerpt is built from
    // doc_comment first (so the preview reads as English) and falls
    // back to body_excerpt when the doc-comment is empty.
    let res = match source_repo {
        Some(id) => query_sql(
            db,
            "SELECT f.symbol AS id, f.signature AS signature, \
             f.doc_comment AS doc_comment, f.body_excerpt AS body_excerpt \
             FROM Function f WHERE f.repo_id = $repo_id ORDER BY f.symbol",
            vec![("repo_id", crate::store::Value::Str(id.to_string()))],
        )?,
        None => query_sql(
            db,
            "SELECT f.symbol AS id, f.signature AS signature, \
             f.doc_comment AS doc_comment, f.body_excerpt AS body_excerpt \
             FROM Function f ORDER BY f.symbol",
            vec![],
        )?,
    };
    let rows = res
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<CorpusRow> = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(obj) = r.as_object() else { continue };
        let Some(id) = obj.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        let signature = obj
            .get("signature")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let doc_comment = obj
            .get("doc_comment")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let body_excerpt = obj
            .get("body_excerpt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // Body: doc_comment if present (preview reads as English),
        // otherwise the code body. Either way, the OTHER text gets
        // appended for ranking — BM25 sees both worlds.
        let body = if doc_comment.is_empty() {
            body_excerpt.to_string()
        } else if body_excerpt.is_empty() {
            doc_comment.to_string()
        } else {
            format!("{doc_comment}\n{body_excerpt}")
        };
        out.push((id, signature, body));
    }
    Ok(out)
}

/// Type-axis BM25 corpus (gap-query-similar-type-axes-limited iter 80).
/// Type nodes lack `repo_id` in the current schema (see audit at
/// [[user-probe-012]]), so this fetcher does NOT accept a
/// `source_repo` arg — the dispatcher rejects the filtered call with
/// a recoverable error before reaching here. Returns rows shaped as
/// `(symbol, signature, doc_comment + body_excerpt)` mirroring the
/// Function-corpus fetcher.
fn fetch_type_corpus(db: &Database) -> Result<Vec<CorpusRow>> {
    let res = query_sql(
        db,
        "SELECT t.symbol AS id, t.signature AS signature, \
         t.doc_comment AS doc_comment, t.body_excerpt AS body_excerpt \
         FROM Type t ORDER BY t.symbol",
        vec![],
    )?;
    let rows = res
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out: Vec<CorpusRow> = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(obj) = r.as_object() else { continue };
        let Some(id) = obj.get("id").and_then(|v| v.as_str()).map(str::to_string) else {
            continue;
        };
        let signature = obj
            .get("signature")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let doc_comment = obj
            .get("doc_comment")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let body_excerpt = obj
            .get("body_excerpt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let body = if doc_comment.is_empty() {
            body_excerpt.to_string()
        } else if body_excerpt.is_empty() {
            doc_comment.to_string()
        } else {
            format!("{doc_comment}\n{body_excerpt}")
        };
        out.push((id, signature, body));
    }
    Ok(out)
}

/// Gap-010 phase 2 (Entity): fetch Entity-corpus rows for BM25,
/// optionally filtered by `Entity.repo_id`. Pass `None` for the
/// unfiltered (cross-repo) ranking.
fn fetch_entity_corpus_filtered(
    db: &Database,
    source_repo: Option<&str>,
) -> Result<Vec<CorpusRow>> {
    let res = match source_repo {
        Some(id) => query_sql(
            db,
            "SELECT e.id AS id, e.display AS title, e.description AS body \
             FROM Entity e WHERE e.repo_id = $repo_id ORDER BY e.id",
            vec![("repo_id", crate::store::Value::Str(id.to_string()))],
        )?,
        None => query_sql(
            db,
            "SELECT e.id AS id, e.display AS title, e.description AS body \
             FROM Entity e ORDER BY e.id",
            vec![],
        )?,
    };
    let mut rows = rows_to_corpus(&res);
    // Per iter-398 BM25 silent-miss fix (4-instance across iter-
    // 385/387/390/397): entity-id tokens (e.g., 'cuda-error',
    // 'normalizer', 'lemmatizer', 'serialize') often DON'T appear
    // in the entity description text — promoted descriptions are
    // semantic narrative without keyword repetition; cluster
    // descriptions name FUNCTIONS not the entity itself. Agent
    // domain queries match the entity ID directly (user thinks
    // 'cuda' → entity-cuda-error). Prepending the id to body so
    // BM25 tokenization includes id-tokens fixes the silent-miss
    // failure mode at the source.
    //
    // Per iter-401 (refinement of iter-398 CAVEAT): also prepend
    // entity DISPLAY (e.display) — the human-readable name often
    // differs from the id in casing/spacing (e.g., id='cuda-error'
    // / display='CUDA Error'; id='normalizer' / display='Response
    // Normalizer'). User queries phrased in natural language
    // ('CUDA Error' or 'response normalizer') match display tokens
    // not id tokens. Including display tokens in body bridges the
    // morphological-mismatch case the iter-398 fix CAVEAT
    // documented. iter-389 entities-by-description-substring +
    // entities-by-id-pattern remain as fallback for cases this
    // fix doesn't cover (e.g., partial keyword matches that don't
    // align with either id or display tokens).
    for row in &mut rows {
        row.2 = format!("{} {} {}", row.0, row.1, row.2);
    }
    Ok(rows)
}

fn rows_to_corpus(res: &serde_json::Value) -> Vec<CorpusRow> {
    let rows = res
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    rows.into_iter()
        .filter_map(|r| {
            let obj = r.as_object()?;
            let id = obj.get("id")?.as_str()?.to_string();
            let title = obj
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let body = obj
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some((id, title, body))
        })
        .collect()
}

/// BM25 over a tokenised corpus. `k1 = 1.5` and `b = 0.75` are the
/// canonical defaults — no tuning surface at v1.
fn bm25_search(
    query: &str,
    corpus: &[CorpusRow],
    top: usize,
    snippet_chars: usize,
) -> Vec<SimilarHit> {
    let q_tokens = tokenize(query);
    if q_tokens.is_empty() || corpus.is_empty() {
        return Vec::new();
    }
    // Per-document token counts (combined title + body).
    let docs: Vec<(String, String, Vec<String>)> = corpus
        .iter()
        .map(|(id, title, body)| {
            let mut t = tokenize(title);
            t.extend(tokenize(body));
            (id.clone(), body.clone(), t)
        })
        .collect();

    let n = docs.len() as f64;
    let avgdl: f64 = if docs.is_empty() {
        0.0
    } else {
        let total: usize = docs.iter().map(|(_, _, t)| t.len()).sum();
        total as f64 / n
    };

    // Document frequency (how many docs contain term t).
    let mut df: HashMap<&str, usize> = HashMap::new();
    for (_, _, toks) in &docs {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for t in toks {
            if seen.insert(t.as_str()) {
                *df.entry(t.as_str()).or_insert(0) += 1;
            }
        }
    }

    let k1 = 1.5_f64;
    let b = 0.75_f64;

    let mut scored: Vec<SimilarHit> = docs
        .iter()
        .map(|(id, body, toks)| {
            let dl = toks.len() as f64;
            let mut tf: HashMap<&str, usize> = HashMap::new();
            for t in toks {
                *tf.entry(t.as_str()).or_insert(0) += 1;
            }
            let mut score = 0.0_f64;
            for qt in &q_tokens {
                let tf_q = *tf.get(qt.as_str()).unwrap_or(&0) as f64;
                if tf_q == 0.0 {
                    continue;
                }
                let df_q = *df.get(qt.as_str()).unwrap_or(&0) as f64;
                let idf = ((n - df_q + 0.5) / (df_q + 0.5)).ln_1p();
                let denom = b.mul_add(dl / avgdl.max(1.0), 1.0 - b).mul_add(k1, tf_q);
                score += idf * (tf_q * (k1 + 1.0) / denom);
            }
            SimilarHit {
                id: id.clone(),
                score,
                text_excerpt: excerpt_around_match(body, &q_tokens, snippet_chars),
                kind: None,
                role: None,
                doc_kind: None,
                lifecycle: None,
                tags: None,
                covers: None,
                doc_id: None,
            }
        })
        .filter(|h| h.score > 0.0)
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(top);
    scored
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            for lower in ch.to_lowercase() {
                buf.push(lower);
            }
        } else if !buf.is_empty() {
            push_token(&mut out, std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        push_token(&mut out, buf);
    }
    out
}

fn push_token(out: &mut Vec<String>, t: String) {
    if t.len() < 2 {
        return;
    }
    if STOPWORDS.contains(&t.as_str()) {
        return;
    }
    out.push(t);
}

/// Tiny stopword list — keeps the v1 implementation a single file.
/// Expand cautiously: every word here is invisible to ranking.
const STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "but", "not", "you", "all", "can", "her", "was", "one", "our",
    "out", "day", "get", "has", "him", "his", "how", "man", "new", "now", "old", "see", "two",
    "way", "who", "boy", "did", "its", "let", "put", "say", "she", "too", "use", "from", "with",
    "this", "that", "they", "have", "what", "when", "your", "which", "their", "would", "there",
    "these", "those", "into", "than", "then", "them", "such", "only", "some", "more", "much",
    "very", "most", "also", "other", "where", "while", "after", "before",
];

/// Roadmap issue #28 v4: smart excerpt — finds the earliest
/// query-token occurrence in `body` (case-insensitive,
/// word-boundary-respecting) and returns a `max_chars`-wide
/// window centered on it. Falls back to `excerpt_head` when no
/// token matches, preserving the v1 behavior for callers that
/// pass an empty token list.
fn excerpt_around_match(body: &str, tokens: &[String], max_chars: usize) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    // Earliest byte offset where any token appears as a substring.
    // Whole-word matching would be nicer but bm25 already tokenises
    // identically; substring match is the cheap correct-enough
    // signal for "show me where the term lives".
    let mut earliest: Option<usize> = None;
    for tok in tokens {
        if tok.is_empty() {
            continue;
        }
        if let Some(pos) = lower.find(tok.as_str()) {
            earliest = Some(earliest.map_or(pos, |p| p.min(pos)));
        }
    }
    let Some(byte_pos) = earliest else {
        return excerpt_head(trimmed, max_chars);
    };
    // Convert the byte position to a char index for the window.
    let char_pos = lower[..byte_pos].chars().count();
    let half = max_chars / 2;
    let total_chars = trimmed.chars().count();
    let start = char_pos.saturating_sub(half);
    // Ensure the window fits — if the match is near the end,
    // slide the start backward so we always return `max_chars`.
    let start = start.min(total_chars.saturating_sub(max_chars));
    let mut out = String::with_capacity(max_chars + 4);
    if start > 0 {
        out.push('…');
    }
    for (i, ch) in trimmed.chars().enumerate().skip(start).take(max_chars) {
        if i >= start + max_chars {
            break;
        }
        out.push(ch);
    }
    if start + max_chars < total_chars {
        out.push('…');
    }
    out
}

/// Fallback for `excerpt_around_match` when no query token matches —
/// same as the v1 implementation (truncate from char 0).
fn excerpt_head(trimmed: &str, max_chars: usize) -> String {
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let mut out = String::with_capacity(max_chars + 1);
    for (i, ch) in trimmed.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}
