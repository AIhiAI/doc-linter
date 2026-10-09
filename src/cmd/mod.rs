//! Subcommand layer for the [[entity-doc-graph]] linter. Each
//! `cmd_*` handler from the pre-#49 `main.rs` lives in its own file
//! here; the `Commands` enum (the clap subcommand definitions) sits in
//! this module so adding a new subcommand is a two-file change — the
//! new `cmd/<name>.rs` plus one variant + one match-arm in
//! [`Commands`] / [`run_command`].

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Subcommand;

use doc_linter::config::LintConfig;
use doc_linter::query::QueryKind;
use doc_linter::{explain as explain_mod, scaffold};

pub(crate) mod check;
pub(crate) mod check_single;
pub(crate) mod cluster;
pub(crate) mod codes;
pub(crate) mod concepts;
pub(crate) mod export;
pub(crate) mod gen_docs;
pub(crate) mod homepage;
pub(crate) mod lsp;
pub(crate) mod mcp;
pub(crate) mod mcp_install;
#[cfg(unix)]
pub(crate) mod mcp_supervisor;
pub(crate) mod migrate_anchor;
pub(crate) mod migrate_endpoint_markers;
pub(crate) mod ontology;
pub(crate) mod query;
pub(crate) mod report;
pub(crate) mod scip_index;
pub(crate) mod util;

pub(crate) use util::OutputFormat;

/// `doc-linter ontology <action>`; bare `ontology` still prints the JSON dump.
#[derive(Subcommand)]
pub(crate) enum OntologyAction {
    /// Propose named candidate concepts from the code graph (needs a prior
    /// `check`). Local and deterministic: no network, no API key. Writes
    /// only the proposals file, never anything under docs/.
    Propose {
        /// How many candidates to keep, largest first.
        #[arg(long, default_value_t = 20)]
        top_n: usize,
        /// Smallest community that becomes a candidate.
        #[arg(long, default_value_t = 3)]
        min_members: usize,
        /// Community detection: lpa, louvain or leiden.
        #[arg(long, default_value = "leiden")]
        algorithm: String,
        // `--embeddings` (also join symbols whose stored doc-comment
        // embeddings are close) is the global flag; needs vectors from
        // `check --embeddings`, skipped with a message when none are readable.
        /// Print the proposals as JSON instead of a table.
        #[arg(long)]
        json: bool,
        /// Proposals file `accept` reads.
        #[arg(long, default_value = concepts::DEFAULT_PROPOSALS)]
        out: PathBuf,
    },
    /// Write an entity and its narrative doc for each named proposal.
    /// Refuses (writing nothing) a name that is not proposed, already
    /// exists, or is too thin to cite from the code. Exit 1 if any name was
    /// refused.
    Accept {
        /// Proposal names to accept (at least one; there is no --all).
        #[arg(required = true, value_name = "NAME")]
        names: Vec<String>,
        /// Accept under another name: `--rename old=new`. Repeatable.
        #[arg(long = "rename", value_name = "OLD=NEW")]
        renames: Vec<String>,
        /// Show what would be written and write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Proposals file written by `propose`.
        #[arg(long, default_value = concepts::DEFAULT_PROPOSALS)]
        from: PathBuf,
    },
}

/// Top-level subcommand enum for the [[entity-doc-graph]] CLI —
/// `check`, `query`, `lsp`, `init`, `scaffold-coverage`, etc.
#[derive(Subcommand)]
pub(crate) enum Commands {
    /// Validate frontmatter, wikilinks, and schema (default when no subcommand given)
    Check,
    /// Query the doc graph — list, neighbors, backlinks, path, subgraph
    Query {
        #[command(subcommand)]
        kind: QueryKind,
    },
    /// Emit the assembled ontology — every axis + value + entity + the
    /// migration history. Cold-start endpoint for AI agents discovering the
    /// vocabulary they should author docs against.
    ///
    /// Subcommands: `propose` groups the code graph into named candidate
    /// concepts with no API key and writes `.doc-lint/proposals.json`;
    /// `accept <name>` turns one proposal into an ontology entity plus a
    /// narrative doc built from the real code. There is no `accept --all`.
    Ontology {
        #[command(subcommand)]
        action: Option<OntologyAction>,
    },
    /// Export the publishable doc corpus to an output directory, filtering
    /// out any doc whose `visibility:` frontmatter is in `--strip`. Drives
    /// publish pipelines that ship a public subset of an internal vault.
    /// Cross-repo docs are NOT exported — only the primary --root tree.
    Export {
        /// Output directory. Created if missing. Existing files are
        /// overwritten in-place so the command is idempotent.
        #[arg(long)]
        out: PathBuf,
        /// Visibility values to strip. Repeatable. Defaults to `internal`.
        #[arg(long = "strip", value_name = "VISIBILITY")]
        strip: Vec<String>,
        /// Glob patterns (relative to --root) selecting which markdown files
        /// to include. Repeatable. Defaults to the entire docs/ tree plus
        /// crate READMEs and root-level docs.
        #[arg(long = "include", value_name = "GLOB")]
        include: Vec<String>,
        /// Skip the pre-export `doc-linter check`. By default, export refuses
        /// to run if lint fails — that's the whole point of the pipeline.
        #[arg(long = "no-prelint")]
        no_prelint: bool,
    },
    /// Zero-config first run: refresh the graph, then print the coverage
    /// table, the stale docs (starter-ontology docs excluded) and the
    /// concept work list, with candidate concepts mined from the code.
    /// Needs no `init` and no ontology.
    Report {
        /// Print one JSON object instead of the tables.
        #[arg(long)]
        json: bool,
        /// Read the existing graph instead of refreshing it first.
        #[arg(long)]
        no_refresh: bool,
    },
    /// Round 3B: produce `.doc-lint/code.scip` by running the SCIP
    /// indexer of every language found in the repo — rust-analyzer,
    /// scip-typescript (TS/JS), scip-dotnet (C#), scip_dart (Dart;
    /// run `pub get` first), scip-java (Java; runs the Gradle/Maven
    /// build), scip-python — and merging their output.
    /// `check` then ingests it. A detected language whose indexer is
    /// not on PATH emits `scip-indexer-missing`.
    ///
    /// IMPORTANT — MEMORY HEAVY. Indexers hold the whole-workspace
    /// symbol table resident (4–6 GiB for rust-analyzer on a mid-sized
    /// workspace; it has crashed 16 GiB hosts). The command refuses to
    /// start below 4 GiB `MemAvailable`, caps rust-analyzer's address
    /// space at 4 GiB (`DOC_LINTER_SCIP_MEMORY_MB`) and runs every
    /// indexer under `nice -n 19` + `ionice -c 3`. On macOS the free
    /// memory comes from `vm_stat` and there is no address-space cap.
    /// Stop docker and close memory-hungry apps first; don't bypass the
    /// pre-flight.
    ScipIndex,
    /// Round 4A: run a Language Server Protocol server on stdio so any
    /// LSP-capable editor (VS Code, Helix, Neovim, Emacs, …) can show
    /// doc-linter diagnostics inline. Re-lints individual files on
    /// open / change / save; does NOT shell out to Vale (per-keystroke
    /// shell-out is unworkable). Server-side debug logs go to
    /// `<root>/.doc-lint/lsp.log` by default — override with
    /// `--log-file <PATH>`.
    Lsp {
        /// Path to write server-side debug logs to. Defaults to
        /// `<root>/.doc-lint/lsp.log`. Server stdout is the JSON-RPC
        /// channel — any other output would corrupt the protocol, so
        /// stderr (where eprintln! / `log_message` warnings flow) is
        /// redirected here.
        #[arg(long)]
        log_file: Option<PathBuf>,
    },
    /// Round 4B: bootstrap a target codebase to use doc-linter. Writes a
    /// starter `.doc-lint.toml`, scaffolds `docs/ontology/` (axes +
    /// values + a placeholder entity + the v1 bootstrap migration),
    /// creates `.doc-lint/` and appends it to `.gitignore`. Refuses to
    /// overwrite an already-populated `docs/ontology/` — your authored
    /// vocabulary is authoritative. Run once per target repo.
    Init,
    /// Phase 4 of roadmap-43: walk every dark Function in the named
    /// crate and emit proposed `///` doc-comment templates. The
    /// entity is auto-inferred from the function name (using the
    /// Phase-1 SCIP-symbol tokenizer); functions whose symbol
    /// doesn't resolve to any ontology entity get a TODO stub plus
    /// the three closest fallback candidates by mention frequency.
    /// Default is dry-run (preview). `--write` applies the edits in
    /// place — refuses to run on a dirty git working tree as a
    /// safety measure. `--json` emits one JSON object per line for
    /// piping into editors / batch scripts.
    ScaffoldCoverage {
        /// The crate to scaffold (matches the `crate` field on
        /// Function nodes). E.g. `pricing-core`, `web-router`. Walks
        /// the entire crate's dark functions; no path filtering for
        /// v1.
        #[arg(value_name = "CRATE")]
        crate_name: String,

        /// Apply the edits in place. Default is dry-run (preview only).
        #[arg(long)]
        write: bool,

        /// Emit one JSON line per proposal instead of human-readable
        /// preview blocks. Useful for piping into editors / batch
        /// scripts.
        #[arg(long)]
        json: bool,
    },
    /// Roadmap-48 Rule A: walk every dark public function in the
    /// freshly-ingested SQLite graph and write their bare names to
    /// `coverage_anchor_exempt_file` (default
    /// `.doc-lint/anchor-exempt.txt`). Idempotent — re-running over
    /// an existing file overwrites with the current snapshot, so a
    /// function that gained an entity link drops out naturally.
    /// Use this once per repo when flipping
    /// `require_anchor_per_pubapi` to default-true to grandfather
    /// the existing dark surface; new public functions added after
    /// the snapshot must reference an ontology entity OR be added
    /// explicitly via a regex in `coverage_function_exempt`.
    MigrateAnchorRequired,
    /// Roadmap-49 phase 2: walk the syntactic endpoint extractor's
    /// output, find each handler's source location via SCIP, stamp
    /// `@endpoint <METHOD> <path>` lines on the handler's doc-comment.
    /// Refuses to run on a dirty git tree (same safety as
    /// `scaffold-coverage --write`). Endpoints whose handler the
    /// regex extractor can't resolve go to a "manual" report at
    /// `/tmp/endpoint-marker-manual.txt` for follow-up by hand.
    MigrateEndpointMarkers {
        /// Print proposals without applying. Default behaviour applies.
        #[arg(long)]
        dry_run: bool,

        /// Process .ts/.tsx handlers too (requires the TS doc-comment
        /// extractor; default off until TS handler resolution
        /// stabilises).
        #[arg(long)]
        include_typescript: bool,
    },
    /// Generate the machine-shaped repo homepage (`MAP.md` by
    /// default) from the current ontology + doc corpus. The
    /// homepage is the cold-start surface for AI agents: every
    /// entity, every narrative doc grouped by kind, every bounded
    /// context, and a pointer to the public-API graph — all linked
    /// via Obsidian-style `[[wikilinks]]`. Output is deterministic
    /// (same inputs → same bytes) so the `homepage-stale` lint
    /// rule can byte-compare the on-disk file against it.
    ///
    /// Default is dry-run: prints the canonical content to stdout.
    /// `--write` applies it to `<homepage_path>` (default `MAP.md`)
    /// and refuses on a dirty git working tree as a safety measure.
    Homepage {
        /// Write the generated content to `<homepage_path>`. Default
        /// is dry-run (print to stdout). Refuses to run on a dirty
        /// git working tree.
        #[arg(long)]
        write: bool,
    },
    /// Phase 4 of roadmap-43: explain a single function by walking
    /// the graph. Given a SCIP symbol (or a substring that uniquely
    /// identifies one), emits the function's metadata, every entity
    /// it `FUNCTION_MENTIONS` with display + summary, every narrative
    /// doc covering those entities, and the top 5 sibling functions
    /// in the same crate ranked by entity-mention overlap.
    Explain {
        /// SCIP symbol or a substring that uniquely identifies one.
        /// E.g. `pricing-core/resolve_price` or just `resolve_price`.
        /// Use `--list` to see candidates when multiple match.
        #[arg(value_name = "SYMBOL")]
        symbol: String,

        /// When the partial match is ambiguous, list candidates
        /// instead of explaining one.
        #[arg(long)]
        list: bool,

        /// Emit JSON instead of the human-readable summary.
        #[arg(long)]
        json: bool,
    },
    /// Emit the canonical (code → meaning → fix) table for every
    /// [[entity-doc-graph]] diagnostic. Backs the checked-in
    /// `docs/error-codes.md`; rerun and redirect when the table
    /// changes (the staleness check on `docs/error-codes.md` flags
    /// drift). Source of truth lives in
    /// `src/validator/code_table.rs`; this subcommand only renders.
    Codes {
        /// Render the table as GitHub-flavored Markdown (the default
        /// and only format today; the flag is reserved for future
        /// JSON / YAML output).
        #[arg(long, default_value_t = true)]
        markdown: bool,
    },
    /// Regenerate the reference docs whose source of truth is code:
    /// `docs/error-codes.md` and `docs/reference/{cli,mcp,config}.md`.
    /// Maintainer tool, run from a doc-linter checkout.
    #[command(hide = true)]
    GenDocs {
        /// Write nothing; exit non-zero if a committed file is stale.
        #[arg(long)]
        check: bool,
    },
    /// Roadmap issue #14 (v0.3.0): cluster the code graph and emit
    /// candidate entity stubs. Runs label-propagation community
    /// detection over `FUNCTION_BELONGS_TO` + `CALLS`, picks each
    /// community's god node, and prints a JSON report
    /// (`{candidates: [...]}`) to stdout. v1 reports only — file
    /// emission to `docs/ontology/entities/candidates/` and the
    /// `status: candidate` validator surface defer to a follow-up.
    Cluster {
        /// How many top communities to surface. Default 200 — large
        /// enough to capture the long tail of small but real
        /// communities (matching the structural coverage external
        /// graph builders surface by treating every AST node as a
        /// graph node), but still bounded so `--write` doesn't dump
        /// thousands of stub files at once.
        #[arg(long, default_value_t = 200)]
        top_n: usize,
        /// Roadmap issue #14 v2: emit a candidate entity stub at
        /// `docs/ontology/entities/candidates/<id>.md` for each
        /// surfaced community. Existing files are left alone so
        /// re-runs are idempotent. Without `--write`, JSON to
        /// stdout is the only output.
        #[arg(long)]
        write: bool,
        /// Roadmap issue #14 v5: ranking key for the top-N cut.
        /// `member-count` (default) surfaces the biggest
        /// communities first; `density` surfaces the tightest
        /// (intra-edges / max-possible) first, useful for finding
        /// crisp architectural units before larger but noisier
        /// ones.
        #[arg(long, default_value = "member-count", value_name = "KEY")]
        order_by: String,
        /// Roadmap issue #14 v6: community-detection algorithm.
        /// `lpa` (default) — Label Propagation, fast but coarse.
        /// `louvain` — modularity-optimization first-level pass,
        /// usually tighter communities at ~3-5x the runtime.
        /// `leiden` — multi-level local-move + refinement +
        /// aggregate; re-partitions each community from singletons
        /// under a phase-1 boundary restriction at every level.
        /// Catches both weakly-bridged sub-clusters (refinement)
        /// and multi-scale hierarchy in hub-heavy graphs
        /// (aggregation).
        #[arg(long, default_value = "lpa", value_name = "NAME")]
        algorithm: String,
        /// Roadmap issue #14 v7: minimum community size to surface
        /// in the report. 2 is the historical default — singletons
        /// always drop. Bumping to 5+ filters noise on big repos
        /// when an operator only cares about candidates that
        /// represent real architectural units.
        #[arg(long, default_value_t = 2)]
        min_members: usize,
        /// Roadmap issue #14 v8: when set, return only the
        /// community containing this Function symbol. Answers the
        /// "who's in the same cluster as X?" query without
        /// scanning the full top-N report. Empty when no such
        /// symbol exists.
        #[arg(long, value_name = "SCIP_SYMBOL")]
        seed_symbol: Option<String>,
        /// Roadmap issue #14 v9: how many representative member
        /// symbols to include in each candidate's `top_members`
        /// preview list. 5 (default) is enough to eyeball most
        /// clusters; reviewers of larger communities can raise
        /// it to 10-20 for richer signal.
        #[arg(long, default_value_t = 5)]
        top_members: usize,
        /// Roadmap issue #14 v11: destination directory (repo-
        /// relative) for `--write`. Defaults to
        /// `docs/ontology/entities/candidates`, the convention
        /// the issue spec proposed and the directory the
        /// validator already recognises for `status: candidate`
        /// docs. Override to a staging dir
        /// (e.g. `.tmp/candidates`) when iterating on cluster
        /// tunings without polluting the vault. Ignored without
        /// `--write`.
        #[arg(
            long,
            default_value = "docs/ontology/entities/candidates",
            value_name = "DIR"
        )]
        output: PathBuf,
        /// Roadmap issue #14 v11 (v0.4.0): promote a reviewed
        /// candidate to a stable entity. Reads
        /// `<output>/<id>.md`, flips `status: candidate` to
        /// `status: stable`, drops the "(candidate)" title suffix,
        /// and moves the file to
        /// `docs/ontology/entities/<id>.md`. Short-circuits the
        /// clustering pass — `<id>` is the `suggested_id` printed
        /// by the report (also the file stem under `<output>`).
        #[arg(long, value_name = "ID")]
        promote: Option<String>,
        /// Modularity resolution parameter γ (Leiden only). Scales
        /// the expected-edge penalty in the modularity-gain formula.
        /// γ = 1.0 (default) is standard modularity. γ > 1 forces
        /// smaller communities — useful when the multi-level pass
        /// merges weakly-related sub-domains into giant macro-
        /// communities due to the modularity resolution limit on
        /// hub-heavy graphs. γ < 1 has the opposite effect. The
        /// LPA and Louvain algorithms ignore this flag.
        #[arg(long, default_value_t = 1.0, value_name = "GAMMA")]
        resolution: f64,
    },
    /// Roadmap issue #35 (v0.3.0): expose the [[entity-doc-graph]] as
    /// an MCP server. Reads line-delimited JSON-RPC requests from
    /// stdin, writes responses to stdout. v1 implements
    /// `initialize`, `tools/list`, `tools/call` over a small set of
    /// read-only tools (sql, list_entities, function_context).
    /// Resources, prompts, and authentication defer to v2.
    Mcp,
    /// Write/merge a `doc-linter` MCP server entry into a client's
    /// project config (`claude-code` -> `.mcp.json`, `cursor` ->
    /// `.cursor/mcp.json`). Other servers in the file are preserved.
    McpInstall {
        /// Target client: `claude-code` or `cursor`.
        #[arg(long, value_name = "CLIENT")]
        client: String,
    },
    /// gap-mcp-supervisor Slice B.1: worker half of the supervisor +
    /// worker MCP architecture. Listens on `--socket` for one
    /// supervisor connection and serves JSON-RPC over it via the
    /// same dispatch the stdio `mcp` subcommand uses. Spawned by
    /// `doc-linter mcp-supervisor` — not normally invoked directly.
    /// Unix-only.
    #[cfg(unix)]
    McpWorker {
        /// Path to the unix-domain socket to listen on. The
        /// supervisor opens this same path on its side of the
        /// proxy. Stale socket files at the path are removed
        /// before bind.
        #[arg(long, value_name = "PATH")]
        socket: PathBuf,
    },
    /// gap-mcp-supervisor Slice B.2: long-lived supervisor that
    /// proxies the Claude Code stdio MCP connection to a child
    /// `mcp-worker` subprocess over a unix-domain socket. Once
    /// `swap_worker` ships (Slice B.3), this is the binary
    /// `.mcp.json` registers — supervisor stays alive across
    /// worker rebuilds so the Claude Code stdio connection never
    /// drops. Unix-only.
    #[cfg(unix)]
    McpSupervisor,
}

/// Per-invocation flags carried over from the top-level `Cli` for the
/// `check` subcommand. Bundled to keep [`run_command`]'s signature
/// readable now that the dispatcher lives separately from `Cli`.
pub(crate) struct CheckArgs {
    pub only_file: Option<PathBuf>,
    pub format: OutputFormat,
    pub no_vale: bool,
    pub lint_code_comments: bool,
    pub coverage_min_global: Option<f32>,
    /// Roadmap issue #33: force a full re-ingest. The v0.3.0
    /// cache-hit fast path (`cmd/check/scip_pipeline.rs:55-94`)
    /// reuses the prior SCIP ingest's code-graph tables and only
    /// rederives cross-bucket edges — `--rebuild` opts out and runs
    /// the full per-Function ingest again. v0.4.0 (#33 v1, PR #159)
    /// added a SHA-256 content hash so the cache no longer accepts
    /// the same-second + same-length collision the v0.3.0 mtime+len
    /// cache acknowledged.
    pub rebuild: bool,
    /// Roadmap issue #28 v3 follow-up (perf): opt-in flag that
    /// gates the post-ingest `populate_embeddings` pass. Default
    /// false because the pass is the dominant wall-clock cost on
    /// a `--rebuild` run (~30-50 min on CPU for a typical mid-sized
    /// corpus) and only feeds `query similar --backend embedding`
    /// — `--backend bm25` (the default) doesn't read the column.
    pub embeddings: bool,
    /// Gap-005 deferred slice: opt-in LLM pairwise contradiction
    /// check across `tags=[design]` Docs. NoOp when the active
    /// Judge is NoOp (default builds, or `--features llm` without
    /// `ANTHROPIC_API_KEY`). O(N^2) per pair across design docs;
    /// only viable for small corpora.
    pub contradictions: bool,
}

/// Dispatches one `Commands` variant to its handler. `Commands::Init`
/// is filtered out upstream (init runs against cold targets, before
/// the corpus walk).
pub(crate) fn run_command(
    command: Option<Commands>,
    root: &std::path::Path,
    config: &LintConfig,
    all_files: &[PathBuf],
    check_args: CheckArgs,
) -> Result<ExitCode> {
    let CheckArgs {
        only_file,
        format,
        no_vale,
        lint_code_comments,
        coverage_min_global,
        rebuild,
        embeddings,
        contradictions,
    } = check_args;
    match command {
        None | Some(Commands::Check) => check::run(
            root,
            config,
            all_files,
            only_file,
            format,
            no_vale,
            lint_code_comments,
            coverage_min_global,
            rebuild,
            embeddings,
            contradictions,
            false,
        )
        .map(|o| o.exit),
        Some(Commands::Query { kind }) => query::run(root, config, all_files, kind),
        Some(Commands::Report { json, no_refresh }) => {
            report::run(root, config, all_files, json, no_refresh)
        }
        Some(Commands::Ontology { action: None }) => ontology::run(root, config, all_files),
        Some(Commands::Ontology {
            action:
                Some(OntologyAction::Propose {
                    top_n,
                    min_members,
                    algorithm,
                    json,
                    out,
                }),
        }) => concepts::propose(
            root,
            config,
            &ontology::load(all_files),
            &concepts::ProposeArgs {
                top_n,
                min_members,
                algorithm,
                embeddings,
                json,
                out,
            },
        ),
        Some(Commands::Ontology {
            action:
                Some(OntologyAction::Accept {
                    names,
                    renames,
                    dry_run,
                    from,
                }),
        }) => concepts::accept(
            root,
            &ontology::load(all_files),
            &concepts::AcceptArgs {
                names,
                renames,
                dry_run,
                from,
            },
        ),
        Some(Commands::Export {
            out,
            strip,
            include,
            no_prelint,
        }) => export::run(root, config, all_files, out, strip, include, no_prelint),
        Some(Commands::ScipIndex) => scip_index::run(root, config),
        Some(Commands::Lsp { log_file }) => lsp::run(root, config, log_file),
        Some(Commands::Init) => {
            // Unreachable — Init is handled in main::run before the
            // markdown corpus walk + config load. Treat as a no-op.
            Ok(ExitCode::SUCCESS)
        }
        Some(Commands::ScaffoldCoverage {
            crate_name,
            write,
            json,
        }) => scaffold::run(root, config, all_files, &crate_name, write, json),
        Some(Commands::Explain { symbol, list, json }) => {
            explain_mod::run(root, config, all_files, &symbol, list, json)
        }
        Some(Commands::MigrateAnchorRequired) => migrate_anchor::run(root, config, all_files),
        Some(Commands::MigrateEndpointMarkers {
            dry_run,
            include_typescript,
        }) => migrate_endpoint_markers::run(root, config, dry_run, include_typescript),
        Some(Commands::Homepage { write }) => homepage::run(root, config, all_files, write),
        Some(Commands::Codes { markdown }) => codes::run(root, markdown),
        Some(Commands::GenDocs { check }) => gen_docs::run(root, check),
        Some(Commands::Cluster {
            top_n,
            write,
            order_by,
            algorithm,
            min_members,
            seed_symbol,
            top_members,
            output,
            promote,
            resolution,
        }) => cluster::run(
            root,
            top_n,
            write,
            &order_by,
            &algorithm,
            min_members,
            seed_symbol.as_deref(),
            top_members,
            &output,
            promote.as_deref(),
            resolution,
        ),
        Some(Commands::Mcp) => mcp::run(root),
        Some(Commands::McpInstall { client }) => mcp_install::run(root, &client),
        #[cfg(unix)]
        Some(Commands::McpWorker { socket }) => mcp::run_worker(root, &socket),
        #[cfg(unix)]
        Some(Commands::McpSupervisor) => mcp_supervisor::run(root),
    }
}
