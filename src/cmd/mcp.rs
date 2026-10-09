//! Roadmap issue #35 (v0.3.0): `doc-linter mcp` — Model Context
//! Protocol server exposing the [[entity-doc-graph]] as agent-
//! callable tools.
//!
//! ## Current scope
//!
//! - Stdio transport only — line-delimited JSON-RPC 2.0 over
//!   stdin/stdout.
//! - Implements the three MCP methods needed for a working tool
//!   round-trip: `initialize`, `tools/list`, `tools/call`.
//! - Read-only tools wrapping the graph helpers:
//!   - `sql` — escape hatch over the typed-edge graph.
//!   - `list_entities` — ranked entity list.
//!   - `function_context` — Function row + its FUNCTION_DEFINED_IN
//!     / FUNCTION_MENTIONS neighbors.
//!   - `query_similar` — BM25 or embedding-based ranking over
//!     docs / functions / entities by free-text query. Per #28
//!     v5 (v0.4.0) the `backend` argument selects between `bm25`
//!     (default, always available) and `embedding` (cosine over
//!     the FLOAT[384] vectors v3 populates during ingest).
//!   - `query_dead_code` — Functions with no inbound CALLS and no
//!     Endpoint binding.
//!   - `query_endpoints` — Endpoint coverage (filterable by kind +
//!     dark-only).
//!   - `query_impact` — transitive blast radius for a Function
//!     symbol.
//!   - `query_at` — inverse `<file>:<line>` lookup.
//!   - `query_schema` — self-describing graph schema (cold-start).
//!   - `query_saved` — curated saved-query catalog.
//!   - `cluster` — community detection over the function graph.
//! - `notifications/*` are accepted and silently ignored (no
//!   response required by spec).
//! - Resources, prompts, sampling, authentication, and the SSE/
//!   HTTP transports defer to follow-ups.
//!
//! ## Why stdio
//!
//! Every shipped MCP client (Claude Desktop, Claude Code, the
//! reference SDKs) speaks stdio. Landing stdio first means an
//! operator can wire `doc-linter mcp` into their config today;
//! SSE/HTTP becomes an additive transport once we have a use
//! case for cross-host access.
//!
//! ## Threat model (issue #206)
//!
//! The MCP `sql` tool is the agent-facing escape hatch and is
//! the only surface that takes untrusted query text. It is
//! **read-only by design**: the store accepts one `SELECT` / `WITH`
//! statement and asks SQLite whether the compiled statement writes
//! (`sqlite3_stmt_readonly`), so a `WITH ... DELETE`, `PRAGMA` or
//! `ATTACH` is refused even if the LLM driving the tool was tricked
//! into emitting one. Schema introspection has its own typed tool
//! (`query_schema`). Writes from a trusted harness drive the doc-linter CLI
//! directly, not the MCP layer.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use doc_linter::graph_read::ReadGraph;
use doc_linter::ids::{DocId, EntityId, FunctionSymbol};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Code-only MCP tools and the node table each needs rows in. When that
/// table is empty (docs-only corpus, or no SCIP ingest yet) the tool is
/// left out of `tools/list`.
const CODE_TOOL_TABLES: &[(&str, &str)] = &[
    ("query_dead_code", "Function"),
    ("query_impact", "Function"),
    ("function_context", "Function"),
    ("query_at", "Function"),
    ("cluster", "Function"),
    ("query_endpoints", "Endpoint"),
    ("classify_file_coupling", "File"),
    ("list_files", "File"),
    ("read_source", "File"),
    ("list_modules", "Module"),
    ("list_types", "Type"),
    ("list_migrations", "Migration"),
];

/// `doc-linter mcp` entry point. Opens the graph once,
/// then enters the stdio request loop. Returns success when
/// stdin closes (normal MCP shutdown); a per-request error is
/// reported as a JSON-RPC error response, not a process exit.
pub(crate) fn run(root: &Path) -> Result<ExitCode> {
    let db = open_serving_db(root).context("open the graph for mcp")?;
    let server = Server::new(root.to_path_buf(), db);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let reader = BufReader::new(stdin.lock());
    let mut writer = stdout.lock();
    // gap-mcp-crash-orphans-the store-lock (iter 174 → 175): start
    // the idle-timeout watchdog before serve_loop. If the MCP
    // harness disconnects without sending EOF on stdin (the
    // user-probe-040 orphan pattern), the watchdog forces
    // process exit so an orphaned server does not linger.
    let last_activity = spawn_idle_watchdog("mcp");
    serve_loop_with_watchdog(&server, reader, &mut writer, Some(&last_activity))?;
    Ok(ExitCode::SUCCESS)
}

/// The full `tools/list` catalog (before per-corpus hiding); also the
/// source of `docs/reference/mcp.md`.
pub(crate) fn tools_list() -> Value {
    Server::tools_list()
}

/// Open the graph read-only for serving, building it first if it doesn't
/// exist. A fresh clone has no `.doc-lint/graph.sqlite`, and without a
/// running server the agent can't reach `reingest` to build one. The build
/// is the same silent check `reingest` runs, minus Vale and embeddings.
fn open_serving_db(root: &Path) -> Result<ReadGraph> {
    if !doc_linter::store_sqlite::graph_path(root).exists() {
        eprintln!(
            "doc-linter: no doc graph at {}, building it before serving",
            root.join(".doc-lint").display()
        );
        let config = doc_linter::config::LintConfig::load(&root.join(".doc-lint.toml"))?;
        let files = super::util::discover_corpus(root, &config)?;
        super::check::run(
            root,
            &config,
            &files,
            None,
            super::util::OutputFormat::Json,
            true, // no_vale: a cold start only needs the graph
            false,
            None,
            false,
            config.embeddings,
            false,
            true, // silent: stdout is the JSON-RPC stream
        )?;
    }
    ReadGraph::open(root)
}

/// gap-mcp-supervisor Slice B.1: worker entry point. Listens on
/// a unix-domain socket at `socket_path`, accepts one connection
/// (from the supervisor), and drives `serve_loop` over it until
/// the supervisor closes its end. Exits cleanly on EOF so the
/// supervisor can spawn a fresh worker process across binary
/// upgrades. Unix-only — the supervisor architecture is the only
/// supported deployment for hot-swap (gap-mcp-supervisor §
/// Proposed fix).
#[cfg(unix)]
pub(crate) fn run_worker(root: &Path, socket_path: &Path) -> Result<ExitCode> {
    use std::os::unix::net::UnixListener;
    let db = open_serving_db(root).context("open the graph for mcp-worker")?;
    let server = Server::new(root.to_path_buf(), db);
    // Clean up any stale socket from a previous worker that didn't
    // get to remove its own file. Bind would error with
    // EADDRINUSE otherwise.
    let _ = std::fs::remove_file(socket_path);
    let listener = UnixListener::bind(socket_path)
        .with_context(|| format!("bind mcp-worker socket at {}", socket_path.display()))?;
    let (stream, _peer) = listener
        .accept()
        .context("accept supervisor connection on mcp-worker socket")?;
    let read_half = stream
        .try_clone()
        .context("clone unix stream for read half")?;
    let reader = BufReader::new(read_half);
    let mut writer = stream;
    // gap-mcp-crash-orphans-the store-lock (iter 175): same orphan-
    // protection as the `run` path. The worker is more important
    // to watchdog than `run` because the supervisor can outlive
    // its workers; the worker's lock-hold blocks every CLI
    // fallback that targets the same root.
    let last_activity = spawn_idle_watchdog("mcp-worker");
    serve_loop_with_watchdog(&server, reader, &mut writer, Some(&last_activity))?;
    // Best-effort cleanup so a subsequent worker won't trip the
    // EADDRINUSE guard. Ignored on failure — the next bind's
    // remove_file above will retry.
    let _ = std::fs::remove_file(socket_path);
    // gap-mcp-supervisor Slice B.3b: voluntary swap exits with
    // code 42 so the supervisor can distinguish from a normal
    // shutdown (code 0).
    if server.swap_requested() {
        Ok(ExitCode::from(42))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

/// gap-mcp-supervisor Slice A: I/O-abstracted JSON-RPC dispatch
/// loop. The same body that `run` used over stdin/stdout is now
/// reusable over any `BufRead + Write` pair — letting the upcoming
/// `mcp-worker` subcommand serve over a unix-domain socket and
/// the supervisor proxy stdin/stdout into that socket without the
/// worker code knowing the difference.
///
/// Behaviour preserved exactly: empty / unreadable lines break out
/// (normal MCP shutdown semantics); notification responses (None)
/// produce no output; every other response is written with a
/// trailing newline + an explicit flush.
#[cfg(test)]
pub(super) fn serve_loop<R, W>(server: &Server, reader: R, writer: &mut W) -> Result<()>
where
    R: BufRead,
    W: Write,
{
    serve_loop_with_watchdog::<R, W>(server, reader, writer, None)
}

/// gap-mcp-crash-orphans-the store-lock (iter 175): variant of
/// `serve_loop` that updates a shared `last_activity`
/// timestamp on each line received. The idle-watchdog thread
/// reads this timestamp and forces `std::process::exit(0)` if
/// no activity has been seen for `IDLE_TIMEOUT`. Solves the
/// user-probe-040 orphan-MCP pattern: when the harness
/// disconnects without sending EOF on stdin, the watchdog
/// forces exit so an orphaned server does not linger.
///
/// Passing `None` for `last_activity` is the back-compat path
/// (used by tests / supervisor's pump where lifecycle is
/// externally managed).
pub(super) fn serve_loop_with_watchdog<R, W>(
    server: &Server,
    reader: R,
    writer: &mut W,
    last_activity: Option<&std::sync::Arc<std::sync::Mutex<std::time::Instant>>>,
) -> Result<()>
where
    R: BufRead,
    W: Write,
{
    for line in reader.lines() {
        let Ok(line) = line else {
            // stdin closed or read error — exit cleanly. MCP
            // clients close the pipe to signal shutdown.
            break;
        };
        // Refresh activity timestamp BEFORE the empty-line skip
        // so the watchdog sees the harness is alive even when
        // probes send blank-line keepalives.
        if let Some(ts) = last_activity {
            if let Ok(mut guard) = ts.lock() {
                *guard = std::time::Instant::now();
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = server.handle_line(trimmed);
        // Notifications return `None` (no response per JSON-RPC).
        if let Some(reply) = response {
            writeln!(writer, "{reply}").context("write MCP response")?;
            writer.flush().context("flush MCP response")?;
        }
        // gap-mcp-supervisor Slice B.3b: if the just-handled call
        // was `swap_worker`, the response has been written + flushed;
        // exit the loop now so the worker process can shut down
        // cleanly with ExitCode(42).
        if server.swap_requested() {
            break;
        }
    }
    Ok(())
}

/// gap-mcp-crash-orphans-the store-lock (iter 175): idle timeout for
/// the MCP server's stdin-blocking-reads. Default 4 hours; a
/// session that's been quiet for 4 hours is almost certainly
/// an orphaned harness-disconnect — exiting frees the
/// file lock so CLI / new MCP connections can open the DB.
/// Override via `DOC_LINTER_MCP_IDLE_TIMEOUT_SECS` for tests
/// and ops debugging.
pub(super) const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 4 * 60 * 60;

/// gap-mcp-crash-orphans-the store-lock (iter 175): read the idle
/// timeout from env var or fall back to the default. Pure
/// function — testable without spawning the watchdog thread.
pub(super) fn read_idle_timeout_from_env(
    env_get: impl Fn(&str) -> Option<String>,
) -> std::time::Duration {
    let secs = env_get("DOC_LINTER_MCP_IDLE_TIMEOUT_SECS")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

/// gap-mcp-crash-orphans-the store-lock (iter 175): decide whether
/// the watchdog should fire. Pure function — `now - last >=
/// timeout`. Extracted so tests don't need real wall-clock waits.
pub(super) fn should_watchdog_fire(
    last: std::time::Instant,
    now: std::time::Instant,
    timeout: std::time::Duration,
) -> bool {
    now.duration_since(last) >= timeout
}

/// gap-mcp-crash-orphans-the store-lock (iter 175): start the
/// watchdog thread that polls `last_activity` every 60 s and
/// forces process exit when the idle threshold is reached.
/// `label` is logged on the watchdog-fired stderr message so
/// the operator can distinguish `mcp` from `mcp-worker`
/// shutdowns.
fn spawn_idle_watchdog(
    label: &'static str,
) -> std::sync::Arc<std::sync::Mutex<std::time::Instant>> {
    let last_activity = std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let last_activity_clone = std::sync::Arc::clone(&last_activity);
    let timeout = read_idle_timeout_from_env(|k| std::env::var(k).ok());
    std::thread::spawn(move || {
        // Poll every 60 s. Test path can override via env var to
        // a small number so the watchdog fires quickly without
        // changing test timeouts.
        let poll = std::time::Duration::from_secs(60);
        loop {
            std::thread::sleep(poll);
            let last = match last_activity_clone.lock() {
                Ok(g) => *g,
                Err(_) => continue,
            };
            let now = std::time::Instant::now();
            if should_watchdog_fire(last, now, timeout) {
                eprintln!(
                    "doc-linter {label}: idle for {:?}, exiting (orphaned server, \
                     gap-mcp-crash-orphans per user-probe-040). \
                     Override with DOC_LINTER_MCP_IDLE_TIMEOUT_SECS.",
                    now.duration_since(last)
                );
                std::process::exit(0);
            }
        }
    });
    last_activity
}

/// MCP protocol revision we advertise during `initialize`. The
/// 2024-11-05 revision is the one every shipped client speaks; we
/// echo whatever the client requests so future-revision clients
/// downgrade cleanly.
const DEFAULT_PROTOCOL_VERSION: &str = "2024-11-05";

/// Default cap on rows returned by the `sql` MCP tool. Per the
/// Agent-Computer Interface principle ([[research-swe-agent]]) MCP
/// tools should return bounded responses by default so the agent
/// can read the result in one window. 200 rows comfortably fits a
/// realistic result while leaving headroom for the column
/// metadata. Callers can override via the `max_rows` argument.
const DEFAULT_SQL_MAX_ROWS: usize = 200;

/// Holds the resources every tool call needs (root path, graph).
/// Constructed once per `run` invocation; the request loop calls
/// methods on it. gap-mcp-supervisor Slice A: visibility raised to
/// `pub(super)` so the upcoming `cmd::mcp_supervisor` module can
/// build one when the worker subcommand spawns.
pub(super) struct Server {
    #[allow(dead_code, reason = "v2 tools (e.g. read file) will use root")]
    root: PathBuf,
    db: std::cell::RefCell<ReadGraph>,
    /// [`ReadGraph::identity`] of the file `db` was opened on. When
    /// another process swaps in a rebuilt graph, the next tool call sees a
    /// different identity and reopens.
    db_identity: std::cell::Cell<Option<(u64, u64)>>,
    /// gap-mcp-supervisor Slice B.3b: set by the `swap_worker`
    /// MCP tool. `serve_loop` checks the flag after each response
    /// and breaks out cleanly; `run_worker` returns ExitCode(42)
    /// so the supervisor can distinguish a voluntary swap from a
    /// regular EOF or crash.
    swap_requested: AtomicBool,
}

impl Server {
    fn new(root: PathBuf, db: impl Into<ReadGraph>) -> Self {
        let db = db.into();
        Server {
            db_identity: std::cell::Cell::new(db.identity(&root)),
            root,
            db: std::cell::RefCell::new(db),
            swap_requested: AtomicBool::new(false),
        }
    }

    /// Reopen the graph read-only if `graph.sqlite` was replaced since we
    /// opened it (a `check` / `reingest` elsewhere swapped in a rebuild).
    /// Without this a long-lived server keeps answering from the old file.
    /// On failure the old handle stays and serving continues.
    fn refresh_db_if_swapped(&self) {
        let now = self.db.borrow().identity(&self.root);
        if now.is_none() || now == self.db_identity.get() {
            return;
        }
        match ReadGraph::open(&self.root) {
            Ok(db) => {
                *self.db.borrow_mut() = db;
                self.db_identity.set(now);
            }
            Err(e) => {
                eprintln!("doc-linter: reopen swapped graph failed, serving the old one: {e:#}");
            }
        }
    }

    /// gap-mcp-supervisor Slice B.3b: did the worker handle a
    /// `swap_worker` request that asked it to exit cleanly? The
    /// dispatch loop calls this after each response.
    pub(super) fn swap_requested(&self) -> bool {
        self.swap_requested.load(Ordering::SeqCst)
    }

    /// Parse one input line as a JSON-RPC request (object) or
    /// JSON-RPC batch (array of requests) and return the stringified
    /// response. `None` means "no response" — the input was a single
    /// notification, an all-notification batch, or malformed in a way
    /// the spec says to drop silently.
    ///
    /// Roadmap issue #214: batch support is required by MCP
    /// 2025-03-26. We detect the array case after parsing to a raw
    /// `Value`, dispatch each element through the per-request
    /// handler, and collect the non-`None` responses into an array.
    /// Per spec, an empty batch is `-32600 Invalid Request`.
    fn handle_line(&self, line: &str) -> Option<String> {
        let parsed: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response_str(
                    Value::Null,
                    -32700,
                    &format!("parse error: {e}"),
                ));
            }
        };
        if let Value::Array(items) = parsed {
            if items.is_empty() {
                return Some(error_response_str(
                    Value::Null,
                    -32600,
                    "Invalid Request: batch must not be empty",
                ));
            }
            let mut responses: Vec<Value> = Vec::new();
            for item in items {
                if let Some(reply) = self.handle_value(item) {
                    if let Ok(parsed_reply) = serde_json::from_str::<Value>(&reply) {
                        responses.push(parsed_reply);
                    }
                }
            }
            if responses.is_empty() {
                // Pure notification batch — JSON-RPC says no reply.
                return None;
            }
            return Some(
                serde_json::to_string(&Value::Array(responses)).unwrap_or_else(|_| "[]".into()),
            );
        }
        self.handle_value(parsed)
    }

    /// Dispatch one already-parsed JSON value (object) as a JSON-RPC
    /// request. Returns the stringified response or `None` for a
    /// notification / silently-dropped malformed entry.
    fn handle_value(&self, value: Value) -> Option<String> {
        let req: Request = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                return Some(error_response_str(
                    Value::Null,
                    -32600,
                    &format!("Invalid Request: {e}"),
                ));
            }
        };
        // Notifications carry no id; we never reply.
        let id = req.id.clone()?;
        let result = self.dispatch(&req.method, req.params.unwrap_or(Value::Null));
        match result {
            Ok(value) => Some(ok_response_str(id, value)),
            Err(err) => Some(error_response_str(id, err.code, &err.message)),
        }
    }

    fn dispatch(&self, method: &str, params: Value) -> std::result::Result<Value, McpError> {
        match method {
            "initialize" => Ok(Self::initialize(params)),
            "tools/list" => Ok(self.tools_list_for_corpus()),
            "tools/call" => self.tools_call(params),
            // Roadmap issue #216: light health-check that doesn't
            // require a full `initialize` handshake. Per MCP spec
            // a ping returns an empty object on success.
            "ping" => Ok(json!({})),
            other => Err(McpError {
                code: -32601,
                message: format!("method not found: {other}"),
            }),
        }
    }

    fn initialize(params: Value) -> Value {
        // Echo the client's protocolVersion when present so a
        // future-revision client gets a familiar negotiation result.
        let protocol_version = params
            .get("protocolVersion")
            .and_then(|v| v.as_str())
            .unwrap_or(DEFAULT_PROTOCOL_VERSION)
            .to_string();
        json!({
            "protocolVersion": protocol_version,
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "doc-linter",
                "version": env!("CARGO_PKG_VERSION")
            }
        })
    }

    /// [`Self::tools_list`] minus code-only tools whose backing table is
    /// empty in this graph. On a docs-only corpus about 15 of 31 tools can
    /// only return nothing; listing them costs every session context.
    /// Hidden tools stay callable — only the advertisement changes.
    fn tools_list_for_corpus(&self) -> Value {
        let empty = self.empty_tables(CODE_TOOL_TABLES.iter().map(|(_, t)| *t));
        let mut list = Self::tools_list();
        if let Some(tools) = list["tools"].as_array_mut() {
            tools.retain(|t| {
                let name = t["name"].as_str().unwrap_or_default();
                !CODE_TOOL_TABLES
                    .iter()
                    .any(|(tool, table)| *tool == name && empty.contains(*table))
            });
        }
        list
    }

    /// Run `sql` on the open graph; the `{columns,row_count,rows}` shape.
    fn q(
        &self,
        sql: &str,
        bindings: Vec<(&str, doc_linter::store::Value)>,
    ) -> anyhow::Result<serde_json::Value> {
        doc_linter::graph_read::query_sql(&**self.db.borrow(), sql, bindings)
    }

    /// Which of `tables` count as empty. A table whose count can't be
    /// read (missing schema) is treated as non-empty: hide nothing on
    /// uncertainty. With no `Function` and no `Type` rows there is no
    /// code, so `File` / `Module` count as empty too: the file walker
    /// also indexes config files (`.toml`, `.json`), and a docs vault
    /// with two of those still has nothing for code tools to work on.
    fn empty_tables<'a>(
        &self,
        tables: impl IntoIterator<Item = &'a str>,
    ) -> std::collections::HashSet<String> {
        let db = self.db.borrow();
        let is_empty = |t: &str| {
            doc_linter::store::StoreRead::query(&**db, &format!("SELECT count(*) FROM {t}"), &[])
                .ok()
                .and_then(|r| r.first()?.first()?.as_i64())
                == Some(0)
        };
        let no_code = is_empty("Function") && is_empty("Type");
        tables
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|t| (no_code && matches!(*t, "File" | "Module")) || is_empty(t))
            .map(str::to_string)
            .collect()
    }

    fn tools_list() -> Value {
        json!({
            "tools": [
                {
                    "name": "sql",
                    "description": "Run a read-only SQL query over the entity-doc-graph (SQLite). One SELECT or WITH statement; anything that could write is rejected with -32602. Tables: one per node kind (Doc, Entity, Function, Type, File, ...) and one per edge kind as (src, dst, props...) (WIKILINK, COVERS, CALLS, ...); list columns live in doc_tags / doc_covers / entity_synonyms side tables. `query_schema` lists every table and column. `REGEXP` matches the whole string. Returns at most 200 rows by default; pass `max_rows` to raise the cap (the response carries `row_count` for what you got and `total_row_count` plus `truncated: true` when the underlying result exceeded the cap). When to use: a question no typed tool answers (joins across edge tables, ad-hoc counts), or exact symbol lookup (SELECT ... FROM Type WHERE symbol = 'X'). When NOT to use: free-text semantic search (use query_similar), enumerating saved catalogs (use query_saved), schema discovery (use `query_schema`), or anything a typed tool or `query_saved` already answers.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": { "type": "string", "description": "A single read-only SELECT / WITH statement." },
                            "params": { "type": "object", "description": "Optional text bindings for `$name` placeholders: {\"name\": \"value\"}." },
                            "max_rows": { "type": "integer", "description": "Row cap for the response (default 200)." }
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "list_entities",
                    "description": "Returns ontology Entities ranked by FUNCTION_MENTIONS edge count. Response always includes `total` (corpus-wide count) and `returned` (rows actually shipped); pass `top` to cap the latter.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "top": {
                                "type": "integer",
                                "description": "Maximum number of entities to return. Omit or pass 0 for no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "list_docs",
                    "description": "Returns every Doc in the vault, ordered by id. Each row includes the full frontmatter projection (role / kind / lifecycle / title / summary / status / updated / tags / covers, plus `attributes`: any other frontmatter keys as `key=value`). Response always includes `total` (corpus-wide count) and `returned` (after the optional `top` cap).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "top": {
                                "type": "integer",
                                "description": "Maximum number of docs to return. Omit or pass 0 for no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            },
                            "attribute": {
                                "type": "string",
                                "description": "Keep only docs whose non-standard frontmatter has this exact `key=value` (e.g. `company=234032`). Lists match per item. `total` counts after this filter. In sql: `WHERE 'company=234032' IN d.attributes`."
                            }
                        }
                    }
                },
                {
                    "name": "list_types",
                    "description": "Returns rows from the Type node table (structs / enums / traits / modules / type_aliases — see issue #32). Optional filters: `kind`, `crate`, `substring` (CONTAINS match on the SCIP symbol). Response includes `total` (after filter) and `returned` (after the optional `top` cap).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": ["struct", "enum", "trait", "module", "type_alias"],
                                "description": "Optional Type.kind filter."
                            },
                            "crate": {
                                "type": "string",
                                "description": "Optional Type.crate filter — scopes the listing to one crate/package."
                            },
                            "substring": {
                                "type": "string",
                                "description": "Optional substring (CONTAINS) match on the SCIP symbol — useful for `PricingRule` etc."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum number of types to return. Omit or pass 0 for no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "list_findings",
                    "description": "Returns Finding node rows (TODO / FIXME / XXX / HACK markers — see issue #31). Optional `severity` filter (`info` or `warning`) and `kind` filter (the lowercased marker token). Response includes `total` (after filter) and `returned` (after the optional `top` cap), grouped-by-severity ordering by default.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "severity": {
                                "type": "string",
                                "enum": ["info", "warning"],
                                "description": "Optional severity filter."
                            },
                            "kind": {
                                "type": "string",
                                "description": "Optional marker token filter (lowercased): `todo`, `fixme`, `xxx`, `hack`."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum number of findings to return. Omit or pass 0 for no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "function_context",
                    "description": "Full Function row + its FUNCTION_DEFINED_IN / FUNCTION_MENTIONS neighbors.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Full SCIP symbol of the function (e.g. \"rust-analyzer cargo my-crate 0.1.0 src/lib.rs/foo().\")."
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "query_similar",
                    "description": "Ranking over docs / functions / entities / types / all by natural-language query. Two backends: `bm25` (lexical, always available) and `embedding` (cosine similarity over FLOAT[384] vectors; requires the doc-linter binary built with --features embeddings and a successful ingest run with DOC_LINTER_EMBED_MODEL set). Every response carries `confidence: high|medium|low` + `confidence_signals` + `met_threshold` (per research-repoformer iter 85/88). When to use: free-text discovery (\"how do I X?\"), retrieval over prose docstrings, surfacing nearest related material when exact lookup isn't possible. When NOT to use: exact symbol lookup (use sql MATCH), categorical filter by attribute (use query_saved + research-by-tag / tools-by-target-artifact / etc.), or when met_threshold=false from a prior call — that signals semantic mismatch; reformulate or fall back to sql.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "text": {
                                "type": "string",
                                "description": "Free-text query to rank against."
                            },
                            "type": {
                                "type": "string",
                                "enum": ["doc", "function", "entity", "type", "section", "all"],
                                "description": "Corpus to search. Defaults to `doc`, which ranks only each doc's title + one-line summary. `section` ranks heading-level sections of doc BODIES (id `<doc-id>#<anchor>`, hit carries `doc_id`) — use it when the answer is a rule or passage inside a long doc (e.g. T7 in traps.md) rather than a whole doc. `all` mixes corpora (including sections) and tags each hit with `kind`. `type` ranks against Type nodes (struct / enum / trait doc-comments + signatures); note that Type lacks `repo_id` in the current schema so the `source_repo` filter is rejected on this axis with a recoverable error — drop the filter or use sql. Every response carries `confidence: high|medium|low` and `confidence_signals: {top_score, spread}` so the agent can decide whether to trust the hits, iterate with a reformulated query, or fall back to deterministic surfaces (sql / saved queries). Per research-repoformer + research-repocoder."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Number of top hits to return (default 5)."
                            },
                            "snippet_chars": {
                                "type": "integer",
                                "description": "Max chars in each hit's text_excerpt window (default 160; floor 40)."
                            },
                            "min_score": {
                                "type": "number",
                                "description": "Drop hits below this score (default 0.0). BM25 scores are unbounded above — bump to ~1.0 to skip weak lexical matches. Cosine scores are in [-1, 1] — bump to ~0.5 to skip loosely related matches when backend=embedding."
                            },
                            "backend": {
                                "type": "string",
                                "enum": ["bm25", "embedding"],
                                "description": "Ranking algorithm. `bm25` (default) is lexical BM25 over the title/body text columns. `embedding` is cosine similarity over the persisted FLOAT[384] vectors — semantically similar text ranks higher even when no surface tokens overlap. Errors with -32000 when the embedder isn't available (`is_available() == false`) OR when the requested corpus has zero populated embedding rows (run `doc-linter check --embeddings` first)."
                            },
                            "source_repo": {
                                "type": "string",
                                "description": "Gap-010 phase 3: restrict the ranking to rows whose `repo_id` matches. Today only the `Doc` corpus respects this filter — Entity / Function searches return rows from every ingested repo regardless. Call `list_repos` to enumerate valid ids; omit to rank across every repo."
                            },
                            "min_confidence": {
                                "type": "string",
                                "enum": ["low", "medium", "high"],
                                "description": "Server-side confidence gate (iter 88) extending iter 85's confidence signal. `low` (default) returns all hits unconditionally. `medium` returns hits ONLY if the computed confidence band is medium or high. `high` returns hits ONLY if the band is high. When the threshold isn't met, the response carries `met_threshold: false` AND an empty `hits` list — the agent reads the signal and decides whether to iterate with a reformulated query (RepoCoder-style) or fall back to sql / saved queries. Per research-repoformer's selective-retrieval principle."
                            },
                            "with_tag": {
                                "type": "string",
                                "description": "Per iter 242 ([[feedback_meta_doc_displaces_research]]): restrict Doc-axis ranking to rows whose `tags` array contains this value. Common values: `research` (only research papers), `interrogation` (only meta-discussions), `user-probe` (only Type V probes). Composes with `without_tag` for finer routing. Doc-axis only — Function / Entity / Type axes silently return all rows. The two-axis tag filter lets an agent split the research-stream from the meta-stream at the retrieval primitive layer instead of post-filtering hits."
                            },
                            "without_tag": {
                                "type": "string",
                                "description": "Per iter 242 ([[feedback_meta_doc_displaces_research]]): drop Doc-axis hits whose `tags` array contains this value. Common values: `interrogation` / `user-probe` (exclude meta-docs so BM25 surfaces research-axis hits cleanly). Sibling to `with_tag`. Doc-axis only."
                            }
                        },
                        "required": ["text"]
                    }
                },
                {
                    "name": "query_dead_code",
                    "description": "Functions with no inbound CALLS edge and no Endpoint binding — agent-cleanable surface. Response includes `total` (corpus-wide) and `returned` (after the optional cap).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "top": {
                                "type": "integer",
                                "description": "Maximum number of rows to return. Omit or pass 0 for no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "query_endpoints",
                    "description": "Every Endpoint (axum / clap / mcp / fastapi / flask / express) with its entity coverage. Filter by `kind`, by `dark` (no ENDPOINT_TOUCHES_ENTITY edge), or by `entity_id` (endpoints touching a specific Entity). Bogus `kind` values are rejected with -32602. Response includes `total` (after filter) and `returned` (after the optional cap).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "enum": ["axum", "clap", "mcp", "fastapi", "flask", "express"],
                                "description": "Optional kind filter — bogus values are rejected with -32602."
                            },
                            "dark": {
                                "type": "boolean",
                                "description": "When true, return only endpoints with no ENDPOINT_TOUCHES_ENTITY edge."
                            },
                            "entity_id": {
                                "type": "string",
                                "description": "When set, return only endpoints with an ENDPOINT_TOUCHES_ENTITY edge to this Entity id. Use `list_entities` to discover ids."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum number of endpoints to return. Omit or pass 0 for no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "query_impact",
                    "description": "Transitive blast-radius view from a Function symbol — callers, touched entities, endpoints.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "symbol": {
                                "type": "string",
                                "description": "Full SCIP symbol of the target function."
                            },
                            "depth": {
                                "type": "integer",
                                "description": "How many CALLS hops to walk backwards (default 3)."
                            }
                        },
                        "required": ["symbol"]
                    }
                },
                {
                    "name": "query_at",
                    "description": "Inverse lookup from `<file>:<line>` to the surrounding graph context — function, file, module, entities, covering docs, nearby endpoints. Adds `past_eof: bool` (set when `line > File.loc`) and `line_in_function: bool` (set when an indexed Function span starts at or before the line and the line is not past EOF) so an agent can branch on those without inferring from missing fields.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "file": {
                                "type": "string",
                                "description": "Repo-relative source file path."
                            },
                            "line": {
                                "type": "integer",
                                "minimum": 1,
                                "description": "1-based line number inside the file. `line=0` is rejected with -32602; lines past File.loc surface as `past_eof: true`."
                            }
                        },
                        "required": ["file", "line"]
                    }
                },
                {
                    "name": "query_schema",
                    "description": "Self-describing graph schema — every node table, every rel table, columns + row counts. Cold-start endpoint for agents discovering the vocabulary they should query against. When to use: at the start of every new session, before crafting sql queries, or whenever you're unsure which node table holds a property. When NOT to use: per-row data lookup (use sql), counting rows in a specific table (use sql MATCH (n:X) RETURN count(n)), or listing tools (the MCP host's tools/list method does that).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "query_saved",
                    "description": "List or run a curated saved SQL query. With no arguments, returns the catalog (each entry includes name + description + params + origin). With `name`, runs that query (with optional `params`). When the name doesn't match the catalog, the error includes a 'Did you mean X?' suggestion by Levenshtein distance. Each catalog entry now carries `origin` per gap-saved-query-catalog-portability (user-probe-021): `compile-time` means universal across every doc-linter install (safe to recommend cross-corpus — e.g. coupled-files, language-distribution, import-fan-out); `corpus-local` means the query came from this corpus's `saved-queries/*.toml` and may not exist on other corpora (e.g. docs-by-kind, audits-by-tag on the design corpus). Filter by origin when crafting cross-corpus recommendations. When to use: category-shape questions ('show me research-* docs tagged graph-similarity'), deterministic listings the catalog supports (recent-research, tag-coverage, docs-by-kind, research-peer-links, audits-by-tag, tools-by-target-artifact), or as the catalog-substitute when query_similar returns met_threshold=false. When NOT to use: free-text discovery (use query_similar) or ad-hoc traversal the catalog doesn't cover (use sql).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "name": {
                                "type": "string",
                                "description": "Saved-query name. Omit to list the catalog."
                            },
                            "params": {
                                "type": "object",
                                "description": "String → string substitutions matching the saved query's declared params."
                            }
                        }
                    }
                },
                {
                    "name": "query_path",
                    "description": "Shortest undirected Doc → Doc path between two doc ids. Returns a list of `{id, title, via_line?, via_edge_type?}` steps (the first step is the source, its `via_*` fields are null). Empty list means no path within `max_hops`.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "from": {
                                "type": "string",
                                "description": "Source Doc id."
                            },
                            "to": {
                                "type": "string",
                                "description": "Destination Doc id."
                            },
                            "max_hops": {
                                "type": "integer",
                                "description": "Upper bound on path length. Default 6 — matches the CLI."
                            }
                        },
                        "required": ["from", "to"]
                    }
                },
                {
                    "name": "query_doc",
                    "description": "Get a single Doc by id with its outbound and inbound edges (frontmatter + neighbors + backlinks). The Doc.id matches the frontmatter `id:` field — pass it verbatim. Returns 404-style error when the id is unknown. Per iter 104: when the id starts with `entity-` (ontology-entity Doc), the inbound list ALSO includes COVERS-inbound edges from the corresponding Entity node — agents asking 'which docs cover entity-X?' now get the full picture in one call (closes the gap surfaced by user-probe-018). Each inbound COVERS row carries `edge_type: \"covers\"`.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Doc id (e.g. \"entity-pricing-rule\", \"crate-doc-linter\")."
                            }
                        },
                        "required": ["id"]
                    }
                },
                {
                    "name": "suggest_tags_for_doc",
                    "description": "Per iter 111's gap-tag-axis-loose-end-detection finding + iter 163's form-drift extension (interrogation-032 Finding C): detect when a Doc's metadata is missing family-tags. Two heuristics run together: (1) `suggested_tags` — summary-text matches kebab/space form of a known tag (the original iter-111 behavior). (2) `family_suggestions` — known tags whose kebab-tokens are close (Levenshtein ≤ 2, both ≥ 4 chars) to one of the doc's CARRIED tag's tokens. Catches form-drift like research-voyage-code-3's `embeddings` plural vs the family tag `embedding-model` singular, where the summary may not name the tag but the carried-tag-stem reveals the family. Returns `{doc_id, current_tags, suggested_tags: [{tag, evidence}], family_suggestions: [{tag, evidence}], summary}`. When to use: surface metadata gaps before authoring a retag. When NOT to use: bulk tag retagging (the loop's ontology-organic-growth memory says to add tags via fresh absorptions, not bulk-edit); also not for non-research docs where the summary's vocabulary varies more.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "doc_id": {
                                "type": "string",
                                "description": "Doc id to check (e.g. \"research-swe-agent\"). Must match a row in the Doc table."
                            }
                        },
                        "required": ["doc_id"]
                    }
                },
                {
                    "name": "context_for",
                    "description": "Per iter 117's closure of gap-007's polymorphic-dispatch + uniform-envelope sub-gaps: one-shot grounding payload for a chosen node, irrespective of kind. Pass the same `id` you'd give query_doc / query_entity / function_context — context_for auto-detects whether it's a Doc, Entity, or Function and returns a uniform `{centre: Node, neighbors: [{node: Node, edge: {kind, line?}}]}` envelope. Every Node carries `{id, kind, title, summary, path?, tags?}` so the agent doesn't have to switch per-centre-type shapes. Collapses the 1+N round-trip pattern documented in gap-007: query_entity returned bare {id, title}; you'd then call query_doc on every neighbor. context_for projects each neighbor's summary inline. When to use: an end-user agent has a single id (from query_similar, query_doc_neighbors, query_saved listing, or wikilink) and wants its full grounding context in one round-trip. When NOT to use: free-text discovery (use query_similar) or aggregate listings (use query_saved); also use query_doc / query_entity directly when you specifically need the per-kind-specialised fields (e.g., Entity.is_god_node, Doc inbound/outbound split).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Node id. Tries Doc.id first, then Entity.id, then Function.symbol. Returns -32602 if none match."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum neighbors to return. Default 20; capped at 50 per ACI bounded-response principle."
                            }
                        },
                        "required": ["id"]
                    }
                },
                {
                    "name": "query_entity_neighbors",
                    "description": "Entity-axis mirror of query_doc_neighbors. Given an Entity id, return neighbor Entities along one of two structural axes: `co-cover` (Entities co-covered by the same Docs — the inverse of doc-axis FOCUS-style CF) or `related` (Entities connected via RELATES_TO edges). Each row carries `id`, `display`, `shared_docs` (co-cover mode) or `relation_type` (related mode), and `shared_count`. Returns empty when the axis yields no edges. When to use: explore entity-level structural relationships AFTER `entities-by-coverage-density` (iter 100 saved query) surfaces the entity hierarchy. When NOT to use: free-text similarity (use query_similar with type=entity) or Doc-axis exploration (use query_doc_neighbors).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Target Entity id (e.g. \"retrieval-primitive\", \"workflow-companion\")."
                            },
                            "by": {
                                "type": "string",
                                "enum": ["co-cover", "related"],
                                "description": "Structural axis. `co-cover` (default) = Entities sharing Doc COVERS-edge sources (returns `shared_docs` list as the explanation). `related` = Entities connected via RELATES_TO edges (returns `relation_type` as the edge metadata)."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum neighbors to return. Default 10; capped at 50 per ACI bounded-response principle."
                            }
                        },
                        "required": ["id"]
                    }
                },
                {
                    "name": "query_doc_neighbors",
                    "description": "Recommend Docs related to the given target Doc along one of two structural axes. Per [[research-focus]]'s CF framing + [[research-isonet]]'s interpretable assignment, each row carries an EXPLICIT explanation of why it's a neighbor — not just a scalar similarity. For `by=covers` (default) the explanation is `shared_entities`: the list of Entity ids both docs cover. For `by=wikilink` the explanation is `shared_count`: the number of wikilink edges. For `by=all` you get both per-axis scores plus the fused total. Returns up to `top` neighbors (default 10, max 50). Empty when the axis yields no edges for the target. When to use: explore which docs in the corpus relate to your draft (the workflow-companion path); compose with query_doc to read the neighbor's frontmatter. When NOT to use: free-text similarity (use query_similar) or category navigation by attribute (use query_saved with research-by-tag / audits-by-tag).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Target Doc id. Must match a row in the Doc table."
                            },
                            "by": {
                                "type": "string",
                                "enum": ["covers", "wikilink", "all"],
                                "description": "Structural axis. `covers` (default) = docs sharing COVERS-edge targets (Entity nodes), returning `shared_entities` list as the explanation. `wikilink` = docs the target wikilinks TO, returning `shared_count` = number of wikilink lines. `all` = MULTI-VIEW FUSION per research-mega: runs covers AND wikilink, merges by neighbor id, returns per-view scores (`covers_score`, `wikilink_score`, plus `shared_entities` for the covers half) alongside `fused_score`. Composes with research-peer-links (the saved query that ranks docs by total wikilink fan-in/out)."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum neighbors to return. Default 10; capped at 50 to keep the response bounded per the ACI principle."
                            }
                        },
                        "required": ["id"]
                    }
                },
                {
                    "name": "query_entity",
                    "description": "Get a single Entity's ego-graph — the Entity row plus every Doc / Function / Endpoint node within `depth` hops along COVERS / FUNCTION_BELONGS_TO / ENDPOINT_TOUCHES_ENTITY / ENTITY_CALLS / RELATES_TO. Use this when you need narrative coverage + structural reach for one entity in one call.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Entity id (e.g. \"pricing-rule\")."
                            },
                            "depth": {
                                "type": "integer",
                                "description": "Hops to walk in the entity ego-graph (default 2)."
                            }
                        },
                        "required": ["id"]
                    }
                },
                {
                    "name": "read_source",
                    "description": "Return a slice of source code from a repo-relative `path` so MCP-only agents can read code without shelling out via Bash. Optional `line` centres the window (1-based); omit it to read from the top. `context_lines` controls how many lines flank the centre (default 30, max 200). Hard caps: path must be repo-relative (no `..`, no absolute paths), files larger than 8 MiB are rejected before read, and total returned bytes are capped at ~256 KiB. Returns `{path, start_line, end_line, content, line_count}` — `content` is the raw text without line-number prefixing. When to use: inspect line-anchored source after a sql / query_at lookup gave you a (file, line) pair, follow a callee found via function_context. When NOT to use: whole-file reads (let the agent shell out to cat or use Bash), discovering paths (use sql MATCH (f:File) RETURN f.path or list_files).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Repo-relative source file path. `..` segments and absolute paths are rejected."
                            },
                            "line": {
                                "type": "integer",
                                "minimum": 1,
                                "description": "Optional 1-based centre line. Omit to read the file from the top."
                            },
                            "context_lines": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 200,
                                "description": "Lines of context to include before and after the centre (default 30, capped at 200)."
                            }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "list_modules",
                    "description": "Returns rows from the Module node table — coarse code groupings (Rust crates, Python packages). Optional filters: `kind`, `substring` (CONTAINS on the module id). Response envelope: `total` (after filter) / `offset` / `returned` (after `top`).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "kind": {
                                "type": "string",
                                "description": "Optional Module.kind filter (e.g. `rust_crate`, `python_package`)."
                            },
                            "substring": {
                                "type": "string",
                                "description": "Optional CONTAINS match on Module.id."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum rows to return. Omit or 0 = no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "list_files",
                    "description": "Returns rows from the File node table (one row per indexed source file). Optional filters: `language` (e.g. `rust`, `python`, `typescript`), `substring` (CONTAINS on File.path). Response envelope: `total` (after filter) / `offset` / `returned` (after `top`).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "language": {
                                "type": "string",
                                "description": "Optional File.language filter (matches the SCIP-ingest detected language token)."
                            },
                            "substring": {
                                "type": "string",
                                "description": "Optional CONTAINS match on File.path."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum rows to return. Omit or 0 = no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "list_repos",
                    "description": "Gap-010 phase 1: list every ingested `Repo` (one row per `--root` plus each `cross_repo_roots` entry from `.doc-lint.toml`). Returns `{id, root_path, name}`. id is the canonical absolute path; root_path mirrors it; name is the basename. Every Doc carries the `repo_id` of the repo it lives in (cross_repo_roots docs are indexed, not validated), so pass an id as query_similar's `source_repo` to search one repo, or filter `d.repo_id` in sql.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "top": {
                                "type": "integer",
                                "description": "Maximum rows to return. Omit or 0 = no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "list_migrations",
                    "description": "Returns rows from the Migration node table — the ontology migration history. Optional filter: `since` (only migrations with `to_version >= since`). Default order is `from_version ASC`.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "since": {
                                "type": "integer",
                                "description": "Optional lower bound on Migration.to_version (inclusive)."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Maximum rows to return. Omit or 0 = no cap."
                            },
                            "offset": {
                                "type": "integer",
                                "minimum": 0,
                                "description": "Zero-based pagination offset. Negative values are rejected with -32602."
                            }
                        }
                    }
                },
                {
                    "name": "cluster",
                    "description": "Community detection over the function graph — surfaces candidate entities from CALLS clusters. Returns the same JSON report `doc-linter cluster` emits; agents can review the top communities before promoting any.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "top_n": {
                                "type": "integer",
                                "description": "How many top communities to surface (default 10)."
                            },
                            "order_by": {
                                "type": "string",
                                "enum": ["member-count", "density"],
                                "description": "Ranking key for the top-N cut. Defaults to `member-count`. `density` surfaces tighter cliques first."
                            },
                            "algorithm": {
                                "type": "string",
                                "enum": ["lpa", "louvain", "leiden"],
                                "description": "Community-detection algorithm. `lpa` (default) is fast O(E) label propagation; `louvain` runs a single-level modularity-optimization pass for tighter communities; `leiden` is the multi-level local-move + refinement + aggregate pipeline that catches both weakly-bridged sub-clusters and hub-heavy hierarchy."
                            },
                            "min_members": {
                                "type": "integer",
                                "description": "Minimum community size to surface. 2 (default) drops singletons only; bump to filter noise communities on big repos."
                            },
                            "seed_symbol": {
                                "type": "string",
                                "description": "When set, return only the community containing this SCIP symbol (exact match on god_node_symbol / suggested_id / top_members)."
                            },
                            "resolution": {
                                "type": "number",
                                "description": "Modularity resolution γ (Leiden only). Default 1.0. γ > 1 forces smaller communities — useful when multi-level Leiden merges weakly-related sub-domains into giant macro-communities due to the modularity resolution limit on hub-heavy CALLS graphs."
                            },
                            "top_members": {
                                "type": "integer",
                                "description": "How many member symbols to include in each candidate's preview list (default 5)."
                            }
                        }
                    }
                },
                {
                    "name": "audit_doc_region",
                    "description": "v1 MVP — audit a local doc against a trusted-repo corpus. Resolves `doc_path` to the matching Doc row, then ranks every Doc whose `repo_id` matches `against` by similarity to the target's summary (BM25 unless `backend=embedding`). Returns the target's frontmatter plus the top hits with id / title / summary / score so an external LLM agent can compare and suggest deltas. The agent supplies the rerank + 'what's missing in your doc' reasoning; the tool supplies the grounding. The response also carries a `rerank.verdicts` array — empty when the in-process Judge is NoOp (default builds), populated when an Anthropic / OpenAI backend lands behind --features llm. Top-level `confidence: high|medium|low` + `confidence_signals: {top_score, spread}` + `met_threshold` (per iter 96, mirroring iter 85/88 on query_similar) — same trust-signal axis at the response root so an agent doesn't have to dig into `hits.confidence`. When to use: you have a local draft and want to compare against a known-good corpus (the workflow-companion path per entity-workflow-companion); read `confidence` first to decide whether to use the hits or iterate. When NOT to use: single-doc lookup (use query_doc), free-text exploration without a target doc (use query_similar with text=your question), or when the trusted corpus has zero matching docs (the response will be obviously empty).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "doc_path": {
                                "type": "string",
                                "description": "Repo-relative path (e.g. `docs/design/foo.md`) OR the Doc.id directly. Resolved against the local repo (the MCP server's --root) — to audit a doc from a SIBLING repo, ingest it as the primary first."
                            },
                            "against": {
                                "type": "string",
                                "description": "Repo id of the trusted corpus to compare against (one of the rows returned by `list_repos`). Determines which docs the similarity ranking pulls from."
                            },
                            "top": {
                                "type": "integer",
                                "description": "Number of trusted-corpus hits to return (default 5)."
                            },
                            "backend": {
                                "type": "string",
                                "enum": ["bm25", "embedding"],
                                "description": "Ranking backend, same semantics as `query_similar`. Default bm25. Embedding falls back to bm25 with a noted warning if the trusted corpus has no populated embeddings."
                            },
                            "with_tag": {
                                "type": "string",
                                "description": "Per iter 242 ([[feedback_meta_doc_displaces_research]]): restrict the trusted-corpus ranking to Docs whose `tags` array contains this value. Common values: `research` (compare against research-axis only), `feature` (compare against shipped-feature notes). Composes with `without_tag`."
                            },
                            "without_tag": {
                                "type": "string",
                                "description": "Per iter 242: drop trusted-corpus Docs whose `tags` array contains this value. Common: `interrogation` / `user-probe` to ignore the loop's self-referential meta-docs when auditing against a corpus that includes them."
                            }
                        },
                        "required": ["doc_path", "against"]
                    }
                },
                {
                    "name": "swap_worker",
                    "description": "gap-mcp-supervisor Slice B.3b: ask the worker to exit cleanly with code 42 so the supervisor can swap in the freshly-built binary without dropping the Claude Code stdio connection. Returns OK immediately; the worker writes the response and shuts down right after. Use after `cargo build --release` to bring a new doc-linter binary live. Optional `reason` is logged.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "reason": {
                                "type": "string",
                                "description": "Optional human-readable reason for the swap (logged to stderr; not stored)."
                            }
                        }
                    }
                },
                {
                    "name": "reload_self",
                    "description": "Hot-reload the doc-linter binary the MCP server is running. Same PID, same stdio pipe — the MCP harness shouldn't notice. Use after `cargo build --release` to pick up new MCP tool registrations / schema changes WITHOUT a /mcp reconnect. Sends `notifications/tools/list_changed` before exec so a spec-compliant harness re-fetches `tools/list`. The CALL completes (you get the response below), and ~50ms later the new binary takes over.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                },
                {
                    "name": "classify_file_coupling",
                    "description": "Per user-probe-034's coupling-spectrum framework: classify a File's COUPLED_WITH neighborhood into one of four categorical tiers — `absence` (0 neighbors — well-encapsulated leaf module), `minimal` (1-2 neighbors at moderate jaccard 0.3-0.7 — small family co-evolution), `heavy` (5+ neighbors at high jaccard ≥ 0.75 — tight bounded context), `moderate` (in between). Returns `{path, tier, neighbor_count, max_jaccard, mean_jaccard, explanation, neighbors: [{path, jaccard}]}` — the agent gets a CATEGORICAL recommendation alongside the raw scores. When to use: refactor / debugging / PR-review workflows where the agent needs to reason about blast radius without re-deriving the thresholds. Composes with `files-coupled-to($path)` (the raw neighbor list) and `files-by-path-substring($pattern)` (the upstream discovery primitive). When NOT to use: free-text similarity (use query_similar) or aggregate corpus-wide coupling stats (use `coupled-files` saved query).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "File path to classify. Must match a row in the File table (repo-relative, e.g. `src/cmd/mcp.rs` or `frontend/src/routes/reset-password.tsx`)."
                            }
                        },
                        "required": ["path"]
                    }
                },
                {
                    "name": "reingest",
                    "description": "Gap-009: re-run `doc-linter check` against the server's --root. Rebuilds the doc graph in a staging file and swaps it in (this server and any other reopen on their next call), optionally repopulates the vector index, and updates docs/sitemap.json. Use this whenever data on disk has changed (new docs, edited frontmatter, restored embeddings) and you want the live MCP queries to see the new state without a server restart.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "embeddings": {
                                "type": "boolean",
                                "description": "Mirror of CLI `--embeddings` / `--no-embeddings`. true runs the embedding pass over Doc / Entity / Section / Function / Type (unchanged text reuses cached vectors, so warm runs are cheap); false skips it but still restores cached vectors. Omitted: `.doc-lint.toml` `embeddings` decides (default false). The response echoes `embeddings` and `embeddings_source` (argument | config)."
                            },
                            "no_vale": {
                                "type": "boolean",
                                "description": "Mirror of CLI `--no-vale`. Skip the Vale vocabulary-closure integration. Default false (Vale runs if `vale_enabled = true` in config)."
                            },
                            "lint_code_comments": {
                                "type": "boolean",
                                "description": "Mirror of CLI `--lint-code-comments`. Walk `///` / `//!` doc-comments and run them through the same `TermIndex` used for markdown. Default false."
                            },
                            "rebuild": {
                                "type": "boolean",
                                "description": "Mirror of CLI `--rebuild`. Force a full re-ingest, ignoring the SCIP ingest cache. Default false (cache-aware)."
                            }
                        }
                    }
                }
            ]
        })
    }

    fn tools_call(&self, params: Value) -> std::result::Result<Value, McpError> {
        self.refresh_db_if_swapped();
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "tools/call: missing `name`".to_string(),
            })?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
        match name {
            "sql" => self.tool_sql(&arguments),
            "list_entities" => self.tool_list_entities(&arguments),
            "list_docs" => self.tool_list_docs(&arguments),
            "list_types" => self.tool_list_types(&arguments),
            "list_findings" => self.tool_list_findings(&arguments),
            "function_context" => self.tool_function_context(&arguments),
            "query_similar" => self.tool_query_similar(&arguments),
            "query_dead_code" => self.tool_query_dead_code(&arguments),
            "query_endpoints" => self.tool_query_endpoints(&arguments),
            "query_impact" => self.tool_query_impact(&arguments),
            "query_at" => self.tool_query_at(&arguments),
            "query_schema" => self.tool_query_schema(),
            "query_saved" => self.tool_query_saved(&arguments),
            "query_path" => self.tool_query_path(&arguments),
            "query_doc" => self.tool_query_doc(&arguments),
            "query_doc_neighbors" => self.tool_query_doc_neighbors(&arguments),
            "query_entity_neighbors" => self.tool_query_entity_neighbors(&arguments),
            "suggest_tags_for_doc" => self.tool_suggest_tags_for_doc(&arguments),
            "context_for" => self.tool_context_for(&arguments),
            "query_entity" => self.tool_query_entity(&arguments),
            "read_source" => self.tool_read_source(&arguments),
            "list_modules" => self.tool_list_modules(&arguments),
            "list_files" => self.tool_list_files(&arguments),
            "list_migrations" => self.tool_list_migrations(&arguments),
            "list_repos" => self.tool_list_repos(&arguments),
            "cluster" => self.tool_cluster(&arguments),
            "audit_doc_region" => self.tool_audit_doc_region(&arguments),
            "classify_file_coupling" => self.tool_classify_file_coupling(&arguments),
            "reingest" => self.tool_reingest(&arguments),
            "reload_self" => self.tool_reload_self(),
            "swap_worker" => self.tool_swap_worker(&arguments),
            other => Err(McpError {
                code: -32602,
                message: format!("tools/call: unknown tool `{other}`"),
            }),
        }
    }

    /// Read-only SQL on the graph. The engine
    /// itself refuses anything but one read-only SELECT / WITH statement,
    /// so no keyword sniffing is needed here.
    fn tool_sql(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let query = arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "sql: missing `query` string argument".to_string(),
            })?;
        let max_rows = nonneg_int_arg(arguments, "max_rows")
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_SQL_MAX_ROWS);
        let bindings: Vec<(&str, doc_linter::store::Value)> = arguments
            .get("params")
            .and_then(|v| v.as_object())
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| {
                        Some((
                            k.as_str(),
                            doc_linter::store::Value::Str(v.as_str()?.to_string()),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let result = self
            .db
            .borrow()
            .run_sql(query, bindings)
            .map_err(|e| McpError {
                code: -32602,
                message: format!("sql: {e:#}"),
            })?;
        Ok(tool_text_result(&cap_result(result, max_rows)))
    }

    fn tool_list_entities(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        reject_negative_int(arguments, "top")?;
        let mut rows = self
            .db
            .borrow()
            .ranked_entities()
            .map_err(|e| McpError::from_anyhow("list_entities", e))?;
        let total = rows.len();
        // Roadmap issue #35 v6/v7: optional `top` cap so a repo
        // with hundreds of entities doesn't ship the whole list
        // on every tool call. 0 (or absent) means "no cap".
        // Roadmap issue #213: pair with `offset` so an agent can
        // page through the tail beyond `top`.
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "entities": rows,
        })))
    }

    /// Roadmap issue #35 v11: `list_docs` MCP tool. The spec
    /// called for `list_docs` alongside `list_entities` /
    /// `list_endpoints`; this closes that gap. Wraps the existing
    /// `list_all_docs` query helper (already powering `query list`
    /// and the sitemap writer) and adds the same `top` cap +
    /// `total` / `returned` envelope every other list tool uses,
    /// so agents iterating over MCP get a uniform shape across
    /// list_*.
    fn tool_list_docs(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        reject_negative_int(arguments, "top")?;
        let mut rows = self
            .db
            .borrow()
            .list_all_docs()
            .map_err(|e| McpError::from_anyhow("list_docs", e))?;
        if let Some(attr) = arguments.get("attribute").and_then(|v| v.as_str()) {
            rows.retain(|d| d.attributes.iter().any(|a| a == attr));
        }
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "docs": rows,
        })))
    }

    /// Roadmap issue #35 v12: `list_types` MCP tool — Type rows from
    /// the dual-write Type node table introduced in #32. Same shape
    /// the `query types` CLI subcommand emits: `{kind?, crate?,
    /// substring?, types: [...]}` with an envelope. Optional filters
    /// (kind / crate / substring) mirror the CLI flags 1:1, plus
    /// the catalog-standard `top` cap.
    fn tool_list_types(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        reject_negative_int(arguments, "top")?;
        let mut sql = String::from(
            "SELECT t.symbol AS symbol, t.kind AS kind, t.crate AS crate, \
             t.file AS file, t.line AS line, t.language AS language FROM Type t",
        );
        let allowed_kinds = ["struct", "enum", "trait", "module", "type_alias"];
        let mut conditions: Vec<&str> = Vec::new();
        let mut bindings: Vec<(&str, doc_linter::store::Value)> = Vec::new();

        if let Some(kind) = arguments.get("kind").and_then(|v| v.as_str()) {
            if !allowed_kinds.contains(&kind) {
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "list_types: unknown kind '{kind}' (valid: {})",
                        allowed_kinds.join(" / ")
                    ),
                });
            }
            conditions.push("t.kind = $kind");
            bindings.push(("kind", doc_linter::store::Value::Str(kind.to_string())));
        }
        if let Some(cr) = arguments.get("crate").and_then(|v| v.as_str()) {
            conditions.push("t.crate = $crate");
            bindings.push(("crate", doc_linter::store::Value::Str(cr.to_string())));
        }
        if let Some(sub) = arguments.get("substring").and_then(|v| v.as_str()) {
            conditions.push("instr(t.symbol, $sub) > 0");
            bindings.push(("sub", doc_linter::store::Value::Str(sub.to_string())));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY t.symbol");

        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("list_types", e))?;
        let mut rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "types": rows,
        })))
    }

    /// Roadmap issue #35 v14 / #31: `list_findings` MCP tool —
    /// Finding rows from the table introduced in #31 (TODO /
    /// FIXME / XXX / HACK markers). Optional severity + kind
    /// filters mirror the schema columns 1:1. Default ordering
    /// is `severity DESC, file ASC, line ASC` so the warnings
    /// surface first when an agent's just asking "what's open?"
    fn tool_list_findings(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        reject_negative_int(arguments, "top")?;
        let mut sql = String::from(
            "SELECT f.id AS id, f.kind AS kind, f.file AS file, f.line AS line, \
             f.message AS message, f.severity AS severity FROM Finding f",
        );
        let mut conditions: Vec<&str> = Vec::new();
        let mut bindings: Vec<(&str, doc_linter::store::Value)> = Vec::new();
        let allowed_severities = ["info", "warning"];

        if let Some(sev) = arguments.get("severity").and_then(|v| v.as_str()) {
            if !allowed_severities.contains(&sev) {
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "list_findings: unknown severity '{sev}' (valid: {})",
                        allowed_severities.join(" / ")
                    ),
                });
            }
            conditions.push("f.severity = $severity");
            bindings.push(("severity", doc_linter::store::Value::Str(sev.to_string())));
        }
        if let Some(kind) = arguments.get("kind").and_then(|v| v.as_str()) {
            conditions.push("f.kind = $kind");
            bindings.push(("kind", doc_linter::store::Value::Str(kind.to_string())));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY f.severity DESC, f.file ASC, f.line ASC, f.id ASC");

        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("list_findings", e))?;
        let mut rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "findings": rows,
        })))
    }

    fn tool_function_context(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let symbol = arguments
            .get("symbol")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "function_context: missing `symbol` string argument".to_string(),
            })?;
        let sym = FunctionSymbol(symbol.to_string());
        // Roadmap issue #208: 404 semantics. `query_doc` and
        // `query_entity` raise -32602 on unknown id; matching the
        // same shape here so an agent doesn't have to remember
        // which tool returns null vs. errors.
        let ctx = self
            .db
            .borrow()
            .function_context(&sym)
            .map_err(|e| McpError::from_anyhow("function_context", e))?
            .ok_or_else(|| McpError {
                code: -32602,
                message: format!(
                    "function_context: unknown function symbol `{symbol}`. \
                     The Function table is populated by SCIP ingest; run \
                     `doc-linter check` if you expect this symbol to exist."
                ),
            })?;
        Ok(tool_text_result(&json!({ "function": ctx })))
    }

    fn tool_query_similar(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let text = arguments
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_similar: missing `text` string argument".to_string(),
            })?
            .to_string();
        let kind = arguments
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("doc")
            .to_string();
        let top = arguments
            .get("top")
            .and_then(serde_json::Value::as_u64)
            .map_or(5, |n| n as usize);
        let snippet_chars = arguments
            .get("snippet_chars")
            .and_then(serde_json::Value::as_u64)
            .map_or(160, |n| n as usize);
        let min_score = arguments
            .get("min_score")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(0.0);
        // Roadmap issue #28 v5 (v0.4.0): `backend` defaults to
        // "bm25" so existing MCP clients see no behaviour change;
        // an agent that explicitly passes `"backend": "embedding"`
        // gets cosine ranking. v5 also surfaces the option in the
        // tool schema (see `tools_list`).
        let backend = arguments
            .get("backend")
            .and_then(|v| v.as_str())
            .unwrap_or("bm25")
            .to_string();
        // Gap-010 phase 3: optional repo_id filter. Today only the
        // Doc corpus respects it (Doc.repo_id is the only repo-stamped
        // table); Entity / Function searches silently return all rows.
        let source_repo = arguments
            .get("source_repo")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let min_confidence = arguments
            .get("min_confidence")
            .and_then(|v| v.as_str())
            .unwrap_or("low")
            .to_string();
        // Per iter 242 ([[feedback_meta_doc_displaces_research]]):
        // tag-axis filters so the agent can split the research-doc
        // vs meta-doc BM25 streams at the retrieval primitive layer.
        let with_tag = arguments
            .get("with_tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let without_tag = arguments
            .get("without_tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let args = doc_linter::query::SimilarArgs {
            text,
            r#type: kind,
            top,
            snippet_chars,
            min_score,
            backend: backend.clone(),
            source_repo,
            min_confidence,
            with_tag,
            without_tag,
        };
        let result = doc_linter::query::similar_for_mcp(&**self.db.borrow(), args)
            .map_err(|e| McpError::from_anyhow("query_similar", e))?;
        // Roadmap issue #210: the embedding backend is ostensibly
        // available (the dispatcher would have bailed otherwise),
        // but the corpus may not have any populated embedding rows
        // — the caller forgot to run `doc-linter check --embeddings`
        // first. Today that came back as `{algorithm: "embedding",
        // corpus_size: 0, hits: []}`, indistinguishable from "no
        // semantically similar matches". Raise -32000 with a hint
        // so the agent can branch on the failure.
        if backend == "embedding" {
            let corpus_size = result
                .get("corpus_size")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if corpus_size == 0 {
                return Err(McpError {
                    code: -32000,
                    message: "query_similar: backend=embedding requested but no rows in the \
                         requested corpus have populated embeddings. Run \
                         `doc-linter check --embeddings` to populate the FLOAT[384] \
                         columns (the populate pass is opt-in because it's the \
                         dominant cost on a full rebuild). Fall back to \
                         backend=bm25 for a lexical ranking that's always available."
                        .to_string(),
                });
            }
        }
        Ok(tool_text_result(&result))
    }

    fn tool_query_dead_code(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        reject_negative_int(arguments, "top")?;
        let mut rows = self
            .db
            .borrow()
            .list_dead_code()
            .map_err(|e| McpError::from_anyhow("query_dead_code", e))?;
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        // `count` retained for back-compat (== `returned`); new
        // `total` reports the un-capped row count so the agent
        // can detect truncation without a second call.
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "count": rows.len(),
            "functions": rows,
        })))
    }

    fn tool_query_endpoints(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        reject_negative_int(arguments, "top")?;
        // Roadmap issue #207: validate `kind` against the EndpointKind
        // enum before dispatch. The schema description had always
        // claimed the enum but neither schema nor handler enforced
        // it — bogus values silently produced `{count: 0}`,
        // indistinguishable from "no endpoints of that kind".
        const ALLOWED_KINDS: &[&str] = &["axum", "clap", "mcp", "fastapi", "flask", "express"];
        let kind = arguments.get("kind").and_then(|v| v.as_str());
        if let Some(k) = kind {
            if !ALLOWED_KINDS.contains(&k) {
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "query_endpoints: unknown kind `{k}` (valid: {})",
                        ALLOWED_KINDS.join(" / ")
                    ),
                });
            }
        }
        let dark = arguments
            .get("dark")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let entity_id = arguments.get("entity_id").and_then(|v| v.as_str());
        let mut rows = self
            .db
            .borrow()
            .list_endpoints(kind, dark)
            .map_err(|e| McpError::from_anyhow("query_endpoints", e))?;
        // Roadmap issue #207 (cont.): `entity_id` filter. An agent
        // that just learned about an Entity via `list_entities`
        // can ask "which endpoints touch this one?" without
        // dropping to raw SQL.
        if let Some(want) = entity_id {
            rows.retain(|row| row.entities.iter().any(|e| e == want));
        }
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "count": rows.len(),
            "endpoints": rows,
        })))
    }

    fn tool_query_impact(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let symbol = arguments
            .get("symbol")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_impact: missing `symbol` string argument".to_string(),
            })?;
        reject_negative_int(arguments, "depth")?;
        let depth = arguments
            .get("depth")
            .and_then(serde_json::Value::as_u64)
            .map_or(3, |n| n as u32);
        // Roadmap issue #208: same 404 semantics as
        // `function_context` / `query_doc` / `query_entity`.
        let result = self
            .db
            .borrow()
            .query_impact(symbol, depth)
            .map_err(|e| McpError::from_anyhow("query_impact", e))?
            .ok_or_else(|| McpError {
                code: -32602,
                message: format!(
                    "query_impact: unknown function symbol `{symbol}`. \
                     Use `function_context` (or `sql` with an `instr()` \
                     match on Function.symbol) to discover available symbols."
                ),
            })?;
        Ok(tool_text_result(&json!({ "impact": result })))
    }

    fn tool_query_at(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let file = arguments
            .get("file")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_at: missing `file` string argument".to_string(),
            })?;
        // Roadmap issue #209: explicit `line` shape validation.
        // The schema description always claimed "1-based" but the
        // handler accepted `line: 0` (returning a partial response
        // with `function` missing) and accepted arbitrarily large
        // values (returning the nearest function past EOF). Both
        // are silent wrong-answer modes for an agent.
        let line_raw = arguments
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_at: missing `line` integer argument".to_string(),
            })?;
        if line_raw < 1 {
            return Err(McpError {
                code: -32602,
                message: "query_at: `line` must be >= 1 (the schema is 1-based — \
                          editor gutter numbering)."
                    .to_string(),
            });
        }
        let line = u32::try_from(line_raw).unwrap_or(u32::MAX);
        let result = self
            .db
            .borrow()
            .query_at(file, line)
            .map_err(|e| McpError::from_anyhow("query_at", e))?;
        // Past-EOF detection: when the File row is present and the
        // requested line exceeds File.loc, surface a `past_eof: true`
        // flag so the agent can branch. We keep the rest of the
        // payload (file / module rows are still meaningful) so the
        // caller doesn't have to re-query just to learn the file
        // exists.
        let file_loc = result.file.as_ref().map(|f| f.loc);
        let past_eof = file_loc.is_some_and(|loc| line > loc);
        // `line_in_function` distinguishes "we found a function that
        // starts at or before this line" (the existing heuristic)
        // from "the line is outside any indexed Function span". For
        // an editor at `file:line` with no symbol on the line, this
        // is the difference between "this is body code" and "this
        // is whitespace between symbols".
        let line_in_function = result.function.is_some() && !past_eof;
        let mut value = serde_json::to_value(&result)
            .map_err(|e| McpError::from_anyhow("query_at", anyhow::anyhow!(e)))?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("past_eof".to_string(), json!(past_eof));
            obj.insert("line_in_function".to_string(), json!(line_in_function));
        }
        Ok(tool_text_result(&value))
    }

    fn tool_query_schema(&self) -> std::result::Result<Value, McpError> {
        // Roadmap issue #215: enriched cold-start endpoint. The
        // pre-#215 shape was bare `CALL show_tables() RETURN *`,
        // which gave agents table names + row counts but not the
        // columns. To author a query the agent had to probe each
        // table one row at a time. Now we pre-process: per-table column metadata
        // (name, dtype, primary-key flag), node/rel table grouping,
        // FROM/TO endpoints on rel tables, and per-table row counts.
        let tables = self
            .db
            .borrow()
            .schema_tables()
            .map_err(|e| McpError::from_anyhow("query_schema", e))?;
        let mut node_tables: Vec<Value> = Vec::new();
        let mut rel_tables: Vec<Value> = Vec::new();
        for t in &tables {
            let properties: Vec<Value> = t
                .columns
                .iter()
                .map(|c| {
                    json!({
                        "name": c.name,
                        "dtype": c.ty,
                        "is_primary_key": c.pk,
                        "default": c.default,
                    })
                })
                .collect();
            if t.is_rel {
                rel_tables.push(json!({
                    "name": t.name,
                    "from": t.from,
                    "to": t.to,
                    "row_count": t.row_count,
                    "properties": properties,
                }));
            } else {
                node_tables.push(json!({
                    "name": t.name,
                    "row_count": t.row_count,
                    "properties": properties,
                }));
            }
        }
        // The legacy `tables` field: the same two columns the old engine's
        // `show_tables()` returned.
        let tables_raw = {
            let rows: Vec<Value> = tables
                .iter()
                .map(|t| json!({"name": t.name, "type": if t.is_rel { "REL" } else { "NODE" }}))
                .collect();
            json!({"columns": ["name", "type"], "row_count": rows.len(), "rows": rows})
        };
        node_tables.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });
        rel_tables.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or("")
                .cmp(b["name"].as_str().unwrap_or(""))
        });

        // Per iter 119: surface corpus freshness so the agent
        // can detect rebuild-lag-vs-self-dogfood per
        // user-probe-022. MAX(Doc.updated) reflects the most
        // recent frontmatter update; MAX(File.last_touched)
        // reflects the most recent git mtime ingested. The
        // agent compares against the user's current git HEAD
        // (which it sees through other channels) to assess
        // whether the corpus is stale relative to the source.
        let max_doc_updated =
            self.scalar_string("SELECT max(updated) AS v FROM Doc WHERE updated IS NOT NULL");
        let max_file_touched = self.scalar_string(
            "SELECT max(last_touched) AS v FROM File WHERE last_touched IS NOT NULL",
        );
        let freshness =
            build_freshness_payload(max_doc_updated.as_deref(), max_file_touched.as_deref());

        // Per iter 124 (interrogation-025 Finding C + D
        // discoverability gap): expose at cold-start which
        // retrieval backends the running binary actually
        // supports. `bm25` is always available; `embedding`
        // depends on (a) `--features embeddings` at build
        // time, (b) DOC_LINTER_EMBED_MODEL set and pointing
        // to a usable ONNX model at process-start. The
        // proxy is `default_embedder().is_available()` —
        // returns false for the NoOpEmbedder (no feature or
        // missing env) and true for OnnxEmbedder.
        let embedding_available = doc_linter::embeddings::default_embedder().is_available();
        let retrieval_backends = build_retrieval_backends_payload(embedding_available);

        Ok(tool_text_result(&json!({
            "node_tables": node_tables,
            "rel_tables": rel_tables,
            "freshness": freshness,
            "retrieval_backends": retrieval_backends,
            // The legacy `tables` field stays for back-compat
            // callers (the CLI / earlier MCP versions); agents
            // should prefer the grouped fields.
            "tables": tables_raw,
        })))
    }

    /// Run a single-row, single-scalar query and return the
    /// `v` column as a string. Used by the corpus-freshness
    /// helpers; tolerant of empty corpora (returns None on no
    /// rows OR NULL value).
    fn scalar_string(&self, sql: &str) -> Option<String> {
        let result = self.q(sql, vec![]).ok()?;
        let rows = result.get("rows")?.as_array()?;
        let first = rows.first()?;
        first.get("v")?.as_str().map(str::to_string)
    }

    fn tool_query_saved(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let name = arguments.get("name").and_then(|v| v.as_str());
        let Some(name) = name else {
            // Hide compile-time queries that need rows in an empty table
            // (269 code-graph queries on a docs-only corpus). Corpus-local
            // queries are the author's own and always listed.
            let mut listing = doc_linter::store::query::list_saved_queries_with_root(&self.root);
            let needs: Vec<_> = listing.queries.iter().map(|q| q.needs.clone()).collect();
            let empty = self.empty_tables(needs.iter().flatten().map(String::as_str));
            let total = listing.queries.len();
            let mut needs = needs.into_iter();
            listing.queries.retain(|q| {
                let needs = needs.next().unwrap_or_default();
                q.origin == "corpus-local" || needs.iter().all(|l| !empty.contains(l))
            });
            listing.count = listing.queries.len();
            let mut empty: Vec<String> = empty.into_iter().collect();
            empty.sort();
            return Ok(tool_text_result(&json!({
                "count": listing.count,
                "hidden": total - listing.count,
                "hidden_reason": "needs rows in an empty table (still runnable by name)",
                "empty_tables": empty,
                "queries": listing.queries,
            })));
        };
        // Roadmap issue #216: `describe: true` returns the catalog
        // entry for `name` without running the query, so an agent
        // can discover the param schema before issuing the call.
        let describe = arguments
            .get("describe")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        // Merge compile-time + runtime `<root>/saved-queries/*.toml`
        // so newly authored queries resolve without a /mcp reconnect.
        let catalog = doc_linter::store::query::merged_catalog(&self.root);
        let Some(entry) = catalog.iter().find(|q| q.name == name) else {
            let available: Vec<String> = catalog.iter().map(|q| q.name.clone()).collect();
            let suggestion = closest_saved_query_suggestion(name, &available);
            return Err(McpError {
                code: -32602,
                message: format!(
                    "query_saved: unknown name `{name}`.{suggestion} Available: {}",
                    available.join(", ")
                ),
            });
        };
        if describe {
            return Ok(tool_text_result(&json!({
                "name": entry.name,
                "description": entry.description,
                "params": entry.params,
                "sql": entry
                    .sql
                    .clone()
                    .or_else(|| doc_linter::store_sqlite::saved::builtin_sql(&entry.name).map(str::to_string)),
            })));
        }
        // Build the param map from the optional `params` object.
        let mut param_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Some(obj) = arguments.get("params").and_then(|v| v.as_object()) {
            for (k, v) in obj {
                let v_str = match v {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    other => other.to_string(),
                };
                param_map.insert(k.clone(), v_str);
            }
        }
        // `name=default` params are optional; `run_saved_query_with_root`
        // fills the default in.
        for declared in entry.params.iter().filter(|p| !p.contains('=')) {
            if !param_map.contains_key(declared) {
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "query_saved {name}: missing required param `{declared}`. \
                         Declared params: {}",
                        entry.params.join(", ")
                    ),
                });
            }
        }
        let result = self
            .db
            .borrow()
            .run_saved_query(Some(&self.root), name, &param_map)
            .map_err(|e| McpError::from_anyhow("query_saved", e))?;
        Ok(tool_text_result(&result))
    }

    /// Roadmap issue #35 v13: `query_path` MCP tool — wraps the
    /// existing `shortest_path` helper that powers `query path`.
    /// Returns the same `PathStep` shape the CLI emits: a list of
    /// `{id, title, via_line?, via_edge_type?}` where the first
    /// entry's `via_*` are absent (it's the source). Empty list
    /// means no path within `max_hops`.
    ///
    /// Closes the spec gap on #35 — the issue's tool catalog listed
    /// `query_path` (shortest path between two doc ids) and the
    /// underlying helper has been in tree since the recursive-CTE BFS
    /// landed; this just exposes it via MCP.
    fn tool_query_path(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        use doc_linter::ids::DocId;
        let from = arguments
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_path: missing `from` string argument".to_string(),
            })?;
        let to = arguments
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_path: missing `to` string argument".to_string(),
            })?;
        reject_negative_int(arguments, "max_hops")?;
        let max_hops = arguments
            .get("max_hops")
            .and_then(serde_json::Value::as_u64)
            .map_or(6_u32, |n| n.min(u64::from(u32::MAX)) as u32)
            .max(1);
        let from_id = DocId(from.to_string());
        let to_id = DocId(to.to_string());
        let steps = self
            .db
            .borrow()
            .shortest_path(&from_id, &to_id, None, max_hops)
            .map_err(|e| McpError::from_anyhow("query_path", e))?;
        Ok(tool_text_result(&json!({
            "from": from,
            "to": to,
            "max_hops": max_hops,
            "steps": steps,
            "found": !steps.is_empty(),
        })))
    }

    /// Roadmap issue #35 v15 (v0.3.0): MCP wrapper around the
    /// existing `query context <doc-id>` CLI handler. Returns the
    /// Doc node plus its outbound + inbound edges in one call —
    /// the agent-facing equivalent of clicking through frontmatter
    /// links + backlinks in the vault.
    fn tool_query_doc(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_doc: missing `id` string argument".to_string(),
            })?;
        let doc_id = DocId::from(id);
        let node = self
            .db
            .borrow()
            .get_doc(&doc_id)
            .map_err(|e| McpError::from_anyhow("query_doc", e))?
            .ok_or_else(|| McpError {
                code: -32602,
                message: format!("query_doc: unknown doc id `{id}`"),
            })?;
        let outbound = self
            .db
            .borrow()
            .outbound(&doc_id)
            .map_err(|e| McpError::from_anyhow("query_doc", e))?;
        let mut inbound = self
            .db
            .borrow()
            .inbound(&doc_id)
            .map_err(|e| McpError::from_anyhow("query_doc", e))?;
        // Per iter 104: when the queried Doc is an ontology-entity
        // (id starts with `entity-`), enumerate COVERS-inbound from
        // the corresponding Entity node and merge into the inbound
        // list. Closes gap-query-doc-covers-inbound-missing surfaced
        // by user-probe-018. Non-entity docs get an empty merge → no
        // behaviour change.
        let mut covers_in = self
            .db
            .borrow()
            .covers_inbound_for_entity_doc(&doc_id)
            .map_err(|e| McpError::from_anyhow("query_doc", e))?;
        inbound.append(&mut covers_in);
        Ok(tool_text_result(&json!({
            "doc": node,
            "outbound": outbound,
            "inbound": inbound,
        })))
    }

    /// Per iter 99: recommend docs that share Entity coverage with
    /// the target. Operationalises [[research-focus]]'s collaborative-
    /// filtering framing (project × API → doc × Entity) on doc-linter's
    /// existing COVERS edges. The shared-entity LIST is the
    /// interpretable assignment per [[research-isonet]]'s principle —
    /// the agent can name which Entities the overlap is built on, not
    /// just a scalar similarity. Default top=10, hard cap 50 per ACI
    /// bounded-response principle.
    fn tool_query_doc_neighbors(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_doc_neighbors: missing `id` string argument".to_string(),
            })?;
        let by = arguments
            .get("by")
            .and_then(|v| v.as_str())
            .unwrap_or("covers");
        let top_raw = top_arg(arguments);
        let top = if top_raw == 0 { 10 } else { top_raw.min(50) };
        if by == "all" {
            // Multi-view fusion per [[research-mega]]. Run BOTH single
            // axes, merge in Rust by neighbor id, return per-view
            // scores alongside the fused total — exactly the
            // interpretable-per-view-score shape MEGA's body proposes.
            // Each underlying axis returns up to `top * 2` candidates
            // so the merged top-`top` has room for fusion-promotion.
            let inner_top = top.saturating_mul(2).min(50);
            let covers_rows = self
                .q(
                    &doc_neighbors_sql("covers", inner_top).unwrap_or_default(),
                    vec![("id", doc_linter::store::Value::Str(id.to_string()))],
                )
                .map_err(|e| McpError::from_anyhow("query_doc_neighbors", e))?
                .get("rows")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let wikilink_rows = self
                .q(
                    &doc_neighbors_sql("wikilink", inner_top).unwrap_or_default(),
                    vec![("id", doc_linter::store::Value::Str(id.to_string()))],
                )
                .map_err(|e| McpError::from_anyhow("query_doc_neighbors", e))?
                .get("rows")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let merged = merge_neighbor_views(&covers_rows, &wikilink_rows, top);
            let total = merged.len();
            return Ok(tool_text_result(&json!({
                "target_id": id,
                "by": "all",
                "neighbors": merged,
                "total": total,
            })));
        }
        let sql = doc_neighbors_sql(by, top).ok_or_else(|| McpError {
            code: -32602,
            message: format!(
                "query_doc_neighbors: unknown `by` value '{by}' (valid: covers, wikilink, all)"
            ),
        })?;
        let bindings = vec![("id", doc_linter::store::Value::Str(id.to_string()))];
        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("query_doc_neighbors", e))?;
        let rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        Ok(tool_text_result(&json!({
            "target_id": id,
            "by": by,
            "neighbors": rows,
            "total": total,
        })))
    }

    /// Roadmap issue #35 v15 (v0.3.0): MCP wrapper around the
    /// existing `query subgraph --entity <id>` CLI handler. Returns
    /// the ego-graph rooted at one Entity — the structural reach
    /// that complements the narrative coverage `query_doc` surfaces.
    /// Per iter 108: entity-axis mirror of query_doc_neighbors.
    /// Operationalises the entities-by-coverage-density (iter 100)
    /// + FOCUS framing (iter 93) on the entity side — given an
    /// Entity id, return co-covered or RELATES_TO-connected entity
    /// neighbors. Interpretable per-row: co-cover returns the
    /// shared_docs list, related returns the relation_type.
    fn tool_query_entity_neighbors(
        &self,
        arguments: &Value,
    ) -> std::result::Result<Value, McpError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_entity_neighbors: missing `id` string argument".to_string(),
            })?;
        let by = arguments
            .get("by")
            .and_then(|v| v.as_str())
            .unwrap_or("co-cover");
        let top_raw = top_arg(arguments);
        let top = if top_raw == 0 { 10 } else { top_raw.min(50) };
        let sql = entity_neighbors_sql(by, top).ok_or_else(|| McpError {
            code: -32602,
            message: format!(
                "query_entity_neighbors: unknown `by` value '{by}' (valid: co-cover, related)"
            ),
        })?;
        let bindings = vec![("id", doc_linter::store::Value::Str(id.to_string()))];
        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("query_entity_neighbors", e))?;
        let rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        Ok(tool_text_result(&json!({
            "target_id": id,
            "by": by,
            "neighbors": rows,
            "total": total,
        })))
    }

    /// Per iter 112: close gap-tag-axis-loose-end-detection (filed
    /// iter 111 interrogation-022). Read the target Doc's summary +
    /// current tags, enumerate all known tags in the corpus, then
    /// return tags the summary mentions but the doc doesn't carry.
    /// Mechanical — no LLM. Mirrors the iter 101 loose-end finding
    /// applied to the tag axis.
    fn tool_suggest_tags_for_doc(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let doc_id = arguments
            .get("doc_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "suggest_tags_for_doc: missing `doc_id` string argument".to_string(),
            })?;
        // Fetch the target doc's summary + current tags.
        let bindings = vec![("id", doc_linter::store::Value::Str(doc_id.to_string()))];
        let doc_result = self
            .q(
                &format!(
                    "SELECT d.summary AS summary, {} AS tags FROM Doc d WHERE d.id = $id",
                    list_sql("doc_tags", "doc_id", "d.id")
                ),
                bindings,
            )
            .map_err(|e| McpError::from_anyhow("suggest_tags_for_doc: doc lookup", e))?;
        let rows = doc_result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let first = rows.first().ok_or_else(|| McpError {
            code: -32602,
            message: format!("suggest_tags_for_doc: unknown doc id `{doc_id}`"),
        })?;
        let summary = first
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let current_tags: Vec<String> = first
            .get("tags")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        // Enumerate all known tags in the corpus.
        let all_tags_result = self
            .q(
                "SELECT DISTINCT value AS tag FROM doc_tags WHERE value IS NOT NULL ORDER BY value",
                vec![],
            )
            .map_err(|e| McpError::from_anyhow("suggest_tags_for_doc: tag enumeration", e))?;
        let known_tags: Vec<String> = all_tags_result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|r| r.get("tag").and_then(|v| v.as_str()).map(str::to_string))
            .collect();
        let suggested = suggest_tags(&summary, &current_tags, &known_tags);
        let family_suggestions = suggest_family_tags(&current_tags, &known_tags);
        Ok(tool_text_result(&json!({
            "doc_id": doc_id,
            "summary": summary,
            "current_tags": current_tags,
            "suggested_tags": suggested,
            "family_suggestions": family_suggestions,
            "total_known_tags": known_tags.len(),
        })))
    }

    /// Per iter 117: close gap-007's polymorphic-dispatch +
    /// uniform-envelope sub-gaps. One MCP call yields a full
    /// grounding payload for any node id (Doc / Entity / Function)
    /// in a single uniform `{centre: Node, neighbors: [{node, edge}]}`
    /// shape — the agent no longer has to call query_doc /
    /// query_entity / function_context based on per-kind metadata
    /// AND no longer has to chase 1+N round-trips to populate
    /// neighbor summaries.
    fn tool_context_for(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "context_for: missing `id` string argument".to_string(),
            })?;
        reject_negative_int(arguments, "top")?;
        let top_raw = top_arg(arguments);
        let top = if top_raw == 0 { 20 } else { top_raw.min(50) };

        let kind = detect_node_kind(&**self.db.borrow(), id)
            .map_err(|e| McpError::from_anyhow("context_for: detect kind", e))?;
        let payload = match kind.as_deref() {
            Some("doc") => context_for_doc_payload(&**self.db.borrow(), id, top)
                .map_err(|e| McpError::from_anyhow("context_for: doc payload", e))?,
            Some("entity") => context_for_entity_payload(&**self.db.borrow(), id, top)
                .map_err(|e| McpError::from_anyhow("context_for: entity payload", e))?,
            Some("function") => context_for_function_payload(&**self.db.borrow(), id, top)
                .map_err(|e| McpError::from_anyhow("context_for: function payload", e))?,
            _ => {
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "context_for: unknown id `{id}` — not found in Doc, Entity, or Function tables. \
                         Run list_docs / list_entities / list_files to discover ids."
                    ),
                });
            }
        };
        Ok(tool_text_result(&payload))
    }

    fn tool_query_entity(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let id = arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "query_entity: missing `id` string argument".to_string(),
            })?;
        reject_negative_int(arguments, "depth")?;
        let depth = arguments
            .get("depth")
            .and_then(serde_json::Value::as_u64)
            .map_or(2_u32, |n| u32::try_from(n).unwrap_or(u32::MAX));
        let _ = self
            .db
            .borrow()
            .get_entity(&EntityId::from(id))
            .map_err(|e| McpError::from_anyhow("query_entity", e))?
            .ok_or_else(|| McpError {
                code: -32602,
                message: format!(
                    "query_entity: unknown entity id `{id}`. \
                     Run `list_entities` to see available ids."
                ),
            })?;
        // Roadmap issue #205: switch to the heterogeneous ego-graph
        // helper. The previous Entity-only walker silently dropped
        // every Doc / Function / Endpoint neighbour that the tool's
        // `tools/list` description had advertised — agents got an
        // empty `{nodes: [centre], edges: []}` for entities whose
        // structural reach lives entirely on the cross-bucket edges
        // (COVERS / FUNCTION_BELONGS_TO / ENDPOINT_TOUCHES_ENTITY).
        let ego = self
            .db
            .borrow()
            .query_entity_ego_graph(id, depth)
            .map_err(|e| McpError::from_anyhow("query_entity", e))?
            .ok_or_else(|| McpError {
                code: -32602,
                message: format!("query_entity: subgraph walk found no entity for id `{id}`"),
            })?;
        Ok(tool_text_result(&json!({ "entity": ego })))
    }

    /// Roadmap issue #211: `read_source` MCP tool. The pre-existing
    /// `query_at` returned graph context (function / module /
    /// entities / covering docs) but never the source bytes
    /// themselves — agents that wanted to see the code had to drop
    /// out of the MCP surface to Bash. This tool keeps the agent on
    /// the typed surface by returning the raw text window around an
    /// optional centre line.
    ///
    /// Safety: every path goes through [`resolve_repo_path`] before
    /// any I/O. The guard rejects absolute paths, anything
    /// containing a `..` component, and any resolved path that
    /// escapes [`Self::root`]. The output byte budget caps total
    /// returned text at `READ_SOURCE_MAX_BYTES`.
    fn tool_read_source(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        const DEFAULT_CONTEXT: u32 = 30;
        const MAX_CONTEXT: u32 = 200;
        const READ_SOURCE_MAX_BYTES: usize = 256 * 1024;

        let rel_path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "read_source: missing `path` string argument".to_string(),
            })?;
        let line = arguments
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX));
        if line == Some(0) {
            return Err(McpError {
                code: -32602,
                message: "read_source: `line` must be >= 1 (1-based)".to_string(),
            });
        }
        let context_lines = arguments
            .get("context_lines")
            .and_then(serde_json::Value::as_u64)
            .map_or(DEFAULT_CONTEXT, |n| u32::try_from(n).unwrap_or(MAX_CONTEXT))
            .clamp(1, MAX_CONTEXT);

        let source_roots = doc_linter::config::LintConfig::load(&self.root.join(".doc-lint.toml"))
            .map(|c| c.code_source_roots)
            .unwrap_or_default();
        let resolved =
            resolve_repo_path(&self.root, &source_roots, rel_path).map_err(|msg| McpError {
                code: -32602,
                message: format!("read_source: {msg}"),
            })?;
        // Cap the *file read* itself, not just the output. Without
        // this an agent could ask for a multi-gigabyte file and the
        // `read_to_string` call would OOM the process before the
        // per-line `READ_SOURCE_MAX_BYTES` cap below ever ran.
        const READ_SOURCE_MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
        let metadata = std::fs::metadata(&resolved).map_err(|e| McpError {
            code: -32602,
            message: format!("read_source: stat `{rel_path}`: {e}"),
        })?;
        if !metadata.is_file() {
            return Err(McpError {
                code: -32602,
                message: format!("read_source: `{rel_path}` is not a regular file"),
            });
        }
        if metadata.len() > READ_SOURCE_MAX_FILE_BYTES {
            return Err(McpError {
                code: -32602,
                message: format!(
                    "read_source: `{rel_path}` is {} bytes; refusing to load \
                     a file larger than {READ_SOURCE_MAX_FILE_BYTES} bytes. \
                     Use `query_at` for graph context on a specific line range, \
                     or shell out for whole-file reads of this scale.",
                    metadata.len()
                ),
            });
        }
        let text = std::fs::read_to_string(&resolved).map_err(|e| McpError {
            code: -32602,
            message: format!("read_source: read `{rel_path}`: {e}"),
        })?;

        let lines: Vec<&str> = text.split_inclusive('\n').collect();
        let total_lines = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        let centre = line.unwrap_or(1).min(total_lines.max(1));
        // 1-based inclusive window. Saturating sub so a centre near
        // the top doesn't underflow; min so a centre near the
        // bottom doesn't overshoot.
        let start = centre.saturating_sub(context_lines).max(1);
        let end = centre.saturating_add(context_lines).min(total_lines);

        let mut content = String::new();
        let mut count = 0_u32;
        for (idx, raw) in lines.iter().enumerate() {
            let line_no = u32::try_from(idx + 1).unwrap_or(u32::MAX);
            if line_no < start {
                continue;
            }
            if line_no > end {
                break;
            }
            if content.len() + raw.len() > READ_SOURCE_MAX_BYTES {
                // Stop adding lines once we'd blow the budget. The
                // caller can re-query with a narrower window.
                break;
            }
            content.push_str(raw);
            count += 1;
        }
        let truncated = count < end.saturating_sub(start).saturating_add(1);
        let end_actual = start.saturating_add(count).saturating_sub(1).max(start);

        Ok(tool_text_result(&json!({
            "path": rel_path,
            "start_line": start,
            "end_line": end_actual,
            "line_count": count,
            "total_lines": total_lines,
            "truncated": truncated,
            "content": content,
        })))
    }

    /// Roadmap issue #212: list rows from the Module node table.
    /// Optional `kind` / `substring` filters; standard pagination
    /// envelope. Returned columns mirror the table schema.
    fn tool_list_modules(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let mut sql = String::from(
            "SELECT m.id AS id, m.kind AS kind, m.path AS path, m.name AS name FROM Module m",
        );
        let mut conditions: Vec<&str> = Vec::new();
        let mut bindings: Vec<(&str, doc_linter::store::Value)> = Vec::new();
        if let Some(k) = arguments.get("kind").and_then(|v| v.as_str()) {
            conditions.push("m.kind = $kind");
            bindings.push(("kind", doc_linter::store::Value::Str(k.to_string())));
        }
        if let Some(sub) = arguments.get("substring").and_then(|v| v.as_str()) {
            conditions.push("instr(m.id, $sub) > 0");
            bindings.push(("sub", doc_linter::store::Value::Str(sub.to_string())));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY m.id");
        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("list_modules", e))?;
        let mut rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "modules": rows,
        })))
    }

    /// Roadmap issue #212: list rows from the File node table.
    /// Optional `language` / `substring` filters; standard
    /// pagination envelope.
    fn tool_list_files(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let mut sql = String::from(
            "SELECT f.path AS path, f.language AS language, f.loc AS loc, \
             f.last_touched AS last_touched FROM File f",
        );
        let mut conditions: Vec<&str> = Vec::new();
        let mut bindings: Vec<(&str, doc_linter::store::Value)> = Vec::new();
        if let Some(lang) = arguments.get("language").and_then(|v| v.as_str()) {
            conditions.push("f.language = $language");
            bindings.push(("language", doc_linter::store::Value::Str(lang.to_string())));
        }
        if let Some(sub) = arguments.get("substring").and_then(|v| v.as_str()) {
            conditions.push("instr(f.path, $sub) > 0");
            bindings.push(("sub", doc_linter::store::Value::Str(sub.to_string())));
        }
        if !conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conditions.join(" AND "));
        }
        sql.push_str(" ORDER BY f.path");
        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("list_files", e))?;
        let mut rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "files": rows,
        })))
    }

    /// Roadmap issue #212: list rows from the Migration node table
    /// (ontology migration history). Optional `since` lower-bounds
    /// `to_version`. Small table by design but the envelope stays
    /// uniform.
    fn tool_list_migrations(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let mut sql = String::from(
            "SELECT m.id AS id, m.from_version AS from_version, m.to_version AS to_version, \
             m.applied AS applied, m.applied_at AS applied_at, m.title AS title, \
             m.summary AS summary FROM Migration m",
        );
        let mut bindings: Vec<(&str, doc_linter::store::Value)> = Vec::new();
        if let Some(since) = arguments.get("since").and_then(serde_json::Value::as_i64) {
            sql.push_str(" WHERE m.to_version >= $since");
            bindings.push(("since", doc_linter::store::Value::Int(since)));
        }
        sql.push_str(" ORDER BY m.from_version ASC, m.id ASC");
        let result = self
            .q(&sql, bindings)
            .map_err(|e| McpError::from_anyhow("list_migrations", e))?;
        let mut rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        // SQLite stores `applied` as 0/1; the wire shape is a boolean.
        for r in &mut rows {
            if let Some(n) = r.get("applied").and_then(serde_json::Value::as_i64) {
                r["applied"] = Value::Bool(n != 0);
            }
        }
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "migrations": rows,
        })))
    }

    /// Gap-010 phase 1: enumerate `Repo` rows (one per ingested
    /// `--root` plus each `cross_repo_roots` entry). The companion
    /// `RepoMeta` singleton is still queryable directly with sql
    /// — it carries the `domain` / `problem_statement` config tuple
    /// that's separate from per-repo identity.
    fn tool_list_repos(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let result = self
            .q(
                "SELECT r.id AS id, r.root_path AS root_path, r.name AS name \
                 FROM Repo r ORDER BY r.id",
                vec![],
            )
            .map_err(|e| McpError::from_anyhow("list_repos", e))?;
        let mut rows = result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let total = rows.len();
        let offset = offset_arg(arguments)?;
        let top = top_arg(arguments);
        paginate(&mut rows, offset, top);
        Ok(tool_text_result(&json!({
            "total": total,
            "offset": offset,
            "returned": rows.len(),
            "repos": rows,
        })))
    }

    fn tool_cluster(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let top_n = arguments
            .get("top_n")
            .and_then(serde_json::Value::as_u64)
            .map_or(10, |n| n as usize);
        let order_by = arguments
            .get("order_by")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let algorithm = arguments
            .get("algorithm")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let min_members = arguments
            .get("min_members")
            .and_then(serde_json::Value::as_u64)
            .map_or(2, |n| n as usize);
        let seed_symbol = arguments
            .get("seed_symbol")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let top_members = arguments
            .get("top_members")
            .and_then(serde_json::Value::as_u64)
            .map_or(0, |n| n as usize);
        let resolution = arguments
            .get("resolution")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(1.0);
        let result = super::cluster::cluster_report_json(
            &self.root,
            top_n,
            order_by,
            algorithm,
            min_members,
            seed_symbol,
            top_members,
            resolution,
        )
        .map_err(|e| McpError::from_anyhow("cluster", e))?;
        Ok(tool_text_result(&result))
    }

    /// v1 MVP `audit_doc_region`: thin facade over `query_doc` +
    /// `query_similar(source_repo=…)`. Returns the target doc's
    /// frontmatter (so the caller can self-check it loaded the right
    /// row) plus the top-N trusted-corpus hits ranked by similarity
    /// to the target's summary. The caller (an LLM agent) supplies
    /// the actual audit reasoning — this tool only supplies the
    /// grounding.
    fn tool_audit_doc_region(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let doc_path = arguments
            .get("doc_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "audit_doc_region: missing `doc_path` string argument".to_string(),
            })?
            .to_string();
        let against_input = arguments
            .get("against")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "audit_doc_region: missing `against` repo_id string argument".to_string(),
            })?
            .to_string();
        let top = arguments
            .get("top")
            .and_then(serde_json::Value::as_u64)
            .map_or(5, |n| n as usize);
        let backend = arguments
            .get("backend")
            .and_then(|v| v.as_str())
            .unwrap_or("bm25")
            .to_string();
        // Per iter 242 ([[feedback_meta_doc_displaces_research]]):
        // optional tag-axis filters so an agent auditing a draft
        // can ask "compare against research-only docs" / "exclude
        // interrogations" in the trusted corpus.
        let with_tag = arguments
            .get("with_tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let without_tag = arguments
            .get("without_tag")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Resolve `against` against the Repo table by id / name /
        // root_path so a slug like "doc-linter-design" or the
        // basename of the absolute root_path both work — not just
        // the canonical id. Surfaces -32602 with the list of valid
        // ids when the input matches nothing or is ambiguous, so
        // calling agents don't silently get a zero-hit response
        // from a typo. Closes interrogation-001 gap-A.
        let against = {
            let resolve = self
                .q(
                    "SELECT r.id AS id, r.name AS name FROM Repo r \
                 WHERE r.id = $key OR r.name = $key OR r.root_path = $key \
                 ORDER BY r.id LIMIT 2",
                    vec![("key", doc_linter::store::Value::Str(against_input.clone()))],
                )
                .map_err(|e| McpError::from_anyhow("audit_doc_region: resolve against", e))?;
            let matches = resolve
                .get("rows")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if matches.is_empty() {
                let known = self
                    .q("SELECT id FROM Repo ORDER BY id LIMIT 10", vec![])
                    .ok()
                    .and_then(|v| v.get("rows").cloned())
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "audit_doc_region: unknown `against` repo identifier `{against_input}`. \
                         Valid ids: [{known}]. Call `list_repos` for the full list."
                    ),
                });
            }
            if matches.len() > 1 {
                let ambig = matches
                    .iter()
                    .filter_map(|r| r.get("id").and_then(|v| v.as_str()).map(String::from))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(McpError {
                    code: -32602,
                    message: format!(
                        "audit_doc_region: `against` identifier `{against_input}` is \
                         ambiguous — matches multiple Repo rows: [{ambig}]. \
                         Pass the canonical id from `list_repos`."
                    ),
                });
            }
            matches[0]
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or(&against_input)
                .to_string()
        };

        // Resolve `doc_path` → Doc row. Try as path first, then as id.
        // The path lookup uses an exact match — callers typically
        // hand in the canonical repo-relative path the lint output
        // displays.
        let resolve = self
            .q(
                &format!(
                    "SELECT d.id AS id, d.path AS path, d.role AS role, d.kind AS kind, \
                 d.title AS title, d.summary AS summary, d.status AS status, \
                 d.updated AS updated, {} AS tags, {} AS covers \
                 FROM Doc d WHERE d.path = $key OR d.id = $key ORDER BY d.id LIMIT 1",
                    list_sql("doc_tags", "doc_id", "d.id"),
                    list_sql("doc_covers", "doc_id", "d.id")
                ),
                vec![("key", doc_linter::store::Value::Str(doc_path.clone()))],
            )
            .map_err(|e| McpError::from_anyhow("audit_doc_region: resolve target", e))?;
        let rows = resolve
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let target = rows.first().cloned().ok_or_else(|| {
            // Per iter 126 (user-probe-023's named workflow gap
            // `gap-audit-doc-region-auto-reingest-on-fresh-target`):
            // when the path-looking input matches an existing file
            // on disk, the agent's natural recovery is to call
            // reingest() — the file was authored AFTER the last
            // ingest pass. Adding the hint here saves the agent a
            // round-trip through list_docs that wouldn't have
            // surfaced the on-disk-but-not-ingested case anyway.
            let on_disk = looks_like_doc_path(&doc_path) && self.root.join(&doc_path).is_file();
            let recovery_hint = if on_disk {
                " — but the file EXISTS on disk at this path. \
                 The graph is stale; call `reingest()` to pick up \
                 newly-authored drafts, then retry."
            } else {
                " Use `list_docs` to enumerate."
            };
            McpError {
                code: -32602,
                message: format!(
                    "audit_doc_region: no Doc found with path or id `{doc_path}`.{recovery_hint}"
                ),
            }
        })?;
        let summary = target
            .get("summary")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if summary.is_empty() {
            return Err(McpError {
                code: -32602,
                message: format!(
                    "audit_doc_region: Doc `{doc_path}` has no `summary` frontmatter — \
                     nothing to rank against. Add a summary line to the doc and reingest."
                ),
            });
        }

        // Rank the trusted corpus against the target's summary.
        // audit_doc_region runs unconditional retrieval — the caller
        // wants the top-K regardless of confidence; downstream agents
        // can re-gate using the per-hit confidence field if needed.
        let args = doc_linter::query::SimilarArgs {
            text: summary,
            r#type: "doc".to_string(),
            top,
            snippet_chars: 200,
            min_score: 0.0,
            backend,
            source_repo: Some(against),
            min_confidence: "low".to_string(),
            with_tag,
            without_tag,
        };
        let hits = doc_linter::query::similar_for_mcp(&**self.db.borrow(), args)
            .map_err(|e| McpError::from_anyhow("audit_doc_region: rank", e))?;

        // Optional LLM rerank surface. NoOpJudge reports
        // `is_available=false` so the verdicts array stays empty
        // until a real backend (Anthropic / OpenAI) ships behind
        // the --features llm Cargo feature. Wiring it now means
        // consumers see the same payload shape today and after
        // the backend lights up — no version-of-doc-linter
        // capability sniffing required.
        let judge = doc_linter::llm::default_judge();
        let rerank = if judge.is_available() {
            let target_summary = target.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            let target_id = target.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let target_claim = doc_linter::llm::Claim {
                text: target_summary.to_string(),
                source_id: Some(target_id.to_string()),
                source_line: None,
            };
            let hit_rows = hits
                .get("hits")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let mut verdicts: Vec<Value> = Vec::with_capacity(hit_rows.len());
            for hit in &hit_rows {
                let hit_id = hit
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let hit_text = hit
                    .get("text_excerpt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let hit_claim = doc_linter::llm::Claim {
                    text: hit_text,
                    source_id: Some(hit_id.clone()),
                    source_line: None,
                };
                if let Ok(verdict) = judge.compare_pair(&target_claim, &hit_claim) {
                    verdicts.push(json!({
                        "hit_id": hit_id,
                        "inconsistent": verdict.inconsistent,
                        "confidence": verdict.confidence,
                        "rationale": verdict.rationale,
                    }));
                }
            }
            json!({
                "judge": judge.name(),
                "verdicts": verdicts,
            })
        } else {
            json!({
                "judge": judge.name(),
                "verdicts": [],
            })
        };

        Ok(tool_text_result(&audit_doc_region_envelope(
            target, hits, rerank,
        )))
    }

    /// Gap-009: in-process `doc-linter check`. Refreshes the graph,
    /// optionally repopulates embeddings, and rewrites `docs/sitemap.json`.
    /// `silent=true` and `OutputFormat::Json` keep stdout clean for the
    /// JSON-RPC transport;
    /// the actual lint report is discarded (callers wanting findings
    /// should still use `query_*` tools after the reingest completes).
    fn tool_reingest(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let embeddings_arg = arguments
            .get("embeddings")
            .and_then(serde_json::Value::as_bool);
        let no_vale = arguments
            .get("no_vale")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let lint_code_comments = arguments
            .get("lint_code_comments")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let rebuild = arguments
            .get("rebuild")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);

        let config_path = self.root.join(".doc-lint.toml");
        let config = doc_linter::config::LintConfig::load(&config_path)
            .map_err(|e| McpError::from_anyhow("reingest: load config", e))?;
        // An explicit argument wins; otherwise `.doc-lint.toml`'s
        // `embeddings` (default false) decides.
        let (embeddings, embeddings_source) = match embeddings_arg {
            Some(b) => (b, "argument"),
            None => (config.embeddings, "config"),
        };
        let all_files = super::util::discover_corpus(&self.root, &config)
            .map_err(|e| McpError::from_anyhow("reingest: walk markdown", e))?;

        let files_scanned = all_files.len();

        // `check` builds into a copy of the graph and swaps it in (see
        // `store_sqlite::build_and_swap`): this server's read-only handle and
        // every other agent's server keep reading the old inode meanwhile,
        // so no lock is needed. Afterwards reopen onto the new file.
        let outcome = super::check::run(
            &self.root,
            &config,
            &all_files,
            None,
            super::util::OutputFormat::Json,
            no_vale,
            lint_code_comments,
            None,
            rebuild,
            embeddings,
            false, // contradictions — opt-in via CLI only, not via reingest
            true,
        )
        .map_err(|e| McpError::from_anyhow("reingest", e))?;
        self.refresh_db_if_swapped();
        let mut payload =
            reingest_payload(&self.root, &config, &outcome, embeddings, files_scanned);
        payload["embeddings_source"] = json!(embeddings_source);
        Ok(tool_text_result(&payload))
    }

    /// Hot-reload the binary: send the MCP `notifications/tools/list_changed`
    /// notification so the harness re-fetches `tools/list`, then `exec()`
    /// the current binary so the new image takes over the same PID + stdio.
    /// The exec fires from a background thread after ~100ms so the
    /// tool-call response has time to flush.
    ///
    /// Eliminates the `/mcp` reconnect cycle for binary rebuilds during
    /// development. Spec-compliant MCP clients honour `list_changed`; if
    /// the harness doesn't, the user can still `/mcp` reconnect manually
    /// — exec is non-destructive.
    #[allow(
        clippy::unused_self,
        reason = "dispatched like every other tool method"
    )]
    #[cfg_attr(not(unix), allow(unreachable_code, unused_variables))]
    fn tool_reload_self(&self) -> std::result::Result<Value, McpError> {
        // ponytail: exec() is unix-only; on Windows the tool reports that
        // instead of swapping the binary. Ceiling: no hot-reload on Windows,
        // restart the MCP server after a rebuild.
        #[cfg(not(unix))]
        {
            return Err(McpError::from_anyhow(
                "reload_self",
                anyhow::anyhow!(
                    "reload_self needs exec(), which is unix-only; restart the MCP server instead"
                ),
            ));
        }
        // Resolve the binary path BEFORE spawning the exec thread so a
        // missing /proc/self/exe (rare) errors here, not silently in
        // the background.
        let exe = super::util::own_exe()
            .map_err(|e| McpError::from_anyhow("reload_self: resolve binary", e))?;
        let argv: Vec<String> = std::env::args().collect();

        // Send the list_changed notification on stdout now so the
        // harness sees it before exec. JSON-RPC notification — no id,
        // no response expected.
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/tools/list_changed"
        });
        let line = serde_json::to_string(&notif).unwrap_or_default();
        {
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            // Best-effort write; if stdout is closed the upcoming exec
            // also fails and the harness sees the disconnect.
            let _ = writeln!(handle, "{line}");
            let _ = handle.flush();
        }

        let exe_str = exe.display().to_string();
        // Schedule the exec on a background thread so the tool-call
        // response (returned from this fn below) flushes first.
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(100));
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                let err = std::process::Command::new(&exe).args(&argv[1..]).exec();
                // `exec` only returns on failure. The old image is still
                // valid, so keep serving rather than dropping the connection.
                eprintln!(
                    "doc-linter: reload_self exec failed, still serving the old binary: {err}"
                );
            }
        });

        Ok(tool_text_result(&json!({
            "ok": true,
            "exe": exe_str,
            "note": "binary will swap in ~100ms; harness should pick up new tools/list automatically",
        })))
    }

    /// Per user-probe-034's coupling-spectrum: wraps the
    /// existing files-coupled-to COUPLED_WITH walk and turns the
    /// raw jaccard distribution into one of four categorical
    /// tiers — `absence` / `minimal` / `heavy` / `moderate`.
    /// The agent gets the recommendation alongside raw scores so
    /// it doesn't have to re-derive thresholds per probe.
    fn tool_classify_file_coupling(
        &self,
        arguments: &Value,
    ) -> std::result::Result<Value, McpError> {
        let path = arguments
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| McpError {
                code: -32602,
                message: "classify_file_coupling: missing `path` string argument".to_string(),
            })?;
        // First confirm the File row exists — distinguish "absent
        // from corpus" (-32602) from "exists with zero neighbors"
        // (the absence-as-signal tier).
        let exists_bindings = vec![("path", doc_linter::store::Value::Str(path.to_string()))];
        let exists_result = self
            .q(
                "SELECT f.path AS path FROM File f WHERE f.path = $path LIMIT 1",
                exists_bindings,
            )
            .map_err(|e| McpError::from_anyhow("classify_file_coupling: file lookup", e))?;
        let exists = exists_result
            .get("rows")
            .and_then(|v| v.as_array())
            .is_some_and(|rs| !rs.is_empty());
        if !exists {
            return Err(McpError {
                code: -32602,
                message: format!(
                    "classify_file_coupling: unknown path `{path}` — not found in File table. \
                     Run files-by-path-substring($pattern) to discover paths."
                ),
            });
        }
        let coupled_bindings = vec![("path", doc_linter::store::Value::Str(path.to_string()))];
        let coupled_result = self
            .q(
                // COUPLED_WITH is matched in either direction, as in the SQL.
                "SELECT b AS coupled_path, jaccard FROM \
                 (SELECT dst AS b, jaccard FROM \"COUPLED_WITH\" WHERE src = $path \
                  UNION ALL SELECT src AS b, jaccard FROM \"COUPLED_WITH\" WHERE dst = $path) \
                 ORDER BY jaccard DESC, b",
                coupled_bindings,
            )
            .map_err(|e| McpError::from_anyhow("classify_file_coupling: neighbor walk", e))?;
        let rows = coupled_result
            .get("rows")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let neighbors: Vec<(String, f64)> = rows
            .iter()
            .filter_map(|r| {
                let p = r.get("coupled_path").and_then(|v| v.as_str())?;
                let j = r.get("jaccard").and_then(serde_json::Value::as_f64)?;
                Some((p.to_string(), j))
            })
            .collect();
        Ok(tool_text_result(&build_coupling_classification(
            path, &neighbors,
        )))
    }

    /// gap-mcp-supervisor Slice B.3b: voluntary worker shutdown.
    /// Sets the swap_requested flag so `serve_loop` exits after
    /// the response flushes and `run_worker` returns ExitCode(42).
    /// The supervisor inspects that exit code to decide whether
    /// to respawn (B.3c) or terminate. Until the supervisor's
    /// outer respawn loop ships, calling this tool produces a
    /// clean worker exit — Claude Code's MCP harness usually
    /// auto-reconnects, which restarts the supervisor and the
    /// freshly-built worker. Not as transparent as the eventual
    /// in-place swap, but a useful stepping stone.
    fn tool_swap_worker(&self, arguments: &Value) -> std::result::Result<Value, McpError> {
        let reason = arguments
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if reason.is_empty() {
            eprintln!("doc-linter mcp-worker: swap_worker requested");
        } else {
            eprintln!("doc-linter mcp-worker: swap_worker requested ({reason})");
        }
        self.swap_requested.store(true, Ordering::SeqCst);
        Ok(tool_text_result(&json!({
            "ok": true,
            "note": "worker will exit with code 42 after this response flushes",
        })))
    }
}

/// JSON-RPC request wire shape (we accept any `id` JSON value
/// per the spec).
#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// Internal error type — promoted to a JSON-RPC error response by
/// `handle_line`.
#[derive(Debug)]
struct McpError {
    code: i32,
    message: String,
}

impl McpError {
    fn from_anyhow(tool: &str, err: anyhow::Error) -> Self {
        McpError {
            // -32000 to -32099 is the JSON-RPC implementation-defined
            // server-error range. -32000 is the conventional generic.
            code: -32000,
            message: format!("{tool}: {err:#}"),
        }
    }
}

/// Roadmap issue #211: path-traversal-safe resolution of a
/// repo-relative file path against the MCP server's root directory.
/// Rejects:
///   - absolute paths (a leading `/` or Windows drive letter),
///   - any `..` component anywhere in the path,
///   - resolved paths that escape `root` (defense-in-depth in case
///     a future symlink-aware path joining is introduced).
///
/// Returns the canonical resolved path on success, or a human-
/// readable rejection reason on failure. Caller wraps the reason
/// in a JSON-RPC error envelope.
fn resolve_repo_path(
    root: &Path,
    source_roots: &[String],
    rel: &str,
) -> std::result::Result<PathBuf, String> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(format!("path `{rel}` is absolute; must be repo-relative"));
    }
    for component in rel_path.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(format!(
                    "path `{rel}` contains a `..` segment; path-traversal is rejected"
                ));
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                return Err(format!(
                    "path `{rel}` contains an absolute component; must be repo-relative"
                ));
            }
            _ => {}
        }
    }
    let canon_root = root
        .canonicalize()
        .map_err(|e| format!("path `{rel}` resolution: cannot canonicalize root: {e}"))?;
    // Try the repo root first, then each configured code-source subdir.
    // This handles graphs whose SCIP index was generated from a
    // sub-project, so `File.path` is relative to that dir, not the root.
    // Every candidate must still resolve to a file INSIDE the repo root.
    let mut bases: Vec<PathBuf> = Vec::with_capacity(1 + source_roots.len());
    bases.push(root.to_path_buf());
    for sr in source_roots {
        bases.push(root.join(sr));
    }
    let mut last_err: Option<String> = None;
    for base in &bases {
        match base.join(rel_path).canonicalize() {
            Ok(canon) if canon.starts_with(&canon_root) => return Ok(canon),
            Ok(_) => {
                last_err = Some(format!("path `{rel}` resolves outside the repo root"));
            }
            Err(e) => {
                last_err = Some(format!("path `{rel}` resolution: cannot canonicalize: {e}"));
            }
        }
    }
    Err(last_err.unwrap_or_else(|| format!("path `{rel}` not found under the repo root")))
}

/// Roadmap issue #35 v6/v7: extract the optional `top` cap shared
/// by list-returning tools. 0 (or absent) means "no cap".
///
/// Roadmap issue #216: a negative value is rejected with -32602.
/// Pre-#216 we relied on `as_u64()` silently returning `None` and
/// defaulting to "no cap" — that surfaced as a confused-looking
/// full corpus dump when the caller meant `top: 5` but typed
/// `top: -5`.
fn top_arg(arguments: &Value) -> usize {
    nonneg_int_arg(arguments, "top").unwrap_or(0)
}

/// Iterative two-row Levenshtein. Inlined to avoid a cross-crate
/// visibility change on the disambiguation copy — the saved-query
/// catalog is small (tens of entries), so the cost is negligible.
fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr: Vec<usize> = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        curr[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Suggest the 1-2 closest saved-query names by Levenshtein distance
/// for the typo'd input. Returns " Did you mean `X` / `Y`?" or "" if
/// nothing is reasonably close. Per the ACI principle ([[research-swe-
/// agent]]), missing-name errors should be recoverable — the agent
/// gets the right correction inline instead of having to list the
/// catalog and re-think.
fn closest_saved_query_suggestion(name: &str, available: &[String]) -> String {
    if name.is_empty() || available.is_empty() {
        return String::new();
    }
    // Cap distance by half the input length so wildly-different names
    // don't get suggested. e.g. "foo" never suggests "research-by-tag".
    let max_distance = name.chars().count().div_ceil(2).max(2);
    let mut scored: Vec<(usize, &str)> = available
        .iter()
        .map(|n| (levenshtein_distance(name, n), n.as_str()))
        .filter(|(d, _)| *d <= max_distance)
        .collect();
    scored.sort_by_key(|(d, n)| (*d, (*n).to_string()));
    let top: Vec<&str> = scored.iter().take(2).map(|(_, n)| *n).collect();
    match top.as_slice() {
        [] => String::new(),
        [one] => format!(" Did you mean `{one}`?"),
        [one, two] => format!(" Did you mean `{one}` or `{two}`?"),
        _ => String::new(),
    }
}

/// A `STRING[]` column as a JSON-array subquery over its SQLite side table
/// (`doc_tags(doc_id, value)`, ...), in insertion order; `owner_expr` is the
/// owning row's key in the enclosing query.
fn list_sql(table: &str, owner_col: &str, owner_expr: &str) -> String {
    format!(
        "(SELECT json_group_array(value) FROM \
         (SELECT value FROM {table} WHERE {owner_col} = {owner_expr} ORDER BY rowid))"
    )
}

/// Every Doc to Doc edge as `(src, dst, edge_line, edge_kind)`; `label(r)`
/// in the SQL is the table name.
const DOC_DOC_EDGES_SQL: &str =
    "SELECT src, dst, line AS edge_line, 'WIKILINK' AS edge_kind FROM \"WIKILINK\" \
     UNION ALL SELECT src, dst, line, 'MD_LINK' FROM \"MD_LINK\" \
     UNION ALL SELECT src, dst, line, 'DEPENDS_ON' FROM \"DEPENDS_ON\" \
     UNION ALL SELECT src, dst, line, 'INFORMED_BY' FROM \"INFORMED_BY\" \
     UNION ALL SELECT src, dst, line, 'SUPERSEDES' FROM \"SUPERSEDES\" \
     UNION ALL SELECT src, dst, line, 'CRATE_REF' FROM \"CRATE_REF\"";

/// Neighbors of a doc along one axis (`covers` or `wikilink`).
fn doc_neighbors_sql(by: &str, top: usize) -> Option<String> {
    match by {
        "covers" => Some(format!(
            "SELECT n.id AS id, n.title AS title, \
                    json_group_array(DISTINCT c2.dst) AS shared_entities, \
                    count(DISTINCT c2.dst) AS shared_count \
             FROM \"COVERS\" c1 JOIN \"COVERS\" c2 ON c2.dst = c1.dst \
             JOIN Doc n ON n.id = c2.src \
             WHERE c1.src = $id AND n.id <> $id \
             GROUP BY n.id ORDER BY shared_count DESC, n.id ASC LIMIT {top}"
        )),
        "wikilink" => Some(format!(
            "SELECT n.id AS id, n.title AS title, json_array() AS shared_entities, \
                    count(*) AS shared_count \
             FROM \"WIKILINK\" w JOIN Doc n ON n.id = w.dst \
             WHERE w.src = $id AND n.id <> $id \
             GROUP BY n.id ORDER BY shared_count DESC, n.id ASC LIMIT {top}"
        )),
        _ => None,
    }
}

/// Neighbors of an entity (`co-cover` or `related`).
fn entity_neighbors_sql(by: &str, top: usize) -> Option<String> {
    match by {
        "co-cover" => Some(format!(
            "SELECT n.id AS id, n.display AS display, \
                    json_group_array(DISTINCT c1.src) AS shared_docs, \
                    count(DISTINCT c1.src) AS shared_count \
             FROM \"COVERS\" c0 JOIN \"COVERS\" c1 ON c1.src = c0.src \
             JOIN Entity n ON n.id = c1.dst \
             WHERE c0.dst = $id AND n.id <> $id \
             GROUP BY n.id ORDER BY shared_count DESC, n.id ASC LIMIT {top}"
        )),
        "related" => Some(format!(
            "SELECT n.id AS id, n.display AS display, r.type AS relation_type, \
                    r.weight AS shared_count \
             FROM \"RELATES_TO\" r JOIN Entity n ON n.id = r.dst \
             WHERE r.src = $id AND n.id <> $id \
             ORDER BY shared_count DESC, n.id ASC LIMIT {top}"
        )),
        _ => None,
    }
}

/// Per iter 112: detect tag-axis loose ends. Given a doc's summary,
/// its current tags, and the set of known tags, return tags the
/// summary mentions but the doc doesn't carry. Match is case-
/// insensitive against BOTH the kebab-case tag and a space-separated
/// form (e.g. "agent-shell" matches both "agent-shell" and "agent
/// shell" in the summary). Returns a structured Value per suggestion
/// — `{tag, evidence}` where evidence is the matched substring from
/// the summary. Pure function — testable without DB.
fn suggest_tags(summary: &str, current_tags: &[String], known_tags: &[String]) -> Vec<Value> {
    let summary_lower = summary.to_lowercase();
    let current_set: std::collections::HashSet<&str> =
        current_tags.iter().map(String::as_str).collect();
    let mut out: Vec<Value> = Vec::new();
    for tag in known_tags {
        if current_set.contains(tag.as_str()) {
            continue;
        }
        let kebab = tag.to_lowercase();
        let spaced = kebab.replace('-', " ");
        // Skip very short tags (1-2 chars) to avoid false-positive
        // common-word matches.
        if kebab.chars().count() < 3 {
            continue;
        }
        let evidence = if summary_lower.contains(&kebab) {
            Some(kebab.clone())
        } else if kebab != spaced && summary_lower.contains(&spaced) {
            Some(spaced.clone())
        } else {
            None
        };
        if let Some(matched) = evidence {
            out.push(json!({
                "tag": tag,
                "evidence": matched,
            }));
        }
    }
    out
}

/// Per iter 163 (interrogation-032 Finding C — 3rd tag-axis-loose-end
/// instance): suggest known tags that share a STEM-TOKEN with one of
/// the doc's CARRIED tags but aren't themselves carried. Two rules
/// run in sequence:
///
/// - Rule 1 (form-drift): kebab-tokenize both sides; if any known-tag's
///   token has Levenshtein distance 1 or 2 from any carried-tag's
///   token (both tokens ≥ 4 chars), emit the known-tag. Catches
///   voyage-code-3 / `embeddings` (plural) → `embedding-model`
///   (`embedding` is Levenshtein-1 from `embeddings`).
///
/// - Rule 2 (exact-token overlap): if any known-tag has a
///   discriminative token (≥ 5 chars, not in the corpus-common
///   stopword set) that EXACTLY appears in any carried-tag's tokens,
///   emit the known-tag. Catches graph2vec / `graph-embedding` →
///   `graph-similarity` (shared token `graph`).
///
/// Stopwords: `research`, `paper`, `tool`, `production`, `framework`,
/// `survey`, `design`, `system` — these appear on too many docs to be
/// discriminative.
///
/// Pure-function — testable without DB. Returns each known-tag at
/// most once (first match wins across both rules).
fn suggest_family_tags(current_tags: &[String], known_tags: &[String]) -> Vec<Value> {
    const STOPWORDS: &[&str] = &[
        "research",
        "paper",
        "tool",
        "production",
        "framework",
        "survey",
        "design",
        "system",
    ];
    let current_set: std::collections::HashSet<&str> =
        current_tags.iter().map(String::as_str).collect();
    let mut out: Vec<Value> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for known in known_tags {
        if current_set.contains(known.as_str()) {
            continue;
        }
        if seen.contains(known) {
            continue;
        }
        let known_lower = known.to_lowercase();
        let known_tokens: Vec<&str> = known_lower.split('-').collect();
        let mut emitted = false;
        // Rule 1 — form-drift via Levenshtein 1-2.
        for current in current_tags {
            if emitted {
                break;
            }
            let current_lower = current.to_lowercase();
            let current_tokens: Vec<&str> = current_lower.split('-').collect();
            'pair: for kt in &known_tokens {
                if kt.chars().count() < 4 {
                    continue;
                }
                for ct in &current_tokens {
                    if ct.chars().count() < 4 {
                        continue;
                    }
                    if *kt == *ct {
                        continue;
                    }
                    let dist = levenshtein_distance(kt, ct);
                    if dist > 0 && dist <= 2 {
                        out.push(json!({
                            "tag": known,
                            "evidence": format!(
                                "stem-token `{kt}` is close (Levenshtein {dist}) to carried tag `{current}`'s token `{ct}`"
                            ),
                        }));
                        seen.insert(known.clone());
                        emitted = true;
                        break 'pair;
                    }
                }
            }
        }
        if emitted {
            continue;
        }
        // Rule 2 — exact-token overlap on discriminative tokens.
        for current in current_tags {
            if emitted {
                break;
            }
            let current_lower = current.to_lowercase();
            let current_tokens: Vec<&str> = current_lower.split('-').collect();
            'pair: for kt in &known_tokens {
                if kt.chars().count() < 5 {
                    continue;
                }
                if STOPWORDS.contains(kt) {
                    continue;
                }
                for ct in &current_tokens {
                    if *kt == *ct {
                        out.push(json!({
                            "tag": known,
                            "evidence": format!(
                                "discriminative token `{kt}` is shared between known tag `{known}` and carried tag `{current}`"
                            ),
                        }));
                        seen.insert(known.clone());
                        emitted = true;
                        break 'pair;
                    }
                }
            }
        }
    }
    out
}

/// Per iter 119 (user-probe-022 Finding A): build the
/// `freshness` block surfaced under query_schema's response.
/// Pure-function — testable without a DB. Takes the two
/// max-timestamp strings (from MAX(Doc.updated) and
/// MAX(File.last_touched)) and packs them into a uniform
/// agent-readable payload. The `hint` string names the
/// build-queue-lag pattern the freshness block exists to
/// surface.
fn build_freshness_payload(max_doc_updated: Option<&str>, max_file_touched: Option<&str>) -> Value {
    let doc_v = max_doc_updated.map_or(Value::Null, |s| Value::String(s.to_string()));
    let file_v = max_file_touched.map_or(Value::Null, |s| Value::String(s.to_string()));
    let both_null = doc_v.is_null() && file_v.is_null();
    let hint = if both_null {
        "no freshness data — empty corpus or freshness columns not yet populated"
    } else {
        "compare max_file_touched against the current git HEAD timestamp \
         to detect rebuild-lag-vs-self-dogfood: source repo's HEAD newer \
         than max_file_touched ⇒ run `doc-linter check` to re-ingest; \
         see user-probe-022 + build-queue.md"
    };
    json!({
        "max_doc_updated": doc_v,
        "max_file_touched": file_v,
        "hint": hint,
    })
}

/// Per iter 126 (user-probe-023 gap-audit-doc-region-auto-reingest-
/// on-fresh-target): cheap heuristic — does `s` look like a
/// repo-relative markdown path the resolver would have tried as a
/// filesystem hit? Pure-function; testable without a DB.
/// True for `docs/audit/x.md`, `docs/research/y.md`, and similar.
/// False for bare Doc ids like `research-mega` or `entity-pricing-rule`.
fn looks_like_doc_path(s: &str) -> bool {
    s.ends_with(".md") || s.contains('/')
}

/// Per user-probe-034's 3-tier coupling spectrum
/// ([[feedback_coupling_spectrum]]): classify a File's
/// COUPLED_WITH neighborhood into one of four categorical tiers.
/// Pure-function; testable without a DB.
///
/// Tiers (thresholds from the iter-155 framework):
/// - `absence`: 0 neighbors — well-encapsulated leaf module.
/// - `heavy`: ≥ 5 neighbors AND max_jaccard ≥ 0.75 — tight
///   bounded context where the file co-changes with a cluster.
/// - `minimal`: 1-2 neighbors AND max_jaccard in [0.3, 0.7) —
///   small family co-evolution (CRUD-pair shape).
/// - `moderate`: anything else (e.g., 3-4 neighbors at any
///   jaccard, 1-2 at very high jaccard, etc.) — review the
///   neighbor list before drawing architectural conclusions.
fn classify_coupling_tier(neighbor_count: usize, max_jaccard: f64) -> &'static str {
    if neighbor_count == 0 {
        "absence"
    } else if neighbor_count >= 5 && max_jaccard >= 0.75 {
        "heavy"
    } else if (1..=2).contains(&neighbor_count) && (0.3..0.7).contains(&max_jaccard) {
        "minimal"
    } else {
        "moderate"
    }
}

/// Per user-probe-034: human-readable explanation per tier.
/// Pure-function; agent surfaces this verbatim in synthesis.
fn coupling_tier_explanation(tier: &str) -> &'static str {
    match tier {
        "absence" => "Well-encapsulated leaf module. Refactors won't ripple; bugs are likely local to this file.",
        "minimal" => "Small family co-evolution — the named neighbor(s) co-change with this file at moderate frequency. Expect the neighbor(s) to need updates when you change this file. CRUD-pair-style coupling.",
        "heavy" => "Tight bounded context — this file co-changes with a cluster of 5+ at high jaccard. Refactors WILL ripple across the cluster; for debugging, `git bisect` should cross the whole cluster, not just this file in isolation.",
        "moderate" => "Partial coupling — between the minimal and heavy tiers. Review the neighbor list (sorted by jaccard) to assess: high-jaccard neighbors behave like minimal coupling; lower-jaccard neighbors are looser co-changes.",
        _ => "Unknown tier.",
    }
}

/// Per user-probe-034: compose the full classification payload from
/// the path + raw neighbor list. Pure-function; testable without a DB.
fn build_coupling_classification(path: &str, neighbors: &[(String, f64)]) -> Value {
    let neighbor_count = neighbors.len();
    let max_jaccard = neighbors.iter().map(|(_, j)| *j).fold(0.0_f64, f64::max);
    let mean_jaccard = if neighbor_count == 0 {
        0.0
    } else {
        neighbors.iter().map(|(_, j)| *j).sum::<f64>() / neighbor_count as f64
    };
    let tier = classify_coupling_tier(neighbor_count, max_jaccard);
    let neighbor_rows: Vec<Value> = neighbors
        .iter()
        .map(|(p, j)| json!({"path": p, "jaccard": j}))
        .collect();
    json!({
        "path": path,
        "tier": tier,
        "neighbor_count": neighbor_count,
        "max_jaccard": max_jaccard,
        "mean_jaccard": mean_jaccard,
        "explanation": coupling_tier_explanation(tier),
        "neighbors": neighbor_rows,
    })
}

/// Per iter 124 (interrogation-025 Finding C + D): build the
/// `retrieval_backends` block surfaced under query_schema's
/// response. Pure-function — testable without an embedder by
/// passing the runtime `embedding_available` flag explicitly.
///
/// Caller computes `embedding_available` from
/// `default_embedder().is_available()`. When false, the
/// embedding entry carries a `reason` field naming the build
/// flag + env var so the agent can recover.
fn build_retrieval_backends_payload(embedding_available: bool) -> Value {
    let mut backends: Vec<Value> = Vec::new();
    backends.push(json!({"name": "bm25", "available": true}));
    if embedding_available {
        backends.push(json!({"name": "embedding", "available": true}));
    } else {
        backends.push(json!({
            "name": "embedding",
            "available": false,
            "reason": "embedder not compiled in (build with `--features embeddings`) or DOC_LINTER_EMBED_MODEL not set (see doc-linter `embeddings` module)",
        }));
    }
    json!({ "backends": backends })
}

/// Per iter 117 gap-007 closure: detect which graph table holds
/// the row whose primary key is `id`. Tries Doc → Entity →
/// Function and returns the first match. Returns `Ok(None)` when
/// the id matches none, distinguishing "unknown id" from "DB error."
fn detect_node_kind(
    db: &dyn doc_linter::graph_read::GraphRead,
    id: &str,
) -> anyhow::Result<Option<String>> {
    let result = doc_linter::graph_read::query_sql(
        db,
        "SELECT 'doc' AS kind FROM Doc WHERE id = $id \
         UNION ALL SELECT 'entity' AS kind FROM Entity WHERE id = $id \
         UNION ALL SELECT 'function' AS kind FROM Function WHERE symbol = $id",
        vec![("id", doc_linter::store::Value::Str(id.to_string()))],
    )?;
    Ok(result
        .get("rows")
        .and_then(|v| v.as_array())
        .and_then(|rows| rows.first())
        .and_then(|r| r.get("kind"))
        .and_then(|k| k.as_str())
        .map(str::to_string))
}

/// Doc-centred context payload. Returns the Doc row + every
/// outbound and inbound Doc neighbor with summary inline. Each
/// neighbor is a `Node`-shaped row alongside the connecting edge.
fn context_for_doc_payload(
    db: &dyn doc_linter::graph_read::GraphRead,
    id: &str,
    top: usize,
) -> anyhow::Result<Value> {
    let doc_edges = DOC_DOC_EDGES_SQL;
    let outbound_sql = format!(
        "WITH e AS ({doc_edges}) \
         SELECT c.id AS centre_id, c.kind AS centre_kind, c.title AS centre_title, \
                c.summary AS centre_summary, c.path AS centre_path, \
                {ctags} AS centre_tags, c.role AS centre_role, \
                n.id AS n_id, n.title AS n_title, n.summary AS n_summary, n.kind AS n_kind, \
                n.path AS n_path, {ntags} AS n_tags, \
                'outbound' AS direction, e.edge_kind AS edge_kind, e.edge_line AS edge_line \
         FROM Doc c LEFT JOIN e ON e.src = c.id LEFT JOIN Doc n ON n.id = e.dst \
         WHERE c.id = $id ORDER BY e.edge_kind, n.id LIMIT {top}",
        ctags = list_sql("doc_tags", "doc_id", "c.id"),
        ntags = list_sql("doc_tags", "doc_id", "n.id"),
        top = top.saturating_mul(2)
    );
    let outbound_raw = doc_linter::graph_read::query_sql(
        db,
        &outbound_sql,
        vec![("id", doc_linter::store::Value::Str(id.to_string()))],
    )?;
    let inbound_sql = format!(
        "WITH e AS ({doc_edges}) \
         SELECT n.id AS n_id, n.title AS n_title, n.summary AS n_summary, n.kind AS n_kind, \
                n.path AS n_path, {ntags} AS n_tags, \
                'inbound' AS direction, e.edge_kind AS edge_kind, e.edge_line AS edge_line \
         FROM Doc c LEFT JOIN e ON e.dst = c.id LEFT JOIN Doc n ON n.id = e.src \
         WHERE c.id = $id ORDER BY e.edge_kind, n.id LIMIT {top}",
        ntags = list_sql("doc_tags", "doc_id", "n.id"),
        top = top.saturating_mul(2)
    );
    let inbound_raw = doc_linter::graph_read::query_sql(
        db,
        &inbound_sql,
        vec![("id", doc_linter::store::Value::Str(id.to_string()))],
    )?;

    let outbound_rows: Vec<Value> = outbound_raw
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // No outbound row: the doc exists (detect_node_kind) but has no edges.
    let centre = outbound_rows.first().cloned().unwrap_or(Value::Null);
    let inbound_rows: Vec<Value> = inbound_raw
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let centre_node = json!({
        "id": id,
        "kind": "doc",
        "title": centre.get("centre_title").cloned().unwrap_or(Value::Null),
        "summary": centre.get("centre_summary").cloned().unwrap_or(Value::Null),
        "path": centre.get("centre_path").cloned().unwrap_or(Value::Null),
        "tags": centre.get("centre_tags").cloned().unwrap_or(Value::Null),
        "role": centre.get("centre_role").cloned().unwrap_or(Value::Null),
        "doc_kind": centre.get("centre_kind").cloned().unwrap_or(Value::Null),
    });

    let mut neighbors: Vec<Value> = Vec::new();
    for row in outbound_rows.iter().chain(inbound_rows.iter()) {
        let Some(n_id) = row.get("n_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if n_id.is_empty() {
            continue;
        }
        neighbors.push(build_neighbor_envelope(row, "doc"));
        if neighbors.len() >= top {
            break;
        }
    }

    Ok(json!({
        "centre": centre_node,
        "neighbors": neighbors,
        "neighbor_count": neighbors.len(),
    }))
}

/// Entity-centred context payload. Returns the Entity row +
/// covering Docs (with summary inline) as neighbors.
fn context_for_entity_payload(
    db: &dyn doc_linter::graph_read::GraphRead,
    id: &str,
    top: usize,
) -> anyhow::Result<Value> {
    let sql = format!(
        "SELECT c.id AS centre_id, c.display AS centre_display, \
                c.description AS centre_description, c.entity_class AS centre_class, \
                d.id AS n_id, d.title AS n_title, d.summary AS n_summary, \
                d.kind AS n_kind, d.path AS n_path, {tags} AS n_tags, \
                'inbound' AS direction, 'COVERS' AS edge_kind, v.line AS edge_line \
         FROM Entity c LEFT JOIN \"COVERS\" v ON v.dst = c.id LEFT JOIN Doc d ON d.id = v.src \
         WHERE c.id = $id ORDER BY d.id LIMIT {top}",
        tags = list_sql("doc_tags", "doc_id", "d.id"),
        top = top.saturating_mul(2)
    );
    let raw = doc_linter::graph_read::query_sql(
        db,
        &sql,
        vec![("id", doc_linter::store::Value::Str(id.to_string()))],
    )?;
    let rows: Vec<Value> = raw
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let centre = rows.first().cloned().unwrap_or(Value::Null);

    let centre_node = json!({
        "id": id,
        "kind": "entity",
        "title": centre.get("centre_display").cloned().unwrap_or(Value::Null),
        "summary": centre.get("centre_description").cloned().unwrap_or(Value::Null),
        "entity_class": centre.get("centre_class").cloned().unwrap_or(Value::Null),
    });

    // The entity's own `entity-<id>` doc doesn't COVER its entity, so an
    // entity no narrative doc covers used to come back with 0 neighbours.
    // Lead with that definition doc (DEFINED_BY), then docs linking to it.
    let def_sql = format!(
        "SELECT def.id AS n_id, def.title AS n_title, def.summary AS n_summary, \
                def.kind AS n_kind, def.path AS n_path, {dtags} AS n_tags, \
                'outbound' AS direction, 'DEFINED_BY' AS edge_kind, NULL AS edge_line \
         FROM Doc def WHERE def.id = $def_id \
         UNION ALL \
         SELECT d.id, d.title, d.summary, d.kind, d.path, {tags}, \
                'inbound', e.edge_kind, e.edge_line \
         FROM (SELECT src, dst, line AS edge_line, 'WIKILINK' AS edge_kind FROM \"WIKILINK\" \
               UNION ALL SELECT src, dst, line, 'MD_LINK' FROM \"MD_LINK\") e \
         JOIN Doc d ON d.id = e.src WHERE e.dst = $def_id",
        dtags = list_sql("doc_tags", "doc_id", "def.id"),
        tags = list_sql("doc_tags", "doc_id", "d.id")
    );
    let def_rows = doc_linter::graph_read::query_sql(
        db,
        &def_sql,
        vec![(
            "def_id",
            doc_linter::store::Value::Str(format!("entity-{id}")),
        )],
    )?;
    let (defined_by, linkers): (Vec<Value>, Vec<Value>) = def_rows
        .get("rows")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .partition(|r| r["edge_kind"] == "DEFINED_BY");

    let mut neighbors: Vec<Value> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for row in defined_by.iter().chain(&rows).chain(&linkers) {
        let Some(n_id) = row.get("n_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if n_id.is_empty() || !seen.insert(n_id.to_string()) {
            continue;
        }
        neighbors.push(build_neighbor_envelope(row, "doc"));
        if neighbors.len() >= top {
            break;
        }
    }

    Ok(json!({
        "centre": centre_node,
        "neighbors": neighbors,
        "neighbor_count": neighbors.len(),
    }))
}

/// Function-centred context payload. Returns the Function row +
/// defining Doc + belonging Entities + adjacent Functions.
fn context_for_function_payload(
    db: &dyn doc_linter::graph_read::GraphRead,
    id: &str,
    top: usize,
) -> anyhow::Result<Value> {
    // Centre row: pull the Function's core metadata.
    let centre_raw = doc_linter::graph_read::query_sql(
        db,
        "SELECT f.symbol AS symbol, f.signature AS signature, f.doc_comment AS doc_comment, \
                f.file AS file, f.line AS line, f.language AS language \
         FROM Function f WHERE f.symbol = $id",
        vec![("id", doc_linter::store::Value::Str(id.to_string()))],
    )?;
    let centre_row = centre_raw
        .get("rows")
        .and_then(|v| v.as_array())
        .and_then(|rs| rs.first())
        .cloned()
        .unwrap_or(Value::Null);
    let centre_node = json!({
        "id": id,
        "kind": "function",
        "title": centre_row.get("symbol").cloned().unwrap_or(Value::Null),
        "summary": centre_row.get("doc_comment").cloned().unwrap_or(Value::Null),
        "signature": centre_row.get("signature").cloned().unwrap_or(Value::Null),
        "path": centre_row.get("file").cloned().unwrap_or(Value::Null),
        "line": centre_row.get("line").cloned().unwrap_or(Value::Null),
        "language": centre_row.get("language").cloned().unwrap_or(Value::Null),
    });

    // Neighbors: callees + callers (Function), defining Doc,
    // belonging Entity. Project each into the uniform envelope.
    // One row, one `rows` column holding callees, callers, defining docs and
    // belonging entities in that order (each part sorted by id).
    let part = |select: &str| format!("SELECT * FROM (SELECT DISTINCT {select} ORDER BY n_id)");
    let neigh_sql = format!(
        "SELECT json_group_array(json_object('n_id', n_id, 'n_title', n_title, \
                'n_summary', n_summary, 'n_kind', n_kind, 'n_path', n_path, \
                'edge_kind', edge_kind)) AS rows FROM ( \
         {callees} UNION ALL {callers} UNION ALL {docs} UNION ALL {ents})",
        callees = part(
            "g.symbol AS n_id, g.symbol AS n_title, g.doc_comment AS n_summary, \
             'function' AS n_kind, g.file AS n_path, 'CALLS_OUT' AS edge_kind \
             FROM \"CALLS\" c JOIN Function g ON g.symbol = c.dst WHERE c.src = $id"
        ),
        callers = part(
            "g.symbol AS n_id, g.symbol AS n_title, g.doc_comment AS n_summary, \
             'function' AS n_kind, g.file AS n_path, 'CALLS_IN' AS edge_kind \
             FROM \"CALLS\" c JOIN Function g ON g.symbol = c.src WHERE c.dst = $id"
        ),
        docs = part(
            "d.id AS n_id, d.title AS n_title, d.summary AS n_summary, \
             'doc' AS n_kind, d.path AS n_path, 'DEFINED_IN' AS edge_kind \
             FROM \"FUNCTION_DEFINED_IN\" x JOIN Doc d ON d.id = x.dst WHERE x.src = $id"
        ),
        ents = part(
            "e.id AS n_id, e.display AS n_title, e.description AS n_summary, \
             'entity' AS n_kind, '' AS n_path, 'BELONGS_TO' AS edge_kind \
             FROM \"FUNCTION_BELONGS_TO\" x JOIN Entity e ON e.id = x.dst WHERE x.src = $id"
        ),
    );
    let neigh_raw = doc_linter::graph_read::query_sql(
        db,
        &neigh_sql,
        vec![("id", doc_linter::store::Value::Str(id.to_string()))],
    )?;
    let neigh_rows: Vec<Value> = neigh_raw
        .get("rows")
        .and_then(|v| v.as_array())
        .and_then(|rs| rs.first())
        .and_then(|r| r.get("rows"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut neighbors: Vec<Value> = Vec::new();
    for row in &neigh_rows {
        let Some(n_id) = row.get("n_id").and_then(|v| v.as_str()) else {
            continue;
        };
        if n_id.is_empty() {
            continue;
        }
        let n_kind = row
            .get("n_kind")
            .and_then(|v| v.as_str())
            .unwrap_or("function");
        neighbors.push(build_neighbor_envelope(row, n_kind));
        if neighbors.len() >= top {
            break;
        }
    }

    Ok(json!({
        "centre": centre_node,
        "neighbors": neighbors,
        "neighbor_count": neighbors.len(),
    }))
}

/// Build the uniform `{node: {...}, edge: {kind, line?}}` envelope
/// from a sql row carrying the standard neighbor column names.
/// Used by all three context_for_*_payload helpers so the wire
/// shape is identical regardless of centre kind.
fn build_neighbor_envelope(row: &Value, default_kind: &str) -> Value {
    let n_id = row.get("n_id").cloned().unwrap_or(Value::Null);
    let title = row.get("n_title").cloned().unwrap_or(Value::Null);
    let summary = row.get("n_summary").cloned().unwrap_or(Value::Null);
    let n_kind = row
        .get("n_kind")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_kind)
        .to_string();
    let path = row.get("n_path").cloned().unwrap_or(Value::Null);
    let tags = row.get("n_tags").cloned().unwrap_or(Value::Null);
    let direction = row.get("direction").cloned();
    let edge_kind = row.get("edge_kind").cloned().unwrap_or(Value::Null);
    let edge_line = row.get("edge_line").cloned().unwrap_or(Value::Null);

    let mut edge = serde_json::Map::new();
    edge.insert("kind".to_string(), edge_kind);
    if !edge_line.is_null() {
        edge.insert("line".to_string(), edge_line);
    }
    if let Some(d) = direction {
        if !d.is_null() {
            edge.insert("direction".to_string(), d);
        }
    }

    json!({
        "node": {
            "id": n_id,
            "kind": n_kind,
            "title": title,
            "summary": summary,
            "path": path,
            "tags": tags,
        },
        "edge": Value::Object(edge),
    })
}

/// Per iter 102: multi-view fusion for query_doc_neighbors `by="all"`
/// per [[research-mega]]. Merges the covers-axis rows and the
/// wikilink-axis rows by neighbor id, producing one row per unique
/// neighbor with: `id`, `title`, `shared_entities` (the covers
/// explanation when present), `covers_score`, `wikilink_score`,
/// `fused_score = covers_score + wikilink_score`. The fused list is
/// sorted by fused_score DESC, then by id ASC for stability, then
/// truncated to `top`. Pure function — testable without DB.
fn merge_neighbor_views(covers_rows: &[Value], wikilink_rows: &[Value], top: usize) -> Vec<Value> {
    use std::collections::BTreeMap;
    let mut by_id: BTreeMap<String, (String, Vec<Value>, i64, i64)> = BTreeMap::new();
    for row in covers_rows {
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let title = row
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let shared = row
            .get("shared_entities")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let score = row
            .get("shared_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        by_id
            .entry(id.to_string())
            .or_insert_with(|| (title, shared, 0, 0))
            .2 = score;
    }
    for row in wikilink_rows {
        let Some(id) = row.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let title = row
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let score = row
            .get("shared_count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0);
        let entry = by_id
            .entry(id.to_string())
            .or_insert_with(|| (title.clone(), Vec::new(), 0, 0));
        entry.3 = score;
        if entry.0.is_empty() {
            entry.0 = title;
        }
    }
    let mut rows: Vec<(String, String, Vec<Value>, i64, i64)> = by_id
        .into_iter()
        .map(|(id, (title, shared, c, w))| (id, title, shared, c, w))
        .collect();
    rows.sort_by(|a, b| {
        let fa = a.3 + a.4;
        let fb = b.3 + b.4;
        fb.cmp(&fa).then(a.0.cmp(&b.0))
    });
    rows.truncate(top);
    rows.into_iter()
        .map(|(id, title, shared, c, w)| {
            json!({
                "id": id,
                "title": title,
                "shared_entities": shared,
                "covers_score": c,
                "wikilink_score": w,
                "fused_score": c + w,
            })
        })
        .collect()
}

/// Per iter 96: shape the audit_doc_region response so the trust
/// signal lives at the top level alongside `target` / `hits` /
/// `rerank`, matching the shape query_similar uses. The fields are
/// also LEFT inside `hits` (the SimilarOutput) for back-compat —
/// agents migrating from the iter-85/88 query_similar API see the
/// same root-level vocabulary on the v1 MVP retrieval tool too.
/// Brings audit_doc_region to feature-parity with query_similar on
/// the confidence axis.
fn audit_doc_region_envelope(target: Value, hits: Value, rerank: Value) -> Value {
    let confidence = hits
        .get("confidence")
        .cloned()
        .unwrap_or(Value::String("low".to_string()));
    let confidence_signals = hits
        .get("confidence_signals")
        .cloned()
        .unwrap_or(json!({"top_score": 0.0, "spread": 0.0}));
    let met_threshold = hits
        .get("met_threshold")
        .cloned()
        .unwrap_or(Value::Bool(true));
    json!({
        "target": target,
        "hits": hits,
        "rerank": rerank,
        "confidence": confidence,
        "confidence_signals": confidence_signals,
        "met_threshold": met_threshold,
    })
}

/// Truncate a `run_sql` JSON result to at most `max_rows` rows,
/// adding `truncated`, `total_row_count`, and `row_count` fields so
/// the calling agent can detect partial results and re-run with a
/// higher `max_rows` if it actually needs more. Pure function — the
/// dispatching `tool_sql` reads `max_rows` from arguments and
/// applies this to the raw query result. Tests below pin the
/// truncation and pass-through cases.
fn cap_result(value: Value, max_rows: usize) -> Value {
    let Value::Object(mut obj) = value else {
        return value;
    };
    let total_rows = obj
        .get("rows")
        .and_then(|v| v.as_array())
        .map_or(0, std::vec::Vec::len);
    if total_rows <= max_rows {
        obj.insert("truncated".to_string(), Value::Bool(false));
        return Value::Object(obj);
    }
    if let Some(Value::Array(rows)) = obj.get_mut("rows") {
        rows.truncate(max_rows);
    }
    obj.insert(
        "total_row_count".to_string(),
        Value::Number(total_rows.into()),
    );
    obj.insert("row_count".to_string(), Value::Number(max_rows.into()));
    obj.insert("truncated".to_string(), Value::Bool(true));
    Value::Object(obj)
}

/// Roadmap issue #216: shared "non-negative integer" parser for
/// args like `top` / `depth` / `max_hops`. Returns:
///   - `Some(n)` for non-negative ints.
///   - `None` for absent / null fields (caller picks the default).
///   - Errors caught at the dispatcher boundary via [`McpError`]
///     would be tidier, but every list-style site already calls
///     this in a defaulting position — so we keep the
///     "unwrap_or(default)" idiom and use a sibling
///     [`reject_negative_int`] helper for the strict path.
fn nonneg_int_arg(arguments: &Value, key: &str) -> Option<usize> {
    let v = arguments.get(key)?;
    if v.is_null() {
        return None;
    }
    let n = v.as_i64()?;
    if n < 0 {
        return None;
    }
    usize::try_from(n).ok()
}

/// Strict variant for top-level integer args (e.g. `depth`,
/// `max_hops`) where a negative value is a contract violation, not
/// a silent default. Returns -32602 with a clear message naming
/// the field and the rejected value.
fn reject_negative_int(arguments: &Value, key: &str) -> std::result::Result<(), McpError> {
    let Some(v) = arguments.get(key) else {
        return Ok(());
    };
    if v.is_null() {
        return Ok(());
    }
    if let Some(n) = v.as_i64() {
        if n < 0 {
            return Err(McpError {
                code: -32602,
                message: format!("`{key}` must be a non-negative integer; got {n}"),
            });
        }
    }
    Ok(())
}

/// Roadmap issue #213: extract the optional `offset` arg shared by
/// list-returning tools. Negative offsets are rejected with -32602
/// so an agent can't accidentally walk backwards. Absent / null /
/// zero all mean "start from the head". The cap is applied before
/// `top` so an agent paging through a corpus gets the expected
/// `[offset .. offset+top)` window.
fn offset_arg(arguments: &Value) -> std::result::Result<usize, McpError> {
    let Some(v) = arguments.get("offset") else {
        return Ok(0);
    };
    if v.is_null() {
        return Ok(0);
    }
    match v.as_i64() {
        Some(n) if n >= 0 => Ok(usize::try_from(n).unwrap_or(usize::MAX)),
        Some(n) => Err(McpError {
            code: -32602,
            message: format!(
                "`offset` must be a non-negative integer; got {n}. \
                 Page from {{offset: 0}} forward using the `total` \
                 field in the response envelope to know when to stop."
            ),
        }),
        None => Err(McpError {
            code: -32602,
            message: "`offset` must be a non-negative integer".to_string(),
        }),
    }
}

/// Roadmap issue #213: apply `offset` + `top` to a row vector
/// **after** computing `total`. Centralized so every list-style
/// tool gets the same pagination semantics — the offset is
/// dropped first, then the head is truncated.
fn paginate<T>(rows: &mut Vec<T>, offset: usize, top: usize) {
    if offset > 0 {
        if offset >= rows.len() {
            rows.clear();
        } else {
            rows.drain(0..offset);
        }
    }
    if top > 0 && rows.len() > top {
        rows.truncate(top);
    }
}

/// MCP `tools/call` result envelope. Per spec, every tool call
/// returns `{content: [...]}` regardless of payload — we render
/// JSON results as a single `text` content block.
fn tool_text_result<T: Serialize>(payload: &T) -> Value {
    let text = serde_json::to_string_pretty(payload).unwrap_or_else(|_| "{}".to_string());
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ]
    })
}

fn ok_response_str(id: Value, result: Value) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    }))
    .unwrap_or_else(|_| String::new())
}

fn error_response_str(id: Value, code: i32, message: &str) -> String {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message
        }
    }))
    .unwrap_or_else(|_| String::new())
}

/// Build the `reingest` response from the check report. `ok` mirrors the
/// CLI exit code; the check runs silently under MCP, so without this its
/// errors (e.g. duplicate ids that ingest drops) never reach the caller.
fn reingest_payload(
    root: &Path,
    config: &doc_linter::config::LintConfig,
    outcome: &super::check::CheckOutcome,
    embeddings: bool,
    files_scanned: usize,
) -> Value {
    const MAX_ISSUES: usize = 50;
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).display().to_string();
    let root_prefix = format!("{}/", root.display());
    let all = || {
        outcome
            .report
            .iter()
            .flat_map(|(path, v)| v.iter().map(move |i| (path, i)))
    };
    let skipped_duplicates: Vec<Value> = all()
        .filter_map(|(path, issue)| match issue {
            doc_linter::validator::Issue::DuplicateId { id, other } => Some(json!({
                "id": id,
                "kept": rel(other),
                "skipped": rel(path),
            })),
            _ => None,
        })
        .collect();
    // Errors before warnings, capped so a noisy corpus can't flood the reply.
    let (errors, warnings): (Vec<_>, Vec<_>) = all().partition(|(_, i)| i.is_error(config));
    let total = errors.len() + warnings.len();
    let issues: Vec<Value> = errors
        .iter()
        .map(|p| (p, "error"))
        .chain(warnings.iter().map(|p| (p, "warning")))
        .take(MAX_ISSUES)
        .map(|((path, issue), severity)| {
            json!({
                "code": issue.extended_code(),
                "path": rel(path),
                "severity": severity,
                // Messages embed absolute paths ("also used by /abs/…");
                // make them repo-relative like `path`.
                "message": issue.to_string().replace(&root_prefix, ""),
            })
        })
        .collect();
    json!({
        "ok": outcome.exit == ExitCode::SUCCESS,
        "embeddings": embeddings,
        "files_scanned": files_scanned,
        "errors": errors.len(),
        "warnings": warnings.len(),
        "issues_truncated": total > issues.len(),
        "issues": issues,
        "skipped_duplicates": skipped_duplicates,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;
    use doc_linter::store::Store;

    fn tmp_root(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-mcp-{label}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Server on an empty graph (schema applied, every table empty).
    fn server() -> Server {
        seed_server(&[])
    }

    fn query_tool() -> &'static str {
        "sql"
    }

    /// A server whose graph has code, so the code tools are advertised (an
    /// empty graph is a docs-only vault).
    fn server_with_code() -> Server {
        seed_server(&[
            Seed::Function("f"),
            Seed::File("a.rs", "rust", 1),
            Seed::Sql("INSERT INTO Type(symbol) VALUES ('t')"),
            Seed::Sql("INSERT INTO Endpoint(id) VALUES ('e')"),
            Seed::Sql("INSERT INTO Module(id) VALUES ('m')"),
            Seed::Sql("INSERT INTO Migration(id) VALUES ('g')"),
        ])
    }

    /// One row to seed, written as an `INSERT`.
    #[allow(dead_code, reason = "not every variant is used by every test")]
    enum Seed<'a> {
        /// id, summary, tags
        Doc(&'a str, &'a str, &'a [&'a str]),
        /// id, display
        Entity(&'a str, &'a str),
        /// symbol
        Function(&'a str),
        /// A raw statement.
        Sql(&'a str),
        /// path, language, loc
        File(&'a str, &'a str, i64),
        /// src doc, dst doc, line
        Wikilink(&'a str, &'a str, i64),
    }

    /// A server over a fresh graph holding `rows`.
    fn seed_server(rows: &[Seed<'_>]) -> Server {
        let dir = tmp_root("seeded");
        let q = |s: &str| s.replace('\'', "''");
        doc_linter::store_sqlite::build_and_swap(&dir, |db| {
            for r in rows {
                let stmts: Vec<String> = match r {
                    Seed::Doc(id, summary, tags) => {
                        let mut v = vec![format!(
                            "INSERT INTO Doc(id, path, role, kind, lifecycle, \
                             bounded_context, title, summary, status, updated) \
                             VALUES ('{id}', '{id}.md', 'doc', 'reference', '', '', \
                             '{id}', '{}', 'stable', '2026-06-01')",
                            q(summary)
                        )];
                        v.extend(tags.iter().map(|t| {
                            format!(
                                "INSERT INTO doc_tags(doc_id, value) VALUES ('{id}', '{}')",
                                q(t)
                            )
                        }));
                        v
                    }
                    Seed::Entity(id, display) => vec![format!(
                        "INSERT INTO Entity(id, display) VALUES ('{id}', '{}')",
                        q(display)
                    )],
                    Seed::Function(symbol) => {
                        vec![format!("INSERT INTO Function(symbol) VALUES ('{symbol}')")]
                    }
                    Seed::Sql(stmt) => vec![(*stmt).to_string()],
                    Seed::File(path, lang, loc) => vec![format!(
                        "INSERT INTO File(path, language, loc) VALUES ('{path}', '{lang}', {loc})"
                    )],
                    Seed::Wikilink(a, b, line) => vec![format!(
                        "INSERT INTO WIKILINK(src, dst, line) VALUES ('{a}', '{b}', {line})"
                    )],
                };
                for st in stmts {
                    db.exec(&st, &[])?;
                }
            }
            Ok(())
        })
        .unwrap();
        Server::new(dir.clone(), ReadGraph::open(&dir).unwrap())
    }

    /// Server backed by a DB with the full graph schema applied.
    /// Tests that need typed tables (Function, Endpoint, Module,
    /// File, …) or `query_schema` introspection call this; the
    /// pre-#208 silent-None semantics show up only when the table
    /// exists and the row is absent.
    fn server_with_schema() -> Server {
        seed_server(&[])
    }

    /// Handoff cpg-os-vault #1: two docs sharing `id: rca` — ingest keeps
    /// one and drops the other. `reingest` used to answer `ok: true`
    /// because the silent check's report and exit code were discarded.

    #[test]
    fn reingest_reports_duplicate_ids_as_not_ok() {
        let s = server();
        for dir in ["a", "b", "c"] {
            let d = s.root.join("rca").join(dir);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(
                d.join("rca.md"),
                "---\nid: rca\ntitle: RCA\nsummary: t\nstatus: stable\nupdated: 2026-09-01\n---\n\n# RCA\n",
            )
            .unwrap();
        }
        let reply = s.tool_reingest(&json!({"no_vale": true})).unwrap();
        let v: Value = serde_json::from_str(reply["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["ok"], json!(false), "{v}");
        assert!(v["errors"].as_u64().unwrap() >= 1);
        let dups = v["skipped_duplicates"].as_array().unwrap();
        assert_eq!(dups.len(), 2, "{v}");
        // Every dupe names the real survivor (first writer), not the
        // previous dupe — the cpg-analyst 3-bundle case.
        for (dup, skipped) in dups.iter().zip(["rca/b/rca.md", "rca/c/rca.md"]) {
            assert_eq!(dup["id"], "rca");
            assert_eq!(dup["kept"], "rca/a/rca.md", "{v}");
            assert_eq!(dup["skipped"], skipped, "{v}");
        }
        let root = format!("{}", s.root.display());
        assert!(
            v["issues"]
                .as_array()
                .unwrap()
                .iter()
                .all(|i| !i["message"].as_str().unwrap().contains(&root)),
            "absolute path leaked into issues[].message: {v}"
        );
    }

    /// cpg-analyst (0.2.2): `embeddings = true` in .doc-lint.toml makes
    /// the embedding pass the reingest default; an explicit argument wins.

    #[test]
    fn reingest_embeddings_default_comes_from_config() {
        let s = server();
        std::fs::write(
            s.root.join(".doc-lint.toml"),
            "vale_enabled = false\nembeddings = true\n",
        )
        .unwrap();
        let text = |r: Value| -> Value {
            serde_json::from_str(r["content"][0]["text"].as_str().unwrap()).unwrap()
        };
        let v = text(s.tool_reingest(&json!({})).unwrap());
        assert_eq!(
            (v["embeddings"].clone(), v["embeddings_source"].clone()),
            (json!(true), json!("config"))
        );
        let v = text(s.tool_reingest(&json!({"embeddings": false})).unwrap());
        assert_eq!(
            (v["embeddings"].clone(), v["embeddings_source"].clone()),
            (json!(false), json!("argument"))
        );
    }

    /// Handoff cpg-os-vault #3: a fresh clone has no graph; `mcp` used to
    /// refuse to start, leaving no way to reach `reingest`.
    #[test]
    fn open_serving_db_builds_missing_graph() {
        let root = std::env::temp_dir().join(format!(
            "doc-linter-mcp-cold-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("note.md"),
            "---\nid: note\ntitle: Note\nsummary: t\nstatus: stable\nupdated: 2026-09-01\n---\n\n# Note\n",
        )
        .unwrap();
        assert!(!doc_linter::store_sqlite::graph_path(&root).exists());
        let db = open_serving_db(&root).unwrap();
        let rows = db.run_sql("SELECT id FROM Doc", vec![]).unwrap();
        assert_eq!(rows["rows"].as_array().unwrap().len(), 1, "{rows}");
    }

    /// Handoff cpg-os-vault #8: a docs-only graph (every code table empty)
    /// advertised all 31 tools and 269 code-graph saved queries.

    #[test]
    fn docs_only_graph_hides_code_tools_and_queries() {
        // cpg-analyst re-probe: the walker indexes config files, so a docs
        // vault has File rows; those must not count as code.
        let s = seed_server(&[Seed::File(".doc-lint.toml", "toml", 3)]);
        let v: Value = serde_json::from_str(
            &s.handle_line(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
                .unwrap(),
        )
        .unwrap();
        let names: Vec<&str> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(names.contains(&"query_doc") && names.contains(&"sql"));
        assert!(!names.contains(&"cypher"));
        for code_tool in [
            "list_files",
            "read_source",
            "classify_file_coupling",
            "query_dead_code",
            "query_endpoints",
        ] {
            assert!(!names.contains(&code_tool), "{code_tool} listed: {names:?}");
        }

        let listing = s.tool_query_saved(&json!({})).unwrap();
        let listing: Value =
            serde_json::from_str(listing["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(
            listing["hidden"].as_u64().unwrap() > 0,
            "{}",
            listing["hidden"]
        );
        assert!(listing["queries"].as_array().unwrap().iter().all(|q| {
            let needs: Vec<&str> = q["needs"]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            !needs.contains(&"Function") && !needs.contains(&"File")
        }));
    }

    /// cpg-os-9c on v0.2.2: with any `mcp` server holding the graph
    /// read-only, `check` could not write the index in place, so it stopped
    /// following merges for every agent. The rebuild now goes to a copy
    /// that is swapped in, and a running server reopens onto it.
    #[test]
    fn rebuild_succeeds_under_a_live_reader_and_the_reader_follows() {
        let root = tmp_root("swap");
        let add_doc = |id: &'static str| {
            move |db: &doc_linter::store_sqlite::SqliteDb| -> anyhow::Result<()> {
                db.exec(
                    &format!(
                        "INSERT INTO Doc(id, path, role, kind, lifecycle, bounded_context, \
                         title, summary, status, updated) VALUES ('{id}', '{id}.md', 'doc', '', \
                         '', '', '{id}', 's', 'stable', '2026-09-01')"
                    ),
                    &[],
                )
            }
        };
        doc_linter::store_sqlite::build_and_swap(&root, add_doc("a")).unwrap();
        let s = Server::new(root.clone(), ReadGraph::open(&root).unwrap());

        // A writer while the server holds its read-only handle.
        doc_linter::store_sqlite::build_and_swap(&root, add_doc("b")).unwrap();

        let reply = s
            .tools_call(
                json!({"name": "sql", "arguments": {"query": "SELECT id FROM Doc ORDER BY id"}}),
            )
            .unwrap();
        let v: Value = serde_json::from_str(reply["content"][0]["text"].as_str().unwrap()).unwrap();
        let ids: Vec<&str> = v["rows"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["id"].as_str())
            .collect();
        assert_eq!(ids, ["a", "b"], "server kept serving the pre-swap graph");
    }

    #[test]
    fn parse_error_returns_minus_32700() {
        let s = server();
        let reply = s.handle_line("{not json").expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32700));
        assert!(v["id"].is_null());
    }

    #[test]
    fn notification_yields_no_response() {
        let s = server();
        let n = s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        assert!(n.is_none(), "notifications never reply");
    }

    #[test]
    fn initialize_echoes_protocol_version_and_advertises_tools() {
        let s = server();
        let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;
        let reply = s.handle_line(req).expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"].as_i64(), Some(1));
        assert_eq!(v["result"]["protocolVersion"].as_str(), Some("2026-05-19"));
        assert!(v["result"]["capabilities"]["tools"].is_object());
        assert_eq!(
            v["result"]["serverInfo"]["name"].as_str(),
            Some("doc-linter")
        );
    }

    #[test]
    fn tools_list_returns_v3_catalog() {
        let s = server_with_code();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let tools = v["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        // v1 trio (the query tool is `sql`)
        assert!(names.contains(&query_tool()));
        assert!(!names.contains(&"cypher"));
        assert!(names.contains(&"list_entities"));
        assert!(names.contains(&"function_context"));
        // v2 additions
        assert!(names.contains(&"query_similar"));
        assert!(names.contains(&"query_dead_code"));
        assert!(names.contains(&"query_endpoints"));
        assert!(names.contains(&"query_impact"));
        // v3 additions
        assert!(names.contains(&"query_at"));
        assert!(names.contains(&"query_schema"));
        assert!(names.contains(&"query_saved"));
        // v4 addition
        assert!(names.contains(&"cluster"));
        // v11 addition
        assert!(names.contains(&"list_docs"));
        // v12 addition
        assert!(names.contains(&"list_types"));
        // v13 addition
        assert!(names.contains(&"query_path"));
        // v14 addition
        assert!(names.contains(&"list_findings"));
        // v15 additions — close the #35 spec catalog
        assert!(names.contains(&"query_doc"));
        assert!(names.contains(&"query_entity"));

        // Roadmap issue #28 v5 (v0.4.0): query_similar's schema
        // must declare the `backend` argument (with `bm25` and
        // `embedding` in its enum) so agents discover the option
        // via `tools/list` rather than guessing.
        let similar = tools
            .iter()
            .find(|t| t["name"] == "query_similar")
            .expect("query_similar tool present");
        let backend_schema = &similar["inputSchema"]["properties"]["backend"];
        assert!(
            backend_schema.is_object(),
            "query_similar must declare a `backend` property: {similar}"
        );
        let empty: Vec<Value> = Vec::new();
        let backend_enum: Vec<&str> = backend_schema["enum"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(
            backend_enum.contains(&"bm25") && backend_enum.contains(&"embedding"),
            "query_similar.backend enum must include `bm25` + `embedding`: {backend_enum:?}"
        );
    }

    /// Roadmap issue #35 v11: `list_docs` is wired through
    /// `tools/call` and returns the same `{total, returned, docs}`
    /// envelope `list_entities` uses. The test fixture has no
    /// SQLite graph so the call returns an error, but the dispatch
    /// path is exercised — verifying the tool name resolves and
    /// reaches the helper. (A full happy-path round-trip needs an
    /// integration fixture; this stays unit-scoped.)

    #[test]
    fn tools_call_list_docs_is_dispatched() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"list_docs","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        // Either the call succeeds (when a graph DB is available
        // in the test env) or it surfaces a graph-side error
        // via -32603. What we MUST NOT see is -32602 "unknown
        // tool" — that would mean dispatch never reached the
        // handler.
        if let Some(err) = v.get("error") {
            assert_ne!(
                err["code"].as_i64(),
                Some(-32602),
                "list_docs must not be unknown to the dispatcher: {v}",
            );
        } else {
            let result = &v["result"];
            assert!(
                result.get("content").is_some(),
                "list_docs success path returns a content envelope: {v}",
            );
        }
    }

    /// Roadmap issue #35 v12: `list_types` rejects unknown kind
    /// strings with -32602 (invalid params) before any DB work.
    /// Mirrors the CLI subcommand's same validation.

    #[test]
    fn tools_call_list_types_rejects_unknown_kind() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"list_types","arguments":{"kind":"function"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            v["error"]["code"].as_i64(),
            Some(-32602),
            "unknown kind must surface as invalid params: {v}",
        );
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("unknown kind 'function'"),
            "error message should name the bad kind: {msg}",
        );
    }

    /// gap-mcp-supervisor Slice B.1: end-to-end `run_worker`
    /// smoke. Binds a temp unix socket, runs the worker on a
    /// background thread, connects from the test thread, fires
    /// one valid `initialize`, reads back the response, then
    /// closes the client so the worker exits on EOF. Asserts the
    /// response parses as a JSON-RPC reply with the expected id.
    /// This is the integration analogue of the prior
    /// `serve_loop_works_over_buffered_io` test — that test
    /// covered the dispatch portability; this one covers the
    /// socket-accept glue the supervisor will speak to.
    #[cfg(unix)]
    #[test]
    fn run_worker_serves_over_unix_socket() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        use std::thread;
        use std::time::Duration;

        // Build a schema-seeded server so initialize has a valid DB.
        // We can't call run_worker directly because it owns the DB
        // open; instead we replicate its body with the temp server.
        let s = server();
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-worker-sock-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let socket_path = dir.join("worker.sock");
        let socket_for_server = socket_path.clone();

        let server_thread = thread::spawn(move || {
            let _ = std::fs::remove_file(&socket_for_server);
            let listener = std::os::unix::net::UnixListener::bind(&socket_for_server).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let read_half = stream.try_clone().unwrap();
            let reader = BufReader::new(read_half);
            let mut writer = stream;
            super::serve_loop(&s, reader, &mut writer).unwrap();
        });

        // Tiny retry: the server thread races to bind before our
        // connect. Up to 1 s of 10ms-spaced retries is plenty.
        let mut client = None;
        for _ in 0..100 {
            if let Ok(c) = UnixStream::connect(&socket_path) {
                client = Some(c);
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let mut client = client.expect("client connects within 1s");

        let req = b"{\"jsonrpc\":\"2.0\",\"id\":42,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2026-05-19\",\"capabilities\":{},\"clientInfo\":{\"name\":\"worker-test\",\"version\":\"0\"}}}\n";
        client.write_all(req).unwrap();
        client.flush().unwrap();

        let mut response_reader = BufReader::new(client.try_clone().unwrap());
        let mut response = String::new();
        response_reader.read_line(&mut response).unwrap();

        // BOTH the original FD (`client`) and the cloned FD inside
        // `response_reader` must drop before the server's BufRead
        // sees EOF and serve_loop exits — drop the reader first,
        // then the client, then join.
        drop(response_reader);
        drop(client);
        server_thread.join().unwrap();
        let _ = std::fs::remove_file(&socket_path);

        let v: Value = serde_json::from_str(response.trim()).unwrap();
        assert_eq!(v["id"].as_i64(), Some(42));
        assert_eq!(v["result"]["protocolVersion"].as_str(), Some("2026-05-19"));
    }

    /// gap-mcp-supervisor Slice B.3b: a `swap_worker` request
    /// sets the Server's flag, the response flushes, and
    /// `serve_loop` exits its read loop cleanly. The companion
    /// `run_worker` returns ExitCode(42) — the supervisor's
    /// outer loop (B.3c) checks that code to decide whether to
    /// respawn. This test pins the flag-flip + early-exit
    /// without needing a child process.

    #[test]
    fn swap_worker_sets_flag_and_breaks_serve_loop() {
        use std::io::Cursor;
        let s = server();
        // Two requests: swap_worker, then a second call we should
        // NOT see processed (serve_loop must break before
        // dispatching it).
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/call\",\"params\":{\"name\":\"swap_worker\",\"arguments\":{\"reason\":\"unit-test\"}}}\n\
                      {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let reader = Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();
        serve_loop(&s, reader, &mut writer).expect("serve_loop should succeed");
        let body = String::from_utf8(writer).expect("writer is utf-8");
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "exactly one response expected (swap_worker); the trailing tools/list must not run: {body}",
        );
        let v: Value = serde_json::from_str(lines[0]).expect("response parses as JSON");
        assert_eq!(v["id"].as_i64(), Some(1));
        assert!(
            s.swap_requested(),
            "swap_requested flag must be set after the handler runs",
        );
    }

    /// gap-mcp-supervisor Slice A: `serve_loop` is I/O-abstract.
    /// Run it against `Cursor<&[u8]>` (reader) + `Vec<u8>` (writer)
    /// with one valid initialize request and assert (a) the reply
    /// reaches the writer with a trailing newline + flush, (b) the
    /// loop exits when the reader is exhausted, and (c) the
    /// response is a valid JSON-RPC initialize result. This is the
    /// regression test that pins the dispatch's portability ahead
    /// of the supervisor + worker split.

    #[test]
    fn serve_loop_works_over_buffered_io() {
        use std::io::Cursor;
        let s = server();
        let input = b"{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2026-05-19\",\"capabilities\":{},\"clientInfo\":{\"name\":\"buf-test\",\"version\":\"0\"}}}\n";
        let reader = Cursor::new(&input[..]);
        let mut writer: Vec<u8> = Vec::new();
        serve_loop(&s, reader, &mut writer).expect("serve_loop should succeed");
        let body = String::from_utf8(writer).expect("writer is utf-8");
        assert!(
            body.ends_with('\n'),
            "every response is newline-terminated: {body:?}",
        );
        let v: Value = serde_json::from_str(body.trim()).expect("response parses as JSON");
        assert_eq!(v["id"].as_i64(), Some(99));
        assert_eq!(v["result"]["protocolVersion"].as_str(), Some("2026-05-19"));
    }

    /// Interrogation-001 gap-A: `audit_doc_region` must surface a
    /// loud -32602 error when `against` doesn't match any Repo
    /// row, not a silent zero-hit response. The schema_seeded
    /// fixture has no Repo rows, so any non-empty `against`
    /// reaches the validation branch and the error path is
    /// exercised. Verifies the canonical-id resolution prevents
    /// the silent-empty-result footgun that the CBR probe
    /// surfaced.

    #[test]
    fn audit_doc_region_rejects_unknown_against() {
        let s = server_with_schema();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":31,"method":"tools/call","params":{"name":"audit_doc_region","arguments":{"doc_path":"gap-005","against":"doc-linter-design"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        // Either the doc_path doesn't resolve (also -32602) OR the
        // against id is rejected (-32602). Both are acceptable
        // failure modes — what we MUST NOT see is a 200 with
        // hits=[] / corpus_size=0, which is the silent footgun.
        let err_code = v["error"]["code"].as_i64();
        assert_eq!(
            err_code,
            Some(-32602),
            "unknown against id must surface as -32602, not a silent empty hits response: {v}",
        );
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("doc-linter-design")
                || msg.contains("gap-005")
                || msg.contains("against")
                || msg.contains("Doc"),
            "error message should name the offending input: {msg}",
        );
    }

    /// Smoke test that `list_types` reaches the dispatcher; success
    /// vs. graph-side error depends on whether a DB is available in
    /// the test env, but -32602 unknown-tool is the regression we
    /// want to fail loudly.

    #[test]
    fn tools_call_list_types_is_dispatched() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"list_types","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        if let Some(err) = v.get("error") {
            assert_ne!(
                err["code"].as_i64(),
                Some(-32602),
                "list_types must be known to the dispatcher: {v}",
            );
        }
    }

    /// Roadmap issue #35 v13: `query_path` requires both `from`
    /// and `to`; missing either surfaces as -32602 invalid params
    /// before any DB work.

    #[test]
    fn query_path_missing_arguments_is_minus_32602() {
        let s = server();
        let missing_to = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":24,"method":"tools/call","params":{"name":"query_path","arguments":{"from":"doc-a"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&missing_to).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));

        let missing_from = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":25,"method":"tools/call","params":{"name":"query_path","arguments":{"to":"doc-b"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&missing_from).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));
    }

    /// Smoke test that `query_path` reaches the dispatcher.
    /// Success vs. graph-side error depends on whether a DB is
    /// available; the only error code we must not see is -32602
    /// "unknown tool".

    #[test]
    fn tools_call_query_path_is_dispatched() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":26,"method":"tools/call","params":{"name":"query_path","arguments":{"from":"doc-a","to":"doc-b"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        if let Some(err) = v.get("error") {
            let code = err["code"].as_i64();
            // -32602 "missing arg" would be a bug too (args are
            // present) but the "unknown tool" sentinel is the
            // primary regression vector.
            assert!(
                code != Some(-32602)
                    || !err["message"]
                        .as_str()
                        .unwrap_or_default()
                        .contains("unknown tool"),
                "query_path must be known to the dispatcher: {v}",
            );
        }
    }

    #[test]
    fn query_at_missing_arguments_is_minus_32602() {
        let s = server();
        let missing_line = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{"name":"query_at","arguments":{"file":"src/lib.rs"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&missing_line).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));

        let missing_file = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":13,"method":"tools/call","params":{"name":"query_at","arguments":{"line":42}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&missing_file).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));
    }

    #[test]
    fn query_similar_missing_text_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"query_similar","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));
    }

    #[test]
    fn query_impact_missing_symbol_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"query_impact","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));
    }

    #[test]
    fn unknown_method_is_minus_32601() {
        let s = server();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":3,"method":"sampling/createMessage"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32601));
    }

    #[test]
    fn tools_call_unknown_tool_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));
    }

    #[test]
    fn sql_missing_argument_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"sql","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));
    }

    /// Roadmap issue #206: the MCP `sql` tool is read-only. Anything that
    /// could write must surface as -32602 (invalid params) before any change
    /// reaches the graph, so an LLM-driven agent can never corrupt the cache
    /// by accident.
    #[test]
    fn sql_rejects_writes_with_minus_32602() {
        let s = server();
        for q in [
            "DELETE FROM Doc",
            "INSERT INTO Doc(id) VALUES ('x-injected')",
            "WITH d AS (SELECT 1) DELETE FROM Doc",
            "UPDATE Doc SET title = 'pwned'",
            "DROP TABLE Doc",
            "PRAGMA journal_mode = DELETE",
            "SELECT 1; DELETE FROM Doc",
            "ATTACH DATABASE ':memory:' AS m",
        ] {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 51,
                "method": "tools/call",
                "params": {"name": "sql", "arguments": {"query": q}},
            });
            let reply = s.handle_line(&req.to_string()).expect("response present");
            let v: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(-32602),
                "query `{q}` should be rejected: {v}"
            );
            let msg = v["error"]["message"].as_str().unwrap_or_default();
            assert!(
                msg.contains("read-only"),
                "error should hint at the read-only model: {msg}"
            );
        }
    }

    /// Read patterns still flow through the guard.
    #[test]
    fn sql_allows_select_and_with() {
        let s = server();
        for q in [
            "SELECT count(*) AS c FROM Doc",
            "WITH d AS (SELECT id FROM Doc) SELECT count(*) AS c FROM d",
        ] {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 52,
                "method": "tools/call",
                "params": {"name": "sql", "arguments": {"query": q}},
            });
            let reply = s.handle_line(&req.to_string()).expect("response present");
            let v: Value = serde_json::from_str(&reply).unwrap();
            assert!(v.get("error").is_none(), "`{q}` must run: {v}");
        }
    }

    /// The SQL tool went away with the old engine: calling it is an unknown tool.
    #[test]
    fn cypher_tool_is_gone() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":53,"method":"tools/call","params":{"name":"cypher","arguments":{"query":"MATCH (n) RETURN n"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "{v}");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"));
    }

    /// Roadmap issue #28 v5 + v6 (v0.4.0): end-to-end check that a
    /// `tools/call query_similar` with `backend: "embedding"` runs
    /// through the full pipeline — schema setup, real bge-small
    /// inference via the bundled model, FLOAT[384] persist + read,
    /// cosine ranking, MCP JSON-RPC response shape.
    ///
    /// Closes the last gap in the v0.4.0 test-coverage audit
    /// (`MCP --backend embedding DB round-trip`). Gated on
    /// `feature = "embeddings"` AND on the build-time bundled
    /// paths being baked in — same gating as
    /// `bundled_embedder_loads_via_default_factory` in
    /// `src/embeddings.rs` so the test no-ops cleanly when an
    /// operator built with `DOC_LINTER_SKIP_MODEL_DOWNLOAD=1`.
    #[cfg(feature = "embeddings")]
    #[test]
    fn mcp_query_similar_backend_embedding_round_trips_through_real_model() {
        if option_env!("DOC_LINTER_BUNDLED_EMBED_MODEL").is_none()
            || option_env!("DOC_LINTER_BUNDLED_EMBED_TOKENIZER").is_none()
        {
            return;
        }
        if std::env::var_os(doc_linter::embeddings::ENV_MODEL_PATH).is_some()
            || std::env::var_os(doc_linter::embeddings::ENV_TOKENIZER_PATH).is_some()
        {
            // Layer-1 override is set; the test asserts what the
            // bundled (layer-2) path produces. Skip rather than
            // accidentally exercise a different host model.
            return;
        }

        // Build a Server over a seeded graph. Three Doc rows with
        // semantically distinct summaries — the ranking assertion below
        // checks the right one wins on a query about cats.
        let rows = [
            (
                "d-cats",
                "Cats are small carnivorous mammals that purr and hunt rodents.",
            ),
            (
                "d-databases",
                "Embedded SQL databases handle ACID transactions on a single host.",
            ),
            (
                "d-music",
                "Jazz improvisation builds on chord changes and swing rhythm.",
            ),
        ];
        let seeds: Vec<Seed<'_>> = rows
            .iter()
            .map(|(id, sum)| Seed::Doc(id, sum, &[]))
            .collect();
        let server = seed_server(&seeds);

        // Populate the vector index via the layer-2 default (bundled
        // bge-small). The factory short-circuiting to NoOp would yield zero
        // vectors and the assertions below would fail with a helpful message.
        let embedder = doc_linter::embeddings::default_embedder();
        assert_eq!(
            embedder.name(),
            "onnx",
            "factory should resolve to ONNX via the layer-2 bundled path"
        );
        doc_linter::store_sqlite::build_and_swap(&server.root, |db| {
            doc_linter::store_sqlite::vectors::populate(
                db,
                Some(embedder.as_ref()),
                doc_linter::store_sqlite::vector::Index::new,
            )?;
            Ok(())
        })
        .unwrap();
        server.refresh_db_if_swapped();

        // Issue tools/call query_similar with the embedding
        // backend. Query text is about felines — the "d-cats"
        // doc should rank above the database / jazz docs by
        // cosine similarity.
        let reply = server
            .handle_line(
                r#"{"jsonrpc":"2.0","id":42,"method":"tools/call","params":{"name":"query_similar","arguments":{"text":"feline pet that purrs","backend":"embedding","top":3}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).expect("valid JSON-RPC envelope");

        assert!(
            v.get("error").is_none(),
            "MCP call returned an error envelope: {v}"
        );
        let content_text = v["result"]["content"][0]["text"]
            .as_str()
            .expect("MCP tool result has content[0].text");
        let result: Value =
            serde_json::from_str(content_text).expect("content text is JSON-serialised result");

        assert_eq!(
            result["algorithm"].as_str(),
            Some("embedding"),
            "algorithm field should report the cosine backend: {result}"
        );
        assert!(
            result["corpus_size"].as_u64().unwrap_or(0) >= 3,
            "corpus_size should reflect the seeded Doc rows: {result}"
        );
        let hits = result["hits"].as_array().expect("hits is an array");
        assert!(
            !hits.is_empty(),
            "embedding backend should return at least one hit: {result}"
        );
        // Top hit must be the cats doc — that's the property
        // that makes semantic search useful (it ranks higher
        // than the lexically-irrelevant databases / jazz docs
        // for a "feline pet that purrs" query).
        let top_id = hits[0]["id"]
            .as_str()
            .expect("top hit should have a string id");
        assert_eq!(
            top_id, "d-cats",
            "top embedding hit for 'feline pet that purrs' must be d-cats; got {top_id} \
             — full hits: {hits:?}"
        );
        // Cosine score is in [-1, 1]; a real semantic match
        // should clear at least a modest positive threshold.
        let top_score = hits[0]["score"].as_f64().unwrap_or(0.0);
        assert!(
            top_score > 0.3,
            "top cosine score should be meaningfully positive; got {top_score}"
        );
    }

    /// Roadmap issue #35 v14: `list_findings` rejects unknown
    /// severity strings with -32602 before any DB work.

    #[test]
    fn tools_call_list_findings_rejects_unknown_severity() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":27,"method":"tools/call","params":{"name":"list_findings","arguments":{"severity":"critical"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            v["error"]["code"].as_i64(),
            Some(-32602),
            "unknown severity must surface as invalid params: {v}",
        );
    }

    /// Smoke test that `list_findings` reaches the dispatcher.

    #[test]
    fn tools_call_list_findings_is_dispatched() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":28,"method":"tools/call","params":{"name":"list_findings","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        if let Some(err) = v.get("error") {
            assert_ne!(
                err["code"].as_i64(),
                Some(-32602),
                "list_findings must be known to the dispatcher: {v}",
            );
        }
    }

    /// Roadmap issue #35 v15: `query_doc` rejects a missing `id`
    /// with -32602 (invalid params), and any argument-shape-valid
    /// call is at least dispatched.

    #[test]
    fn query_doc_missing_id_is_minus_32602_and_dispatches_otherwise() {
        let s = server();
        let missing = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":20,"method":"tools/call","params":{"name":"query_doc","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&missing).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));

        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":21,"method":"tools/call","params":{"name":"query_doc","arguments":{"id":"does-not-exist"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        if let Some(err) = v.get("error") {
            assert_ne!(
                err["code"].as_i64(),
                Some(-32601),
                "query_doc must not be unknown to the dispatcher: {v}",
            );
        } else {
            assert!(
                v["result"].get("content").is_some(),
                "query_doc success path returns a content envelope: {v}",
            );
        }
    }

    /// Roadmap issue #211: `read_source` must reject paths that
    /// escape the repo root. Absolute paths, `..` traversal, and
    /// anything that canonicalizes outside `Server.root` all
    /// surface as -32602.

    #[test]
    fn read_source_rejects_path_traversal() {
        let s = server();
        for bad in ["/etc/passwd", "../../../etc/passwd", "../target"] {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 70,
                "method": "tools/call",
                "params": {"name": "read_source", "arguments": {"path": bad}},
            });
            let reply = s.handle_line(&req.to_string()).expect("response present");
            let v: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(-32602),
                "path `{bad}` must be rejected: {v}"
            );
        }
    }

    /// `read_source` returns a content envelope with the requested
    /// window. Smoke test against a freshly-written file inside
    /// the fixture's root.

    #[test]
    fn read_source_returns_window_around_centre_line() {
        let s = server();
        let path = s.root.join("sample.rs");
        std::fs::write(
            &path,
            "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n",
        )
        .unwrap();
        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 71,
            "method": "tools/call",
            "params": {
                "name": "read_source",
                "arguments": {"path": "sample.rs", "line": 3, "context_lines": 1},
            },
        });
        let reply = s.handle_line(&req.to_string()).expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let content_text = v["result"]["content"][0]["text"]
            .as_str()
            .expect("text envelope present");
        let result: Value = serde_json::from_str(content_text).expect("inner JSON");
        assert_eq!(result["start_line"].as_u64(), Some(2));
        assert_eq!(result["end_line"].as_u64(), Some(4));
        assert_eq!(result["line_count"].as_u64(), Some(3));
        let returned = result["content"].as_str().unwrap_or_default();
        assert!(returned.contains("fn two"), "got: {returned}");
        assert!(returned.contains("fn three"), "got: {returned}");
        assert!(returned.contains("fn four"), "got: {returned}");
        assert!(!returned.contains("fn one"), "got: {returned}");
    }

    /// `read_source` refuses files larger than the 8 MiB cap
    /// *before* the read, so an agent asking for a multi-GiB file
    /// can't OOM the process. The output cap alone wouldn't
    /// protect against that.

    #[test]
    fn read_source_rejects_oversized_file() {
        let s = server();
        let path = s.root.join("big.bin");
        // Write just over 8 MiB so the stat cap fires; we don't
        // need to actually fault any pages.
        let payload = vec![b'a'; (8 * 1024 * 1024 + 1) as usize];
        std::fs::write(&path, payload).unwrap();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":73,"method":"tools/call","params":{"name":"read_source","arguments":{"path":"big.bin"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "reply: {v}");
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("larger than"),
            "error should explain the cap: {msg}"
        );
    }

    /// `read_source` rejects `line: 0` mirroring `query_at`'s
    /// 1-based contract.

    #[test]
    fn read_source_rejects_line_zero() {
        let s = server();
        let path = s.root.join("sample.rs");
        std::fs::write(&path, "fn one() {}\n").unwrap();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":72,"method":"tools/call","params":{"name":"read_source","arguments":{"path":"sample.rs","line":0}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "reply: {v}");
    }

    /// Roadmap issue #212: new list_modules / list_files /
    /// list_migrations tools must reach the dispatcher (catches
    /// regressions like a missing match arm). Each accepts a
    /// schema-empty fixture without surfacing -32602 unknown tool.

    #[test]
    fn list_modules_files_migrations_dispatched() {
        let s = server_with_schema();
        for name in ["list_modules", "list_files", "list_migrations"] {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 80,
                "method": "tools/call",
                "params": {"name": name, "arguments": {}},
            });
            let reply = s.handle_line(&req.to_string()).expect("response present");
            let v: Value = serde_json::from_str(&reply).unwrap();
            if let Some(err) = v.get("error") {
                let msg = err["message"].as_str().unwrap_or_default();
                assert!(
                    !msg.contains("unknown tool"),
                    "{name} must be known to dispatcher: {v}"
                );
            }
            let result = &v["result"];
            assert!(
                result.get("content").is_some(),
                "{name} success path returns content envelope: {v}",
            );
        }
    }

    /// All three new tools advertise in `tools/list`.

    #[test]
    fn tools_list_includes_new_list_tools() {
        let s = server_with_code();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":81,"method":"tools/list"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let tools = v["result"]["tools"].as_array().expect("tools array");
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"list_modules"), "{names:?}");
        assert!(names.contains(&"list_files"), "{names:?}");
        assert!(names.contains(&"list_migrations"), "{names:?}");
    }

    /// Per user-probe-034: the new classify_file_coupling tool must
    /// surface in tools/list with the 3-tier framework in the description.

    #[test]
    fn tools_list_includes_classify_file_coupling() {
        let s = server_with_code();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":155,"method":"tools/list"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let tools = v["result"]["tools"].as_array().expect("tools array");
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some("classify_file_coupling"))
            .expect("classify_file_coupling missing");
        let desc = tool["description"].as_str().unwrap_or_default();
        // The description must name all four tiers so the agent can
        // choose to call this vs files-coupled-to based on the
        // catalog row alone.
        assert!(
            desc.contains("absence"),
            "tier name `absence` missing: {desc}"
        );
        assert!(
            desc.contains("minimal"),
            "tier name `minimal` missing: {desc}"
        );
        assert!(desc.contains("heavy"), "tier name `heavy` missing: {desc}");
        assert!(
            desc.contains("moderate"),
            "tier name `moderate` missing: {desc}"
        );
        assert!(
            desc.contains("When to use") && desc.contains("When NOT to use"),
            "ACI use-guidance lines missing: {desc}"
        );
        // The path arg must be required so the dispatch error is loud.
        let required = tool["inputSchema"]["required"]
            .as_array()
            .expect("required array");
        assert!(
            required.iter().any(|v| v.as_str() == Some("path")),
            "path must be required: {required:?}",
        );
    }

    /// Per iter 99: the new query_doc_neighbors tool must surface in
    /// tools/list with its FOCUS+IsoNet framing in the description.

    #[test]
    fn tools_list_includes_query_doc_neighbors() {
        let s = server();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":99,"method":"tools/list"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let tools = v["result"]["tools"].as_array().expect("tools array");
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some("query_doc_neighbors"))
            .expect("query_doc_neighbors missing");
        let desc = tool["description"].as_str().unwrap_or_default();
        assert!(
            desc.contains("shared_entities"),
            "description should name the interpretable shared_entities field: {desc}"
        );
        assert!(
            desc.contains("When to use") && desc.contains("When NOT to use"),
            "description should carry the ACI purpose lines: {desc}"
        );
        // Check input schema names `id` (required) + `top` (optional).
        let schema = &tool["inputSchema"];
        assert_eq!(schema["properties"]["id"]["type"], "string");
        assert_eq!(schema["properties"]["top"]["type"], "integer");
        let required = schema["required"].as_array().expect("required array");
        let required_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert_eq!(required_strs, vec!["id"]);
    }

    #[test]
    fn merge_neighbor_views_fuses_scores_per_neighbor() {
        let covers = vec![
            json!({"id": "a", "title": "A", "shared_entities": ["e1"], "shared_count": 2}),
            json!({"id": "b", "title": "B", "shared_entities": ["e1", "e2"], "shared_count": 3}),
        ];
        let wikilink = vec![
            json!({"id": "a", "title": "A", "shared_count": 5}),
            json!({"id": "c", "title": "C", "shared_count": 1}),
        ];
        let merged = merge_neighbor_views(&covers, &wikilink, 10);
        assert_eq!(merged.len(), 3, "3 unique neighbors after merge");
        // a appears in both → fused = 2 + 5 = 7
        let a = merged.iter().find(|r| r["id"] == "a").unwrap();
        assert_eq!(a["covers_score"], 2);
        assert_eq!(a["wikilink_score"], 5);
        assert_eq!(a["fused_score"], 7);
        assert_eq!(a["shared_entities"], json!(["e1"]));
        // b only in covers → wikilink_score = 0, fused = 3
        let b = merged.iter().find(|r| r["id"] == "b").unwrap();
        assert_eq!(b["covers_score"], 3);
        assert_eq!(b["wikilink_score"], 0);
        assert_eq!(b["fused_score"], 3);
        // c only in wikilink → covers_score = 0, fused = 1
        let c = merged.iter().find(|r| r["id"] == "c").unwrap();
        assert_eq!(c["covers_score"], 0);
        assert_eq!(c["wikilink_score"], 1);
        assert_eq!(c["fused_score"], 1);
        // Ordering: fused DESC, so a (7) > b (3) > c (1)
        assert_eq!(merged[0]["id"], "a");
        assert_eq!(merged[1]["id"], "b");
        assert_eq!(merged[2]["id"], "c");
    }

    #[test]
    fn merge_neighbor_views_truncates_to_top() {
        let covers = vec![
            json!({"id": "a", "title": "A", "shared_entities": [], "shared_count": 5}),
            json!({"id": "b", "title": "B", "shared_entities": [], "shared_count": 3}),
            json!({"id": "c", "title": "C", "shared_entities": [], "shared_count": 1}),
        ];
        let merged = merge_neighbor_views(&covers, &[], 2);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0]["id"], "a");
        assert_eq!(merged[1]["id"], "b");
    }

    #[test]
    fn merge_neighbor_views_handles_empty_inputs() {
        let merged = merge_neighbor_views(&[], &[], 10);
        assert!(merged.is_empty());
    }

    #[test]
    fn suggest_tags_matches_kebab_form_in_summary() {
        let known = vec![
            "agent-shell".to_string(),
            "code-rag".to_string(),
            "graph-rag".to_string(),
        ];
        let summary = "An open-source agent-shell for VS Code";
        let current: Vec<String> = vec!["research".to_string(), "tool".to_string()];
        let out = suggest_tags(summary, &current, &known);
        assert_eq!(out.len(), 1, "only agent-shell should match: {out:?}");
        assert_eq!(out[0]["tag"], "agent-shell");
        assert_eq!(out[0]["evidence"], "agent-shell");
    }

    #[test]
    fn suggest_tags_matches_space_form_in_summary() {
        let known = vec!["agent-shell".to_string()];
        // SWE-agent's actual summary case: uses "agent shell" space form.
        let summary = "Ships a constrained tool surface inside an agent shell.";
        let current: Vec<String> = vec!["research".to_string()];
        let out = suggest_tags(summary, &current, &known);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["tag"], "agent-shell");
        assert_eq!(out[0]["evidence"], "agent shell");
    }

    #[test]
    fn suggest_tags_skips_already_carried_tag() {
        let known = vec!["agent-shell".to_string()];
        let summary = "An agent shell ships a fixed tool list";
        let current: Vec<String> = vec!["agent-shell".to_string(), "research".to_string()];
        let out = suggest_tags(summary, &current, &known);
        assert!(
            out.is_empty(),
            "already-carried tag should not appear: {out:?}"
        );
    }

    #[test]
    fn suggest_tags_skips_short_common_word_tags() {
        // Defensive: a 2-char tag like "ai" would false-positive on any
        // summary mentioning "AI". Skip short tags.
        let known = vec!["ai".to_string(), "agent-shell".to_string()];
        let summary = "AI agent shell";
        let current: Vec<String> = vec![];
        let out = suggest_tags(summary, &current, &known);
        let tags: Vec<&str> = out.iter().filter_map(|v| v["tag"].as_str()).collect();
        assert!(
            !tags.contains(&"ai"),
            "short 2-char tag should be skipped: {tags:?}"
        );
        assert!(tags.contains(&"agent-shell"));
    }

    #[test]
    fn suggest_tags_is_case_insensitive() {
        let known = vec!["graph-rag".to_string()];
        let summary = "Builds a GRAPH RAG over a corpus of docs";
        let current: Vec<String> = vec![];
        let out = suggest_tags(summary, &current, &known);
        assert_eq!(out.len(), 1, "should match despite uppercase: {out:?}");
    }

    #[test]
    fn suggest_tags_handles_empty_inputs() {
        assert!(suggest_tags("", &[], &[]).is_empty());
        assert!(suggest_tags("some summary", &[], &[]).is_empty());
    }

    #[test]
    fn should_watchdog_fire_when_elapsed_exceeds_timeout() {
        // gap-mcp-crash-orphans-the store-lock (iter 175): the watchdog
        // fires when (now - last) >= timeout. Use synthetic Instants
        // by sleeping briefly — Instant arithmetic doesn't allow
        // construction from arbitrary durations.
        let start = std::time::Instant::now();
        let later = start + std::time::Duration::from_secs(10);
        // Just under: don't fire.
        assert!(
            !should_watchdog_fire(start, later, std::time::Duration::from_secs(11)),
            "must not fire when elapsed < timeout",
        );
        // Exactly at: fires (per >= semantics).
        assert!(
            should_watchdog_fire(start, later, std::time::Duration::from_secs(10)),
            "must fire at the exact timeout boundary",
        );
        // Well past: fires.
        assert!(
            should_watchdog_fire(start, later, std::time::Duration::from_secs(5)),
            "must fire when elapsed exceeds timeout",
        );
    }

    #[test]
    fn read_idle_timeout_from_env_falls_back_to_default() {
        // No env var set → DEFAULT_IDLE_TIMEOUT_SECS.
        let timeout = read_idle_timeout_from_env(|_| None);
        assert_eq!(
            timeout,
            std::time::Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            "must fall back to DEFAULT when env var unset",
        );
        // Default is 4 hours per user-probe-040 / orphan-MCP recovery.
        assert_eq!(DEFAULT_IDLE_TIMEOUT_SECS, 4 * 60 * 60);
    }

    #[test]
    fn read_idle_timeout_from_env_honors_override() {
        let timeout = read_idle_timeout_from_env(|k| {
            if k == "DOC_LINTER_MCP_IDLE_TIMEOUT_SECS" {
                Some("90".to_string())
            } else {
                None
            }
        });
        assert_eq!(timeout, std::time::Duration::from_secs(90));
    }

    #[test]
    fn read_idle_timeout_from_env_falls_back_on_non_numeric() {
        // Garbage env var → fall back to default (don't crash).
        let timeout = read_idle_timeout_from_env(|k| {
            if k == "DOC_LINTER_MCP_IDLE_TIMEOUT_SECS" {
                Some("not-a-number".to_string())
            } else {
                None
            }
        });
        assert_eq!(
            timeout,
            std::time::Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            "garbage env value must fall back to default, not panic",
        );
    }

    #[test]
    fn suggest_family_tags_catches_voyage_code_3_embeddings_plural_drift() {
        // Per iter 163 (interrogation-032 Finding C): research-voyage-
        // code-3 carries `embeddings` (plural); family tag is
        // `embedding-model` (singular). Stem `embedding` is Levenshtein-1
        // from `embeddings`. Both tokens are ≥ 4 chars. Suggest.
        let carried: Vec<String> = vec![
            "research".to_string(),
            "embeddings".to_string(),
            "code-rag".to_string(),
            "tool".to_string(),
            "production".to_string(),
        ];
        let known: Vec<String> = vec![
            "embedding-model".to_string(),
            "graph-similarity".to_string(),
            "production".to_string(),
        ];
        let suggestions = suggest_family_tags(&carried, &known);
        let tags: Vec<&str> = suggestions
            .iter()
            .filter_map(|s| s.get("tag").and_then(|v| v.as_str()))
            .collect();
        assert!(
            tags.contains(&"embedding-model"),
            "must surface embedding-model from embeddings ≈ embedding: {tags:?}",
        );
        // Should NOT suggest `production` — already carried.
        assert!(
            !tags.contains(&"production"),
            "must not suggest already-carried tag: {tags:?}",
        );
        // Should NOT suggest `graph-similarity` — no carried-tag's
        // token shares a 4+ char stem with any of {graph, similarity}.
        assert!(
            !tags.contains(&"graph-similarity"),
            "must not suggest unrelated tag: {tags:?}",
        );
    }

    #[test]
    fn suggest_family_tags_catches_graph2vec_via_exact_token_overlap_rule2() {
        // Per iter 163 (interrogation-032 Finding C, second instance):
        // research-graph2vec carries `graph-embedding` +
        // `structural-similarity`. Family tag `graph-similarity` shares
        // discriminative token `graph` (5 chars, NOT in stopword set)
        // with carried `graph-embedding`. Rule 2 emits it.
        let carried: Vec<String> = vec![
            "research".to_string(),
            "graph-embedding".to_string(),
            "structural-similarity".to_string(),
        ];
        let known: Vec<String> = vec![
            "graph-similarity".to_string(),
            "embedding-model".to_string(),
            "production".to_string(), // already carried-equivalent NOT
        ];
        let suggestions = suggest_family_tags(&carried, &known);
        let tags: Vec<&str> = suggestions
            .iter()
            .filter_map(|s| s.get("tag").and_then(|v| v.as_str()))
            .collect();
        assert!(
            tags.contains(&"graph-similarity"),
            "Rule 2 must surface graph-similarity via shared discriminative token `graph`: {tags:?}",
        );
        // embedding-model: known token `embedding` ≥ 5 chars NOT
        // stopword; carried `graph-embedding` token `embedding` exact
        // match. ALSO surfaces via Rule 2.
        assert!(
            tags.contains(&"embedding-model"),
            "Rule 2 must surface embedding-model via shared token `embedding`: {tags:?}",
        );
    }

    #[test]
    fn suggest_family_tags_skips_stopword_only_matches() {
        // `research` is in the stopword set. A doc that carries
        // `research` + literally nothing else discriminative must NOT
        // get every known tag containing `research` suggested.
        let carried: Vec<String> = vec!["research".to_string(), "tool".to_string()];
        let known: Vec<String> = vec!["research-direction".to_string()];
        let suggestions = suggest_family_tags(&carried, &known);
        let tags: Vec<&str> = suggestions
            .iter()
            .filter_map(|s| s.get("tag").and_then(|v| v.as_str()))
            .collect();
        assert!(
            !tags.contains(&"research-direction"),
            "stopword-only overlap must NOT emit a suggestion: {tags:?}",
        );
    }

    #[test]
    fn suggest_family_tags_returns_each_known_at_most_once() {
        // Even if two carried tags share stems with the same known tag,
        // the suggestion appears once (first match wins).
        let carried: Vec<String> = vec!["embeddings".to_string(), "embedded".to_string()];
        let known: Vec<String> = vec!["embedding-model".to_string()];
        let suggestions = suggest_family_tags(&carried, &known);
        assert_eq!(
            suggestions.len(),
            1,
            "embedding-model must appear at most once even with two close carried tags: {suggestions:?}",
        );
    }

    /// Smoke test that `query_doc_neighbors` reaches the dispatcher
    /// (gets past tools/call → dispatch). Without a real DB the call
    /// may still error on the sql execution, but we must NOT see
    /// the -32602 unknown-tool variant — that's the regression this
    /// pins.

    #[test]
    fn tools_call_query_doc_neighbors_is_dispatched() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":100,"method":"tools/call","params":{"name":"query_doc_neighbors","arguments":{"id":"any-doc-id"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let err_code = v["error"]["code"].as_i64();
        // -32601 (method not found) or unknown-tool variants would
        // signal the dispatch arm is missing; that's the regression.
        // Any OTHER outcome (success, graph-side error, etc) is fine.
        if let Some(code) = err_code {
            let msg = v["error"]["message"].as_str().unwrap_or_default();
            assert_ne!(
                code, -32601,
                "query_doc_neighbors must be reachable from dispatch, not -32601: {msg}"
            );
            assert!(
                !msg.contains("unknown tool") && !msg.contains("Unknown tool"),
                "query_doc_neighbors must be reachable from dispatch, got unknown-tool: {msg}"
            );
        }
    }

    /// Per iter 91 + research-swe-agent's ACI principle: the
    /// most-called MCP tools must carry purpose-when-applied
    /// discoverability lines in their descriptions so the agent's
    /// tools/list cold-start reveals "when to use" / "when NOT to
    /// use" instead of only what-the-tool-does. Closes the
    /// tool_purpose discoverability axis user-probe-011 named.

    #[test]
    fn tools_list_carries_purpose_when_applied_lines() {
        let s = server_with_code();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":91,"method":"tools/list"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let tools = v["result"]["tools"].as_array().expect("tools array");
        for required in [
            query_tool(),
            "query_similar",
            "query_saved",
            "query_schema",
            "read_source",
            "audit_doc_region",
        ] {
            let tool = tools
                .iter()
                .find(|t| t["name"].as_str() == Some(required))
                .unwrap_or_else(|| panic!("tool `{required}` missing from tools/list"));
            let desc = tool["description"].as_str().unwrap_or_default();
            assert!(
                desc.contains("When to use"),
                "tool `{required}` description missing `When to use:`\n{desc}"
            );
            assert!(
                desc.contains("When NOT to use"),
                "tool `{required}` description missing `When NOT to use:`\n{desc}"
            );
        }
    }

    /// Roadmap issue #213: negative offset is rejected with -32602
    /// across every list-style tool.

    #[test]
    fn list_tools_reject_negative_offset() {
        let s = server_with_schema();
        for name in [
            "list_entities",
            "list_docs",
            "list_types",
            "list_findings",
            "list_modules",
            "list_files",
            "list_migrations",
            "query_dead_code",
            "query_endpoints",
        ] {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 82,
                "method": "tools/call",
                "params": {"name": name, "arguments": {"offset": -1}},
            });
            let reply = s.handle_line(&req.to_string()).expect("response present");
            let v: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(-32602),
                "{name} must reject offset=-1: {v}"
            );
            let msg = v["error"]["message"].as_str().unwrap_or_default();
            assert!(
                msg.contains("offset"),
                "{name} error message should name offset: {msg}"
            );
        }
    }

    /// `paginate` applies offset before truncation; the envelope
    /// echoes the offset so a paging client can checkpoint.
    #[test]
    fn paginate_drops_offset_then_truncates() {
        let mut rows: Vec<i32> = (0..10).collect();
        paginate(&mut rows, 3, 4);
        assert_eq!(rows, vec![3, 4, 5, 6]);
        let mut rows: Vec<i32> = vec![1, 2, 3];
        paginate(&mut rows, 5, 0);
        assert!(rows.is_empty(), "offset past end clears the slice");
        let mut rows: Vec<i32> = vec![1, 2, 3, 4, 5];
        paginate(&mut rows, 0, 0);
        assert_eq!(rows, vec![1, 2, 3, 4, 5], "0/0 is a noop");
    }

    /// Roadmap issue #214: batched JSON-RPC requests return an
    /// array of responses (per spec, even a single-element batch
    /// returns an array, not a bare object).

    #[test]
    fn jsonrpc_batch_returns_array_of_responses() {
        let s = server();
        let reply = s
            .handle_line(
                r#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"},{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{}}}]"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).expect("array reply");
        let arr = v.as_array().expect("response is array");
        assert_eq!(arr.len(), 2, "two requests → two responses: {v}");
        let ids: Vec<i64> = arr.iter().filter_map(|r| r["id"].as_i64()).collect();
        assert!(ids.contains(&1) && ids.contains(&2), "ids preserved: {v}");
    }

    /// Empty batch is -32600 Invalid Request per spec.

    #[test]
    fn jsonrpc_empty_batch_is_minus_32600() {
        let s = server();
        let reply = s.handle_line("[]").expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32600));
        assert!(v["id"].is_null());
    }

    /// Notification-only batches produce no output (per spec).
    /// Mixed batches (request + notification) emit only the
    /// request's response.

    #[test]
    fn jsonrpc_batch_handles_notifications() {
        let s = server();
        let no_reply = s.handle_line(
            r#"[{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","method":"notifications/cancelled"}]"#,
        );
        assert!(
            no_reply.is_none(),
            "notification-only batch yields no reply"
        );

        let mixed = s
            .handle_line(
                r#"[{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":99,"method":"tools/list"}]"#,
            )
            .expect("mixed batch has at least one reply");
        let v: Value = serde_json::from_str(&mixed).unwrap();
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 1, "only the request gets a reply: {v}");
        assert_eq!(arr[0]["id"].as_i64(), Some(99));
    }

    /// Roadmap issue #215: `query_schema` returns enriched
    /// metadata — node_tables / rel_tables grouping, each with
    /// per-column properties (name + dtype + is_primary_key).

    #[test]
    fn query_schema_returns_enriched_shape() {
        let s = server_with_schema();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":90,"method":"tools/call","params":{"name":"query_schema","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let content_text = v["result"]["content"][0]["text"]
            .as_str()
            .expect("text envelope");
        let result: Value = serde_json::from_str(content_text).unwrap();
        let nodes = result["node_tables"].as_array().expect("node_tables array");
        let rels = result["rel_tables"].as_array().expect("rel_tables array");
        assert!(!nodes.is_empty(), "schema has node tables: {result}");
        assert!(!rels.is_empty(), "schema has rel tables: {result}");
        let doc = nodes
            .iter()
            .find(|n| n["name"] == "Doc")
            .expect("Doc node table");
        let props = doc["properties"].as_array().expect("Doc properties array");
        let id_col = props.iter().find(|p| p["name"] == "id").expect("Doc.id");
        assert_eq!(id_col["is_primary_key"].as_bool(), Some(true));
        assert_eq!(
            id_col["dtype"].as_str(),
            Some("TEXT"),
            "Doc.id dtype should be TEXT: {id_col}"
        );
        // Rel tables carry from/to endpoints.
        let belongs = rels
            .iter()
            .find(|r| r["name"] == "FUNCTION_BELONGS_TO")
            .expect("FUNCTION_BELONGS_TO rel");
        assert_eq!(belongs["from"].as_str(), Some("Function"));
        assert_eq!(belongs["to"].as_str(), Some("Entity"));
    }

    /// Roadmap issue #216: `query_saved` with `describe: true`
    /// returns the catalog row without running the query.

    #[test]
    fn query_saved_describe_returns_catalog_row() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":91,"method":"tools/call","params":{"name":"query_saved","arguments":{"name":"orphan-entities","describe":true}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        let content_text = v["result"]["content"][0]["text"]
            .as_str()
            .expect("text envelope");
        let result: Value = serde_json::from_str(content_text).unwrap();
        assert_eq!(result["name"].as_str(), Some("orphan-entities"));
        assert!(result["sql"].as_str().is_some(), "sql present: {result}");
        assert!(result["params"].is_array());
    }

    /// Roadmap issue #216: `query_saved` missing required param
    /// surfaces as -32602 (was -32000 pre-fix).

    #[test]
    fn query_saved_missing_param_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":92,"method":"tools/call","params":{"name":"query_saved","arguments":{"name":"entity-callers"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "reply: {v}");
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("entity"),
            "error names the missing param: {msg}"
        );
    }

    /// Roadmap issue #216: `query_saved` with an unknown name is
    /// -32602 (was -32000 pre-fix). The catalog is listed in the
    /// error message.

    #[test]
    fn query_saved_unknown_name_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":93,"method":"tools/call","params":{"name":"query_saved","arguments":{"name":"no-such-thing"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "reply: {v}");
    }

    /// Roadmap issue #216: negative integer args are rejected with
    /// -32602 across the surface — `top`, `depth`, `max_hops`. The
    /// rejection must fire before any DB work, so an agent talking
    /// to a degraded graph still sees the parameter violation
    /// instead of a generic -32000 from the DB layer.

    #[test]
    fn negative_int_args_rejected_with_minus_32602() {
        let s = server();
        for (name, args) in [
            ("list_entities", r#"{"top": -1}"#),
            ("list_docs", r#"{"top": -1}"#),
            ("list_types", r#"{"top": -1}"#),
            ("list_findings", r#"{"top": -1}"#),
            ("query_dead_code", r#"{"top": -1}"#),
            ("query_endpoints", r#"{"top": -1}"#),
            ("query_path", r#"{"from":"a","to":"b","max_hops":-1}"#),
            (
                "query_impact",
                r#"{"symbol":"rust-analyzer cargo x 0 src/lib.rs/f().","depth":-1}"#,
            ),
            ("query_entity", r#"{"id":"a","depth":-1}"#),
        ] {
            let req = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 94,
                "method": "tools/call",
                "params": {"name": name, "arguments": serde_json::from_str::<Value>(args).unwrap()},
            });
            let reply = s.handle_line(&req.to_string()).expect("response present");
            let v: Value = serde_json::from_str(&reply).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(-32602),
                "{name} should reject negative int: {v}"
            );
        }
    }

    /// Roadmap issue #216: `ping` method returns an empty object
    /// without needing a full `initialize` round-trip.

    #[test]
    fn ping_returns_empty_object() {
        let s = server();
        let reply = s
            .handle_line(r#"{"jsonrpc":"2.0","id":95,"method":"ping"}"#)
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert!(v.get("error").is_none(), "ping shouldn't error: {v}");
        assert_eq!(v["result"], json!({}));
    }

    /// Same shape check for `query_entity`.

    #[test]
    fn query_entity_missing_id_is_minus_32602_and_dispatches_otherwise() {
        let s = server();
        let missing = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":22,"method":"tools/call","params":{"name":"query_entity","arguments":{}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&missing).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602));

        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":23,"method":"tools/call","params":{"name":"query_entity","arguments":{"id":"nope-entity"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        if let Some(err) = v.get("error") {
            assert_ne!(
                err["code"].as_i64(),
                Some(-32601),
                "query_entity must not be unknown to the dispatcher: {v}",
            );
        } else {
            assert!(
                v["result"].get("content").is_some(),
                "query_entity success path returns a content envelope: {v}",
            );
        }
    }

    /// Roadmap issue #207: bogus `kind` on `query_endpoints`
    /// must surface as -32602 before any DB work, naming the
    /// rejected value and the valid enum.

    #[test]
    fn query_endpoints_rejects_unknown_kind() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":60,"method":"tools/call","params":{"name":"query_endpoints","arguments":{"kind":"bogusframework"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "reply: {v}");
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("bogusframework"),
            "error should name the bad kind: {msg}"
        );
        assert!(
            msg.contains("axum") && msg.contains("express"),
            "error should list the valid enum: {msg}"
        );
    }

    /// `query_endpoints` accepts the `entity_id` filter without
    /// crashing on an empty fixture.

    #[test]
    fn query_endpoints_accepts_entity_id_arg() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":61,"method":"tools/call","params":{"name":"query_endpoints","arguments":{"entity_id":"doc-graph"}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        // -32602 here would mean "unknown tool" or "bad arg shape".
        // The DB may be empty (graph-side error allowed) but
        // entity_id must be accepted as an argument.
        if let Some(err) = v.get("error") {
            let msg = err["message"].as_str().unwrap_or_default();
            assert!(
                !msg.contains("entity_id"),
                "entity_id should be a valid arg: {msg}"
            );
        }
    }

    /// Roadmap issue #208: `function_context` raises -32602 on an
    /// unknown symbol, mirroring `query_doc` / `query_entity`.
    /// Without a seeded Function, the fixture's empty DB returns
    /// None — the regression target.

    #[test]
    fn function_context_unknown_symbol_is_minus_32602() {
        let s = server_with_schema();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":62,"method":"tools/call","params":{"name":"function_context","arguments":{"symbol":"does/not/exist."}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            v["error"]["code"].as_i64(),
            Some(-32602),
            "unknown symbol must surface as -32602: {v}"
        );
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("does/not/exist."),
            "error should include the searched symbol: {msg}"
        );
    }

    /// `query_impact` matches the same -32602 semantics on an
    /// unknown symbol.

    #[test]
    fn query_impact_unknown_symbol_is_minus_32602() {
        let s = server_with_schema();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":63,"method":"tools/call","params":{"name":"query_impact","arguments":{"symbol":"does/not/exist."}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(
            v["error"]["code"].as_i64(),
            Some(-32602),
            "unknown symbol must surface as -32602: {v}"
        );
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("does/not/exist."),
            "error should include the searched symbol: {msg}"
        );
    }

    /// Roadmap issue #209: `line < 1` is rejected before any DB
    /// work with a clear -32602 hint about 1-based indexing.

    #[test]
    fn query_at_line_zero_is_minus_32602() {
        let s = server();
        let reply = s
            .handle_line(
                r#"{"jsonrpc":"2.0","id":64,"method":"tools/call","params":{"name":"query_at","arguments":{"file":"src/lib.rs","line":0}}}"#,
            )
            .expect("response present");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(-32602), "reply: {v}");
        let msg = v["error"]["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("1-based") || msg.contains(">= 1"),
            "error should explain the 1-based contract: {msg}"
        );
    }

    fn rows_result(n: usize) -> Value {
        let rows: Vec<Value> = (0..n).map(|i| json!({"id": format!("row-{i}")})).collect();
        json!({
            "columns": ["id"],
            "row_count": rows.len(),
            "rows": rows,
        })
    }

    #[test]
    fn cap_result_passes_through_when_under_cap() {
        let result = rows_result(5);
        let capped = cap_result(result, 200);
        assert_eq!(capped["row_count"].as_u64(), Some(5));
        assert_eq!(capped["rows"].as_array().unwrap().len(), 5);
        assert_eq!(capped["truncated"].as_bool(), Some(false));
        assert!(capped.get("total_row_count").is_none());
    }

    #[test]
    fn cap_result_truncates_and_sets_total_when_over_cap() {
        let result = rows_result(500);
        let capped = cap_result(result, 200);
        assert_eq!(capped["row_count"].as_u64(), Some(200));
        assert_eq!(capped["rows"].as_array().unwrap().len(), 200);
        assert_eq!(capped["truncated"].as_bool(), Some(true));
        assert_eq!(capped["total_row_count"].as_u64(), Some(500));
        let first = &capped["rows"][0];
        assert_eq!(first["id"].as_str(), Some("row-0"));
        let last = &capped["rows"][199];
        assert_eq!(last["id"].as_str(), Some("row-199"));
    }

    #[test]
    fn cap_result_equal_to_cap_is_not_truncated() {
        let result = rows_result(200);
        let capped = cap_result(result, 200);
        assert_eq!(capped["row_count"].as_u64(), Some(200));
        assert_eq!(capped["truncated"].as_bool(), Some(false));
        assert!(capped.get("total_row_count").is_none());
    }

    fn catalog_names() -> Vec<String> {
        vec![
            "recent-research".to_string(),
            "recent-audits".to_string(),
            "research-by-tag".to_string(),
            "tag-coverage".to_string(),
            "docs-by-kind".to_string(),
        ]
    }

    #[test]
    fn closest_saved_query_suggestion_single_typo() {
        let s = closest_saved_query_suggestion("recent-resarch", &catalog_names());
        assert!(s.contains("recent-research"), "got: {s}");
        assert!(s.starts_with(" Did you mean"), "got: {s}");
    }

    #[test]
    fn closest_saved_query_suggestion_two_close_matches() {
        let s = closest_saved_query_suggestion("recent-audit", &catalog_names());
        assert!(s.contains("recent-audits"), "got: {s}");
    }

    #[test]
    fn closest_saved_query_suggestion_no_match_when_far() {
        let s = closest_saved_query_suggestion("foo", &catalog_names());
        assert_eq!(s, "", "wildly different name should suggest nothing: {s:?}");
    }

    #[test]
    fn audit_doc_region_envelope_promotes_confidence_to_top_level() {
        // The hits payload mimics what similar_for_mcp returns
        // (iter 85/88): contains confidence + confidence_signals +
        // met_threshold nested inside.
        let target = json!({"id": "doc-x", "summary": "..."});
        let hits = json!({
            "query": "x",
            "type": "doc",
            "algorithm": "bm25",
            "corpus_size": 50,
            "hits": [{"id": "y", "score": 20.0, "text_excerpt": "..."}],
            "confidence": "high",
            "confidence_signals": {"top_score": 20.0, "spread": 7.0},
            "met_threshold": true,
        });
        let rerank = json!({"judge": "noop", "verdicts": []});
        let env = audit_doc_region_envelope(target.clone(), hits.clone(), rerank.clone());
        // Top-level fields preserved:
        assert_eq!(env["target"], target);
        assert_eq!(env["hits"], hits);
        assert_eq!(env["rerank"], rerank);
        // Confidence signal promoted to top level:
        assert_eq!(env["confidence"], "high");
        assert_eq!(env["confidence_signals"]["top_score"], 20.0);
        assert_eq!(env["confidence_signals"]["spread"], 7.0);
        assert_eq!(env["met_threshold"], true);
    }

    #[test]
    fn audit_doc_region_envelope_defaults_when_hits_lacks_confidence() {
        // Defensive path — if the hits payload is somehow missing the
        // confidence fields (e.g. an older similar_for_mcp version),
        // the envelope still emits sensible defaults at the top level.
        let target = json!({"id": "doc-x"});
        let hits = json!({"hits": []});
        let rerank = json!({"judge": "noop", "verdicts": []});
        let env = audit_doc_region_envelope(target, hits, rerank);
        assert_eq!(env["confidence"], "low");
        assert_eq!(env["confidence_signals"]["top_score"], 0.0);
        assert_eq!(env["confidence_signals"]["spread"], 0.0);
        assert_eq!(env["met_threshold"], true);
    }

    #[test]
    fn closest_saved_query_suggestion_empty_catalog() {
        let s = closest_saved_query_suggestion("anything", &[]);
        assert_eq!(s, "");
    }

    #[test]
    fn looks_like_doc_path_matches_md_paths_and_subdir_inputs() {
        // user-probe-023's natural inputs:
        assert!(looks_like_doc_path("docs/audit/interrogation-025.md"));
        assert!(looks_like_doc_path("docs/research/research-fixr.md"));
        assert!(looks_like_doc_path("README.md"));
        // Subdir without .md (rarer but should still be treated as a
        // path attempt — the file-existence check filters):
        assert!(looks_like_doc_path("docs/audit/something"));
    }

    #[test]
    fn looks_like_doc_path_rejects_bare_doc_ids() {
        // Common Doc ids from the design corpus:
        assert!(!looks_like_doc_path("research-mega"));
        assert!(!looks_like_doc_path("entity-pricing-rule"));
        assert!(!looks_like_doc_path("interrogation-023"));
        assert!(!looks_like_doc_path("doc-design-technical-design-v1"));
        // Edge: empty string. Returns false — there's nothing to test
        // for existence anyway.
        assert!(!looks_like_doc_path(""));
    }

    #[test]
    fn build_retrieval_backends_payload_embedding_available() {
        // Happy path: build is configured + embedder loaded.
        // Both backends present, both marked available, no
        // reason fields.
        let p = build_retrieval_backends_payload(true);
        let backends = p["backends"].as_array().unwrap();
        assert_eq!(backends.len(), 2);
        assert_eq!(backends[0]["name"], "bm25");
        assert_eq!(backends[0]["available"], true);
        assert_eq!(backends[1]["name"], "embedding");
        assert_eq!(backends[1]["available"], true);
        assert!(
            backends[1].get("reason").is_none(),
            "reason field omitted when embedding is available; got {p}"
        );
    }

    #[test]
    fn build_retrieval_backends_payload_embedding_unavailable() {
        // The interrogation-025 case: feature not compiled OR
        // model env var unset. bm25 still available, embedding
        // shows up but flagged + carries a reason naming the
        // recovery action.
        let p = build_retrieval_backends_payload(false);
        let backends = p["backends"].as_array().unwrap();
        assert_eq!(backends.len(), 2);
        assert_eq!(backends[0]["name"], "bm25");
        assert_eq!(backends[0]["available"], true);
        assert_eq!(backends[1]["name"], "embedding");
        assert_eq!(backends[1]["available"], false);
        let reason = backends[1]["reason"].as_str().unwrap();
        assert!(
            reason.contains("features embeddings"),
            "reason must name the build flag: {reason}"
        );
        assert!(
            reason.contains("DOC_LINTER_EMBED_MODEL"),
            "reason must name the env var: {reason}"
        );
    }

    #[test]
    fn build_freshness_payload_populated_both_timestamps() {
        // Happy path: both Doc.updated and File.last_touched
        // had max-rows. The hint must direct the agent to
        // compare against git HEAD.
        let p = build_freshness_payload(Some("2026-06-01"), Some("2026-04-12T08:00:00Z"));
        assert_eq!(p["max_doc_updated"], "2026-06-01");
        assert_eq!(p["max_file_touched"], "2026-04-12T08:00:00Z");
        let hint = p["hint"].as_str().unwrap();
        assert!(
            hint.contains("git HEAD"),
            "hint must direct comparison to git HEAD: {hint}"
        );
        assert!(
            hint.contains("doc-linter check"),
            "hint must name the remediation command: {hint}"
        );
    }

    #[test]
    fn build_freshness_payload_both_null_returns_empty_corpus_hint() {
        // No max values (e.g., fresh-init corpus). The hint
        // must NOT direct an agent to compare against git
        // HEAD — there's nothing to compare yet.
        let p = build_freshness_payload(None, None);
        assert!(p["max_doc_updated"].is_null());
        assert!(p["max_file_touched"].is_null());
        let hint = p["hint"].as_str().unwrap();
        assert!(
            hint.contains("empty corpus"),
            "hint must name the empty case: {hint}"
        );
        assert!(
            !hint.contains("git HEAD"),
            "no git-HEAD compare prompt for empty corpus: {hint}"
        );
    }

    #[test]
    fn build_freshness_payload_one_null_still_directs_compare() {
        // Doc-sparse corpus case (cf. user-probe-021's trusted
        // corpus): File.last_touched is populated but
        // Doc.updated may be null. Still useful — agent can
        // compare File timestamps against git HEAD.
        let p = build_freshness_payload(None, Some("2026-02-09T19:59:05Z"));
        assert!(p["max_doc_updated"].is_null());
        assert_eq!(p["max_file_touched"], "2026-02-09T19:59:05Z");
        assert!(p["hint"].as_str().unwrap().contains("git HEAD"));
    }

    #[test]
    fn classify_coupling_tier_absence_at_zero_neighbors() {
        // Per user-probe-034: 0 neighbors → absence-as-signal.
        // max_jaccard is meaningless when neighbor_count is 0 (the
        // f64::max fold yields 0.0); the count alone decides.
        assert_eq!(classify_coupling_tier(0, 0.0), "absence");
    }

    #[test]
    fn classify_coupling_tier_heavy_at_five_plus_high_jaccard() {
        // Per user-probe-034: 5+ neighbors AND max_jaccard ≥ 0.75
        // → heavy (tight bounded context, e.g. auth+dashboard cluster).
        // 7 neighbors at 1.0 — the iter-155 reset-password.tsx case.
        assert_eq!(classify_coupling_tier(7, 1.0), "heavy");
        // Boundary: exactly 5 at exactly 0.75.
        assert_eq!(classify_coupling_tier(5, 0.75), "heavy");
        // Just under the count threshold falls to moderate.
        assert_eq!(classify_coupling_tier(4, 1.0), "moderate");
        // At-count, just under jaccard threshold falls to moderate.
        assert_eq!(classify_coupling_tier(5, 0.74), "moderate");
    }

    #[test]
    fn classify_coupling_tier_minimal_at_one_or_two_moderate_jaccard() {
        // Per user-probe-032's items.py ↔ users.py at jaccard=0.5
        // and 1 neighbor — the CRUD-pair family pattern.
        assert_eq!(classify_coupling_tier(1, 0.5), "minimal");
        assert_eq!(classify_coupling_tier(2, 0.5), "minimal");
        // Boundary: exactly 0.3 is minimal.
        assert_eq!(classify_coupling_tier(1, 0.3), "minimal");
        // Just under 0.3 falls to moderate (couldn't tell apart from
        // background noise).
        assert_eq!(classify_coupling_tier(1, 0.29), "moderate");
        // At or above 0.7 falls to moderate (too tight for the
        // small-family interpretation).
        assert_eq!(classify_coupling_tier(2, 0.7), "moderate");
    }

    #[test]
    fn build_coupling_classification_zero_neighbors_is_absence() {
        // Per user-probe-026 src/embeddings.rs / user-probe-030
        // src/cmd/mcp.rs: the absence-as-signal demonstration.
        // No neighbors → tier=absence, neighbor list empty,
        // max/mean = 0.0, explanation names "leaf module".
        let payload = build_coupling_classification("src/embeddings.rs", &[]);
        assert_eq!(payload["path"], "src/embeddings.rs");
        assert_eq!(payload["tier"], "absence");
        assert_eq!(payload["neighbor_count"], 0);
        assert_eq!(payload["max_jaccard"], 0.0);
        assert_eq!(payload["mean_jaccard"], 0.0);
        assert_eq!(payload["neighbors"].as_array().unwrap().len(), 0);
        assert!(payload["explanation"]
            .as_str()
            .unwrap()
            .contains("leaf module"));
    }

    #[test]
    fn build_coupling_classification_heavy_carries_synthesis_hint() {
        // Per user-probe-034: reset-password.tsx neighborhood
        // (7 neighbors, 5 at 1.0, 2 at 0.75).
        let neighbors = vec![
            ("frontend/src/routes/_layout/index.tsx".to_string(), 1.0),
            ("frontend/src/routes/_layout/items.tsx".to_string(), 1.0),
            ("frontend/src/routes/_layout/settings.tsx".to_string(), 1.0),
            ("frontend/src/routes/recover-password.tsx".to_string(), 1.0),
            ("frontend/src/routes/signup.tsx".to_string(), 1.0),
            ("frontend/src/routes/_layout/admin.tsx".to_string(), 0.75),
            ("frontend/src/routes/login.tsx".to_string(), 0.75),
        ];
        let payload =
            build_coupling_classification("frontend/src/routes/reset-password.tsx", &neighbors);
        assert_eq!(payload["tier"], "heavy");
        assert_eq!(payload["neighbor_count"], 7);
        assert_eq!(payload["max_jaccard"], 1.0);
        // The synthesis hint must name the "git bisect" guidance so
        // the agent's debugging plan picks it up verbatim (cf.
        // user-probe-034's Finding B implication).
        let explanation = payload["explanation"].as_str().unwrap();
        assert!(
            explanation.contains("git bisect"),
            "heavy tier explanation must surface the bisect guidance: {explanation}",
        );
    }

    #[test]
    fn build_neighbor_envelope_uniform_shape_doc() {
        // Per gap-007 closure: every neighbor regardless of centre
        // kind must serialise as `{node: {...}, edge: {kind, line?}}`.
        // Pure-function check: synthesised row → expected envelope.
        let row = json!({
            "n_id": "research-mega",
            "n_title": "Research: MEGA",
            "n_summary": "Multi-view fusion paper",
            "n_kind": "doc",
            "n_path": "docs/research/research-mega.md",
            "n_tags": ["research", "code-rag"],
            "edge_kind": "WIKILINK",
            "edge_line": 42,
            "direction": "outbound",
        });
        let env = build_neighbor_envelope(&row, "doc");
        assert_eq!(env["node"]["id"], "research-mega");
        assert_eq!(env["node"]["kind"], "doc");
        assert_eq!(env["node"]["summary"], "Multi-view fusion paper");
        assert_eq!(env["edge"]["kind"], "WIKILINK");
        assert_eq!(env["edge"]["line"], 42);
        assert_eq!(env["edge"]["direction"], "outbound");
    }

    #[test]
    fn build_neighbor_envelope_omits_line_when_null() {
        // line is optional — when the sql row has no edge_line
        // the envelope must NOT carry a line: null key (clutter).
        let row = json!({
            "n_id": "entity-x",
            "n_title": "x",
            "n_summary": "y",
            "n_kind": "entity",
            "n_path": "",
            "edge_kind": "COVERS",
            "edge_line": null,
        });
        let env = build_neighbor_envelope(&row, "entity");
        assert!(
            env["edge"].get("line").is_none(),
            "line key should be omitted when null; got {env}"
        );
    }

    #[test]
    fn build_neighbor_envelope_default_kind_when_row_omits_it() {
        // When the row's n_kind is empty (e.g., Function rows
        // produce string columns that may be NULL), the caller's
        // default_kind argument kicks in. This is the
        // function-centred path's safety net.
        let row = json!({
            "n_id": "some_symbol()",
            "n_title": "some_symbol",
            "n_kind": "",  // empty string
            "edge_kind": "CALLS",
        });
        let env = build_neighbor_envelope(&row, "function");
        assert_eq!(env["node"]["kind"], "function");
    }

    #[test]
    fn build_neighbor_envelope_uses_row_kind_over_default() {
        // When the row DOES specify a kind, it wins over the
        // caller's default. (Function-centred payload's combined
        // row set has function + doc + entity neighbors — the
        // per-row kind must be preserved.)
        let row = json!({
            "n_id": "x",
            "n_kind": "doc",
            "edge_kind": "DEFINED_IN",
        });
        let env = build_neighbor_envelope(&row, "function");
        assert_eq!(env["node"]["kind"], "doc");
    }

    /// Handoff cpg-os-vault #4: an entity covered by no narrative doc
    /// returned 0 neighbours, though `entity-<id>` defines it and other
    /// docs wikilink that definition.

    #[test]
    fn context_for_entity_includes_definition_doc_and_linkers() {
        let s = seed_server(&[
            Seed::Entity("gross-predicate", "Gross predicate"),
            Seed::Doc("entity-gross-predicate", "s", &[]),
            Seed::Doc("traps", "s", &[]),
            Seed::Wikilink("traps", "entity-gross-predicate", 7),
        ]);
        let v = context_for_entity_payload(&**s.db.borrow(), "gross-predicate", 10).unwrap();
        let ids: Vec<&str> = v["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| n["node"]["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["entity-gross-predicate", "traps"], "{v}");
        assert_eq!(v["neighbors"][0]["edge"]["kind"], "DEFINED_BY");
        assert_eq!(v["neighbors"][1]["edge"]["kind"], "WIKILINK");
    }

    // Per iter 242 ([[feedback_meta_doc_displaces_research]]):
    // helper for tag-filter tests — seed 3 docs with distinct
    // tags so the agent can partition the corpus by the natural
    // research / interrogation / user-probe axes.
    fn three_role_tagged_docs() -> Server {
        seed_server(&[
            Seed::Doc(
                "research-foo",
                "Foundational graph similarity ranking research paper.",
                &["research"],
            ),
            Seed::Doc(
                "interrogation-foo",
                "Graph similarity ranking probe of the corpus.",
                &["interrogation"],
            ),
            Seed::Doc(
                "user-probe-foo",
                "Graph similarity ranking validation against trusted corpus.",
                &["user-probe"],
            ),
        ])
    }

    fn run_query_similar_text_only(
        server: &Server,
        with_tag: Option<&str>,
        without_tag: Option<&str>,
    ) -> Vec<String> {
        let mut args = json!({"text": "graph similarity ranking"});
        if let Some(t) = with_tag {
            args["with_tag"] = json!(t);
        }
        if let Some(t) = without_tag {
            args["without_tag"] = json!(t);
        }
        let req = json!({
            "jsonrpc": "2.0",
            "id": 99,
            "method": "tools/call",
            "params": {"name": "query_similar", "arguments": args},
        });
        let reply = server.handle_line(&req.to_string()).expect("response");
        let v: Value = serde_json::from_str(&reply).unwrap();
        assert!(v.get("error").is_none(), "got error envelope: {v}");
        let content_text = v["result"]["content"][0]["text"].as_str().unwrap();
        let result: Value = serde_json::from_str(content_text).unwrap();
        result["hits"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|h| h["id"].as_str().map(str::to_string))
            .collect()
    }

    #[test]
    fn query_similar_with_tag_restricts_to_matching_tag() {
        // Per iter 242: with_tag="research" must return ONLY
        // research-foo even though all three docs match the
        // text vocabulary. Operationalises the iter-241
        // meta-doc-displaces-research memory at the retrieval
        // primitive layer.
        let server = three_role_tagged_docs();
        let ids = run_query_similar_text_only(&server, Some("research"), None);
        assert_eq!(
            ids,
            vec!["research-foo"],
            "with_tag=research must return only the research-tagged doc; got {ids:?}",
        );
    }

    #[test]
    fn query_similar_without_tag_excludes_matching_tag() {
        // Per iter 242: without_tag="interrogation" drops the
        // interrogation-foo doc, leaving research-foo +
        // user-probe-foo. The agent uses this to ask "find
        // anything relevant EXCEPT meta-discussions".
        let server = three_role_tagged_docs();
        let mut ids = run_query_similar_text_only(&server, None, Some("interrogation"));
        ids.sort();
        assert_eq!(
            ids,
            vec!["research-foo".to_string(), "user-probe-foo".to_string()],
            "without_tag=interrogation must drop interrogation-foo: {ids:?}",
        );
    }

    #[test]
    fn query_similar_tag_filters_compose() {
        // Per iter 242: passing BOTH with_tag and without_tag
        // composes as AND. with_tag="research" + without_tag=
        // "interrogation" returns only research-tagged
        // non-interrogation docs (research-foo here).
        let server = three_role_tagged_docs();
        let ids = run_query_similar_text_only(&server, Some("research"), Some("interrogation"));
        assert_eq!(
            ids,
            vec!["research-foo"],
            "with_tag=research AND without_tag=interrogation must return only research-foo; got {ids:?}",
        );
    }

    #[test]
    fn query_similar_no_tag_filter_returns_all_matching_docs() {
        // Sanity check: pre-iter-242 behavior preserved when
        // neither tag arg is supplied. All 3 docs match the
        // vocabulary so all 3 appear (in some BM25 order).
        let server = three_role_tagged_docs();
        let mut ids = run_query_similar_text_only(&server, None, None);
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "interrogation-foo".to_string(),
                "research-foo".to_string(),
                "user-probe-foo".to_string(),
            ],
            "no tag filter must return all 3 matching docs: {ids:?}",
        );
    }
}
