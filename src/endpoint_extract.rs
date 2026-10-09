//! Phase 2 of roadmap-43: Endpoint extraction.
//!
//! Walks every `.rs` file under the workspace and pulls out three kinds
//! of "endpoint" — user-facing surfaces that should each map to at
//! least one ontology entity:
//!
//!   - **2a — axum routes**: `Router::new().route("/path", get(fn))`,
//!     `.nest("/v1", subrouter())`, etc. Path strings (with parameters
//!     like `:id`) are kept verbatim.
//!   - **2b — clap subcommands**: every variant of an enum that derives
//!     `Subcommand`. Variant names are kebab-cased
//!     (`ScipIndex` → `scip-index`).
//!   - **2c — MCP tools**: `McpTool { name: "tool", ... }` struct
//!     literals or `register_tool("tool", ...)` calls in any crate
//!     under `*mcp*` (best-effort — `platform-mcp-sdk`'s shape is
//!     still in flux per the roadmap).
//!
//! The extractor is best-effort and resilient — a malformed `.rs` file
//! emits a warning and is skipped, never panics. Cross-crate router
//! composition is out of scope for v1; `.nest("/v1", outlets_router())`
//! only path-prefixes the inner routes when the inner router is defined
//! in the same file.
//!
//! ## Handler symbol resolution
//!
//! Each extracted endpoint records the LOCAL handler-function name as
//! it appears at the call site (`get(invite_distributor_handler)` →
//! `invite_distributor_handler`). The full SCIP symbol is resolved
//! later in [`crate::store::ingest_endpoints`] against the live
//! `FunctionFact` set — best-effort: a same-file match wins, falling
//! back to crate-wide search, dropping the symbol field on ambiguity.

use crate::config::LintConfig;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser, TreeCursor};
use walkdir::WalkDir;

/// One extracted endpoint, ready for [`crate::store::ingest_endpoints`].
/// The unit of supply for the [[entity-doc-graph]] Endpoint node table —
/// each EndpointFact becomes one node connected to its handler `Function`
/// and (via doc-comment prose) to ontology entities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndpointFact {
    /// `<kind>:<method>:<path>` — globally unique. axum endpoints look
    /// like `axum:GET:/v1/outlets/:id`; clap like `clap:cli:scip-index`;
    /// MCP like `mcp:mcp:read_published_state`.
    pub id: String,
    pub kind: EndpointKind,
    /// HTTP method for axum (uppercase: `GET`, `POST`, ...), `cli` for
    /// clap, `mcp` for MCP tools.
    pub method: String,
    /// Route path (axum), kebab-cased subcommand (clap), or tool name (mcp).
    pub path: String,
    /// LOCAL handler-function name from the call site, when known. The
    /// store layer matches this against SCIP symbols. `None` for
    /// axum endpoints whose handler isn't a bare ident (e.g. closures);
    /// `None` for MCP endpoints when no handler-type is detectable.
    pub handler_local: Option<String>,
    /// Repo-relative source file.
    pub source_file: String,
    /// 1-based line where the registration appears.
    pub source_line: u32,
}

/// Coarse classifier for endpoint origin used in the [[entity-doc-graph]]
/// Endpoint node `kind` column (axum HTTP route / clap subcommand / MCP tool).
///
/// Serializes as the bare lowercase token (`"axum"` / `"clap"` / `"mcp"`)
/// — matches both the on-disk the store Endpoint.kind column and the historic
/// JSON shape of [`EndpointRow`] / [`DarkEndpointRow`] /
/// [`crate::coverage::EndpointClassSection`], so the typed-enum migration
/// (#51) is a pure code-side change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum EndpointKind {
    Axum,
    Clap,
    Mcp,
    /// Roadmap issue #19 (v0.3.0): FastAPI routes — `@app.get("/path")`,
    /// `@router.post("/path")`, etc. on Python `def handler(...)`
    /// definitions.
    Fastapi,
    /// Roadmap issue #19 (v0.3.0): Flask routes —
    /// `@app.route("/path")` and `@blueprint.route("/path",
    /// methods=[...])`.
    Flask,
    /// Roadmap issue #19 (v0.3.0): Node/Express routes —
    /// `app.get("/path", handler)`, `router.post("/path", mw, handler)`,
    /// etc. on `.js` / `.ts` files.
    Express,
    /// JAX-RS resources — class-level `@Path` joined with each method's
    /// `@GET` / `@POST` / … and optional method-level `@Path`.
    Jaxrs,
}

impl EndpointKind {
    /// String form of the endpoint kind (`"axum"`/`"clap"`/`"mcp"`/`"fastapi"`)
    /// used by the [[entity-doc-graph]] endpoint ingest when emitting
    /// graph rows.
    pub fn as_str(self) -> &'static str {
        match self {
            EndpointKind::Axum => "axum",
            EndpointKind::Clap => "clap",
            EndpointKind::Mcp => "mcp",
            EndpointKind::Fastapi => "fastapi",
            EndpointKind::Flask => "flask",
            EndpointKind::Express => "express",
            EndpointKind::Jaxrs => "jaxrs",
        }
    }

    /// Parses the lowercase token written into the `Endpoint.kind`
    /// column back into the typed enum. Returns `None` on an unknown
    /// value — callers at the store boundary decide whether to default
    /// to [`EndpointKind::Axum`] or surface the bad row.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "axum" => Some(Self::Axum),
            "clap" => Some(Self::Clap),
            "mcp" => Some(Self::Mcp),
            "fastapi" => Some(Self::Fastapi),
            "flask" => Some(Self::Flask),
            "express" => Some(Self::Express),
            "jaxrs" => Some(Self::Jaxrs),
            _ => None,
        }
    }
}

impl std::fmt::Display for EndpointKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod endpoint_kind_proptests {
    use super::EndpointKind;
    use proptest::prelude::*;

    /// Strategy over every [`EndpointKind`] variant. See the matching
    /// `arb_edge_kind` rationale in `graph::edge_kind_proptests`.
    fn arb_endpoint_kind() -> impl Strategy<Value = EndpointKind> {
        prop_oneof![
            Just(EndpointKind::Axum),
            Just(EndpointKind::Clap),
            Just(EndpointKind::Mcp),
        ]
    }

    proptest! {
        /// `EndpointKind::parse(k.as_str()) == Some(k)` for every
        /// variant. Round-trip invariant on the lowercase projection
        /// used by the `Endpoint.kind` column.
        #[test]
        fn endpoint_kind_as_str_parse_roundtrip(kind in arb_endpoint_kind()) {
            let s = kind.as_str();
            prop_assert_eq!(EndpointKind::parse(s), Some(kind));
        }

        /// Display agrees with `as_str` — useful because the migrator
        /// renders `EndpointKind` via `{}` (not `as_str()`) in its
        /// manual report, and the graph schema relies on the lowercase
        /// projection.
        #[test]
        fn endpoint_kind_display_matches_as_str(kind in arb_endpoint_kind()) {
            prop_assert_eq!(format!("{kind}"), kind.as_str());
        }
    }
}

/// Roadmap-49 phase 1d: build [`EndpointFact`]s from `@endpoint`
/// doc-comment markers across the workspace. Walks every `.rs` file
/// under `root` (using the same include/exclude globs the regex
/// extractor uses), extracts each file's doc-comments via
/// [`crate::code_comments::extract_doc_comments`], scans those
/// comments for `@endpoint <METHOD> <path>` lines, and lifts every
/// match into an [`EndpointFact`].
///
/// Marker → fact mapping:
///   - `kind` = `Clap` when method is `CLI`, `Mcp` when `MCP`,
///     otherwise `Axum` (covers the HTTP verbs and any forward-
///     compat marker like `WS`).
///   - `handler_local` = the comment's `attached_to` (already
///     resolved by the tree-sitter pass to the function/struct
///     immediately following the doc-comment).
///   - `id` mirrors the syntactic extractor's shape:
///     `axum:GET:/v1/outlets/:id` / `clap:cli:scip-index` /
///     `mcp:mcp:read_state`.
///
/// Returns the merged list across all files, sorted by `id` and
/// deduped on the `(id, source_file, source_line)` triple — multiple
/// files declaring the same `(method, path)` would be a real bug,
/// but markers within one file should never collide.
pub fn extract_endpoint_markers_workspace(
    root: &Path,
    config: &LintConfig,
) -> Result<Vec<EndpointFact>> {
    use crate::code_comments::{extract_doc_comments, extract_endpoint_markers, EndpointMarker};

    let rs_files = discover_rs_files(root, config)?;
    let mut out: Vec<EndpointFact> = Vec::new();

    for file in &rs_files {
        let rel = file.strip_prefix(root).unwrap_or(file);
        let rel_str = rel.display().to_string().replace('\\', "/");

        let extraction = match extract_doc_comments(file) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("doc-linter: endpoint-marker-extract: skipping {rel_str} ({e})");
                continue;
            }
        };
        let markers: Vec<EndpointMarker> = extract_endpoint_markers(&extraction, &rel_str);
        for m in markers {
            out.push(marker_to_fact(&m));
        }
    }

    out.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.source_file.cmp(&b.source_file))
            .then_with(|| a.source_line.cmp(&b.source_line))
    });
    Ok(out)
}

/// Roadmap-49 phase 1f: merge two `EndpointFact` lists (markers
/// first, regex/syntactic second) and dedupe on the
/// `(method, path, handler_local)` triple in the [[entity-doc-graph]]
/// endpoint pipeline. The marker-derived fact wins on collision — the
/// marker is the source-of-truth declaration; the syntactic extractor is
/// the fallback.
///
/// Returns `(merged_list, marker_count, regex_count, kept_from_marker_dedupe)`:
///   - `merged_list`: the deduped facts, sorted by id.
///   - `marker_count`: input markers received.
///   - `regex_count`: input regex/syntactic facts received.
///   - `kept_from_marker_dedupe`: number of regex facts dropped
///     because a marker declared the same (method, path, handler).
pub fn merge_endpoint_facts(
    markers: Vec<EndpointFact>,
    regex: Vec<EndpointFact>,
) -> (Vec<EndpointFact>, usize, usize, usize) {
    use std::collections::HashSet;
    let marker_count = markers.len();
    let regex_count = regex.len();
    // Build a key set from the marker-derived facts so the regex
    // pass can be filtered against it.
    let mut keys: HashSet<(String, String, Option<String>)> = HashSet::new();
    let mut out: Vec<EndpointFact> = Vec::with_capacity(marker_count + regex_count);
    for f in markers {
        keys.insert((f.method.clone(), f.path.clone(), f.handler_local.clone()));
        out.push(f);
    }
    let mut dropped = 0usize;
    for f in regex {
        let key = (f.method.clone(), f.path.clone(), f.handler_local.clone());
        if keys.contains(&key) {
            dropped += 1;
            continue;
        }
        // Also drop on (method, path) alone when the marker side
        // already covers the route — handler_local can legitimately
        // differ between the marker (free, from `attached_to`) and
        // the regex extractor (which guesses from the registration
        // site). The marker is the source of truth; suppress the
        // regex twin.
        let pair_match = out.iter().any(|m| m.method == f.method && m.path == f.path);
        if pair_match {
            dropped += 1;
            continue;
        }
        keys.insert(key);
        out.push(f);
    }
    out.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.source_file.cmp(&b.source_file))
            .then_with(|| a.source_line.cmp(&b.source_line))
    });
    out.dedup_by(|a, b| a.id == b.id);
    (out, marker_count, regex_count, dropped)
}

/// Lift a single [`crate::code_comments::EndpointMarker`] into the
/// [`EndpointFact`] shape the graph ingest expects. Pure mapping —
/// no IO.
fn marker_to_fact(m: &crate::code_comments::EndpointMarker) -> EndpointFact {
    let (kind, kind_str, method) = match m.method.as_str() {
        "CLI" => (EndpointKind::Clap, "clap", "cli".to_string()),
        "MCP" => (EndpointKind::Mcp, "mcp", "mcp".to_string()),
        other => (EndpointKind::Axum, "axum", other.to_string()),
    };
    let id = format!("{kind_str}:{method}:{path}", path = m.path);
    EndpointFact {
        id,
        kind,
        method,
        path: m.path.clone(),
        handler_local: m.handler_symbol.clone(),
        source_file: m.source_file.clone(),
        source_line: m.source_line,
    }
}

/// Walk every `.rs` file under `root` and return every endpoint we can
/// extract for ingest into the [[entity-doc-graph]] endpoint table.
/// Excludes are taken from `LintConfig.code_comment_excludes`
/// (defaults: `target/**`, `**/tests/**`, `**/build.rs`).
///
/// Bad / unparseable Rust source is logged to stderr as a single
/// warning per file and skipped — never propagated as an error.
pub fn extract_endpoints(root: &Path, config: &LintConfig) -> Result<Vec<EndpointFact>> {
    let rs_files = discover_rs_files(root, config)?;
    let mut out: Vec<EndpointFact> = Vec::new();
    let clap_crates: Vec<String> = if config.coverage.clap_crates.is_empty() {
        vec!["doc-linter".to_string()]
    } else {
        config.coverage.clap_crates.clone()
    };

    for file in &rs_files {
        let rel = file.strip_prefix(root).unwrap_or(file);
        let rel_str = rel.display().to_string().replace('\\', "/");

        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };

        match extract_from_str(&content, &rel_str, &clap_crates) {
            Ok(mut found) => out.append(&mut found),
            Err(e) => {
                eprintln!("doc-linter: endpoint-extract: skipping {rel_str} ({e})");
            }
        }
    }

    // Roadmap issue #19 (v0.3.0): walk .py files for FastAPI
    // decorators. Regex-driven for v1 (`@<obj>.<method>("/path")`);
    // tree-sitter-python promotion + Flask + Express extractors
    // defer to follow-ups. Errors from individual files are
    // logged but don't fail the overall extract.
    for file in discover_py_files(root, config)? {
        let rel = file.strip_prefix(root).unwrap_or(&file);
        let rel_str = rel.display().to_string().replace('\\', "/");
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        extract_fastapi(&content, &rel_str, &mut out);
        extract_flask(&content, &rel_str, &mut out);
    }

    // Roadmap issue #19 (v0.3.0): walk .js / .ts files for Express
    // route registrations. Regex-driven for v1 — `app.get(...)`,
    // `router.post(...)`, etc. Express middleware mounting
    // (`app.use("/api", router)`) defers to a follow-up.
    for file in discover_js_files(root, config)? {
        let rel = file.strip_prefix(root).unwrap_or(&file);
        let rel_str = rel.display().to_string().replace('\\', "/");
        let Ok(content) = std::fs::read_to_string(&file) else {
            continue;
        };
        extract_express(&content, &rel_str, &mut out);
    }

    let java: Vec<(String, String)> = discover_java_files(root, config)?
        .into_iter()
        .filter_map(|file| {
            let content = std::fs::read_to_string(&file).ok()?;
            let rel = file.strip_prefix(root).unwrap_or(&file);
            Some((rel.display().to_string().replace('\\', "/"), content))
        })
        .collect();
    let consts = java_string_constants(java.iter().map(|(_, c)| c.as_str()));
    for (rel_str, content) in &java {
        extract_jaxrs(content, rel_str, &consts, &mut out);
    }
    out.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then_with(|| a.source_line.cmp(&b.source_line))
    });
    out.dedup_by(|a, b| a.id == b.id);
    Ok(out)
}

/// Internal entry-point used by tests too. Parses one in-memory Rust
/// source string and returns every endpoint discovered for the
/// [[entity-doc-graph]] endpoint pipeline. The `clap_crates` list governs
/// whether we look for `Subcommand` enums in this file — the file's crate
/// is determined from `rel_path` (its `crates/<name>/...` prefix).
pub fn extract_from_str(
    content: &str,
    rel_path: &str,
    clap_crates: &[String],
) -> Result<Vec<EndpointFact>> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_rust::language())
        .context("load tree-sitter-rust grammar")?;
    let tree = parser
        .parse(content, None)
        .context("tree-sitter parse returned None")?;
    let bytes = content.as_bytes();
    let mut out: Vec<EndpointFact> = Vec::new();

    // 2a: axum routes. We collect every `.route(path, ...)` and
    // `.nest(path, sub_call)` first; then within the same file, we
    // try to resolve nested router calls back to a function whose
    // body itself contains `.route(...)` calls and prepend the nest
    // prefix.
    extract_axum(tree.root_node(), bytes, rel_path, &mut out);

    // 2b: clap Subcommand variants. Only run when the file's crate
    // appears in `clap_crates`.
    let crate_in_scope = file_crate_in_clap_list(rel_path, clap_crates);
    if crate_in_scope {
        extract_clap(tree.root_node(), bytes, rel_path, &mut out);
    }

    // 2c: MCP tools. Only run when the file lives in or under an MCP
    // crate — best-effort gating that matches the roadmap deferral
    // for non-MCP files.
    if rel_path.contains("mcp") {
        extract_mcp(tree.root_node(), bytes, rel_path, &mut out);
        // Tree-sitter-rust treats macro contents (`vec![...]`,
        // `Registry::from_tools(vec![...])`, etc.) as opaque
        // `token_tree` nodes — no `struct_expression` inside, so
        // `extract_mcp` misses every `McpTool { ... }` written inside
        // a macro invocation. That's the dominant pattern in this
        // codebase (jsonrpc.rs, registry.rs, sample tool registries).
        // Run a regex fallback over the raw source to catch them; we
        // dedup against names already extracted by tree-sitter so
        // double-counting doesn't happen.
        extract_mcp_regex_fallback(content, rel_path, &mut out);
    }

    Ok(out)
}

/// Scans raw source for `McpTool { name: "..." }` patterns that
/// `extract_mcp` missed because they live inside a macro invocation
/// (tree-sitter-rust parses macro contents as opaque token_trees).
/// Dedups against tool names already extracted by tree-sitter so the
/// fallback never produces double-counted endpoints.
fn extract_mcp_regex_fallback(content: &str, rel_path: &str, out: &mut Vec<EndpointFact>) {
    use std::sync::OnceLock;
    static MCP_LITERAL: OnceLock<regex::Regex> = OnceLock::new();
    // Matches: any `McpTool { ... name: "<name>" ... }` literal,
    // including paths like `platform_mcp_sdk::McpTool`. The captured
    // name field must come within ~12 lines of the opening brace; in
    // practice McpTool literals are short (~5 fields) so this is
    // more than enough.
    let re = MCP_LITERAL.get_or_init(|| {
        #[allow(clippy::expect_used, reason = "literal regex; compile is infallible at runtime")]
        regex::Regex::new(
            r#"(?m)(?:^|\W)(?:[A-Za-z_][A-Za-z0-9_]*::)*McpTool\s*\{(?:[^{}]{0,2000}?)\bname\s*:\s*"([^"]+)""#,
        )
        .expect("MCP_LITERAL regex compiles")
    });
    let already: std::collections::HashSet<String> = out
        .iter()
        .filter(|e| e.kind == EndpointKind::Mcp)
        .map(|e| e.path.clone())
        .collect();
    for cap in re.captures_iter(content) {
        let Some(name_match) = cap.get(1) else {
            continue;
        };
        let name = name_match.as_str().to_string();
        if already.contains(&name) {
            continue;
        }
        // Compute 1-based line number for the `name:` match.
        let prefix_bytes = name_match.start();
        let line = content.as_bytes()[..prefix_bytes]
            .iter()
            .filter(|&&b| b == b'\n')
            .count() as u32
            + 1;
        out.push(make_mcp_endpoint(&name, None, rel_path, line));
    }
}

// ---------- 2a: axum ----------

/// True when the (line-wrapped) call chain `call.method(...)` has the
/// matching method name. Used to spot `.route("...", get(handler))`.
fn call_method_name<'a>(call: Node<'a>, bytes: &'a [u8]) -> Option<&'a str> {
    // call_expression `function:` is a `field_expression` for method calls;
    // its `field` identifier is the method name.
    let func = call.child_by_field_name("function")?;
    if func.kind() != "field_expression" {
        return None;
    }
    let field = func.child_by_field_name("field")?;
    node_text(field, bytes)
}

/// Walks an axum route-builder expression tree and emits one
/// `EndpointFact` per `Router::route(path, handler)` call for the
/// [[entity-doc-graph]] endpoint ingest.
fn extract_axum(root: Node<'_>, bytes: &[u8], rel_path: &str, out: &mut Vec<EndpointFact>) {
    // First pass: collect every `.route()` and `.nest()` call along
    // with the function-item it lives in. The function-item is used in
    // pass 2 to resolve `.nest("/v1", inner_router())` cross-references
    // within the same file.
    let mut routes_raw: Vec<RawRouteCall> = Vec::new();
    // Map from "router-ish function name" → routes that function
    // builds, so we can resolve `.nest("/v1", outlets_router())` even
    // when `outlets_router` is defined later in the file.
    let mut router_fns: HashMap<String, Vec<RawRouteCall>> = HashMap::new();

    walk(root, &mut |node| {
        if node.kind() != "call_expression" {
            return;
        }
        let Some(method) = call_method_name(node, bytes) else {
            return;
        };
        if method != "route" && method != "nest" {
            return;
        }
        let args = match node.child_by_field_name("arguments") {
            Some(a) => a,
            None => return,
        };
        // First arg = path string literal.
        let mut arg_iter = args
            .named_children(&mut args.walk())
            .filter(|n| n.kind() != "line_comment" && n.kind() != "block_comment")
            .collect::<Vec<_>>();
        if arg_iter.is_empty() {
            return;
        }
        let path_arg = arg_iter.remove(0);
        let Some(path_str) = string_literal_value(path_arg, bytes) else {
            return;
        };
        // Second arg = either a method-router call (`get(handler)`) or
        // some other expression (`outlets_router()`).
        let second_arg = arg_iter.first().copied();
        let line = node.start_position().row as u32 + 1;
        let raw = RawRouteCall {
            kind: if method == "nest" {
                RawRouteKind::Nest
            } else {
                RawRouteKind::Route
            },
            path: path_str,
            second_arg_kind: second_arg.map(|n| classify_axum_second_arg(n, bytes)),
            line,
        };
        // Find the enclosing function-item's name (if any) so router
        // helper-fns can be indexed.
        let enclosing = enclosing_fn_name(node, bytes);
        if let Some(name) = enclosing {
            router_fns.entry(name).or_default().push(raw.clone());
        }
        routes_raw.push(raw);
    });

    // Second pass: emit endpoints. `.route()` becomes one or more
    // EndpointFacts (one per HTTP method on the chained method-router).
    // `.nest()` whose second-arg is `<ident>()` calling a known router
    // helper expands into prefixed routes; otherwise it's recorded as
    // a single opaque "nest" endpoint with method `NEST` so it still
    // appears in coverage queries.
    for raw in &routes_raw {
        match raw.kind {
            RawRouteKind::Route => match &raw.second_arg_kind {
                Some(SecondArgKind::MethodRouter { methods }) => {
                    for (m, handler) in methods {
                        out.push(make_axum_endpoint(
                            &raw.path,
                            m,
                            handler.clone(),
                            rel_path,
                            raw.line,
                        ));
                    }
                }
                _ => {
                    out.push(make_axum_endpoint(
                        &raw.path, "ANY", None, rel_path, raw.line,
                    ));
                }
            },
            RawRouteKind::Nest => {
                // Try to resolve the nested router via known router fns.
                let inner_name = match &raw.second_arg_kind {
                    Some(SecondArgKind::FnCall { name }) => Some(name.clone()),
                    _ => None,
                };
                if let Some(name) = inner_name.as_ref() {
                    if let Some(inner_routes) = router_fns.get(name) {
                        for inner in inner_routes {
                            // Prefix the inner route's path with the
                            // outer nest prefix. Don't double-emit if
                            // the inner is itself a nest (rare, but we
                            // recurse one level which covers the
                            // common cases).
                            let combined = join_axum_path(&raw.path, &inner.path);
                            match (&inner.kind, &inner.second_arg_kind) {
                                (
                                    RawRouteKind::Route,
                                    Some(SecondArgKind::MethodRouter { methods }),
                                ) => {
                                    for (m, handler) in methods {
                                        out.push(make_axum_endpoint(
                                            &combined,
                                            m,
                                            handler.clone(),
                                            rel_path,
                                            raw.line,
                                        ));
                                    }
                                }
                                _ => {
                                    out.push(make_axum_endpoint(
                                        &combined, "ANY", None, rel_path, raw.line,
                                    ));
                                }
                            }
                        }
                        continue;
                    }
                }
                // Unresolved nest: record the prefix as a dark endpoint
                // so coverage notices it.
                out.push(make_axum_endpoint(
                    &raw.path, "NEST", None, rel_path, raw.line,
                ));
            }
        }
    }
}

/// Combine `/v1` + `/outlets/:id` → `/v1/outlets/:id`. Trims a
/// trailing slash on the prefix and a leading slash on the suffix
/// before joining; drops bare empty segments. Mirrors what axum's
/// router does at runtime.
fn join_axum_path(prefix: &str, suffix: &str) -> String {
    let p = prefix.trim_end_matches('/');
    let s = suffix.trim_start_matches('/');
    if p.is_empty() {
        format!("/{s}")
    } else if s.is_empty() {
        p.to_string()
    } else {
        format!("{p}/{s}")
    }
}

#[derive(Debug, Clone)]
struct RawRouteCall {
    kind: RawRouteKind,
    path: String,
    second_arg_kind: Option<SecondArgKind>,
    line: u32,
}

#[derive(Debug, Clone)]
enum RawRouteKind {
    Route,
    Nest,
}

/// Classification of the second argument of an axum `.route(path, ...)`
/// call — a method-router chain, a bare fn call (sub-router), or anything
/// else — used by the [[entity-doc-graph]] axum extractor to recover
/// handler identifiers.
#[derive(Debug, Clone)]
enum SecondArgKind {
    /// The second arg is a method-router builder like `get(h)`,
    /// `post(h).get(other)`. Each `(method, handler-ident)` pair is
    /// captured. Method names are uppercased.
    MethodRouter {
        methods: Vec<(String, Option<String>)>,
    },
    /// The second arg is a bare function call `<ident>(...)` — used by
    /// `.nest()` to compose subrouters.
    FnCall { name: String },
    /// Anything else (closure, complex expression, ...) — the
    /// extractor records the route but with no handler info.
    #[allow(dead_code)]
    // exhaustive-match marker; the variant is constructed but never inspected
    Other,
}

/// Classifies the second argument of `.route(path, ...)` so the
/// [[entity-doc-graph]] axum extractor can decide whether to expand a
/// method-router chain or record a sub-router nest.
fn classify_axum_second_arg(node: Node<'_>, bytes: &[u8]) -> SecondArgKind {
    // Method-router calls are chained method calls on top of `get(h)`,
    // `post(h)`, etc. Walk the chain and collect every `<method>(<ident>)`
    // segment.
    let mut methods: Vec<(String, Option<String>)> = Vec::new();
    let mut cursor: Node = node;
    loop {
        if cursor.kind() != "call_expression" {
            break;
        }
        // Function: either an `identifier` (top-level `get(handler)`)
        // or a `field_expression` (chained `.get(handler)`).
        let func = match cursor.child_by_field_name("function") {
            Some(f) => f,
            None => break,
        };
        let args = match cursor.child_by_field_name("arguments") {
            Some(a) => a,
            None => break,
        };
        let handler_ident = first_arg_ident(args, bytes);
        if func.kind() == "identifier" {
            // Top-level `get(handler)`-style call.
            if let Some(name) = node_text(func, bytes) {
                let upper = name.to_ascii_uppercase();
                if is_axum_method(name) {
                    methods.push((upper, handler_ident));
                } else {
                    // Bare function call `outlets_router()` — that's a
                    // nest target, not a method-router.
                    return SecondArgKind::FnCall {
                        name: name.to_string(),
                    };
                }
            }
            break;
        } else if func.kind() == "field_expression" {
            // `<receiver>.<method>(args)`. Capture this method, then
            // recurse on the receiver.
            let field = match func.child_by_field_name("field") {
                Some(f) => f,
                None => break,
            };
            let receiver = match func.child_by_field_name("value") {
                Some(v) => v,
                None => break,
            };
            if let Some(name) = node_text(field, bytes) {
                if is_axum_method(name) {
                    methods.push((name.to_ascii_uppercase(), handler_ident));
                }
            }
            cursor = receiver;
        } else {
            break;
        }
    }
    methods.reverse();
    if methods.is_empty() {
        SecondArgKind::Other
    } else {
        SecondArgKind::MethodRouter { methods }
    }
}

/// True when `name` is one of the axum method-router builders
/// (`get`/`post`/...) used by the [[entity-doc-graph]] extractor to walk
/// chained route declarations.
fn is_axum_method(name: &str) -> bool {
    matches!(
        name,
        "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace"
    )
}

/// First positional argument of a `(...)` argument list, when it's a
/// bare identifier. Returns `None` for closures / complex expressions.
/// Used by the [[entity-doc-graph]] endpoint extractor to recover handler
/// names from axum/clap registration calls.
fn first_arg_ident(args: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut walker = args.walk();
    for child in args.named_children(&mut walker) {
        if child.kind() == "line_comment" || child.kind() == "block_comment" {
            continue;
        }
        if child.kind() == "identifier" {
            return node_text(child, bytes).map(str::to_string);
        }
        // Path expression like `module::handler` — return the trailing
        // identifier so it can be matched against SCIP symbols.
        if child.kind() == "scoped_identifier" {
            if let Some(name) = child.child_by_field_name("name") {
                return node_text(name, bytes).map(str::to_string);
            }
        }
        return None;
    }
    None
}

/// Constructs an `EndpointFact` for one axum `(method, path, handler)`
/// triple discovered by the [[entity-doc-graph]] route walker.
fn make_axum_endpoint(
    path: &str,
    method: &str,
    handler: Option<String>,
    rel_path: &str,
    line: u32,
) -> EndpointFact {
    let id = format!("axum:{method}:{path}");
    EndpointFact {
        id,
        kind: EndpointKind::Axum,
        method: method.to_string(),
        path: path.to_string(),
        handler_local: handler,
        source_file: rel_path.to_string(),
        source_line: line,
    }
}

// ---------- 2b: clap ----------

/// Walks a clap `#[derive(Subcommand)]` enum and emits one
/// `EndpointFact` per variant for the [[entity-doc-graph]] CLI-endpoint
/// ingest.
fn extract_clap(root: Node<'_>, bytes: &[u8], rel_path: &str, out: &mut Vec<EndpointFact>) {
    walk(root, &mut |node| {
        if node.kind() != "enum_item" {
            return;
        }
        if !enum_has_subcommand_derive(node, bytes) {
            return;
        }
        let Some(body) = node.child_by_field_name("body") else {
            return;
        };
        // enum_variant_list with enum_variant children. Each variant
        // becomes one endpoint.
        let mut walker = body.walk();
        for variant in body.named_children(&mut walker) {
            if variant.kind() != "enum_variant" {
                continue;
            }
            let Some(name_node) = variant.child_by_field_name("name") else {
                continue;
            };
            let Some(name) = node_text(name_node, bytes) else {
                continue;
            };
            let kebab = pascal_to_kebab(name);
            let line = variant.start_position().row as u32 + 1;
            // Map the variant name to a probable cmd_<snake_case> handler.
            let handler_guess = format!("cmd_{}", kebab.replace('-', "_"));
            out.push(EndpointFact {
                id: format!("clap:cli:{kebab}"),
                kind: EndpointKind::Clap,
                method: "cli".to_string(),
                path: kebab,
                handler_local: Some(handler_guess),
                source_file: rel_path.to_string(),
                source_line: line,
            });
        }
    });
}

/// True when the enum carries a `#[derive(Subcommand)]` attribute — the
/// gate the [[entity-doc-graph]] clap extractor uses to decide whether
/// to mint endpoints for the variants.
fn enum_has_subcommand_derive(node: Node<'_>, bytes: &[u8]) -> bool {
    // Attribute_items are siblings before `enum_item` in tree-sitter-rust.
    // We walk up to the parent and look back for attributes immediately
    // preceding this enum.
    let Some(parent) = node.parent() else {
        return false;
    };
    let mut walker = parent.walk();
    let mut prev_was_attr = false;
    let mut attrs_text: Vec<String> = Vec::new();
    for child in parent.named_children(&mut walker) {
        if child.id() == node.id() {
            break;
        }
        if child.kind() == "attribute_item" || child.kind() == "inner_attribute_item" {
            prev_was_attr = true;
            if let Some(t) = node_text(child, bytes) {
                attrs_text.push(t.to_string());
            }
        } else {
            prev_was_attr = false;
            attrs_text.clear();
        }
    }
    let _ = prev_was_attr;
    attrs_text.iter().any(|a| a.contains("Subcommand"))
}

/// Converts `PascalCase` clap variant names to `kebab-case` so the
/// [[entity-doc-graph]] CLI-endpoint paths match what users actually
/// type on the command line.
fn pascal_to_kebab(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

// ---------- 2c: MCP ----------

fn extract_mcp(root: Node<'_>, bytes: &[u8], rel_path: &str, out: &mut Vec<EndpointFact>) {
    walk(root, &mut |node| {
        // Pattern A: `register_tool("name", ...)` — direct calls.
        if node.kind() == "call_expression" {
            if let Some(name) = call_method_name(node, bytes) {
                if name == "register_tool" {
                    if let Some(args) = node.child_by_field_name("arguments") {
                        let mut walker = args.walk();
                        for arg in args.named_children(&mut walker) {
                            if let Some(s) = string_literal_value(arg, bytes) {
                                let line = node.start_position().row as u32 + 1;
                                out.push(make_mcp_endpoint(&s, None, rel_path, line));
                                break;
                            }
                        }
                    }
                }
            }
            // `register_tool` may also be called directly (not as a method).
            if let Some(func) = node.child_by_field_name("function") {
                if func.kind() == "identifier" {
                    if let Some(n) = node_text(func, bytes) {
                        if n == "register_tool" {
                            if let Some(args) = node.child_by_field_name("arguments") {
                                let mut walker = args.walk();
                                for arg in args.named_children(&mut walker) {
                                    if let Some(s) = string_literal_value(arg, bytes) {
                                        let line = node.start_position().row as u32 + 1;
                                        out.push(make_mcp_endpoint(&s, None, rel_path, line));
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Pattern B: `McpTool { name: "...", handler: Arc::new(<HandlerType>), ... }`
        // struct literals. We accept any struct-expression whose type
        // ends in `McpTool` (covers `McpTool` and `platform_mcp_sdk::McpTool`).
        if node.kind() == "struct_expression" {
            let Some(type_node) = node.child_by_field_name("name") else {
                return;
            };
            let Some(type_text) = node_text(type_node, bytes) else {
                return;
            };
            if !type_text.ends_with("McpTool") {
                return;
            }
            let Some(body) = node.child_by_field_name("body") else {
                return;
            };
            let mut tool_name: Option<String> = None;
            let mut handler_type: Option<String> = None;
            let mut walker = body.walk();
            for field in body.named_children(&mut walker) {
                if field.kind() != "field_initializer"
                    && field.kind() != "shorthand_field_initializer"
                {
                    continue;
                }
                let Some(field_name_node) = field.child_by_field_name("field") else {
                    continue;
                };
                let Some(field_name) = node_text(field_name_node, bytes) else {
                    continue;
                };
                if field_name == "name" {
                    if let Some(value) = field.child_by_field_name("value") {
                        if let Some(s) = string_literal_value(value, bytes) {
                            tool_name = Some(s);
                        }
                    }
                } else if field_name == "handler" {
                    if let Some(value) = field.child_by_field_name("value") {
                        // Look for `Arc::new(<TypeName>)` or `<TypeName>::new()`
                        // patterns; pull the bare type-ident.
                        handler_type = handler_type_from_expr(value, bytes);
                    }
                }
            }
            if let Some(name) = tool_name {
                let line = node.start_position().row as u32 + 1;
                out.push(make_mcp_endpoint(&name, handler_type, rel_path, line));
            }
        }

        // Pattern C: hand-written match dispatch — `"X" => self.tool_X(...)`.
        // Used by impls (like doc-linter's own `src/cmd/mcp.rs`) that route
        // tool calls through a plain match instead of a `McpTool` literal
        // table. We require the arm string to equal the called method's
        // `tool_` suffix so we don't false-positive on other string-keyed
        // dispatch.
        if node.kind() == "call_expression" {
            let Some(func) = node.child_by_field_name("function") else {
                return;
            };
            if func.kind() != "field_expression" {
                return;
            }
            let Some(receiver) = func.child_by_field_name("value") else {
                return;
            };
            if node_text(receiver, bytes) != Some("self") {
                return;
            }
            let Some(field) = func.child_by_field_name("field") else {
                return;
            };
            let Some(method_name) = node_text(field, bytes) else {
                return;
            };
            let Some(arm_name) = method_name.strip_prefix("tool_") else {
                return;
            };
            let mut cur = node;
            while let Some(parent) = cur.parent() {
                if parent.kind() == "match_arm" {
                    let mut walker = parent.walk();
                    let kids: Vec<Node<'_>> = parent.named_children(&mut walker).collect();
                    if kids.is_empty() {
                        return;
                    }
                    let pattern = kids[0];
                    if let Some(pat_str) = first_string_literal(pattern, bytes) {
                        if pat_str == arm_name {
                            let line = parent.start_position().row as u32 + 1;
                            out.push(make_mcp_endpoint(
                                arm_name,
                                Some(method_name.to_string()),
                                rel_path,
                                line,
                            ));
                        }
                    }
                    return;
                }
                cur = parent;
            }
        }
    });
}

/// Returns the first `string_literal` descendant's value found by DFS,
/// or `None`. Used by the MCP match-dispatch extractor to pull the arm
/// pattern out of nested `match_pattern > literal_pattern > string_literal`
/// wrappers without hard-coding the grammar shape.
fn first_string_literal(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() == "string_literal" {
        return string_literal_value(node, bytes);
    }
    let mut walker = node.walk();
    for child in node.named_children(&mut walker) {
        if let Some(s) = first_string_literal(child, bytes) {
            return Some(s);
        }
    }
    None
}

/// Recovers a handler's type-name from an axum route expression so the
/// [[entity-doc-graph]] extractor can record `Endpoint -> Function` edges
/// even when the handler is a method, not a free function.
fn handler_type_from_expr(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    // `Arc::new(Foo)` → "Foo"; `Arc::new(FooTool)` → "FooTool";
    // `Foo::new()` → "Foo".
    if node.kind() == "call_expression" {
        let func = node.child_by_field_name("function")?;
        let args = node.child_by_field_name("arguments")?;
        // Arc::new(x) — pull x.
        if let Some(name) = node_text(func, bytes) {
            if name.ends_with("::new") || name.ends_with("Arc::new") {
                let mut walker = args.walk();
                for arg in args.named_children(&mut walker) {
                    if arg.kind() == "identifier" {
                        return node_text(arg, bytes).map(str::to_string);
                    }
                    if arg.kind() == "scoped_identifier" {
                        if let Some(n) = arg.child_by_field_name("name") {
                            return node_text(n, bytes).map(str::to_string);
                        }
                    }
                    // Type literal like `Foo {}` — grab the type name.
                    if arg.kind() == "struct_expression" {
                        if let Some(t) = arg.child_by_field_name("name") {
                            return node_text(t, bytes).map(str::to_string);
                        }
                    }
                }
            }
        }
        // `Foo::new()` — pull `Foo`.
        if func.kind() == "scoped_identifier" {
            if let Some(path) = func.child_by_field_name("path") {
                return node_text(path, bytes).map(str::to_string);
            }
        }
    }
    if node.kind() == "identifier" {
        return node_text(node, bytes).map(str::to_string);
    }
    None
}

fn make_mcp_endpoint(
    name: &str,
    handler: Option<String>,
    rel_path: &str,
    line: u32,
) -> EndpointFact {
    EndpointFact {
        id: format!("mcp:mcp:{name}"),
        kind: EndpointKind::Mcp,
        method: "mcp".to_string(),
        path: name.to_string(),
        handler_local: handler,
        source_file: rel_path.to_string(),
        source_line: line,
    }
}

// ---------- helpers ----------

/// Recursively visit every node in the tree, invoking `f` once per
/// node. Avoids recursion-depth issues by using an explicit cursor
/// stack.
fn walk<F: FnMut(Node<'_>)>(root: Node<'_>, f: &mut F) {
    let mut cursor: TreeCursor = root.walk();
    loop {
        f(cursor.node());
        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// Borrows the source slice covered by a tree-sitter node for the
/// [[entity-doc-graph]] endpoint extractor.
fn node_text<'a>(node: Node<'_>, bytes: &'a [u8]) -> Option<&'a str> {
    let range = node.byte_range();
    bytes.get(range).and_then(|b| std::str::from_utf8(b).ok())
}

/// Pull a string literal's contents out of a `string_literal` node,
/// stripping the quotes and unescaping the trivial cases (newline,
/// tab, quote, backslash). Returns `None` for raw-string literals
/// where the kind is `raw_string_literal` — those rarely appear in
/// route paths so we ignore them.
fn string_literal_value(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    if node.kind() != "string_literal" {
        return None;
    }
    let raw = node_text(node, bytes)?;
    // Strip the surrounding quotes; the lexer guarantees they're ASCII.
    let inner = raw.strip_prefix('"').and_then(|s| s.strip_suffix('"'))?;
    // Cheap unescape — route paths don't contain interesting escapes.
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => out.push(other),
                None => break,
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// Find the enclosing `function_item` (if any) for a tree-sitter node and
/// return its name. Used by the [[entity-doc-graph]] endpoint extractor to
/// index router-helper-fns so `.nest()` calls can resolve cross-fn within
/// a single file.
fn enclosing_fn_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    let mut cur = node.parent()?;
    loop {
        if cur.kind() == "function_item" {
            let n = cur.child_by_field_name("name")?;
            return node_text(n, bytes).map(str::to_string);
        }
        cur = cur.parent()?;
    }
}

/// Walk `<root>` for `.rs` files for the [[entity-doc-graph]] endpoint
/// extractor, applying the same include/exclude glob set that the
/// code-comment lint uses (so the two passes agree on what's source code
/// worth scanning).
fn discover_rs_files(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        if !config.matches_code_comment(path, root) {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    Ok(out)
}

/// Roadmap issue #19 (v0.3.0): walk `.py` files under `root`,
/// honoring the same skip-dir / include rules as
/// `discover_rs_files`. Used as input to the FastAPI extractor.
fn discover_py_files(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("py") {
            continue;
        }
        if !config.matches_code_comment(path, root) {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    Ok(out)
}

/// Roadmap issue #19 (v0.3.0): walk `.js` / `.ts` files under `root`
/// for the Express extractor. `.mjs` / `.cjs` / `.jsx` / `.tsx` are
/// deliberately out-of-scope for v1 — most Express handlers live in
/// plain `.js`/`.ts`, and broadening the file set is a follow-up.
fn discover_js_files(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("js" | "ts")) {
            continue;
        }
        if !config.matches_code_comment(path, root) {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    Ok(out)
}

/// Roadmap issue #19 (v0.3.0): regex-driven FastAPI extractor.
///
/// Matches `@<obj>.<method>("/path", ...)` decorators (and the
/// `@<obj>.<method>('/path', ...)` single-quote variant), where
/// `<method>` is one of the HTTP verbs FastAPI exposes
/// (`get`, `post`, `put`, `delete`, `patch`, `head`, `options`).
/// The handler is the `def <name>(` / `async def <name>(` line
/// that follows on a subsequent non-comment, non-decorator line.
///
/// Limitations (acceptable for v1):
///   - Multi-line decorators are partially supported — the path
///     literal must live on the first line of the call.
///   - `APIRouter(prefix="/v1")` prefix composition isn't
///     resolved; routes register under their literal path. A
///     follow-up tree-sitter pass can layer that in.
///   - Decorators on async closures / lambda handlers fall
///     through with `handler_local = None`, same convention
///     as the axum extractor.
fn extract_fastapi(content: &str, rel_path: &str, out: &mut Vec<EndpointFact>) {
    use std::sync::OnceLock;
    static DECO: OnceLock<regex::Regex> = OnceLock::new();
    static DEF: OnceLock<regex::Regex> = OnceLock::new();

    // Decorator: `@<word>.<method>("/path"...)` or with single quotes.
    let deco = DECO.get_or_init(|| {
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(
            r#"^\s*@(\w+)\.(get|post|put|delete|patch|head|options)\s*\(\s*["']([^"']+)["']"#,
        )
        .expect("hardcoded fastapi decorator regex must compile")
    });
    // def / async def: capture the function name.
    let def = DEF.get_or_init(|| {
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(r"^\s*(?:async\s+)?def\s+(\w+)\s*\(")
            .expect("hardcoded fastapi def regex must compile")
    });

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(deco_cap) = deco.captures(line) else {
            i += 1;
            continue;
        };
        let method = deco_cap
            .get(2)
            .map_or("", |m| m.as_str())
            .to_ascii_uppercase();
        let path = deco_cap.get(3).map_or("", |m| m.as_str()).to_string();
        let deco_line_1based = (i as u32) + 1;

        // Skip subsequent decorator lines + blank / comment
        // lines until we find a `def` / `async def`. Stop at 20
        // lines forward — a deeper gap means the decorator
        // isn't directly attached to a handler (rare; treat as
        // unbound).
        let mut handler_local: Option<String> = None;
        let mut j = i + 1;
        let cap_j = (i + 20).min(lines.len());
        while j < cap_j {
            let nxt = lines[j];
            let trimmed = nxt.trim_start();
            if trimmed.starts_with('@') || trimmed.is_empty() || trimmed.starts_with('#') {
                j += 1;
                continue;
            }
            if let Some(def_cap) = def.captures(nxt) {
                handler_local = Some(def_cap.get(1).map_or("", |m| m.as_str()).to_string());
            }
            break;
        }

        let id = format!("fastapi:{method}:{path}");
        out.push(EndpointFact {
            id,
            kind: EndpointKind::Fastapi,
            method,
            path,
            handler_local,
            source_file: rel_path.to_string(),
            source_line: deco_line_1based,
        });
        i += 1;
    }
}

/// Roadmap issue #19 (v0.3.0): regex-driven Flask extractor.
///
/// Matches `@<obj>.route("/path", methods=["GET", "POST"])`
/// (and the single-quote variants). `<obj>` is typically `app`
/// or a `Blueprint` instance. When `methods=[...]` is missing,
/// Flask defaults to GET — we emit a single GET fact.
///
/// Each (method, path) pair yields one EndpointFact. A route
/// declaring `methods=["GET", "POST"]` produces two facts with
/// distinct `id` values (`flask:GET:/path` + `flask:POST:/path`).
///
/// Limitations (acceptable for v1, documented inline):
///   - `Blueprint(url_prefix=...)` composition isn't resolved.
///   - Multi-line decorators where the methods list spans more
///     than the first line aren't fully captured (the regex
///     only sees what's on the decorator's first line).
///   - Flask 2.0+ shortcut decorators (`@app.get`, `@app.post`)
///     are caught by `extract_fastapi` already and land as
///     `Fastapi`-kind — they share the FastAPI shape.
fn extract_flask(content: &str, rel_path: &str, out: &mut Vec<EndpointFact>) {
    use std::sync::OnceLock;
    static DECO: OnceLock<regex::Regex> = OnceLock::new();
    static DEF: OnceLock<regex::Regex> = OnceLock::new();
    static METHOD: OnceLock<regex::Regex> = OnceLock::new();

    let deco = DECO.get_or_init(|| {
        // Captures: (1) object, (2) path, (3) optional inline
        // methods literal (the bracket contents).
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(
            r#"^\s*@(\w+)\.route\s*\(\s*["']([^"']+)["'](?:\s*,\s*methods\s*=\s*\[([^\]]*)\])?"#,
        )
        .expect("hardcoded flask decorator regex must compile")
    });
    let def = DEF.get_or_init(|| {
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(r"^\s*(?:async\s+)?def\s+(\w+)\s*\(")
            .expect("hardcoded flask def regex must compile")
    });
    let method_re = METHOD.get_or_init(|| {
        // Pulls each quoted method out of `methods=["GET", "POST"]`.
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(r#"["']([A-Za-z]+)["']"#)
            .expect("hardcoded flask methods regex must compile")
    });

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let Some(deco_cap) = deco.captures(line) else {
            i += 1;
            continue;
        };
        let path = deco_cap.get(2).map_or("", |m| m.as_str()).to_string();
        let deco_line_1based = (i as u32) + 1;

        // Parse the methods list. Default to GET when missing.
        let methods: Vec<String> = match deco_cap.get(3) {
            Some(list) => {
                let raw = list.as_str();
                let parsed: Vec<String> = method_re
                    .captures_iter(raw)
                    .map(|c| c[1].to_ascii_uppercase())
                    .collect();
                if parsed.is_empty() {
                    vec!["GET".to_string()]
                } else {
                    parsed
                }
            }
            None => vec!["GET".to_string()],
        };

        // Resolve handler (same shape as the FastAPI extractor —
        // walk forward past decorators / blank / comment lines).
        let mut handler_local: Option<String> = None;
        let mut j = i + 1;
        let cap_j = (i + 20).min(lines.len());
        while j < cap_j {
            let nxt = lines[j];
            let trimmed = nxt.trim_start();
            if trimmed.starts_with('@') || trimmed.is_empty() || trimmed.starts_with('#') {
                j += 1;
                continue;
            }
            if let Some(def_cap) = def.captures(nxt) {
                handler_local = Some(def_cap.get(1).map_or("", |m| m.as_str()).to_string());
            }
            break;
        }

        for method in methods {
            let id = format!("flask:{method}:{path}");
            out.push(EndpointFact {
                id,
                kind: EndpointKind::Flask,
                method,
                path: path.clone(),
                handler_local: handler_local.clone(),
                source_file: rel_path.to_string(),
                source_line: deco_line_1based,
            });
        }
        i += 1;
    }
}

/// Roadmap issue #19 (v0.3.0): regex-driven Node/Express extractor.
///
/// Matches `app.get("/path", handler)`-style route registrations on
/// `.js` / `.ts` files where the verb is one of Express's HTTP
/// methods (`get`, `post`, `put`, `delete`, `patch`, `head`,
/// `options`, `all`). The receiver can be any identifier — typically
/// `app`, `router`, or a Router instance.
///
/// The handler local name is recovered when the last argument on
/// the same line is a bare identifier — covering the dominant
/// `app.get(path, handler)` and `app.get(path, mw, handler)`
/// shapes. Anonymous handlers (arrow functions, `function(...)`,
/// inline callbacks) land with `handler_local = None`, same
/// convention as the axum extractor.
///
/// Limitations (acceptable for v1):
///   - `app.use("/prefix", router)` mounting is not resolved;
///     each route lands under its literal path.
///   - Multi-line route calls (path on line N, handler on line
///     N+1) only see the first line.
///   - NestJS / decorator-based routing is out of scope — that's
///     a separate framework with a different shape.
fn extract_express(content: &str, rel_path: &str, out: &mut Vec<EndpointFact>) {
    use std::sync::OnceLock;
    static CALL: OnceLock<regex::Regex> = OnceLock::new();
    static TRAILING_IDENT: OnceLock<regex::Regex> = OnceLock::new();

    let call = CALL.get_or_init(|| {
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(
            r#"^\s*(?:(?:export|return|await|const|let|var)\s+)?(\w+)\.(get|post|put|delete|patch|head|options|all)\s*\(\s*["'`]([^"'`]+)["'`](.*)$"#,
        )
        .expect("hardcoded express call regex must compile")
    });
    let trailing = TRAILING_IDENT.get_or_init(|| {
        #[allow(
            clippy::expect_used,
            reason = "literal regex; compile is infallible at runtime"
        )]
        regex::Regex::new(r",\s*([A-Za-z_$][A-Za-z0-9_$]*)\s*\)\s*;?\s*$")
            .expect("hardcoded express trailing ident regex must compile")
    });

    for (idx, line) in content.lines().enumerate() {
        let Some(cap) = call.captures(line) else {
            continue;
        };
        let method = cap.get(2).map_or("", |m| m.as_str()).to_ascii_uppercase();
        let path = cap.get(3).map_or("", |m| m.as_str()).to_string();
        let rest = cap.get(4).map_or("", |m| m.as_str());

        let handler_local: Option<String> = trailing
            .captures(rest)
            .map(|c| c[1].to_string())
            .filter(|s| !is_express_reserved(s));

        let id = format!("express:{method}:{path}");
        out.push(EndpointFact {
            id,
            kind: EndpointKind::Express,
            method,
            path,
            handler_local,
            source_file: rel_path.to_string(),
            source_line: (idx as u32) + 1,
        });
    }
}

/// Filters out JS reserved tokens that the trailing-identifier regex
/// could otherwise pick up (e.g. when the route is `app.get(path)`
/// with no handler and the line ends in `);`). Cheap allow-list,
/// not a parser.
fn is_express_reserved(s: &str) -> bool {
    matches!(
        s,
        "function" | "async" | "await" | "return" | "true" | "false" | "null" | "undefined"
    )
}

/// Every `.java` file the code-comment globs admit, for the JAX-RS
/// extractor.
fn discover_java_files(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("java") {
            continue;
        }
        if !config.matches_code_comment(path, root) {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    Ok(out)
}

/// Line-scanning JAX-RS extractor. The first type's `@Path` is the
/// prefix; each method carrying an HTTP-verb annotation (`@GET`, `@POST`,
/// …, fully qualified or not) becomes one endpoint at the prefix joined
/// with the method's own `@Path`. The handler is the method name.
///
/// Limitations: `@Path(SomeClass.CONSTANT)` (non-literal) paths are
/// treated as absent; nested resource classes share the outer prefix;
/// sub-resource locators (a `@Path` method with no verb) are not
/// followed.
fn extract_jaxrs(
    content: &str,
    rel_path: &str,
    consts: &HashMap<String, Option<String>>,
    out: &mut Vec<EndpointFact>,
) {
    use std::sync::OnceLock;
    static TYPE_DECL: OnceLock<regex::Regex> = OnceLock::new();
    if !content.contains(".ws.rs") {
        return;
    }
    #[allow(
        clippy::expect_used,
        reason = "literal regex; compile is infallible at runtime"
    )]
    let type_decl = TYPE_DECL.get_or_init(|| {
        regex::Regex::new(r"\b(?:class|interface|enum|record)\s+[A-Za-z_]")
            .expect("hardcoded jaxrs type regex must compile")
    });

    let mut prefix: Option<String> = None;
    let mut pending_path: Option<String> = None;
    let mut pending_verb: Option<(String, u32)> = None;
    let mut depth = 0;
    let mut in_block_comment = false;
    let content = crate::code_comments_java::blank_text_blocks(content);
    let local = java_string_constants(std::iter::once(content.as_ref()));
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if in_block_comment {
            in_block_comment = !line.contains("*/");
            continue;
        }
        if depth > 0 {
            depth += crate::code_comments_java::paren_delta(line);
            continue;
        }
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with("/*") {
            in_block_comment = !line.contains("*/");
            continue;
        }
        let mut rest = line;
        while rest.starts_with('@') && !rest.starts_with("@interface") {
            let name_end = rest[1..]
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
                .map_or(rest.len(), |n| n + 1);
            let simple = rest[1..name_end].rsplit('.').next().unwrap_or("");
            let after = rest[name_end..].trim_start();
            if simple == "Path" {
                pending_path = balanced_paren_end(after)
                    .map(|end| eval_java_string_expr(&after[1..end - 1], &[&local, consts]));
            } else if matches!(
                simple,
                "GET" | "POST" | "PUT" | "DELETE" | "PATCH" | "HEAD" | "OPTIONS"
            ) {
                pending_verb = Some((simple.to_string(), (i as u32) + 1));
            }
            if after.starts_with('(') {
                if let Some(end) = balanced_paren_end(after) {
                    rest = after[end..].trim_start();
                } else {
                    depth = crate::code_comments_java::paren_delta(after);
                    rest = "";
                }
            } else {
                rest = after;
            }
        }
        // A trailing comment after the annotations is not a declaration.
        if rest.is_empty() || rest.starts_with("//") || rest.starts_with("/*") {
            continue;
        }
        if type_decl.is_match(rest) {
            if prefix.is_none() {
                prefix = Some(pending_path.take().unwrap_or_default());
            }
        } else if let Some((method, source_line)) = pending_verb.take() {
            let path = join_jaxrs_path(
                prefix.as_deref().unwrap_or(""),
                pending_path.as_deref().unwrap_or(""),
            );
            out.push(EndpointFact {
                id: format!("jaxrs:{method}:{path}"),
                kind: EndpointKind::Jaxrs,
                method,
                path,
                handler_local: crate::code_comments_slash::decl_name(rest),
                source_file: rel_path.to_string(),
                source_line,
            });
        }
        pending_path = None;
        pending_verb = None;
    }
}

/// Every `static final String NAME = "literal";` across the Java
/// sources, keyed by simple name and by `Type.NAME` (the file's first
/// declared type). A key bound to two different values maps to `None`
/// (ambiguous — rendered as a `{NAME}` placeholder).
fn java_string_constants<'a>(
    sources: impl Iterator<Item = &'a str>,
) -> HashMap<String, Option<String>> {
    use std::sync::OnceLock;
    static CONST: OnceLock<regex::Regex> = OnceLock::new();
    static TYPE_NAME: OnceLock<regex::Regex> = OnceLock::new();
    #[allow(
        clippy::expect_used,
        reason = "literal regex; compile is infallible at runtime"
    )]
    let type_name = TYPE_NAME.get_or_init(|| {
        regex::Regex::new(r"\b(?:class|interface|enum|record)\s+([A-Za-z_]\w*)")
            .expect("hardcoded java type-name regex must compile")
    });
    #[allow(
        clippy::expect_used,
        reason = "literal regex; compile is infallible at runtime"
    )]
    let re = CONST.get_or_init(|| {
        regex::Regex::new(
            r#"\b(?:static\s+final|final\s+static)\s+String\s+([A-Za-z_]\w*)\s*=\s*"((?:[^"\\]|\\.)*)"\s*;"#,
        )
        .expect("hardcoded java constant regex must compile")
    });
    let mut out: HashMap<String, Option<String>> = HashMap::new();
    let mut bind = |key: String, value: &str| {
        out.entry(key)
            .and_modify(|v| {
                if v.as_deref() != Some(value) {
                    *v = None;
                }
            })
            .or_insert_with(|| Some(value.to_string()));
    };
    for src in sources {
        let ty = type_name.captures(src).map(|c| c[1].to_string());
        for c in re.captures_iter(src) {
            bind(c[1].to_string(), &c[2]);
            if let Some(ty) = &ty {
                bind(format!("{ty}.{}", &c[1]), &c[2]);
            }
        }
    }
    out
}

/// Evaluates a `@Path` argument — string literals and constant names
/// joined by `+` — against [`java_string_constants`]. `value = ` is
/// accepted; names resolve against `scopes` in order (the file's own
/// constants first, as Java does for an unqualified name), and an
/// unresolved name becomes `{NAME}`.
fn eval_java_string_expr(expr: &str, scopes: &[&HashMap<String, Option<String>>]) -> String {
    let expr = expr.trim();
    let expr = expr
        .strip_prefix("value")
        .and_then(|r| r.trim_start().strip_prefix('='))
        .unwrap_or(expr);
    let mut parts = Vec::new();
    let mut quote = false;
    let mut escaped = false;
    let mut start = 0;
    for (i, c) in expr.char_indices() {
        if quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quote = false;
            }
        } else if c == '"' {
            quote = true;
        } else if c == '+' {
            parts.push(&expr[start..i]);
            start = i + 1;
        }
    }
    parts.push(&expr[start..]);
    parts
        .iter()
        .map(|p| {
            let p = p.trim();
            if let Some(lit) = p.strip_prefix('"').and_then(|r| r.strip_suffix('"')) {
                return lit.to_string();
            }
            let name = p.rsplit('.').next().unwrap_or(p);
            // `a.b.Type.NAME` → try `Type.NAME`, then `NAME`.
            let qualified = p.rsplitn(3, '.').take(2).collect::<Vec<_>>();
            let qualified =
                (qualified.len() == 2).then(|| format!("{}.{}", qualified[1], qualified[0]));
            scopes
                .iter()
                .find_map(|s| {
                    qualified
                        .as_ref()
                        .and_then(|q| s.get(q).cloned().flatten())
                        .or_else(|| s.get(name).cloned().flatten())
                })
                .unwrap_or_else(|| format!("{{{name}}}"))
        })
        .collect()
}

/// Byte offset just past the `)` that closes the `(` at the start of `s`,
/// skipping string and char literals; `None` when it closes on a later
/// line.
fn balanced_paren_end(s: &str) -> Option<usize> {
    let mut depth = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Joins a class-level and method-level JAX-RS path: one `/` between
/// segments, a leading `/`, no trailing `/` (both are optional in
/// `@Path`).
fn join_jaxrs_path(prefix: &str, suffix: &str) -> String {
    let joined = [prefix, suffix]
        .iter()
        .map(|p| p.trim_matches('/'))
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    format!("/{joined}")
}

/// Returns true when `rel_path` lives in `crates/<X>/...` and `X` is in
/// the `clap_crates` list, OR when the file is the top-level
/// `src/main.rs` of a non-crates layout (rare; we accept it permissively
/// so simple projects with one binary work without extra config). Gates
/// CLI-subcommand discovery in the [[entity-doc-graph]] endpoint extractor.
fn file_crate_in_clap_list(rel_path: &str, clap_crates: &[String]) -> bool {
    if let Some(rest) = rel_path.strip_prefix("crates/") {
        if let Some((name, _)) = rest.split_once('/') {
            return clap_crates.iter().any(|c| c == name);
        }
    }
    false
}

/// Endpoint-extraction tests for the [[entity-doc-graph]] axum / clap /
/// MCP route walkers.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    fn extract_axum_only(src: &str) -> Vec<EndpointFact> {
        extract_from_str(src, "crates/test/src/lib.rs", &[])
            .expect("parse")
            .into_iter()
            .filter(|e| e.kind == EndpointKind::Axum)
            .collect()
    }

    #[test]
    fn extracts_three_axum_routes() {
        let src = r#"
            use axum::Router;
            use axum::routing::{get, post};
            fn router() -> Router {
                Router::new()
                    .route("/healthz", get(healthz))
                    .route("/v1/outlets/:id", get(get_outlet))
                    .route("/v1/outlets/:id", post(create_outlet))
            }
        "#;
        let eps = extract_axum_only(src);
        assert_eq!(eps.len(), 3, "three routes: {eps:#?}");
        let methods: Vec<&str> = eps.iter().map(|e| e.method.as_str()).collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"POST"));
        let paths: Vec<&str> = eps.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"/healthz"));
        assert!(paths.contains(&"/v1/outlets/:id"));
        // Handler ident captured.
        let handlers: Vec<Option<&str>> = eps.iter().map(|e| e.handler_local.as_deref()).collect();
        assert!(handlers.contains(&Some("healthz")));
        assert!(handlers.contains(&Some("get_outlet")));
        assert!(handlers.contains(&Some("create_outlet")));
    }

    #[test]
    fn nest_path_prefixes_inner_routes() {
        let src = r#"
            use axum::Router;
            use axum::routing::{get, post};
            fn outlets_router() -> Router {
                Router::new()
                    .route("/outlets/:id", get(get_outlet))
                    .route("/outlets", post(create_outlet))
            }
            fn root_router() -> Router {
                Router::new().nest("/v1", outlets_router())
            }
        "#;
        let eps = extract_axum_only(src);
        let paths: Vec<&str> = eps.iter().map(|e| e.path.as_str()).collect();
        assert!(
            paths.contains(&"/v1/outlets/:id"),
            "nested route path-prefixed: {eps:#?}"
        );
        assert!(
            paths.contains(&"/v1/outlets"),
            "second nested route: {eps:#?}"
        );
    }

    /// Asserts that a clap `#[derive(Subcommand)]` enum yields one
    /// kebab-case endpoint path per variant in the [[entity-doc-graph]]
    /// extractor.
    #[test]
    fn clap_subcommand_enum_yields_kebab_paths() {
        let src = r"
            use clap::Subcommand;
            #[derive(Subcommand)]
            enum Commands {
                Check,
                ScipIndex,
                Lsp,
                Init,
            }
        ";
        let eps = extract_from_str(
            src,
            "crates/doc-linter/src/main.rs",
            &["doc-linter".to_string()],
        )
        .unwrap();
        let clap_eps: Vec<&EndpointFact> = eps
            .iter()
            .filter(|e| e.kind == EndpointKind::Clap)
            .collect();
        assert_eq!(clap_eps.len(), 4, "four variants: {clap_eps:#?}");
        let paths: Vec<&str> = clap_eps.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"check"));
        assert!(paths.contains(&"scip-index"));
        assert!(paths.contains(&"lsp"));
        assert!(paths.contains(&"init"));
    }

    /// Asserts that the [[entity-doc-graph]] extractor only emits clap
    /// endpoints from crates explicitly listed in `clap_crates`.
    #[test]
    fn clap_skipped_for_crate_not_in_list() {
        // Same enum but file is in a crate NOT in `clap_crates` →
        // no clap endpoints emitted.
        let src = r"
            use clap::Subcommand;
            #[derive(Subcommand)]
            enum Commands { A, B }
        ";
        let eps =
            extract_from_str(src, "crates/other/src/main.rs", &["doc-linter".to_string()]).unwrap();
        assert!(eps.iter().all(|e| e.kind != EndpointKind::Clap));
    }

    /// Asserts that an empty source string yields no endpoints in the
    /// [[entity-doc-graph]] extractor (degenerate input is not an error).
    #[test]
    fn empty_input_returns_empty_vec() {
        let eps = extract_from_str("", "crates/x/src/lib.rs", &[]).unwrap();
        assert!(eps.is_empty());
    }

    /// Asserts that malformed Rust source doesn't panic the
    /// [[entity-doc-graph]] extractor — tree-sitter's recovery mode is
    /// trusted to produce a usable parse tree.
    #[test]
    fn malformed_rust_does_not_panic() {
        // Missing closing brace etc. — tree-sitter recovers; we just
        // shouldn't crash. We assert the call returns Ok (parser is
        // lenient) and contains no endpoints.
        let src = r"
            fn router() -> Router {
                Router::new().route(
        ";
        let eps = extract_from_str(src, "crates/x/src/lib.rs", &[]).unwrap();
        assert!(eps.is_empty(), "malformed input: {eps:#?}");
    }

    #[test]
    fn mcp_struct_literal_yields_endpoint() {
        // McpTool struct literal with a string `name:` field.
        let src = r#"
            fn tools() -> Vec<McpTool> {
                vec![
                    McpTool {
                        name: "read_state",
                        description: "x",
                        handler: Arc::new(ReadStateTool),
                    },
                ]
            }
        "#;
        let eps = extract_from_str(src, "crates/mcp-tools/src/lib.rs", &[]).unwrap();
        let mcp_eps: Vec<&EndpointFact> =
            eps.iter().filter(|e| e.kind == EndpointKind::Mcp).collect();
        assert_eq!(mcp_eps.len(), 1, "one MCP tool: {mcp_eps:#?}");
        assert_eq!(mcp_eps[0].path, "read_state");
        assert_eq!(mcp_eps[0].method, "mcp");
    }

    #[test]
    fn mcp_match_dispatch_yields_endpoints() {
        let src = r#"
            impl Server {
                fn dispatch(&self, name: &str, arguments: Value) -> Result<Value> {
                    match name {
                        "query_similar" => self.tool_query_similar(&arguments),
                        "query_path" => self.tool_query_path(&arguments),
                        _ => Err(McpError::UnknownTool),
                    }
                }
            }
        "#;
        let eps = extract_from_str(src, "src/cmd/mcp.rs", &[]).unwrap();
        let mut paths: Vec<&str> = eps
            .iter()
            .filter(|e| e.kind == EndpointKind::Mcp)
            .map(|e| e.path.as_str())
            .collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["query_path", "query_similar"]);
    }

    #[test]
    fn mcp_match_dispatch_rejects_unmatched_pairs() {
        let src = r#"
            impl Handler {
                fn handle(&self, kind: &str) {
                    match kind {
                        "alpha" => self.handle_alpha(),
                        "beta" => self.tool_gamma(),
                        _ => (),
                    }
                }
            }
        "#;
        let eps = extract_from_str(src, "src/handler.rs", &[]).unwrap();
        let mcp_eps: Vec<&EndpointFact> =
            eps.iter().filter(|e| e.kind == EndpointKind::Mcp).collect();
        assert!(
            mcp_eps.is_empty(),
            "no MCP eps from mismatched arms: {mcp_eps:#?}"
        );
    }

    /// Asserts that `pascal_to_kebab` produces the kebab forms used by
    /// clap argv conventions in the [[entity-doc-graph]] extractor.
    #[test]
    fn pascal_to_kebab_handles_typical_inputs() {
        assert_eq!(pascal_to_kebab("Check"), "check");
        assert_eq!(pascal_to_kebab("ScipIndex"), "scip-index");
        assert_eq!(pascal_to_kebab("Lsp"), "lsp");
    }

    /// Asserts that `join_axum_path` collapses redundant slashes when the
    /// [[entity-doc-graph]] extractor concatenates a `.nest()` prefix with
    /// an inner route.
    #[test]
    fn join_axum_path_handles_slashes() {
        assert_eq!(join_axum_path("/v1", "/outlets"), "/v1/outlets");
        assert_eq!(join_axum_path("/v1/", "/outlets"), "/v1/outlets");
        assert_eq!(join_axum_path("/v1", "outlets"), "/v1/outlets");
        assert_eq!(join_axum_path("", "/x"), "/x");
    }

    // Roadmap issue #19: FastAPI extractor tests.

    fn run_fastapi(src: &str) -> Vec<EndpointFact> {
        let mut out = Vec::new();
        extract_fastapi(src, "backend/api.py", &mut out);
        out
    }

    #[test]
    fn fastapi_extracts_app_get_with_handler() {
        let src = r#"
from fastapi import FastAPI
app = FastAPI()

@app.get("/v1/items")
def list_items():
    return []
"#;
        let eps = run_fastapi(src);
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.kind, EndpointKind::Fastapi);
        assert_eq!(e.method, "GET");
        assert_eq!(e.path, "/v1/items");
        assert_eq!(e.handler_local.as_deref(), Some("list_items"));
        assert_eq!(e.id, "fastapi:GET:/v1/items");
        assert_eq!(e.source_file, "backend/api.py");
        assert_eq!(e.source_line, 5, "decorator is on line 5");
    }

    #[test]
    fn fastapi_extracts_router_post_with_async_handler() {
        let src = r"
@router.post('/users')
async def create_user():
    pass
";
        let eps = run_fastapi(src);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].method, "POST");
        assert_eq!(eps[0].path, "/users");
        assert_eq!(eps[0].handler_local.as_deref(), Some("create_user"));
    }

    #[test]
    fn fastapi_handles_multiple_decorators() {
        let src = r#"
@app.get("/healthz")
def healthz():
    return {"ok": True}

@app.post("/v1/orders")
def create_order():
    pass

@app.delete("/v1/orders/{id}")
def delete_order(id: int):
    pass
"#;
        let eps = run_fastapi(src);
        assert_eq!(eps.len(), 3);
        let methods: Vec<&str> = eps.iter().map(|e| e.method.as_str()).collect();
        assert_eq!(methods, vec!["GET", "POST", "DELETE"]);
    }

    #[test]
    fn fastapi_skips_intervening_decorators_and_finds_handler() {
        // Real-world FastAPI code often has multiple decorators
        // on the same handler (e.g. `@app.get` + `@require_auth`).
        let src = r#"
@app.get("/v1/secret")
@require_auth
def secret():
    pass
"#;
        let eps = run_fastapi(src);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].handler_local.as_deref(), Some("secret"));
    }

    #[test]
    fn fastapi_ignores_non_http_decorators() {
        // `@some.thing("path")` with a non-HTTP-verb method doesn't match.
        let src = "\n@some.register(\"/not-a-route\")\ndef helper():\n    pass\n";
        let eps = run_fastapi(src);
        assert!(eps.is_empty(), "non-HTTP decorator should not match");
    }

    // Roadmap issue #19: Flask extractor tests.

    fn run_flask(src: &str) -> Vec<EndpointFact> {
        let mut out = Vec::new();
        extract_flask(src, "backend/api.py", &mut out);
        out
    }

    #[test]
    fn flask_route_defaults_to_get() {
        let src = "\n@app.route(\"/users\")\ndef list_users():\n    return []\n";
        let eps = run_flask(src);
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.kind, EndpointKind::Flask);
        assert_eq!(e.method, "GET");
        assert_eq!(e.path, "/users");
        assert_eq!(e.handler_local.as_deref(), Some("list_users"));
        assert_eq!(e.id, "flask:GET:/users");
    }

    #[test]
    fn flask_route_with_methods_emits_one_fact_per_method() {
        let src = "\n@app.route('/items', methods=['GET', 'POST'])\ndef items():\n    pass\n";
        let mut eps = run_flask(src);
        eps.sort_by(|a, b| a.method.cmp(&b.method));
        assert_eq!(eps.len(), 2);
        assert_eq!(eps[0].method, "GET");
        assert_eq!(eps[0].path, "/items");
        assert_eq!(eps[1].method, "POST");
        assert_eq!(eps[1].path, "/items");
    }

    #[test]
    fn flask_blueprint_route_matches() {
        // Blueprints use the same `@<obj>.route(...)` shape; the
        // url_prefix composition is documented as out-of-scope.
        let src = "\n@api_v1.route(\"/health\")\ndef healthz():\n    return 'ok'\n";
        let eps = run_flask(src);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].path, "/health");
    }

    #[test]
    fn flask_skips_unrelated_decorators() {
        let src = "\n@auth.required\ndef helper():\n    pass\n";
        let eps = run_flask(src);
        assert!(eps.is_empty());
    }

    // JAX-RS extractor tests.

    fn run_jaxrs(src: &str) -> Vec<EndpointFact> {
        let mut out = Vec::new();
        extract_jaxrs(
            src,
            "fineract-loan/src/main/java/LoansApiResource.java",
            &HashMap::new(),
            &mut out,
        );
        out
    }

    #[test]
    fn jaxrs_resolves_constant_path_expressions() {
        let c_src = "class C { public static final String JOB_ID = \"jobId\";\n\
                     static final String SHORT = \"short-name\";\n\
                     static final String DUP = \"a\"; }";
        let d_src = "class D { static final String DUP = \"b\"; }";
        let consts = java_string_constants([c_src, d_src].into_iter());
        assert_eq!(consts["DUP"], None);
        assert_eq!(
            java_string_constants(std::iter::once(
                "class E { private final static String FS = \"fs\"; }"
            ))["FS"]
                .as_deref(),
            Some("fs")
        );
        let src = [
            "import jakarta.ws.rs.*;",
            "@Path(\"/v1/jobs\")",
            "public class Jobs {",
            "  @GET @Path(\"{\" + SchedulerJobApiConstants.JOB_ID + \"}/runhistory\")",
            "  public String history() { return null; }",
            "  @GET @Path(SHORT + \"/{shortName}\")",
            "  public String byShort() { return null; }",
            "  @PUT @Path(value = DUP)",
            "  public String dup() { return null; }",
            "  static final String DUP = \"local\";",
            "}",
        ]
        .join("\n");
        let mut out = Vec::new();
        extract_jaxrs(&src, "Jobs.java", &consts, &mut out);
        let ids: Vec<_> = out.iter().map(|e| e.id.as_str()).collect();
        let mut unresolved = Vec::new();
        extract_jaxrs(
            "import jakarta.ws.rs.*;\n@Path(DUP)\nclass R {\n  @GET\n  public String a() { return null; }\n}\n",
            "R.java",
            &consts,
            &mut unresolved,
        );
        assert_eq!(unresolved[0].id, "jaxrs:GET:/{DUP}");
        let mut qualified = Vec::new();
        extract_jaxrs(
            "import jakarta.ws.rs.*;\n@Path(\"/v1/\" + org.x.D.DUP)\nclass R {\n  @GET\n  public String a() { return null; }\n}\n",
            "R.java",
            &consts,
            &mut qualified,
        );
        assert_eq!(qualified[0].id, "jaxrs:GET:/v1/b");
        assert_eq!(
            ids,
            [
                "jaxrs:GET:/v1/jobs/{jobId}/runhistory",
                "jaxrs:GET:/v1/jobs/short-name/{shortName}",
                "jaxrs:PUT:/v1/jobs/local",
            ]
        );
    }

    #[test]
    fn jaxrs_joins_class_and_method_paths_past_multiline_annotations() {
        let src = [
            "import jakarta.ws.rs.GET;",
            "/** Loans. */",
            "@Path(\"/v1/loans\") // api/v1/",
            "@Component",
            "@Tag(name = \"Loans\", description = \"The API (for) loans\")",
            "public class LoansApiResource {",
            "    @GET",
            "    @Produces({ MediaType.APPLICATION_JSON })",
            "    @Operation(summary = \"List (all) loans\",",
            "            description = \"\"\"",
            "            The client's loans (paged",
            "            \"\"\")",
            "    public String retrieveAll(@Context final UriInfo uriInfo,",
            "            @QueryParam(\"clientId\") final Long clientId) {",
            "        return null;",
            "    }",
            "    @POST @Path(\"{loanId}/\")",
            "    public String submit(@PathParam(\"loanId\") final Long loanId) { return null; }",
            "    @jakarta.ws.rs.DELETE",
            "    @Path(value = \"/{loanId}\")",
            "    public String delete(final Long loanId) { return null; }",
            "    public String helper() { return null; }",
            "}",
        ]
        .join("\n");
        let eps = run_jaxrs(&src);
        let got: Vec<_> = eps
            .iter()
            .map(|e| (e.id.as_str(), e.handler_local.as_deref(), e.source_line))
            .collect();
        assert_eq!(
            got,
            [
                ("jaxrs:GET:/v1/loans", Some("retrieveAll"), 7),
                ("jaxrs:POST:/v1/loans/{loanId}", Some("submit"), 17),
                ("jaxrs:DELETE:/v1/loans/{loanId}", Some("delete"), 19),
            ]
        );
        assert!(eps.iter().all(|e| e.kind == EndpointKind::Jaxrs));
    }

    #[test]
    fn jaxrs_ignores_files_without_jaxrs_imports() {
        let src = "@Path(\"/x\")\npublic class A {\n  @GET\n  public void a() {}\n}\n";
        assert!(run_jaxrs(src).is_empty());
    }

    // Roadmap issue #19: Express extractor tests.

    fn run_express(src: &str) -> Vec<EndpointFact> {
        let mut out = Vec::new();
        extract_express(src, "server/routes.ts", &mut out);
        out
    }

    #[test]
    fn express_app_get_with_named_handler() {
        let src = "app.get(\"/users\", listUsers);\n";
        let eps = run_express(src);
        assert_eq!(eps.len(), 1);
        let e = &eps[0];
        assert_eq!(e.kind, EndpointKind::Express);
        assert_eq!(e.method, "GET");
        assert_eq!(e.path, "/users");
        assert_eq!(e.handler_local.as_deref(), Some("listUsers"));
        assert_eq!(e.id, "express:GET:/users");
    }

    #[test]
    fn express_router_post_with_middleware_and_handler() {
        // Trailing identifier after the middleware list is the handler.
        let src = "router.post(\"/users\", requireAuth, validate, createUser);\n";
        let eps = run_express(src);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].method, "POST");
        assert_eq!(eps[0].path, "/users");
        assert_eq!(eps[0].handler_local.as_deref(), Some("createUser"));
    }

    #[test]
    fn express_anonymous_arrow_handler_has_no_local() {
        let src = "app.get('/health', (req, res) => res.send('ok'));\n";
        let eps = run_express(src);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].method, "GET");
        assert_eq!(eps[0].path, "/health");
        assert!(eps[0].handler_local.is_none());
    }

    #[test]
    fn express_ignores_app_use_mounting() {
        // `app.use("/api", router)` is middleware mounting, not a
        // route — out-of-scope for v1 and shouldn't match.
        let src = "app.use(\"/api\", router);\n";
        let eps = run_express(src);
        assert!(eps.is_empty(), "app.use is not a verb-route");
    }

    #[test]
    fn express_supports_all_method() {
        let src = "app.all('/admin', adminHandler);\n";
        let eps = run_express(src);
        assert_eq!(eps.len(), 1);
        assert_eq!(eps[0].method, "ALL");
        assert_eq!(eps[0].handler_local.as_deref(), Some("adminHandler"));
    }
}
