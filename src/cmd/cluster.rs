//! Roadmap issue #14 (v0.3.0): `doc-linter cluster` — community
//! detection over the function graph, surfacing candidate entities.
//!
//! ## v1 scope
//!
//! - Reads `Function` nodes + `CALLS` edges from the SQLite graph.
//! - Builds an undirected graph in memory.
//! - Runs Label Propagation Algorithm (LPA): each node starts with
//!   its own label, then on each pass adopts the most-common label
//!   among its neighbors; iterates until labels stop changing (or
//!   a step ceiling is hit). LPA is O(E) per pass and converges in
//!   a handful of passes — coarser than Louvain modularity
//!   optimisation but ships in one file and produces useful
//!   communities on real graphs. The richer Louvain pass is a
//!   v0.4.0 swap.
//! - Computes per-community god-node (highest in-degree in the
//!   community), member count, and intra-community-density (the
//!   modularity surrogate).
//! - Emits a JSON `{candidates: [...]}` report to stdout sorted by
//!   member count.
//!
//! File emission to `docs/ontology/entities/candidates/` and the
//! matching `status: candidate` validator surface landed alongside
//! v1 (see `emit_candidate_stubs` + `default_statuses` in
//! `src/config/lint_rules.rs`). v0.4.0 (#14 v11) adds the entity
//! swap: `cluster --promote <id>` flips a reviewed candidate from
//! `status: candidate` to `status: stable` and moves the file out
//! of `candidates/` into the canonical `entities/` directory in one
//! shot, so a reviewer can promote without hand-editing frontmatter
//! and remembering to `git mv`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use doc_linter::store::typed::{self, NodeTable};
use serde::Serialize;

/// One row in the report — one candidate entity surfaced by LPA.
#[derive(Debug, Serialize)]
struct Candidate {
    /// Suggested entity id for the community. Derived in priority
    /// order: (1) highest-in-degree `Entity` member's id, (2) slug
    /// of highest-in-degree `Doc` member's id, (3) god-function's
    /// file-stem (the historical pre-doc-aware behaviour).
    suggested_id: String,
    /// Highest in-degree node inside the community — used as the
    /// candidate's anchor. May be a Function symbol, Doc id,
    /// Entity id, Type symbol, or Module id depending on what
    /// kind landed at the top of the in-degree list.
    god_node_symbol: String,
    /// Source file of the god-node (Function/Type `file`,
    /// Doc/Module `path`). Empty when the god-node is an Entity.
    god_node_file: String,
    /// `Function` / `Doc` / `Entity` / `Type` / `Module` —
    /// callers eyeballing the JSON can tell at a glance what
    /// shape the anchor is.
    god_node_kind: NodeKind,
    /// Number of nodes in the community (all kinds).
    member_count: usize,
    /// Per-kind census of the community. Lets the lint say
    /// "this code cluster has no `Entity` member" without
    /// re-walking the top_members list.
    kind_counts: BTreeMap<NodeKind, usize>,
    /// `intra_edges / max_possible_intra_edges`, a 0..1 ratio.
    /// Higher = tighter cluster.
    density: f64,
    /// Up to N representative member node ids (highest in-degree),
    /// for the operator's "what kind of community is this?" eyeball.
    top_members: Vec<String>,
    /// Distinct source files of community members, deduped + sorted.
    /// Entity members contribute nothing (no file). Capped at 50
    /// — beyond that, the candidate stub becomes review-hostile and
    /// a human should either accept the cluster or split it before
    /// fleshing out `source_modules:`.
    source_files: Vec<String>,
}

/// Top-level JSON report shape.
#[derive(Debug, Serialize)]
struct Report {
    /// Roadmap issue #14 v10: which community-detection algorithm
    /// produced this report (`"lpa"` or `"louvain"`). Lets
    /// operators comparing two reports tell them apart without
    /// re-reading their invocation history.
    algorithm: &'static str,
    /// Roadmap issue #14 v10: which ranking key sorted the
    /// candidate list (`"member-count"` or `"density"`).
    order_by: &'static str,
    /// LPA / Louvain iteration count — diagnostic, helps tune.
    iterations: usize,
    /// Total number of communities the algorithm settled on,
    /// before the `--top-n` truncation.
    total_communities: usize,
    candidates: Vec<Candidate>,
    /// Paths (repo-relative) of any candidate stub files written
    /// this invocation. Empty when `--write` is not set or when
    /// every emitted candidate already had an existing file on
    /// disk (re-runs are idempotent).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    written: Vec<String>,
}

impl OrderBy {
    fn label(self) -> &'static str {
        match self {
            OrderBy::MemberCount => "member-count",
            OrderBy::Density => "density",
        }
    }
}

impl Algorithm {
    fn label(self) -> &'static str {
        match self {
            Algorithm::Lpa => "lpa",
            Algorithm::Louvain => "louvain",
            Algorithm::Leiden => "leiden",
        }
    }
}

/// `doc-linter cluster` entry point. Reads the SQLite graph at the
/// canonical `<root>/.doc-lint/graph.the store` location, runs LPA, and
/// emits a JSON report. With `--write`, also writes one candidate
/// entity stub per community to
/// `docs/ontology/entities/candidates/`. Exits successfully even
/// when the corpus is empty (no Function rows yet).
/// Ranking key for the top-N cut. Member-count is the v1 default
/// and what every prior PR exercised; density-first surfaces
/// crisp clusters before larger but noisier ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderBy {
    MemberCount,
    Density,
}

impl OrderBy {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "member-count" | "members" | "size" => Ok(OrderBy::MemberCount),
            "density" => Ok(OrderBy::Density),
            other => anyhow::bail!("unknown --order-by '{other}' (valid: member-count, density)"),
        }
    }
}

/// Community-detection algorithm. LPA is the v1 default — O(E)
/// per pass, converges fast, deterministic tie-break. Louvain is
/// the v6 addition (a single-level modularity-optimization pass)
/// — produces tighter communities at ~3-5x the runtime. Leiden
/// (Traag, Waltman & van Eck 2019) is the multi-level
/// local-move + refinement + aggregate pipeline; refinement
/// splits weakly-bridged sub-clusters that Louvain's
/// order-dependent merging glued together, and the aggregation
/// loop pulls multi-scale hierarchy out of hub-heavy graphs
/// (where single-level refinement alone can't split a star).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Lpa,
    Louvain,
    Leiden,
}

impl Algorithm {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "lpa" | "label-propagation" => Ok(Algorithm::Lpa),
            "louvain" | "modularity" => Ok(Algorithm::Louvain),
            "leiden" => Ok(Algorithm::Leiden),
            other => anyhow::bail!("unknown --algorithm '{other}' (valid: lpa, louvain, leiden)"),
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "CLI flags grow incrementally; grouping into a struct would mostly serialise the same set"
)]
pub(crate) fn run(
    root: &Path,
    top_n: usize,
    write: bool,
    order_by: &str,
    algorithm: &str,
    min_members: usize,
    seed_symbol: Option<&str>,
    top_members: usize,
    output: &Path,
    promote: Option<&str>,
    resolution: f64,
) -> Result<ExitCode> {
    // Roadmap issue #14 v11: --promote short-circuits clustering.
    // The flag's job is the file swap, not the community detection
    // — running the LPA/Louvain pass first would burn seconds on a
    // big repo for a result we'd throw away.
    if let Some(id) = promote {
        let report = promote_candidate(root, output, id)?;
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(ExitCode::SUCCESS);
    }

    let ordering = OrderBy::parse(order_by)?;
    let algo = Algorithm::parse(algorithm)?;
    // For the seed-symbol filter, ignore top_n / ordering — we
    // return at most one community, so neither makes sense.
    let effective_top = if seed_symbol.is_some() {
        usize::MAX
    } else {
        top_n
    };
    let mut report = compute_report(
        root,
        effective_top,
        ordering,
        algo,
        min_members.max(2),
        top_members.max(1),
        resolution,
    )?;
    if let Some(sym) = seed_symbol {
        apply_seed_filter(&mut report, sym);
        if report.candidates.is_empty() {
            // Be explicit — empty `candidates` could otherwise mean
            // either "no clusters at all" or "no match". Surface a
            // synthetic note so the JSON reader can distinguish.
            eprintln!(
                "doc-linter: cluster — no community contains symbol `{sym}` \
                 (check spelling; try a top-5 member name from a recent run)"
            );
        }
    }
    if write {
        let written = emit_candidate_stubs(root, output, &report.candidates)
            .context("emit candidate entity stubs")?;
        report.written = written;
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(ExitCode::SUCCESS)
}

/// Roadmap issue #14 v8: filter the report to the community
/// containing `seed`. Shared between the CLI `run` path and the
/// MCP `cluster_report_json` path. Match is exact on
/// `god_node_symbol`, `suggested_id`, or any `top_members` entry —
/// fuzzy / substring lookups defer to the agent layer.
fn apply_seed_filter(report: &mut Report, seed: &str) {
    report.candidates.retain(|c| {
        c.god_node_symbol == seed
            || c.top_members.iter().any(|m| m == seed)
            || c.suggested_id == seed
    });
}

/// Reusable entry point that returns the cluster report as a
/// structured JSON value without printing or writing files. Used
/// by the MCP `cluster` tool so an agent can call it without
/// shelling out to the CLI.
///
/// `top_n` clamps the candidate list; pass 0 for the natural
/// ordering with no cap. `order_by` accepts the same keys as
/// the CLI's `--order-by` flag (`member-count` /
/// `members` / `size` / `density`); pass `""` for the default.
/// File emission is intentionally absent — callers wanting
/// `--write` semantics should invoke `run` (which runs in a
/// separate process) so the mutation surface stays behind the
/// CLI boundary.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors run's CLI flags for the MCP cluster tool"
)]
pub(crate) fn cluster_report_json(
    root: &Path,
    top_n: usize,
    order_by: &str,
    algorithm: &str,
    min_members: usize,
    seed_symbol: &str,
    top_members: usize,
    resolution: f64,
) -> Result<serde_json::Value> {
    let ordering = if order_by.is_empty() {
        OrderBy::MemberCount
    } else {
        OrderBy::parse(order_by)?
    };
    let algo = if algorithm.is_empty() {
        Algorithm::Lpa
    } else {
        Algorithm::parse(algorithm)?
    };
    // Seed-symbol filtering ignores top_n (same as the CLI path)
    // — we return at most one community.
    let effective_top = if seed_symbol.is_empty() {
        top_n
    } else {
        usize::MAX
    };
    let mut report = compute_report(
        root,
        effective_top,
        ordering,
        algo,
        min_members.max(2),
        if top_members == 0 { 5 } else { top_members },
        resolution,
    )?;
    if !seed_symbol.is_empty() {
        apply_seed_filter(&mut report, seed_symbol);
    }
    Ok(serde_json::to_value(report)?)
}

/// Subset of [`Candidate`] exposed to the check-time
/// `unauthored-cluster` lint. The lint only needs enough to decide
/// "is this cluster's suggested id already an entity?" and to phrase
/// a useful diagnostic — top members and source files would be
/// noise in a CLI error message.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredCluster {
    pub suggested_id: String,
    pub god_node_file: String,
    pub member_count: usize,
    pub density: f64,
}

/// Roadmap issue #14 follow-up (self-healing ontology): run LPA over
/// the live Function+CALLS graph reachable via `conn` and return every
/// community whose member count meets `min_members`, sorted by member
/// count descending. Called by the check-time `unauthored-cluster`
/// lint so it can flag clusters with no matching registered entity.
///
/// Sharing the caller's connection (vs the `cluster_report_json`
/// path which opens its own DB handle) lets the lint run inside
/// `cmd_check`'s existing DB-open scope without colliding with the store's
/// single-writer lock.
pub(crate) fn discover_clusters(
    conn: &(impl doc_linter::store::StoreRead + ?Sized),
    min_members: usize,
) -> Result<Vec<DiscoveredCluster>> {
    let nodes = read_graph_nodes(conn).context("read graph nodes for cluster lint")?;
    if nodes.is_empty() {
        return Ok(Vec::new());
    }
    let edges = read_graph_edges(conn).context("read graph edges for cluster lint")?;
    let graph = Graph::build(&nodes, &edges);
    let (labels, _iterations) = label_propagation(&graph, 50);
    let report = build_report(
        &graph,
        &labels,
        /* iterations */ 0,
        /* top_n */ usize::MAX,
        OrderBy::MemberCount,
        min_members.max(2),
        /* top_members */ 5,
        Algorithm::Lpa,
    );
    Ok(report
        .candidates
        .into_iter()
        // A community of only docs / entities (the ontology's own
        // axis-and-value web) is not a code concept awaiting an entity.
        .filter(|c| {
            [NodeKind::Function, NodeKind::Type]
                .iter()
                .any(|k| c.kind_counts.get(k).copied().unwrap_or(0) > 0)
        })
        .map(|c| DiscoveredCluster {
            suggested_id: c.suggested_id,
            god_node_file: c.god_node_file,
            member_count: c.member_count,
            density: c.density,
        })
        .collect())
}

/// Communities of an id/edge list over the shared algorithms, for callers
/// (the concept proposer) that bring their own nodes and edges. Each
/// community is sorted; the list is largest first, ties by first id, so
/// the output does not depend on input order.
pub(crate) fn communities_of(
    ids: &[String],
    edges: &[(String, String)],
    algorithm: &str,
    min_members: usize,
) -> Result<Vec<Vec<String>>> {
    let algo = Algorithm::parse(algorithm)?;
    let nodes: Vec<NodeRow> = ids
        .iter()
        .map(|id| NodeRow::new(NodeKind::Function, id.clone(), String::new()))
        .collect();
    let keyed: Vec<(String, String)> = edges
        .iter()
        .map(|(a, b)| (format!("fn:{a}"), format!("fn:{b}")))
        .collect();
    let graph = Graph::build(&nodes, &keyed);
    let (labels, _) = match algo {
        Algorithm::Lpa => label_propagation(&graph, 50),
        Algorithm::Louvain => louvain_first_level(&graph, 50),
        Algorithm::Leiden => leiden_multilevel(&graph, 50, 1.0),
    };
    let mut by_label: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (i, l) in labels.iter().enumerate() {
        by_label
            .entry(*l)
            .or_default()
            .push(graph.nodes[i].id.clone());
    }
    let mut out: Vec<Vec<String>> = by_label
        .into_values()
        .filter(|m| m.len() >= min_members.max(2))
        .map(|mut m| {
            m.sort();
            m
        })
        .collect();
    out.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    Ok(out)
}

fn compute_report(
    root: &Path,
    top_n: usize,
    ordering: OrderBy,
    algo: Algorithm,
    min_members: usize,
    top_members: usize,
    gamma: f64,
) -> Result<Report> {
    let db = doc_linter::graph_read::ReadGraph::open(root).context("open graph for cluster")?;
    let conn = &*db;

    let nodes = read_graph_nodes(conn).context("read graph nodes")?;
    if nodes.is_empty() {
        return Ok(Report {
            algorithm: algo.label(),
            order_by: ordering.label(),
            iterations: 0,
            total_communities: 0,
            candidates: Vec::new(),
            written: Vec::new(),
        });
    }

    let edges = read_graph_edges(conn).context("read graph edges")?;
    let graph = Graph::build(&nodes, &edges);
    let (labels, iterations) = match algo {
        Algorithm::Lpa => label_propagation(&graph, 50),
        Algorithm::Louvain => louvain_first_level(&graph, 50),
        Algorithm::Leiden => leiden_multilevel(&graph, 50, gamma),
    };
    Ok(build_report(
        &graph,
        &labels,
        iterations,
        top_n,
        ordering,
        min_members,
        top_members,
        algo,
    ))
}

/// Discriminator for the heterogeneous graph the cluster command
/// reads. Function / Doc / Entity / Type / Module each map to a
/// node table in the graph schema. Skipped on purpose: `File`
/// (metadata only), `Finding` (every TODO would inflate the
/// vertex set), `Migration` / `RepoMeta` (historical/admin),
/// `Endpoint` (would need a paired `ENDPOINT_TOUCHES_ENTITY`
/// surface we're alone in pulling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
enum NodeKind {
    Function,
    Doc,
    Entity,
    Type,
    Module,
}

impl NodeKind {
    /// Short kind prefix used when building the globally-unique
    /// `key` field of [`NodeRow`]. Encoding the kind prevents
    /// `<Doc id> == <Entity id>` collisions from merging two
    /// distinct vertices into one.
    fn prefix(self) -> &'static str {
        match self {
            NodeKind::Function => "fn",
            NodeKind::Doc => "doc",
            NodeKind::Entity => "ent",
            NodeKind::Type => "ty",
            NodeKind::Module => "mod",
        }
    }
}

#[derive(Debug, Clone)]
struct NodeRow {
    kind: NodeKind,
    /// Globally-unique vertex key — `<prefix>:<id>`. Used as the
    /// hashmap key when wiring the adjacency. Callers wanting the
    /// natural id should read [`Self::id`].
    key: String,
    /// Native id (Function/Type SCIP symbol, Doc id, Entity id,
    /// Module id) — what shows up in the JSON report.
    id: String,
    /// Source file when applicable (Function/Type `file`,
    /// Doc `path`, Module `path`). Empty for Entity.
    file: String,
}

impl NodeRow {
    fn new(kind: NodeKind, id: String, file: String) -> Self {
        let key = format!("{}:{}", kind.prefix(), id);
        NodeRow {
            kind,
            key,
            id,
            file,
        }
    }
}

/// Pull every node we cluster — Function, Doc, Entity, Type, Module.
/// Each query reads the primary-key column under a stable `id` alias
/// plus whatever locates-the-source column the table has (Function/
/// Type `file`, Doc `path`, Module `path`). Tables that fail to
/// query (e.g. a fresh `init` repo without doc ingest yet) are
/// skipped rather than aborting the whole cluster pass.
fn read_graph_nodes(conn: &(impl doc_linter::store::StoreRead + ?Sized)) -> Result<Vec<NodeRow>> {
    let mut out: Vec<NodeRow> = Vec::new();
    for table in [
        NodeTable::Function,
        NodeTable::Doc,
        NodeTable::Entity,
        NodeTable::Type,
        NodeTable::Module,
    ] {
        for (id, file) in typed::cluster_nodes(conn, table) {
            if !id.is_empty() {
                out.push(NodeRow::new(node_kind(table), id, file));
            }
        }
    }
    Ok(out)
}

fn node_kind(t: NodeTable) -> NodeKind {
    match t {
        NodeTable::Function => NodeKind::Function,
        NodeTable::Doc => NodeKind::Doc,
        NodeTable::Entity => NodeKind::Entity,
        NodeTable::Type => NodeKind::Type,
        NodeTable::Module => NodeKind::Module,
    }
}

/// Pull every edge that connects nodes we cluster. Each edge query
/// returns the source/target ids plus their *expected* `NodeKind`s
/// so the wiring step can re-derive the prefixed key. Edges are
/// treated as undirected at graph-build time (the `CALLS`
/// callee↔caller distinction never mattered for community
/// detection).
///
/// Skipped edge tables: `CRATE_REF` (mostly mechanical inter-repo
/// dep noise) and `ENDPOINT_TOUCHES_ENTITY` (paired with the
/// Endpoint node kind we already omit).
fn read_graph_edges(
    conn: &(impl doc_linter::store::StoreRead + ?Sized),
) -> Result<Vec<(String, String)>> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (from, to, a, b) in typed::cluster_edges(conn) {
        let a_key = format!("{}:{}", node_kind(from).prefix(), a);
        let b_key = format!("{}:{}", node_kind(to).prefix(), b);
        if a_key != b_key {
            out.push((a_key, b_key));
        }
    }
    Ok(out)
}

/// In-memory graph built once per `cluster` invocation. Vertex
/// indices are stable integers so the LPA inner loops can use
/// `Vec<usize>` instead of hash maps for hot reads.
struct Graph {
    /// `nodes[i]` is the symbol/file pair for vertex `i`.
    nodes: Vec<NodeRow>,
    /// Adjacency list — undirected. `adj[i]` lists every other
    /// vertex `i` shares an edge with (de-duped).
    adj: Vec<Vec<usize>>,
    /// In-degree of vertex `i` on the original directed CALLS graph
    /// — used as the god-node tiebreak.
    in_degree: Vec<u32>,
}

impl Graph {
    fn build(nodes: &[NodeRow], edges: &[(String, String)]) -> Self {
        // Canonical vertex order. the store returns rows in scan order, which
        // varies run to run, and LPA / Louvain results depend on vertex
        // numbering — unsorted, the same graph clustered differently on
        // back-to-back `check` runs.
        let mut nodes = nodes.to_vec();
        nodes.sort_by(|a, b| a.key.cmp(&b.key));
        let mut key_to_idx: HashMap<&str, usize> = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            key_to_idx.insert(n.key.as_str(), i);
        }
        let n = nodes.len();
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut in_degree: Vec<u32> = vec![0; n];
        for (a, b) in edges {
            let (Some(&i), Some(&j)) = (key_to_idx.get(a.as_str()), key_to_idx.get(b.as_str()))
            else {
                continue;
            };
            if i == j {
                continue;
            }
            adj[i].push(j);
            adj[j].push(i);
            in_degree[j] = in_degree[j].saturating_add(1);
        }
        // Dedupe each neighbor list — repeated CALLS edges (same
        // call site, different occurrences) would otherwise give a
        // vote-weighted LPA pass we don't want at v1.
        for list in &mut adj {
            list.sort_unstable();
            list.dedup();
        }
        Graph {
            nodes,
            adj,
            in_degree,
        }
    }
}

/// Label Propagation. Each vertex starts with a unique label
/// (its own index). On each pass, every vertex adopts the most-
/// common label among its neighbors, breaking ties deterministically
/// by lowest label value (so the order of `adj[i]` doesn't change
/// the result). Returns `(labels, iterations_run)`. Halts as soon
/// as a full pass yields no change.
fn label_propagation(graph: &Graph, max_iter: usize) -> (Vec<usize>, usize) {
    let n = graph.nodes.len();
    let mut labels: Vec<usize> = (0..n).collect();
    let mut iter = 0;
    for _ in 0..max_iter {
        iter += 1;
        let mut changed = false;
        // Walking 0..n in natural order is fine for determinism
        // — randomization is the textbook upgrade but adds an RNG
        //   dependency we don't need at v1.
        for v in 0..n {
            let neighbors = &graph.adj[v];
            if neighbors.is_empty() {
                continue;
            }
            let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
            for &u in neighbors {
                *counts.entry(labels[u]).or_insert(0) += 1;
            }
            // Pick max count, lowest label on ties.
            let (best, _) = counts
                .iter()
                .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
                .map_or((labels[v], 0), |(k, v)| (*k, *v));
            if labels[v] != best {
                labels[v] = best;
                changed = true;
            }
        }
        if !changed {
            return (labels, iter);
        }
    }
    (labels, iter)
}

/// Louvain first-level pass. Single round of modularity-greedy
/// node moves — for each vertex, evaluate the modularity gain of
/// moving it into each of its neighbor communities and adopt the
/// best move (if any). Iterates until a full sweep produces no
/// improvement.
///
/// Modularity formulation (undirected, unit-weight graph):
///   Q = (1/2m) * Σ_v Σ_u [ A_uv - (k_u * k_v) / (2m) ] * δ(c_u, c_v)
/// Move gain (close form):
///   ΔQ = (k_v_inside_c / m) - (Σ_tot_c * k_v) / (2 m²)
/// where k_v_inside_c is the number of v's edges into community c,
/// Σ_tot_c is the total degree of c, and k_v is v's degree.
///
/// v1 is single-level: no coarsen-and-recurse. The full Louvain
/// recurses on a graph where each community becomes a super-node;
/// adding the recursion is a follow-up. Single-level already
/// captures most of the modularity gain on the small-to-medium
/// graphs this subcommand sees.
fn louvain_first_level(graph: &Graph, max_iter: usize) -> (Vec<usize>, usize) {
    let n = graph.nodes.len();
    if n == 0 {
        return (Vec::new(), 0);
    }
    let mut labels: Vec<usize> = (0..n).collect();
    // Degree of each vertex on the undirected adjacency.
    let degree: Vec<usize> = (0..n).map(|i| graph.adj[i].len()).collect();
    // 2m — sum of all degrees.
    let two_m: usize = degree.iter().sum();
    if two_m == 0 {
        return (labels, 0);
    }
    let two_m_f = two_m as f64;
    let m_f = two_m_f / 2.0;
    let two_m_sq = two_m_f * two_m_f;
    // Total degree inside each community (key = label).
    let mut sigma_tot: HashMap<usize, usize> = HashMap::with_capacity(n);
    for (v, &lbl) in labels.iter().enumerate() {
        *sigma_tot.entry(lbl).or_insert(0) += degree[v];
    }

    let mut iter = 0;
    for _ in 0..max_iter {
        iter += 1;
        let mut changed = false;
        for v in 0..n {
            let neighbors = &graph.adj[v];
            if neighbors.is_empty() {
                continue;
            }
            // Neighbor-community → edge-count from v into that community.
            let mut k_in_by_c: BTreeMap<usize, usize> = BTreeMap::new();
            for &u in neighbors {
                if u == v {
                    continue;
                }
                *k_in_by_c.entry(labels[u]).or_insert(0) += 1;
            }
            let current_c = labels[v];
            let k_v = degree[v];
            // Remove v from its current community for the gain calc.
            // (Standard Louvain trick: gain of leaving + gain of joining.)
            let tot_current_minus_v = sigma_tot
                .get(&current_c)
                .copied()
                .unwrap_or(0)
                .saturating_sub(k_v) as f64;
            let k_in_current = *k_in_by_c.get(&current_c).unwrap_or(&0) as f64;

            let mut best_c = current_c;
            let mut best_gain = 0.0_f64;
            // Evaluate moving v to each neighbor community c.
            for (&c, &k_in_c_usize) in &k_in_by_c {
                if c == current_c {
                    continue;
                }
                let k_in_c = k_in_c_usize as f64;
                let tot_c = sigma_tot.get(&c).copied().unwrap_or(0) as f64;
                // Net gain = join(c) - leave(current)
                //   join(c)         = k_in_c / m - tot_c * k_v / (2m²)
                //   leave(current)  = k_in_current / m - tot_current_minus_v * k_v / (2m²)
                let join = k_in_c / m_f - tot_c * (k_v as f64) / two_m_sq;
                let leave = k_in_current / m_f - tot_current_minus_v * (k_v as f64) / two_m_sq;
                let gain = join - leave;
                // Tie-break on lower label index for determinism.
                // f64 equality is fine here — `gain` and `best_gain`
                // come from the same arithmetic so a literal tie is
                // bit-equal, and the `c < best_c` branch is the
                // tie-break we want.
                #[allow(
                    clippy::float_cmp,
                    reason = "exact f64 equality is the documented tie-break"
                )]
                let tie = gain == best_gain && c < best_c;
                if gain > 0.0 && (gain > best_gain || tie) {
                    best_gain = gain;
                    best_c = c;
                }
            }
            if best_c != current_c {
                // Commit the move: update labels + sigma_tot.
                labels[v] = best_c;
                let entry_old = sigma_tot.entry(current_c).or_insert(0);
                *entry_old = entry_old.saturating_sub(k_v);
                *sigma_tot.entry(best_c).or_insert(0) += k_v;
                changed = true;
            }
        }
        if !changed {
            return (labels, iter);
        }
    }
    (labels, iter)
}

/// Edge-weighted multigraph used for Leiden's coarsen-and-recurse
/// pipeline. Level 0 is the original unit-weight graph; aggregated
/// levels accumulate edge weights as communities collapse into
/// super-nodes (and intra-community edges become self-loops).
///
/// All weights are `u32`. The doc-linter corpus has tens of
/// thousands of CALLS edges; even after several aggregation
/// rounds a single super-node's self-loop weight stays well inside
/// `u32::MAX`. Totals (`two_m`, sigma_tot) use `u64` for headroom.
struct WeightedGraph {
    /// External adjacency: `adj[v]` lists `(u, weight)` pairs for
    /// every neighbour `u != v`. Symmetric (each edge appears at
    /// both endpoints) so the modularity-gain math reads the same
    /// way at every level.
    adj: Vec<Vec<(usize, u32)>>,
    /// Self-loop weight per node — intra-community edges that
    /// survived the last aggregation. Always zero on a freshly
    /// converted level-0 graph because [`Graph::build`] drops
    /// `i == j` rows during ingest.
    self_loop: Vec<u32>,
    /// Cached `Σ_(u≠v) w(v,u) + 2·self_loop[v]` per node. The
    /// `2·self_loop` term is the textbook convention that keeps
    /// `sum_v degree[v] == 2m` even with self-loops.
    degree: Vec<u64>,
    /// `2m` for the level — sum of all `degree[v]`. Cached so the
    /// modularity-gain inner loop doesn't recompute it each pass.
    two_m: u64,
}

impl WeightedGraph {
    /// Build a level-0 weighted graph from the unit-weight
    /// [`Graph`] used by LPA / Louvain / single-level helpers.
    /// Each adjacency entry becomes a `(neighbour, 1)` pair;
    /// parallel edges in the underlying CALLS multi-set were
    /// already deduplicated by `Graph::build` so every neighbour
    /// shows up exactly once.
    fn from_simple(graph: &Graph) -> Self {
        let n = graph.nodes.len();
        let mut adj: Vec<Vec<(usize, u32)>> = Vec::with_capacity(n);
        for nb in &graph.adj {
            adj.push(nb.iter().map(|&u| (u, 1u32)).collect());
        }
        let self_loop = vec![0u32; n];
        let degree: Vec<u64> = adj
            .iter()
            .map(|nb| nb.iter().map(|(_, w)| u64::from(*w)).sum::<u64>())
            .collect();
        let two_m: u64 = degree.iter().sum();
        WeightedGraph {
            adj,
            self_loop,
            degree,
            two_m,
        }
    }

    fn n(&self) -> usize {
        self.adj.len()
    }

    /// Collapse communities into super-nodes. Returns the coarsened
    /// graph and a [`BTreeMap`] (deterministic iteration) mapping
    /// each input label to its 0-based super-node id. Intra-community
    /// edges become contributions to the super-node's self-loop;
    /// inter-community edges accumulate symmetrically on both
    /// endpoint super-nodes.
    fn aggregate(&self, labels: &[usize]) -> (WeightedGraph, BTreeMap<usize, usize>) {
        let mut label_to_super: BTreeMap<usize, usize> = BTreeMap::new();
        for &lbl in labels {
            let next = label_to_super.len();
            label_to_super.entry(lbl).or_insert(next);
        }
        let k = label_to_super.len();
        let mut adj_acc: Vec<BTreeMap<usize, u32>> = vec![BTreeMap::new(); k];
        let mut self_loop = vec![0u32; k];

        for v in 0..self.n() {
            let sv = label_to_super[&labels[v]];
            // Carry the existing self-loop over verbatim — it's
            // already a self-loop on the destination super-node.
            self_loop[sv] = self_loop[sv].saturating_add(self.self_loop[v]);
            for &(u, w) in &self.adj[v] {
                let su = label_to_super[&labels[u]];
                if sv == su {
                    // Intra-community edge. `self.adj` is symmetric,
                    // so each underlying edge contributes twice
                    // (once at v, once at u). Guard with v < u to
                    // count it once.
                    if v < u {
                        self_loop[sv] = self_loop[sv].saturating_add(w);
                    }
                } else {
                    // Inter-community edge. Symmetric processing
                    // gives `adj_acc[sv][su] = w` and
                    // `adj_acc[su][sv] = w`, which is what we want.
                    let slot = adj_acc[sv].entry(su).or_insert(0);
                    *slot = slot.saturating_add(w);
                }
            }
        }

        let adj: Vec<Vec<(usize, u32)>> = adj_acc
            .into_iter()
            .map(|m| m.into_iter().collect())
            .collect();
        let degree: Vec<u64> = adj
            .iter()
            .enumerate()
            .map(|(i, nb)| {
                let ext: u64 = nb.iter().map(|(_, w)| u64::from(*w)).sum();
                ext + 2 * u64::from(self_loop[i])
            })
            .collect();
        let two_m: u64 = degree.iter().sum();

        (
            WeightedGraph {
                adj,
                self_loop,
                degree,
                two_m,
            },
            label_to_super,
        )
    }
}

/// Weighted Louvain local-move pass. Runs until a full sweep
/// produces no improvement or `max_iter` passes have elapsed.
/// Caller-supplied `initial_labels` lets the multi-level driver
/// seed each level with the parent-partition mapping so the
/// outer loop can detect "this level didn't improve anything"
/// without a separate reference partition.
///
/// `gamma` is the modularity resolution parameter — it scales the
/// expected-edge penalty `k_u·k_v / 2m` in the gain formula.
/// γ = 1 is standard modularity; γ > 1 makes joining a
/// community more costly and produces smaller communities; γ < 1
/// is the opposite. The Leiden multi-level pipeline runs into
/// the textbook "resolution limit" on hub-heavy graphs at γ = 1,
/// where the modularity-optimal partition merges weakly-related
/// sub-domains into giant macro-communities. Bumping γ pulls
/// them apart.
fn weighted_louvain_local_move(
    graph: &WeightedGraph,
    initial_labels: &[usize],
    max_iter: usize,
    gamma: f64,
) -> (Vec<usize>, usize) {
    let n = graph.n();
    if n == 0 || graph.two_m == 0 {
        return (initial_labels.to_vec(), 0);
    }
    let mut labels = initial_labels.to_vec();
    let mut sigma_tot: HashMap<usize, u64> = HashMap::with_capacity(n);
    for (&label, &degree) in labels.iter().zip(&graph.degree) {
        *sigma_tot.entry(label).or_insert(0) += degree;
    }
    let two_m_f = graph.two_m as f64;
    let m_f = two_m_f / 2.0;
    let two_m_sq = two_m_f * two_m_f;

    let mut iter = 0;
    for _ in 0..max_iter {
        iter += 1;
        let mut changed = false;
        for v in 0..n {
            let neighbors = &graph.adj[v];
            if neighbors.is_empty() {
                continue;
            }
            let mut k_in_by_c: BTreeMap<usize, u64> = BTreeMap::new();
            for &(u, w) in neighbors {
                *k_in_by_c.entry(labels[u]).or_insert(0) += u64::from(w);
            }
            let current_c = labels[v];
            let k_v = graph.degree[v];
            let tot_current_minus_v = sigma_tot
                .get(&current_c)
                .copied()
                .unwrap_or(0)
                .saturating_sub(k_v) as f64;
            let k_in_current = *k_in_by_c.get(&current_c).unwrap_or(&0) as f64;

            let mut best_c = current_c;
            let mut best_gain = 0.0_f64;
            for (&c, &k_in_c_u64) in &k_in_by_c {
                if c == current_c {
                    continue;
                }
                let k_in_c = k_in_c_u64 as f64;
                let tot_c = sigma_tot.get(&c).copied().unwrap_or(0) as f64;
                let join = k_in_c / m_f - gamma * tot_c * (k_v as f64) / two_m_sq;
                let leave =
                    k_in_current / m_f - gamma * tot_current_minus_v * (k_v as f64) / two_m_sq;
                let gain = join - leave;
                #[allow(
                    clippy::float_cmp,
                    reason = "exact f64 equality is the documented tie-break"
                )]
                let tie = gain == best_gain && c < best_c;
                if gain > 0.0 && (gain > best_gain || tie) {
                    best_gain = gain;
                    best_c = c;
                }
            }
            if best_c != current_c {
                labels[v] = best_c;
                let entry_old = sigma_tot.entry(current_c).or_insert(0);
                *entry_old = entry_old.saturating_sub(k_v);
                *sigma_tot.entry(best_c).or_insert(0) += k_v;
                changed = true;
            }
        }
        if !changed {
            return (labels, iter);
        }
    }
    (labels, iter)
}

/// Weighted Leiden refinement. Starts each vertex in its own
/// singleton sub-community, then runs Louvain-style local moves
/// with one extra filter: vertex `v` can only join the community
/// of a neighbour `u` if `u` shared `v`'s `phase1` community.
/// Cross-boundary neighbours still contribute to the global 2m
/// and degree totals (those are level-invariant) but they cannot
/// appear as candidate destinations for `v`. `gamma` is the same
/// resolution parameter as in [`weighted_louvain_local_move`].
fn weighted_leiden_refine(
    graph: &WeightedGraph,
    phase1: &[usize],
    max_iter: usize,
    gamma: f64,
) -> Vec<usize> {
    let n = graph.n();
    if n == 0 || graph.two_m == 0 {
        return (0..n).collect();
    }
    let singletons: Vec<usize> = (0..n).collect();
    let two_m_f = graph.two_m as f64;
    let m_f = two_m_f / 2.0;
    let two_m_sq = two_m_f * two_m_f;
    let mut labels = singletons;
    let mut sigma_tot: HashMap<usize, u64> = HashMap::with_capacity(n);
    for (&label, &degree) in labels.iter().zip(&graph.degree) {
        *sigma_tot.entry(label).or_insert(0) += degree;
    }

    for _ in 0..max_iter {
        let mut changed = false;
        for v in 0..n {
            let neighbors = &graph.adj[v];
            if neighbors.is_empty() {
                continue;
            }
            let phase1_v = phase1[v];
            let mut k_in_by_c: BTreeMap<usize, u64> = BTreeMap::new();
            for &(u, w) in neighbors {
                if phase1[u] != phase1_v {
                    continue;
                }
                *k_in_by_c.entry(labels[u]).or_insert(0) += u64::from(w);
            }
            let current_c = labels[v];
            let k_v = graph.degree[v];
            let tot_current_minus_v = sigma_tot
                .get(&current_c)
                .copied()
                .unwrap_or(0)
                .saturating_sub(k_v) as f64;
            let k_in_current = *k_in_by_c.get(&current_c).unwrap_or(&0) as f64;

            let mut best_c = current_c;
            let mut best_gain = 0.0_f64;
            for (&c, &k_in_c_u64) in &k_in_by_c {
                if c == current_c {
                    continue;
                }
                let k_in_c = k_in_c_u64 as f64;
                let tot_c = sigma_tot.get(&c).copied().unwrap_or(0) as f64;
                let join = k_in_c / m_f - gamma * tot_c * (k_v as f64) / two_m_sq;
                let leave =
                    k_in_current / m_f - gamma * tot_current_minus_v * (k_v as f64) / two_m_sq;
                let gain = join - leave;
                #[allow(
                    clippy::float_cmp,
                    reason = "exact f64 equality is the documented tie-break"
                )]
                let tie = gain == best_gain && c < best_c;
                if gain > 0.0 && (gain > best_gain || tie) {
                    best_gain = gain;
                    best_c = c;
                }
            }
            if best_c != current_c {
                labels[v] = best_c;
                let entry_old = sigma_tot.entry(current_c).or_insert(0);
                *entry_old = entry_old.saturating_sub(k_v);
                *sigma_tot.entry(best_c).or_insert(0) += k_v;
                changed = true;
            }
        }
        if !changed {
            return labels;
        }
    }
    labels
}

/// Thin wrapper kept for the unit test that pins refinement's
/// phase-1 boundary invariant. Converts a unit-weight [`Graph`]
/// to the weighted form and delegates at γ = 1.0 (the partition
/// invariant we pin is independent of resolution).
#[cfg(test)]
fn leiden_refine(graph: &Graph, phase1: &[usize], max_iter: usize) -> Vec<usize> {
    weighted_leiden_refine(&WeightedGraph::from_simple(graph), phase1, max_iter, 1.0)
}

/// Multi-level Leiden (Traag, Waltman & van Eck 2019). The
/// real algorithm — single-level Leiden was a stepping stone.
/// Each level runs:
///
/// 1. **Local moving** on the current (possibly aggregated) graph,
///    seeded with the parent-partition mapping. On level 0 every
///    node starts in its own singleton; subsequent levels start
///    every super-node in the community its members shared at the
///    previous phase-1.
/// 2. **Refinement** — re-partition each phase-1 community from
///    singletons under the boundary restriction. This is what
///    splits hub-and-spoke "communities" that local moving glued
///    together.
/// 3. **Aggregation** — collapse refined communities into
///    super-nodes; intra-community edges become self-loops,
///    inter-community edges accumulate weight.
///
/// Convergence: stop when either the local-move pass returns
/// labels identical to the level's initial partition (no
/// improvement) or when aggregation produces no further
/// coarsening (`new_n == current_n`). The returned label vector
/// is over the **original** (level-0) nodes; multi-level Leiden
/// tracks each level's super-node mapping back to original
/// vertices so callers don't need to walk the hierarchy
/// themselves. Returned `usize` is the cumulative pass count
/// across all levels (informational; reported in the JSON
/// `iterations` field).
fn leiden_multilevel(graph: &Graph, max_iter: usize, gamma: f64) -> (Vec<usize>, usize) {
    let n0 = graph.nodes.len();
    if n0 == 0 {
        return (Vec::new(), 0);
    }
    let mut current = WeightedGraph::from_simple(graph);
    // node_to_super[v] = which super-node in `current` contains
    // original vertex v. Level 0 is the identity.
    let mut node_to_super: Vec<usize> = (0..n0).collect();
    // Initial labels at level 0: each super-node is its own
    // community. At deeper levels this gets replaced with the
    // parent-partition mapping (one label per refined community).
    let mut initial_labels: Vec<usize> = (0..current.n()).collect();
    let mut total_iter = 0usize;
    let level_cap = 32; // safety stop — real graphs converge in <10

    for _ in 0..level_cap {
        let (phase1, iter) =
            weighted_louvain_local_move(&current, &initial_labels, max_iter, gamma);
        total_iter = total_iter.saturating_add(iter);

        // Did this level improve anything? If phase 1 returns the
        // initial labels untouched, every super-node is already in
        // its locally-optimal community — we're done.
        if phase1 == initial_labels {
            let final_labels: Vec<usize> =
                node_to_super.iter().map(|&s| initial_labels[s]).collect();
            return (final_labels, total_iter);
        }

        let refined = weighted_leiden_refine(&current, &phase1, max_iter, gamma);
        let (new_graph, label_to_super) = current.aggregate(&refined);

        // No further coarsening possible — every refined community
        // is a singleton. Use phase1 as the final labelling.
        if new_graph.n() == current.n() {
            let final_labels: Vec<usize> = node_to_super.iter().map(|&s| phase1[s]).collect();
            return (final_labels, total_iter);
        }

        // Walk every original vertex through the new aggregation
        // step. `refined[old_super]` is the refined community that
        // contained `old_super`; that community now maps to a
        // single super-node at the new level.
        for old_super in &mut node_to_super {
            *old_super = label_to_super[&refined[*old_super]];
        }

        // Each new super-node inherits the phase-1 label of any of
        // its (refined-community) members. Refinement guarantees
        // every member of a refined community shares one phase-1
        // label, so picking any constituent works.
        let new_n = new_graph.n();
        let mut next_initial = vec![0usize; new_n];
        for old in 0..current.n() {
            let new_super = label_to_super[&refined[old]];
            next_initial[new_super] = phase1[old];
        }

        current = new_graph;
        initial_labels = next_initial;
    }

    // Safety-stop fallthrough — shouldn't be reached on real graphs.
    let final_labels: Vec<usize> = node_to_super.iter().map(|&s| initial_labels[s]).collect();
    (final_labels, total_iter)
}

/// Bucket vertices by their final label, compute per-community
/// stats, and pick the top-`top_n` per the requested ordering.
/// `min_members` filters out communities below the threshold —
/// pass 2 for the historical default (drop singletons only).
#[allow(
    clippy::too_many_arguments,
    reason = "report-builder accumulates per-flag args incrementally"
)]
fn build_report(
    graph: &Graph,
    labels: &[usize],
    iterations: usize,
    top_n: usize,
    ordering: OrderBy,
    min_members: usize,
    top_members: usize,
    algo: Algorithm,
) -> Report {
    let mut by_label: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (idx, &lbl) in labels.iter().enumerate() {
        by_label.entry(lbl).or_default().push(idx);
    }

    let total_communities = by_label.len();
    let floor = min_members.max(2);
    let preview_cap = top_members.max(1);

    let mut communities: Vec<Candidate> = by_label
        .into_values()
        .filter(|members| members.len() >= floor)
        .map(|members| build_candidate(graph, &members, preview_cap))
        .collect();
    let cmp_density = |a: &Candidate, b: &Candidate| {
        b.density
            .partial_cmp(&a.density)
            .unwrap_or(std::cmp::Ordering::Equal)
    };
    let cmp_members = |a: &Candidate, b: &Candidate| b.member_count.cmp(&a.member_count);
    communities.sort_by(|a, b| match ordering {
        OrderBy::MemberCount => cmp_members(a, b).then_with(|| cmp_density(a, b)),
        OrderBy::Density => cmp_density(a, b).then_with(|| cmp_members(a, b)),
    });
    communities.truncate(top_n);

    Report {
        algorithm: algo.label(),
        order_by: ordering.label(),
        iterations,
        total_communities,
        candidates: communities,
        written: Vec::new(),
    }
}

/// Write one markdown stub per candidate to `<output_dir>/<id>.md`,
/// where `output_dir` is repo-relative (the default
/// `docs/ontology/entities/candidates` matches the v0.3.0 issue
/// spec and is what the validator recognises for
/// `status: candidate` docs; iteration / staging workflows can
/// point elsewhere). Skips emission for candidates whose stub
/// already exists — re-runs are idempotent so an operator can
/// iterate on `--write` without losing manual edits. Returns the
/// repo-relative paths actually written.
fn emit_candidate_stubs(
    root: &Path,
    output_dir: &Path,
    candidates: &[Candidate],
) -> Result<Vec<String>> {
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    // Absolute paths are honored verbatim; relative paths resolve
    // against root so `--output staging/x` lands at `<root>/staging/x`.
    let dir = if output_dir.is_absolute() {
        output_dir.to_path_buf()
    } else {
        root.join(output_dir)
    };
    // Issue #180 dedupe: skip when a canonical entity already owns the
    // id. The canonical layout puts authored entities under
    // `docs/ontology/entities/<id>.md`; check that path even when
    // `--output` targets a different directory.
    let entities_dir = root.join("docs/ontology/entities");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let mut written: Vec<String> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in candidates {
        // Dedupe within a single run — two communities can derive
        // the same suggested_id if their god-nodes share a file
        // stem; we'd otherwise overwrite ourselves mid-loop.
        if !seen_ids.insert(c.suggested_id.clone()) {
            continue;
        }
        // Skip when a canonical (human-authored) entity already owns
        // this id — emitting under `candidates/` would create a
        // duplicate `entity-<id>` doc id and trip the dupe-id lint.
        // The human-authored doc wins; cluster output stays read-only
        // with respect to the existing ontology.
        if entities_dir.join(format!("{}.md", c.suggested_id)).exists() {
            continue;
        }
        let path = dir.join(format!("{}.md", c.suggested_id));
        if path.exists() {
            continue;
        }
        let body = render_candidate_stub(c, &today);
        std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string()
            .replace('\\', "/");
        written.push(rel);
    }
    Ok(written)
}

/// Roadmap issue #14 v11: `cluster --promote <id>` JSON report
/// shape. Distinct from the clustering [`Report`] so the
/// promotion path's output schema can evolve independently — a
/// reviewer scripting around it knows they're getting promotion
/// metadata, not a community report.
#[derive(Debug, Serialize, PartialEq, Eq)]
struct PromoteReport {
    /// Stable string for callers to dispatch on. `"promoted"` on
    /// success; this is the only variant today, but reserving the
    /// field keeps room for `"already-promoted"` / `"not-found"`
    /// successor variants without an output-format break.
    action: &'static str,
    /// The candidate id the reviewer passed on the CLI.
    id: String,
    /// Repo-relative source path that was moved out.
    from: String,
    /// Repo-relative destination path the candidate was promoted to.
    to: String,
}

/// Roadmap issue #14 v11: promote a reviewed candidate from
/// `<output>/<id>.md` to `<root>/docs/ontology/entities/<id>.md`,
/// flipping the frontmatter `status: candidate` to `status: stable`
/// and dropping the "(candidate)" title suffix in one shot.
///
/// Refuses to overwrite an existing entity at the destination —
/// promoting twice (or onto a hand-authored entity that happens to
/// share the id) would be silent data loss. The reviewer is
/// expected to either pick a different `--promote <id>` or to
/// delete the existing entity first.
fn promote_candidate(root: &Path, output: &Path, id: &str) -> Result<PromoteReport> {
    if id.is_empty() {
        anyhow::bail!("--promote requires a candidate id (e.g. `--promote parser_module`)");
    }

    let src_dir = if output.is_absolute() {
        output.to_path_buf()
    } else {
        root.join(output)
    };
    let src = src_dir.join(format!("{id}.md"));
    if !src.exists() {
        anyhow::bail!(
            "no candidate at {} — pass an id that exists under `{}` (the same dir \
             `cluster --write [--output]` emits to)",
            src.display(),
            src_dir.display(),
        );
    }

    // The destination is hardcoded to the canonical entities tree.
    // Operators with non-standard layouts can move the file manually
    // after promotion; the v1 shape favours convention over a second
    // CLI knob.
    let dest_dir = root.join("docs/ontology/entities");
    let dest = dest_dir.join(format!("{id}.md"));
    if dest.exists() {
        anyhow::bail!(
            "destination already exists at {} — refusing to overwrite (delete the \
             existing entity first if the promotion really is intentional)",
            dest.display(),
        );
    }

    let body = std::fs::read_to_string(&src).with_context(|| format!("read {}", src.display()))?;
    let rewritten = rewrite_candidate_to_stable(&body);

    std::fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    std::fs::write(&dest, rewritten).with_context(|| format!("write {}", dest.display()))?;
    std::fs::remove_file(&src).with_context(|| format!("remove {}", src.display()))?;

    Ok(PromoteReport {
        action: "promoted",
        id: id.to_string(),
        from: rel_to_root(root, &src),
        to: rel_to_root(root, &dest),
    })
}

/// Rewrite a candidate stub's body so the `status:` and `title:`
/// lines reflect a promoted entity. Operates on the raw markdown +
/// frontmatter so we don't need to round-trip through serde_yaml
/// (which would re-serialise the file and lose comments / ordering
/// the reviewer may have introduced during review).
fn rewrite_candidate_to_stable(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "status: candidate" {
            out.push_str("status: stable\n");
        } else if let Some(rest) = trimmed.strip_prefix("title: ") {
            // Drop a trailing " (candidate)" inside a quoted title.
            // The candidate stub renders titles like:
            //   title: "Entity: parser_module (candidate)"
            // The promoted form is:
            //   title: "Entity: parser_module"
            let cleaned = rest.strip_suffix("(candidate)\"").map_or_else(
                || rest.to_string(),
                |prefix| format!("{}\"", prefix.trim_end()),
            );
            out.push_str("title: ");
            out.push_str(&cleaned);
            out.push('\n');
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Format a path as repo-relative + forward-slashed for the JSON
/// report. Falls back to the absolute path when stripping fails
/// (e.g. an absolute --output outside the repo) — the reviewer
/// still gets something they can copy-paste.
fn rel_to_root(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Issue #179: synthesize a per-candidate description from the
/// data already in `Candidate` so each stub is semantically distinct
/// (the embedding backend needs discriminative text, and a human
/// reviewer needs to know what the community actually does at a
/// glance). Inputs: the god-node file path, the top members' SCIP
/// symbols, the set of source files. No external deps.
fn synthesize_description(c: &Candidate) -> String {
    // 1. Module label from god-node file: `app/pattern_detector.py`
    //    -> "pattern detector".
    let module_label = c
        .god_node_file
        .rsplit('/')
        .next()
        .unwrap_or(&c.god_node_file)
        .trim_end_matches(".py")
        .trim_end_matches(".rs")
        .trim_end_matches(".ts")
        .trim_end_matches(".tsx")
        .replace('_', " ");

    // 2. Method names from top_members. Strip SCIP package prefix
    //    and pull the trailing descriptor (function name).
    let mut method_names: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for sym in &c.top_members {
        if let Some(name) = scip_local_descriptor(sym) {
            let cleaned = name
                .trim_end_matches("().")
                .trim_end_matches(')')
                .trim_end_matches('(')
                .to_string();
            if cleaned.len() < 2 || cleaned.starts_with('_') {
                continue;
            }
            if seen.insert(cleaned.clone()) {
                method_names.push(cleaned);
                if method_names.len() >= 4 {
                    break;
                }
            }
        }
    }

    // 3. Top directories from source_files.
    let mut dirs: Vec<String> = Vec::new();
    let mut dir_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for f in &c.source_files {
        let dir = f.rsplit_once('/').map_or(".", |(d, _)| d);
        // Just the first path segment is usually the cleanest label.
        let top = dir.split('/').next().unwrap_or(dir).to_string();
        if top.is_empty() {
            continue;
        }
        if dir_seen.insert(top.clone()) {
            dirs.push(top);
            if dirs.len() >= 3 {
                break;
            }
        }
    }

    let methods_phrase = if method_names.is_empty() {
        String::new()
    } else {
        format!(" Surfaces around {}.", method_names.join(", "))
    };
    let dirs_phrase = if dirs.is_empty() {
        String::new()
    } else {
        format!(" Spans {}.", dirs.join(", "))
    };

    format!(
        "Cluster anchored on {module_label} ({} functions across {} file(s), density {:.2}).{methods_phrase}{dirs_phrase} Auto-generated by doc-linter cluster; review and promote to status: stable before relying on it.",
        c.member_count,
        c.source_files.len(),
        c.density,
    )
}

/// Pull the trailing function/method descriptor out of a SCIP symbol.
/// Example: `scip-python python . abc 'pkg.module'/Class#method().`
/// -> "method", or for module-level: `.../mod'/func().` -> "func".
fn scip_local_descriptor(symbol: &str) -> Option<String> {
    // Walk back to the last `/` and then take whatever's after it,
    // dropping the surrounding `#` / `()` / `.` decorations.
    let after_slash = symbol.rsplit('/').next()?;
    // Strip backtick-quoted module if it's the whole segment.
    let trimmed = after_slash.trim_matches('`');
    // For `Class#method().` keep just `method`.
    if let Some((_class, rest)) = trimmed.split_once('#') {
        let m = rest.trim_end_matches("().").trim_end_matches('.');
        return Some(m.to_string());
    }
    let m = trimmed.trim_end_matches("().").trim_end_matches('.');
    if m.is_empty() {
        None
    } else {
        Some(m.to_string())
    }
}

/// Render one candidate as a doc-linter ontology-entity stub.
///
/// Issue #180: emits `status: auto` so the entity participates in
/// FUNCTION_MENTIONS (graph + ranking) but is exempt from coverage /
/// disambiguation lint. A human review can promote the stub to
/// `status: stable` (gaining enforcement) or delete the file.
/// Old v0.3.0 stubs may still carry `status: candidate` — those
/// remain excluded from the term index entirely.
fn render_candidate_stub(c: &Candidate, today: &str) -> String {
    let mut top = String::new();
    for (i, m) in c.top_members.iter().enumerate() {
        if i > 0 {
            top.push_str(", ");
        }
        top.push_str(m);
    }

    // YAML list of source files. Fallback to the god-node file when
    // the cluster ingest didn't capture file paths (e.g. SCIP symbols
    // without document attribution).
    let mut source_modules_yaml = String::new();
    let files: Vec<&str> = if c.source_files.is_empty() {
        vec![c.god_node_file.as_str()]
    } else {
        c.source_files.iter().map(String::as_str).collect()
    };
    for f in &files {
        source_modules_yaml.push_str("  - ");
        source_modules_yaml.push_str(f);
        source_modules_yaml.push('\n');
    }

    format!(
        "---\n\
         id: entity-{id}\n\
         role: ontology-entity\n\
         title: \"Entity: {id} (auto)\"\n\
         summary: cluster-derived auto-entity; review before promoting\n\
         status: auto\n\
         confidence: {density:.2}\n\
         updated: {today}\n\
         axis_id: covers\n\
         value_id: {id}\n\
         display: {id}\n\
         description: \"{synthesized}\"\n\
         source_modules:\n\
         {source_modules}\
         ---\n\
         \n\
         # Candidate entity derived from cluster analysis\n\
         \n\
         **God node**: `{god}` (file: `{file}`)\n\
         **Community size**: {size} functions\n\
         **Confidence (density)**: {density:.2}\n\
         **Files**: {file_count}\n\
         \n\
         ## Top members\n\
         \n\
         {top}\n\
         \n\
         ## Promotion checklist\n\
         \n\
         - [ ] Does this represent a real architectural entity?\n\
         - [ ] If yes: change `status` to `stable`, write a real description, \
         and let `doc-linter check` enforce coverage.\n\
         - [ ] If no: delete this file.\n",
        id = c.suggested_id,
        today = today,
        density = c.density,
        god = c.god_node_symbol,
        synthesized = synthesize_description(c).replace('"', "'"),
        file = c.god_node_file,
        size = c.member_count,
        top = top,
        source_modules = source_modules_yaml,
        file_count = files.len(),
    )
}

/// Builds one [`Candidate`] from a community's member-index list:
/// picks the god node, derives a name (preferring Entity, then Doc,
/// then god-function file-stem), and computes intra-community
/// density.
fn build_candidate(graph: &Graph, members: &[usize], top_members_cap: usize) -> Candidate {
    let member_count = members.len();

    // Member set for O(1) "is intra-edge?" checks.
    let member_set: std::collections::HashSet<usize> = members.iter().copied().collect();

    // Per-kind census. BTreeMap so the JSON is ordered the same
    // way every run (Function / Doc / Entity / ...).
    let mut kind_counts: BTreeMap<NodeKind, usize> = BTreeMap::new();
    for &v in members {
        *kind_counts.entry(graph.nodes[v].kind).or_insert(0) += 1;
    }

    // Pick the highest-in-degree member of a given kind; returns
    // None if the community has no member of that kind. Tiebreak
    // on the id string reversed, matching the historical Function
    // behaviour (so `parser/parse_doc` wins over `parser/parse_x`).
    let best_of_kind = |kind: NodeKind| -> Option<usize> {
        members
            .iter()
            .copied()
            .filter(|&i| graph.nodes[i].kind == kind)
            .max_by(|&a, &b| {
                graph.in_degree[a]
                    .cmp(&graph.in_degree[b])
                    .then_with(|| graph.nodes[a].id.cmp(&graph.nodes[b].id).reverse())
            })
    };

    // God node = max in-degree among ALL members. Ties broken by id.
    let &god_idx = members
        .iter()
        .max_by(|&&a, &&b| {
            graph.in_degree[a]
                .cmp(&graph.in_degree[b])
                .then_with(|| graph.nodes[a].id.cmp(&graph.nodes[b].id).reverse())
        })
        .unwrap_or(&members[0]);
    let god_node_symbol = graph.nodes[god_idx].id.clone();
    let god_node_file = graph.nodes[god_idx].file.clone();
    let god_node_kind = graph.nodes[god_idx].kind;

    // Name derivation, in priority order. Entity > Doc > god-file.
    // An Entity in the community means the cluster already has a
    // canonical name — use it verbatim (Entity ids are already
    // kebab-case). A Doc in the community gives us a slug-able
    // doc id. Otherwise fall back to the historical heuristic.
    let suggested_id = if let Some(idx) = best_of_kind(NodeKind::Entity) {
        graph.nodes[idx].id.clone()
    } else if let Some(idx) = best_of_kind(NodeKind::Doc) {
        slugify(&graph.nodes[idx].id)
    } else {
        derive_entity_id(&god_node_file, &god_node_symbol)
    };

    // Intra-community edge count. `adj` is undirected so each
    // intra-edge appears twice — halve at the end.
    let mut intra_edge_count: usize = 0;
    for &v in members {
        for &u in &graph.adj[v] {
            if member_set.contains(&u) {
                intra_edge_count += 1;
            }
        }
    }
    let intra_edges = intra_edge_count / 2;
    let max_possible = member_count * member_count.saturating_sub(1) / 2;
    let density = if max_possible == 0 {
        0.0
    } else {
        intra_edges as f64 / max_possible as f64
    };

    // Top members by in-degree. v9: cap is now configurable via
    // the caller (defaults to 5 from the CLI flag).
    let mut sorted_members: Vec<usize> = members.to_vec();
    sorted_members.sort_by(|&a, &b| {
        graph.in_degree[b]
            .cmp(&graph.in_degree[a])
            .then_with(|| graph.nodes[a].id.cmp(&graph.nodes[b].id))
    });
    let top_members: Vec<String> = sorted_members
        .into_iter()
        .take(top_members_cap)
        .map(|i| graph.nodes[i].id.clone())
        .collect();

    // Distinct member files. Entity members have empty `file` and
    // get filtered out. Cap at 50 — beyond that the stub becomes
    // review-hostile and a human should split the cluster.
    let mut source_files: Vec<String> = members
        .iter()
        .map(|&i| graph.nodes[i].file.clone())
        .filter(|f| !f.is_empty())
        .collect();
    source_files.sort();
    source_files.dedup();
    if source_files.len() > 50 {
        source_files.truncate(50);
    }

    Candidate {
        suggested_id,
        god_node_symbol,
        god_node_file,
        god_node_kind,
        member_count,
        kind_counts,
        density,
        top_members,
        source_files,
    }
}

/// Derives a kebab-cased entity-id hint from a SCIP file path.
/// `crates/pricing-core/src/lib.rs` → `pricing-core`,
/// `src/parser/utils.rs` → `utils`, with a final fallback on the
/// god-node symbol's local descriptor.
fn derive_entity_id(file: &str, symbol: &str) -> String {
    let file = file.replace('\\', "/");
    let parts: Vec<&str> = file.split('/').filter(|s| !s.is_empty()).collect();

    // `crates/<name>/...` → use <name>.
    if parts.first().copied() == Some("crates") {
        if let Some(crate_name) = parts.get(1) {
            return slugify(crate_name);
        }
    }

    // Otherwise take the file stem (minus extension).
    if let Some(last) = parts.last() {
        let stem = last.split('.').next().unwrap_or(last);
        if !stem.is_empty() && stem != "lib" && stem != "mod" {
            return slugify(stem);
        }
        // `lib.rs` / `mod.rs` → use the parent dir name.
        if let Some(parent) = parts.iter().rev().nth(1) {
            return slugify(parent);
        }
    }

    // Fall back to the symbol's trailing descriptor.
    let stem = symbol
        .rsplit('/')
        .next()
        .unwrap_or(symbol)
        .trim_end_matches("().")
        .trim_end_matches('.')
        .trim_end_matches('#');
    slugify(stem)
}

/// Convert an arbitrary identifier into a safe kebab-cased entity
/// id. Conservative: alphanumeric runs are kept; everything else
/// becomes a `-`; consecutive dashes collapse; lowercase.
fn slugify(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    #[test]
    fn clustering_ignores_input_row_order() {
        // Two triangles and a bridge vertex `x` tied equally to both: which
        // side `x` joins is decided by tie-breaking, i.e. by vertex order.
        let node = |id: &str| {
            super::NodeRow::new(super::NodeKind::Function, id.to_string(), String::new())
        };
        let pairs = [
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
            ("x", "a1"),
            ("x", "b1"),
        ];
        let edges: Vec<(String, String)> = pairs
            .iter()
            .map(|(p, q)| (node(p).key, node(q).key))
            .collect();
        let communities = |order: &[&str]| {
            let nodes: Vec<_> = order.iter().map(|id| node(id)).collect();
            let g = super::Graph::build(&nodes, &edges);
            let (labels, _) = super::label_propagation(&g, 50);
            let mut by_label = std::collections::BTreeMap::<usize, Vec<String>>::new();
            for (i, l) in labels.iter().enumerate() {
                by_label.entry(*l).or_default().push(g.nodes[i].key.clone());
            }
            let mut groups: Vec<Vec<String>> = by_label
                .into_values()
                .map(|mut m| {
                    m.sort();
                    m
                })
                .collect();
            groups.sort();
            groups
        };
        let forward = ["a1", "a2", "a3", "x", "b1", "b2", "b3"];
        let backward = ["b3", "b2", "b1", "x", "a3", "a2", "a1"];
        assert_eq!(communities(&forward), communities(&backward));
    }

    use super::*;

    /// Test helper — every (id, file) pair becomes a Function
    /// `NodeRow`. Existing tests pre-date the heterogeneous graph
    /// and assume CALLS-only inputs, so defaulting to Function
    /// keeps them valid. Tests that exercise the new kind-aware
    /// paths construct `NodeRow::new(kind, id, file)` directly.
    fn nodes(names: &[(&str, &str)]) -> Vec<NodeRow> {
        names
            .iter()
            .map(|(s, f)| NodeRow::new(NodeKind::Function, (*s).to_string(), (*f).to_string()))
            .collect()
    }
    /// Test helper — emit `(a_key, b_key)` pairs in the kind-prefixed
    /// form `Graph::build` now expects. All inputs default to
    /// Function-to-Function edges, matching what `nodes()` produces.
    fn edges(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (format!("fn:{a}"), format!("fn:{b}")))
            .collect()
    }

    #[test]
    fn lpa_finds_two_disjoint_triangles() {
        // Two K3 cliques with no cross-edges → two communities.
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
        ]);
        let e = edges(&[
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let (labels, _iter) = label_propagation(&g, 50);
        // Every member of the first triangle shares one label;
        // every member of the second shares another (different) one.
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[1], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[4], labels[5]);
        assert_ne!(labels[0], labels[3], "the cliques are distinct communities");
    }

    #[test]
    fn build_candidate_prefers_entity_id_when_community_has_entity() {
        // Mixed community: 2 Functions calling each other + 1 Entity
        // they both belong to. Suggested_id should be the entity's id,
        // not a file-stem.
        let nodes = vec![
            NodeRow::new(NodeKind::Function, "fn_a".to_string(), "a.rs".to_string()),
            NodeRow::new(NodeKind::Function, "fn_b".to_string(), "b.rs".to_string()),
            NodeRow::new(NodeKind::Entity, "my-entity".to_string(), String::new()),
        ];
        let edges = vec![
            ("fn:fn_a".to_string(), "fn:fn_b".to_string()),
            ("fn:fn_a".to_string(), "ent:my-entity".to_string()),
            ("fn:fn_b".to_string(), "ent:my-entity".to_string()),
        ];
        let g = Graph::build(&nodes, &edges);
        let c = build_candidate(&g, &[0, 1, 2], 5);
        assert_eq!(c.suggested_id, "my-entity");
        assert_eq!(c.kind_counts.get(&NodeKind::Function).copied(), Some(2));
        assert_eq!(c.kind_counts.get(&NodeKind::Entity).copied(), Some(1));
    }

    #[test]
    fn build_candidate_falls_back_to_doc_when_no_entity() {
        // Function + Doc, no Entity. Suggested_id should slug the
        // doc's id.
        let nodes = vec![
            NodeRow::new(NodeKind::Function, "fn_a".to_string(), "a.rs".to_string()),
            NodeRow::new(
                NodeKind::Doc,
                "explanation-parser-pipeline".to_string(),
                "docs/explanations/parser.md".to_string(),
            ),
        ];
        let edges = vec![(
            "fn:fn_a".to_string(),
            "doc:explanation-parser-pipeline".to_string(),
        )];
        let g = Graph::build(&nodes, &edges);
        let c = build_candidate(&g, &[0, 1], 5);
        assert_eq!(c.suggested_id, "explanation-parser-pipeline");
        assert!(c.kind_counts.contains_key(&NodeKind::Doc));
    }

    #[test]
    fn build_candidate_falls_back_to_file_stem_for_code_only_cluster() {
        // No Entity, no Doc — historical behaviour preserved.
        let nodes = vec![
            NodeRow::new(
                NodeKind::Function,
                "fn_a".to_string(),
                "src/scorer.rs".to_string(),
            ),
            NodeRow::new(
                NodeKind::Function,
                "fn_b".to_string(),
                "src/scorer.rs".to_string(),
            ),
        ];
        let edges = vec![("fn:fn_a".to_string(), "fn:fn_b".to_string())];
        let g = Graph::build(&nodes, &edges);
        let c = build_candidate(&g, &[0, 1], 5);
        assert_eq!(c.suggested_id, "scorer");
    }

    #[test]
    fn report_picks_god_node_by_in_degree() {
        // 3-node star: hub h is called by leaves l1, l2 — hub has
        // in-degree 2; either leaf has 0.
        let n = nodes(&[("h", "h.rs"), ("l1", "l1.rs"), ("l2", "l2.rs")]);
        let e = edges(&[("l1", "h"), ("l2", "h")]);
        let g = Graph::build(&n, &e);
        let (labels, iter) = label_propagation(&g, 50);
        let report = build_report(
            &g,
            &labels,
            iter,
            10,
            OrderBy::MemberCount,
            2,
            5,
            Algorithm::Lpa,
        );
        assert!(!report.candidates.is_empty(), "star yields 1 community");
        let c = &report.candidates[0];
        assert_eq!(c.god_node_symbol, "h");
        assert_eq!(c.member_count, 3);
    }

    #[test]
    fn order_by_density_prefers_tighter_cliques() {
        // Triangle (density 1.0, 3 members) vs 4-node path
        // (density 1/2 = 0.5, 4 members). MemberCount ranks the
        // path first; Density ranks the triangle first.
        let n = nodes(&[
            ("t1", "t.rs"),
            ("t2", "t.rs"),
            ("t3", "t.rs"),
            ("p1", "p.rs"),
            ("p2", "p.rs"),
            ("p3", "p.rs"),
            ("p4", "p.rs"),
        ]);
        let e = edges(&[
            ("t1", "t2"),
            ("t2", "t3"),
            ("t3", "t1"),
            ("p1", "p2"),
            ("p2", "p3"),
            ("p3", "p4"),
        ]);
        let g = Graph::build(&n, &e);
        let (labels, iter) = label_propagation(&g, 50);

        let mc = build_report(
            &g,
            &labels,
            iter,
            10,
            OrderBy::MemberCount,
            2,
            5,
            Algorithm::Lpa,
        );
        let de = build_report(
            &g,
            &labels,
            iter,
            10,
            OrderBy::Density,
            2,
            5,
            Algorithm::Lpa,
        );

        assert!(mc.candidates.len() == 2 && de.candidates.len() == 2);
        // MemberCount: path (4 members) first; Density: triangle (1.0) first.
        assert!(mc.candidates[0].member_count >= mc.candidates[1].member_count);
        assert!(de.candidates[0].density >= de.candidates[1].density);
        assert_ne!(
            mc.candidates[0].god_node_symbol, de.candidates[0].god_node_symbol,
            "MemberCount and Density orderings should land on different #1 candidates here"
        );
    }

    #[test]
    fn order_by_parse_rejects_unknown_key() {
        assert!(OrderBy::parse("zoo").is_err());
        assert_eq!(
            OrderBy::parse("member-count").unwrap(),
            OrderBy::MemberCount
        );
        assert_eq!(OrderBy::parse("members").unwrap(), OrderBy::MemberCount);
        assert_eq!(OrderBy::parse("density").unwrap(), OrderBy::Density);
    }

    #[test]
    fn algorithm_parse_accepts_aliases_and_rejects_unknown() {
        assert_eq!(Algorithm::parse("lpa").unwrap(), Algorithm::Lpa);
        assert_eq!(
            Algorithm::parse("label-propagation").unwrap(),
            Algorithm::Lpa
        );
        assert_eq!(Algorithm::parse("louvain").unwrap(), Algorithm::Louvain);
        assert_eq!(Algorithm::parse("modularity").unwrap(), Algorithm::Louvain);
        assert_eq!(Algorithm::parse("leiden").unwrap(), Algorithm::Leiden);
        assert!(Algorithm::parse("k-means").is_err());
    }

    #[test]
    fn louvain_separates_disjoint_triangles() {
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
        ]);
        let e = edges(&[
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let (labels, _iter) = louvain_first_level(&g, 50);
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[1], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[4], labels[5]);
        assert_ne!(
            labels[0], labels[3],
            "two triangles should be distinct communities"
        );
    }

    #[test]
    fn louvain_handles_empty_graph() {
        let g = Graph::build(&[], &[]);
        let (labels, iter) = louvain_first_level(&g, 50);
        assert!(labels.is_empty());
        assert_eq!(iter, 0);
    }

    #[test]
    fn leiden_separates_disjoint_triangles() {
        // Parity with the Louvain test — two K3 cliques, no cross
        // edges. Multi-level Leiden converges in one level here
        // (each triangle is already its own modularity-optimal
        // community) and returns two distinct labels.
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
        ]);
        let e = edges(&[
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let (labels, _iter) = leiden_multilevel(&g, 50, 1.0);
        assert_eq!(labels[0], labels[1]);
        assert_eq!(labels[1], labels[2]);
        assert_eq!(labels[3], labels[4]);
        assert_eq!(labels[4], labels[5]);
        assert_ne!(
            labels[0], labels[3],
            "two triangles should be distinct communities"
        );
    }

    #[test]
    fn leiden_handles_empty_graph() {
        let g = Graph::build(&[], &[]);
        let (labels, iter) = leiden_multilevel(&g, 50, 1.0);
        assert!(labels.is_empty());
        assert_eq!(iter, 0);
    }

    #[test]
    fn weighted_graph_from_simple_carries_unit_weights() {
        // Triangle: every node has degree 2, total 2m = 6.
        let n = nodes(&[("a", "a.rs"), ("b", "b.rs"), ("c", "c.rs")]);
        let e = edges(&[("a", "b"), ("b", "c"), ("c", "a")]);
        let g = Graph::build(&n, &e);
        let wg = WeightedGraph::from_simple(&g);
        assert_eq!(wg.n(), 3);
        assert_eq!(wg.two_m, 6);
        for v in 0..3 {
            assert_eq!(wg.degree[v], 2);
            assert_eq!(wg.self_loop[v], 0);
            assert_eq!(wg.adj[v].len(), 2);
            for &(_, w) in &wg.adj[v] {
                assert_eq!(w, 1);
            }
        }
    }

    #[test]
    fn weighted_graph_aggregate_collapses_triangles_to_self_loops() {
        // Two disjoint triangles. Aggregating by the trivial
        // partition `[0,0,0,1,1,1]` should produce 2 super-nodes,
        // each with self_loop weight 3 (three intra-edges per K3)
        // and no inter-community adjacency.
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
        ]);
        let e = edges(&[
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let wg = WeightedGraph::from_simple(&g);
        let (agg, remap) = wg.aggregate(&[0, 0, 0, 1, 1, 1]);
        assert_eq!(agg.n(), 2);
        assert_eq!(remap.len(), 2);
        assert_eq!(agg.self_loop[0], 3);
        assert_eq!(agg.self_loop[1], 3);
        assert!(agg.adj[0].is_empty());
        assert!(agg.adj[1].is_empty());
        // Each super-node's degree counts self-loops twice.
        assert_eq!(agg.degree[0], 6);
        assert_eq!(agg.degree[1], 6);
        assert_eq!(agg.two_m, 12);
    }

    #[test]
    fn weighted_graph_aggregate_preserves_inter_community_edges() {
        // Two triangles connected by a single bridge a3-b1.
        // After aggregation by `[0,0,0,1,1,1]`, the bridge becomes
        // a symmetric inter-community edge of weight 1.
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
        ]);
        let e = edges(&[
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
            ("a3", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let wg = WeightedGraph::from_simple(&g);
        let (agg, _remap) = wg.aggregate(&[0, 0, 0, 1, 1, 1]);
        assert_eq!(agg.n(), 2);
        // Self-loops unchanged from the two-triangle case.
        assert_eq!(agg.self_loop[0], 3);
        assert_eq!(agg.self_loop[1], 3);
        // Bridge survives at weight 1, symmetric.
        let weight_0_to_1: u32 = agg.adj[0]
            .iter()
            .find(|(u, _)| *u == 1)
            .map_or(0, |(_, w)| *w);
        let weight_1_to_0: u32 = agg.adj[1]
            .iter()
            .find(|(u, _)| *u == 0)
            .map_or(0, |(_, w)| *w);
        assert_eq!(weight_0_to_1, 1);
        assert_eq!(weight_1_to_0, 1);
        // 2m bumps up by 2 (each end of the bridge contributes 1).
        assert_eq!(agg.two_m, 14);
    }

    #[test]
    fn leiden_multilevel_finds_two_dense_subgraphs_with_bridge() {
        // Two K4 cliques bridged by a single edge (3 → 4). At
        // single-level, Louvain or single-level Leiden may glue
        // the smaller side into the other community via the bridge;
        // multi-level Leiden should consistently split them.
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("a4", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
            ("b4", "b.rs"),
        ]);
        let e = edges(&[
            // K4 on a
            ("a1", "a2"),
            ("a1", "a3"),
            ("a1", "a4"),
            ("a2", "a3"),
            ("a2", "a4"),
            ("a3", "a4"),
            // K4 on b
            ("b1", "b2"),
            ("b1", "b3"),
            ("b1", "b4"),
            ("b2", "b3"),
            ("b2", "b4"),
            ("b3", "b4"),
            // bridge
            ("a4", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let (labels, _iter) = leiden_multilevel(&g, 50, 1.0);
        let left: std::collections::HashSet<_> = labels[..4].iter().copied().collect();
        let right: std::collections::HashSet<_> = labels[4..].iter().copied().collect();
        assert_eq!(left.len(), 1, "K4 on a should stay together: {labels:?}");
        assert_eq!(right.len(), 1, "K4 on b should stay together: {labels:?}");
        assert!(
            left.is_disjoint(&right),
            "the two K4s should be distinct communities despite the bridge: {labels:?}",
        );
    }

    #[test]
    fn leiden_refine_respects_phase1_boundaries() {
        // Pin the refinement-only invariant: when phase 1 has already
        // split the graph into two communities, the refinement pass
        // must never produce a sub-community whose members span both
        // phase-1 communities — even if a cross-boundary merge would
        // otherwise be modularity-positive.
        //
        // Topology: two K3 cliques {0,1,2} and {3,4,5} with a bridge
        // 2→3. Phase-1 labels are hand-set so node 2 sits with
        // {0,1} and node 3 sits with {4,5}. After refinement, no
        // refined label should appear in both halves.
        let n = nodes(&[
            ("a1", "a.rs"),
            ("a2", "a.rs"),
            ("a3", "a.rs"),
            ("b1", "b.rs"),
            ("b2", "b.rs"),
            ("b3", "b.rs"),
        ]);
        let e = edges(&[
            ("a1", "a2"),
            ("a2", "a3"),
            ("a3", "a1"),
            ("b1", "b2"),
            ("b2", "b3"),
            ("b3", "b1"),
            ("a3", "b1"),
        ]);
        let g = Graph::build(&n, &e);
        let phase1 = vec![0, 0, 0, 1, 1, 1];
        let refined = leiden_refine(&g, &phase1, 50);
        let left: std::collections::HashSet<_> = refined[..3].iter().copied().collect();
        let right: std::collections::HashSet<_> = refined[3..].iter().copied().collect();
        assert!(
            left.is_disjoint(&right),
            "refined labels must not cross phase-1 boundaries: left={left:?} right={right:?}",
        );
    }

    #[test]
    fn singletons_dropped_from_report() {
        // Isolated nodes shouldn't surface as 1-member communities —
        // they don't yet form a candidate.
        let n = nodes(&[("solo", "solo.rs")]);
        let e = edges(&[]);
        let g = Graph::build(&n, &e);
        let (labels, iter) = label_propagation(&g, 50);
        let report = build_report(
            &g,
            &labels,
            iter,
            10,
            OrderBy::MemberCount,
            2,
            5,
            Algorithm::Lpa,
        );
        assert!(report.candidates.is_empty());
        assert_eq!(report.total_communities, 1);
    }

    #[test]
    fn derive_id_uses_crate_name_under_crates_layout() {
        assert_eq!(
            derive_entity_id("crates/pricing-core/src/lib.rs", "pricing-core::compute"),
            "pricing-core"
        );
    }

    #[test]
    fn derive_id_handles_mod_rs_via_parent_dir() {
        assert_eq!(
            derive_entity_id("src/parser/mod.rs", "parser::parse"),
            "parser"
        );
    }

    #[test]
    fn derive_id_handles_top_level_file() {
        assert_eq!(derive_entity_id("src/scorer.py", "scorer.Scorer"), "scorer");
    }

    #[test]
    fn slugify_strips_punctuation_and_lowercases() {
        assert_eq!(slugify("Foo_Bar.Baz"), "foo-bar-baz");
        assert_eq!(slugify("__hello__"), "hello");
        assert_eq!(slugify("a.b.c"), "a-b-c");
    }

    fn unique_tmpdir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("doc-linter-cluster-{label}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_candidate(id: &str) -> Candidate {
        let mut kind_counts = BTreeMap::new();
        kind_counts.insert(NodeKind::Function, 12);
        Candidate {
            suggested_id: id.to_string(),
            god_node_symbol: format!("scip {id}#"),
            god_node_file: format!("crates/{id}/src/lib.rs"),
            god_node_kind: NodeKind::Function,
            member_count: 12,
            kind_counts,
            density: 0.42,
            top_members: vec!["a".to_string(), "b".to_string()],
            source_files: vec![
                format!("crates/{id}/src/lib.rs"),
                format!("crates/{id}/src/handlers.rs"),
            ],
        }
    }

    #[test]
    fn emit_stubs_writes_one_file_per_candidate() {
        let root = unique_tmpdir("emit-stubs");
        let cands = vec![fake_candidate("scorer"), fake_candidate("matcher")];
        let written = emit_candidate_stubs(
            &root,
            std::path::Path::new("docs/ontology/entities/candidates"),
            &cands,
        )
        .unwrap();
        assert_eq!(written.len(), 2);
        for path in &written {
            assert!(root.join(path).exists(), "expected file {path}");
        }
        // Issue #180: frontmatter declares the auto-participation
        // status — included in the term index, exempt from lint.
        let body =
            std::fs::read_to_string(root.join("docs/ontology/entities/candidates/scorer.md"))
                .unwrap();
        assert!(body.contains("status: auto"));
        assert!(body.contains("id: entity-scorer"));
        assert!(body.contains("value_id: scorer"));
    }

    #[test]
    fn emit_stubs_is_idempotent_on_existing_file() {
        let root = unique_tmpdir("emit-idempotent");
        let cands = vec![fake_candidate("scorer")];
        emit_candidate_stubs(
            &root,
            std::path::Path::new("docs/ontology/entities/candidates"),
            &cands,
        )
        .unwrap();
        // Mutate the file to confirm we don't overwrite it.
        let path = root.join("docs/ontology/entities/candidates/scorer.md");
        std::fs::write(&path, "MANUAL EDIT").unwrap();
        let second = emit_candidate_stubs(
            &root,
            std::path::Path::new("docs/ontology/entities/candidates"),
            &cands,
        )
        .unwrap();
        assert!(second.is_empty(), "second pass should skip existing files");
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body, "MANUAL EDIT");
    }

    #[test]
    fn stub_frontmatter_carries_confidence_and_full_source_modules() {
        let root = unique_tmpdir("emit-frontmatter");
        let cands = vec![fake_candidate("scorer")];
        emit_candidate_stubs(
            &root,
            std::path::Path::new("docs/ontology/entities/candidates"),
            &cands,
        )
        .unwrap();
        let body =
            std::fs::read_to_string(root.join("docs/ontology/entities/candidates/scorer.md"))
                .unwrap();
        // `confidence:` is the issue's frontmatter shape — not just
        // a phrase buried in the description text.
        assert!(body.contains("confidence: 0.42"), "{body}");
        // Every member file lands under source_modules, not just the
        // god-node's file.
        assert!(body.contains("  - crates/scorer/src/lib.rs"), "{body}");
        assert!(body.contains("  - crates/scorer/src/handlers.rs"), "{body}");
    }

    #[test]
    fn stub_falls_back_to_god_node_file_when_source_files_empty() {
        let root = unique_tmpdir("emit-fallback");
        let mut c = fake_candidate("matcher");
        c.source_files.clear();
        emit_candidate_stubs(
            &root,
            std::path::Path::new("docs/ontology/entities/candidates"),
            &[c],
        )
        .unwrap();
        let body =
            std::fs::read_to_string(root.join("docs/ontology/entities/candidates/matcher.md"))
                .unwrap();
        assert!(body.contains("  - crates/matcher/src/lib.rs"));
    }

    #[test]
    fn emit_stubs_dedupes_within_a_single_run() {
        let root = unique_tmpdir("emit-dedup");
        // Two candidates with the same suggested_id — only one stub
        // should land. (Real-world: two communities whose god nodes
        // share a file stem.)
        let cands = vec![fake_candidate("scorer"), fake_candidate("scorer")];
        let written = emit_candidate_stubs(
            &root,
            std::path::Path::new("docs/ontology/entities/candidates"),
            &cands,
        )
        .unwrap();
        assert_eq!(written.len(), 1);
    }

    /// Roadmap issue #14 v11: the `--output` flag overrides the
    /// default `docs/ontology/entities/candidates` destination so
    /// operators can iterate on cluster tunings without polluting
    /// the vault. The reported paths in the JSON output are
    /// repo-relative to `<root>` — `staging/candidates/<id>.md`
    /// here, not the absolute tmpdir path.
    #[test]
    fn emit_stubs_honors_custom_output_dir() {
        let root = unique_tmpdir("emit-output-dir");
        let cands = vec![fake_candidate("scorer")];
        let custom = std::path::Path::new("staging/candidates");
        let written = emit_candidate_stubs(&root, custom, &cands).unwrap();
        assert!(
            root.join("staging/candidates/scorer.md").exists(),
            "stub should land under the custom dir"
        );
        assert!(
            !root
                .join("docs/ontology/entities/candidates/scorer.md")
                .exists(),
            "default dir must NOT be touched when --output overrides it",
        );
        assert_eq!(written, vec!["staging/candidates/scorer.md".to_string()]);
    }

    /// Absolute `--output` paths are honored verbatim, not
    /// resolved against root. Lets an operator stage candidates
    /// in `/tmp/...` during exploratory work without
    /// accidentally writing into the repo.
    #[test]
    fn emit_stubs_accepts_absolute_output_dir() {
        let root = unique_tmpdir("emit-output-abs-root");
        let abs_out = unique_tmpdir("emit-output-abs-target");
        let cands = vec![fake_candidate("scorer")];
        emit_candidate_stubs(&root, &abs_out, &cands).unwrap();
        assert!(
            abs_out.join("scorer.md").exists(),
            "absolute --output path should be honored verbatim",
        );
        assert!(
            !root
                .join("docs/ontology/entities/candidates/scorer.md")
                .exists(),
            "root tree must be untouched when --output is absolute",
        );
    }

    // --- Roadmap issue #14 v11: cluster --promote --------------------

    /// The rewrite operates on raw text (not via serde_yaml) so
    /// reviewer-added comments + frontmatter ordering survive
    /// promotion. This test pins both the status flip and the
    /// title-suffix strip.
    #[test]
    fn rewrite_flips_status_and_strips_candidate_suffix() {
        let body = "---\n\
                    id: entity-parser_module\n\
                    role: ontology-entity\n\
                    title: \"Entity: parser_module (candidate)\"\n\
                    summary: cluster-derived candidate; review before promoting\n\
                    status: candidate\n\
                    confidence: 0.42\n\
                    updated: 2026-05-24\n\
                    display: parser_module\n\
                    ---\n\
                    \n\
                    # Candidate entity\n";
        let out = rewrite_candidate_to_stable(body);
        assert!(out.contains("status: stable\n"), "status flipped");
        assert!(!out.contains("status: candidate"), "old status gone");
        assert!(
            out.contains("title: \"Entity: parser_module\"\n"),
            "title suffix stripped (got: {})",
            out.lines().find(|l| l.starts_with("title:")).unwrap_or("?")
        );
        // Reviewer-added body markdown must round-trip untouched.
        assert!(out.contains("# Candidate entity\n"));
    }

    /// A title without the "(candidate)" suffix must pass through
    /// untouched — promoting a candidate the reviewer already
    /// hand-edited shouldn't introduce drift.
    #[test]
    fn rewrite_leaves_clean_title_alone() {
        let body = "---\n\
                    title: \"Entity: scorer\"\n\
                    status: candidate\n\
                    ---\n";
        let out = rewrite_candidate_to_stable(body);
        assert!(out.contains("title: \"Entity: scorer\"\n"));
        assert!(out.contains("status: stable\n"));
    }

    /// End-to-end: write a candidate stub, promote it, observe
    /// the file moved to `docs/ontology/entities/<id>.md` and the
    /// JSON report carries the right paths.
    #[test]
    fn promote_moves_candidate_and_rewrites_status() {
        let root = unique_tmpdir("promote-happy");
        let output = root.join("docs/ontology/entities/candidates");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join("parser_module.md"),
            "---\n\
             id: entity-parser_module\n\
             role: ontology-entity\n\
             title: \"Entity: parser_module (candidate)\"\n\
             status: candidate\n\
             display: parser_module\n\
             ---\n\
             \n\
             # Body\n",
        )
        .unwrap();

        let report = promote_candidate(&root, &output, "parser_module").unwrap();
        assert_eq!(report.action, "promoted");
        assert_eq!(report.id, "parser_module");
        assert_eq!(
            report.from,
            "docs/ontology/entities/candidates/parser_module.md"
        );
        assert_eq!(report.to, "docs/ontology/entities/parser_module.md");

        let dest = root.join("docs/ontology/entities/parser_module.md");
        let src = output.join("parser_module.md");
        assert!(dest.exists(), "promoted entity should exist at destination");
        assert!(!src.exists(), "source candidate should be removed");
        let dest_body = std::fs::read_to_string(&dest).unwrap();
        assert!(dest_body.contains("status: stable\n"));
        assert!(dest_body.contains("title: \"Entity: parser_module\"\n"));
        assert!(dest_body.contains("# Body\n"), "body preserved");
    }

    /// Promotion must refuse to overwrite an existing entity at
    /// the destination — that would be silent data loss. The
    /// reviewer is expected to delete the existing entity first
    /// if they really mean to replace it.
    #[test]
    fn promote_refuses_to_overwrite_existing_entity() {
        let root = unique_tmpdir("promote-conflict");
        let output = root.join("docs/ontology/entities/candidates");
        let entities = root.join("docs/ontology/entities");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir_all(&entities).unwrap();
        std::fs::write(output.join("scorer.md"), "---\nstatus: candidate\n---\n").unwrap();
        std::fs::write(entities.join("scorer.md"), "existing entity").unwrap();

        let err = promote_candidate(&root, &output, "scorer").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("destination already exists"),
            "should refuse overwrite, got: {msg}",
        );
        // Source must be untouched — promotion is all-or-nothing.
        assert!(
            output.join("scorer.md").exists(),
            "source preserved on conflict"
        );
        assert_eq!(
            std::fs::read_to_string(entities.join("scorer.md")).unwrap(),
            "existing entity",
            "existing destination preserved on conflict",
        );
    }

    /// Promoting a missing id surfaces the directory we looked in
    /// so the reviewer can fix the typo without guessing.
    #[test]
    fn promote_missing_candidate_surfaces_search_path() {
        let root = unique_tmpdir("promote-missing");
        let output = root.join("docs/ontology/entities/candidates");
        std::fs::create_dir_all(&output).unwrap();
        let err = promote_candidate(&root, &output, "never-existed").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("never-existed.md"),
            "missing path in error: {msg}"
        );
        assert!(
            msg.contains("candidates"),
            "search dir hinted in error: {msg}"
        );
    }

    /// Empty id is a usage error, not a file-system probe.
    #[test]
    fn promote_rejects_empty_id() {
        let root = unique_tmpdir("promote-empty");
        let output = root.join("docs/ontology/entities/candidates");
        let err = promote_candidate(&root, &output, "").unwrap_err();
        assert!(format!("{err:#}").contains("requires a candidate id"));
    }
}
