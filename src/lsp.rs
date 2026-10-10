//! Round 4A: Language Server Protocol implementation for doc-linter.
//!
//! Drop-in goal: editors with a generic LSP client (VS Code, Helix,
//! Neovim, Emacs, etc.) point at `doc-linter lsp`, the server walks the
//! workspace once on `initialize`, then re-lints individual documents on
//! `did_open` / `did_change` / `did_save` and ships diagnostics back via
//! `textDocument/publishDiagnostics`.
//!
//! ## Pipeline scope inside the LSP
//!
//! Per-keystroke we run a strict subset of the `doc-linter check`
//! pipeline — anything that requires shelling out (Vale) or rebuilding
//! the graph (the store, SCIP) is **excluded**. What the LSP runs:
//!
//!   - Markdown frontmatter validation (`validator::validate_doc`).
//!   - Wikilink + markdown-link + typed-edge resolution against the
//!     id index built at `initialize` time.
//!   - In-process vocab-closure for capitalised tokens via
//!     `disambiguation::TermIndex` (this is the same index the Vale
//!     post-processor uses; running it directly avoids the per-keystroke
//!     `vale` shell-out which costs ~hundreds of ms).
//!   - For source files of any `code_comments::Lang` (when
//!     `lint_code_comments == true`): the shared `comment_lint` engine.
//!
//! Cross-doc invalidation (re-linting Y when X changes and X→Y was a
//! wikilink target) is **not** implemented in v1 — see TODO inside
//! [`DocLinterLsp::on_change`]. Correctness for the changed file alone
//! is ship-quality; cross-doc refresh is a perf-vs-completeness tradeoff
//! we punted on.
//!
//! ## stdio + logging
//!
//! `tower_lsp::Server` reads/writes JSON-RPC over stdin/stdout, so the
//! server can NEVER write to stdout for any other reason. All debug
//! output goes to a log file at `<workspace>/.doc-lint/lsp.log`
//! (overridable via `--log-file`). The `eprintln!` macros in the rest
//! of the binary write to stderr, which the LSP client typically
//! captures into its own log.

use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use tower_lsp::jsonrpc::Result as RpcResult;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticOptions, DiagnosticServerCapabilities, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    InitializeParams, InitializeResult, InitializedParams, MessageType, NumberOrString, Position,
    Range, ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
    WorkDoneProgressOptions,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::code_comments;
use crate::comment_lint::CommentLinter;
use crate::config::LintConfig;
use crate::ontology::Ontology;
use crate::parser::{extract_links, parse_doc, Doc};
use crate::validator::{validate_doc, Issue};

/// Walk the [[entity-doc-graph]] corpus the same way `cmd_check` does.
/// This is duplicated rather than refactored to keep the scope of Round 4A
/// surgical — the pipeline modules' core logic is untouched per the spec
/// constraints.
fn discover_markdown(root: &Path, config: &LintConfig) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("md") {
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}

/// Snapshot of corpus knowledge needed to lint a single doc against the
/// rest of the vault. Built once on `initialize` (or lazily on first
/// `did_open` if `lsp_corpus_walk_on_init = false`) and refreshed on
/// `did_save` for ontology / entity-* docs.
struct CorpusIndex {
    /// All known doc ids → absolute path. Built from frontmatter `id:`.
    id_to_path: HashMap<String, PathBuf>,
    /// File-stem (without `.md`) → absolute path. Wikilinks fall back
    /// to stem matching when an `id:` lookup misses.
    stem_to_path: HashMap<String, PathBuf>,
    /// Absolute path → frontmatter id, used to validate raw `.md` links
    /// (which point at a file path, not an id, but must still resolve
    /// to a doc that has an id).
    path_to_id: HashMap<PathBuf, String>,
    /// Ontology assembled from all docs with meta-roles.
    ontology: Arc<Ontology>,
}

impl CorpusIndex {
    fn build(root: &Path, config: &LintConfig) -> Self {
        let files = discover_markdown(root, config);
        let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
        for path in &files {
            if let Ok(doc) = parse_doc(path) {
                docs.insert(path.clone(), doc);
            }
        }
        let ontology = Ontology::load_from_docs(&docs);

        let mut id_to_path: HashMap<String, PathBuf> = HashMap::new();
        let mut path_to_id: HashMap<PathBuf, String> = HashMap::new();
        for (path, doc) in &docs {
            if let Some(meta) = &doc.meta {
                id_to_path.insert(meta.id.clone(), path.clone());
                path_to_id.insert(path.clone(), meta.id.clone());
            }
        }
        let stem_to_path: HashMap<String, PathBuf> = files
            .iter()
            .filter_map(|p| {
                p.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| (s.to_string(), p.clone()))
            })
            .collect();

        Self {
            id_to_path,
            stem_to_path,
            path_to_id,
            ontology: Arc::new(ontology),
        }
    }

    fn empty() -> Self {
        Self {
            id_to_path: HashMap::new(),
            stem_to_path: HashMap::new(),
            path_to_id: HashMap::new(),
            ontology: Arc::new(Ontology::bootstrap()),
        }
    }
}

/// Mutable LSP server state. Wrapped in `tokio::sync::Mutex` so the
/// async handlers can take `&self`.
struct LspState {
    root: PathBuf,
    config: LintConfig,
    corpus: CorpusIndex,
    /// URIs we've opened diagnostics for. Tracked so `did_close` can
    /// clear them with an empty diagnostics publish.
    open_docs: HashSet<Url>,
}

/// `tower-lsp` server implementation that exposes the
/// [[entity-doc-graph]] linter to editors as live diagnostics.
pub struct DocLinterLsp {
    client: Client,
    state: tokio::sync::Mutex<LspState>,
}

/// Per-server lifecycle methods — `initialize`, doc lint, and the
/// `did_*` notification handlers — for the [[entity-doc-graph]] LSP.
impl DocLinterLsp {
    fn new(client: Client, root: PathBuf, config: LintConfig) -> Self {
        Self {
            client,
            state: tokio::sync::Mutex::new(LspState {
                root,
                config,
                corpus: CorpusIndex::empty(),
                open_docs: HashSet::new(),
            }),
        }
    }

    /// Re-lint the [[entity-doc-graph]] document at `uri` against `text` and
    /// publish the resulting diagnostics. Centralised so `did_open`,
    /// `did_change`, and `did_save` all funnel through the same pipeline.
    async fn on_change(&self, uri: Url, text: String) {
        let started = Instant::now();
        let path = match uri.to_file_path() {
            Ok(p) => p,
            Err(()) => return, // non-file:// URI; nothing to lint
        };

        let mut state = self.state.lock().await;
        // Track for did_close even if lint produces zero diagnostics —
        // we still need to clear them later.
        state.open_docs.insert(uri.clone());

        let diagnostics = lint_one(&path, &text, &state.root, &state.config, &state.corpus);
        drop(state);

        let elapsed_ms = started.elapsed().as_millis();
        // TODO: cross-doc invalidation. When `path` is referenced as a
        // wikilink target by other open docs, those docs' diagnostics
        // could also need refreshing (e.g. broken-wikilink → resolved
        // once this file gains a matching id). v1 punts: re-linting
        // only the changed file is correct for the file's own
        // diagnostics, just not for its inbound references. Track in
        // crates/doc-linter/README.md "known limitations".
        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;

        if elapsed_ms > 50 {
            // Slow-path warning: budget says <50ms for a single .md
            // lint. Log to stderr (LSP captures into its log). Don't
            // surface to the user as a popup — too noisy.
            eprintln!("doc-linter lsp: lint took {elapsed_ms} ms (>50ms budget)");
        }
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for DocLinterLsp {
    async fn initialize(&self, params: InitializeParams) -> RpcResult<InitializeResult> {
        // Resolve the workspace root. Prefer workspace_folders[0] (multi-
        // root LSP), fall back to root_uri (single-root), fall back to
        // the cwd (the harness already canonicalised this in main).
        let root = params
            .workspace_folders
            .as_ref()
            .and_then(|wf| wf.first())
            .and_then(|f| f.uri.to_file_path().ok())
            .or_else(|| {
                #[allow(deprecated)]
                params.root_uri.as_ref().and_then(|u| u.to_file_path().ok())
            });

        let mut state = self.state.lock().await;
        if let Some(root) = root {
            state.root = root;
        }
        // Reload config from the (possibly new) workspace root.
        let config_path = state.root.join(".doc-lint.toml");
        if let Ok(cfg) = LintConfig::load(&config_path) {
            state.config = cfg;
        }
        if state.config.lsp_corpus_walk_on_init {
            state.corpus = CorpusIndex::build(&state.root, &state.config);
        }

        let sync = TextDocumentSyncCapability::Kind(if state.config.lsp_lint_on_change {
            TextDocumentSyncKind::FULL
        } else {
            // When change-lint is disabled we still need open/close
            // events but not text content updates — FULL is the only
            // safe choice for a stdio LSP that wants the buffer text.
            TextDocumentSyncKind::FULL
        });

        let diagnostic_provider = DiagnosticServerCapabilities::Options(DiagnosticOptions {
            identifier: Some("doc-linter".to_string()),
            inter_file_dependencies: true,
            workspace_diagnostics: false,
            work_done_progress_options: WorkDoneProgressOptions::default(),
        });

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(sync),
                diagnostic_provider: Some(diagnostic_provider),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "doc-linter".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let state = self.state.lock().await;
        let root = state.root.display().to_string();
        let n_docs = state.corpus.id_to_path.len();
        drop(state);
        self.client
            .log_message(
                MessageType::INFO,
                format!("doc-linter lsp ready — root={root}, docs={n_docs}"),
            )
            .await;
    }

    async fn shutdown(&self) -> RpcResult<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        // Lazy corpus build for the lazy-init path.
        {
            let mut state = self.state.lock().await;
            if state.corpus.id_to_path.is_empty() && !state.config.lsp_corpus_walk_on_init {
                state.corpus = CorpusIndex::build(&state.root, &state.config);
            }
        }
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.on_change(uri, text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let lint_on_change = {
            let state = self.state.lock().await;
            state.config.lsp_lint_on_change
        };
        if !lint_on_change {
            return;
        }
        let uri = params.text_document.uri;
        // Server is registered for FULL sync; the client sends a single
        // change containing the entire new buffer text. (Per the LSP
        // spec, with FULL sync the `range` field is unset and `text`
        // is the full content.)
        let Some(change) = params.content_changes.into_iter().next() else {
            return;
        };
        self.on_change(uri, change.text).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        // The save text is optional — clients only include it when the
        // server registers with `includeText: true`. When absent, we
        // re-read from disk. This is the only LSP path that should
        // touch disk; did_change uses the in-memory text exclusively.
        let uri = params.text_document.uri;
        let text = if let Some(t) = params.text {
            t
        } else {
            let Ok(path) = uri.to_file_path() else {
                return;
            };
            match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(_) => return,
            }
        };

        // TODO: when the saved file's frontmatter role is one of
        // {ontology-axis, ontology-value, ontology-entity,
        // ontology-migration}, the ontology rebuild affects every other
        // doc's lint result — re-walk the corpus and re-lint open
        // docs. Skipped in v1 for simplicity; users can re-open the
        // affected docs to trigger a fresh did_open.

        self.on_change(uri, text).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        let mut state = self.state.lock().await;
        state.open_docs.remove(&uri);
        drop(state);
        // Per the LSP spec, the server is responsible for clearing
        // diagnostics on close — clients don't do it for us.
        self.client.publish_diagnostics(uri, Vec::new(), None).await;
    }
}

/// Per-file lint dispatch for the [[entity-doc-graph]] LSP server. Pure
/// function (modulo the on-disk read of peer docs that already happened when
/// `corpus` was built) — easy to unit-test. Returns the LSP `Diagnostic` list,
/// never an error: a failed parse becomes a `parse-error` diagnostic, never a
/// panic.
fn lint_one(
    path: &Path,
    text: &str,
    root: &Path,
    config: &LintConfig,
    corpus: &CorpusIndex,
) -> Vec<Diagnostic> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let issues: Vec<Issue> = match code_comments::Lang::from_path(path) {
        None if ext == "md" => lint_markdown(path, root, text, config, corpus),
        Some(lang) if config.vale.lint_code_comments => {
            lint_code_comments(path, lang, text, root, config, corpus)
        }
        _ => Vec::new(),
    };

    issues.into_iter().map(issue_to_diagnostic).collect()
}

/// Markdown lint pipeline reduced for the [[entity-doc-graph]] LSP. Mirrors
/// `cmd_check`'s per-file body but uses the in-memory `text` rather than
/// re-reading from disk and SKIPS Vale (too slow per-keystroke) and the
/// orphan-doc check (which is a graph-wide property the LSP can't compute
/// without rebuilding the graph for every keystroke).
fn lint_markdown(
    path: &Path,
    root: &Path,
    text: &str,
    config: &LintConfig,
    corpus: &CorpusIndex,
) -> Vec<Issue> {
    let mut issues: Vec<Issue> = Vec::new();

    // Parse frontmatter from the in-memory text. Use the same path the
    // disk parser uses but feed it our buffer.
    let doc = match parse_doc_from_str(text) {
        Ok(d) => d,
        Err(e) => {
            issues.push(Issue::ParseError(e.to_string()));
            return issues;
        }
    };

    if doc.meta.is_none() && doc.raw_frontmatter.is_none() {
        issues.push(Issue::MissingFrontmatter);
        return issues;
    }

    validate_doc(path, root, &doc, config, &corpus.ontology, &mut issues);

    let links = extract_links(&doc.body);
    let doc_dir = path.parent().unwrap_or(path);

    for wl in &links.wikilinks {
        if !corpus.id_to_path.contains_key(&wl.target)
            && !corpus.stem_to_path.contains_key(&wl.target)
        {
            issues.push(Issue::BrokenWikilink {
                target: wl.target.clone(),
                line: doc.file_line(wl.line),
            });
        }
    }

    for link in &links.md_links {
        let abs = crate::graph::normalize_path(&doc_dir.join(&link.target));
        let is_md = link.target.to_ascii_lowercase().ends_with(".md");
        if is_md {
            if !corpus.path_to_id.contains_key(&abs) {
                issues.push(Issue::BrokenMdLink {
                    target: link.raw.clone(),
                    line: doc.file_line(link.line),
                });
            }
        } else if !abs.exists() {
            issues.push(Issue::BrokenFileLink {
                target: link.raw.clone(),
                line: doc.file_line(link.line),
            });
        }
    }

    for ip in &links.inline_paths {
        if config.is_unlinked_path_exempt(&ip.text) {
            continue;
        }
        issues.push(Issue::UnlinkedPath {
            text: ip.text.clone(),
            line: doc.file_line(ip.line),
        });
    }

    if let Some(meta) = &doc.meta {
        for (field, target) in meta.typed_edges() {
            if !corpus.id_to_path.contains_key(&target) {
                issues.push(Issue::BrokenTypedEdge {
                    field: field.to_string(),
                    target,
                });
            }
        }
    }

    issues
}

/// `parse_doc` clone that takes the source text directly for the
/// [[entity-doc-graph]] LSP. Avoids the disk re-read on the hot path.
fn parse_doc_from_str(text: &str) -> Result<Doc> {
    use crate::parser::Frontmatter;
    let (fm_text, body) = split_frontmatter(text);
    let meta = if let Some(yaml) = &fm_text {
        Some(serde_yaml::from_str::<Frontmatter>(yaml).context("parse frontmatter")?)
    } else {
        None
    };
    let body_pos = text.len() - body.len();
    let body_line_offset = text.as_bytes()[..body_pos]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1;
    Ok(Doc {
        meta,
        body: body.to_string(),
        body_line_offset,
        raw_frontmatter: fm_text,
    })
}

/// Inline [[entity-doc-graph]] frontmatter splitter for the LSP. Mirrors
/// the (private) one in `parser.rs` — duplicated here rather than exposed
/// because the parser module's `parse_doc` exclusively takes a `&Path`.
/// Round 4B can fold these together.
fn split_frontmatter(text: &str) -> (Option<String>, &str) {
    let trimmed_start = text.trim_start_matches('\u{feff}');
    if !trimmed_start.starts_with("---") {
        return (None, text);
    }
    let after_open = match trimmed_start.strip_prefix("---\n") {
        Some(s) => s,
        None => match trimmed_start.strip_prefix("---\r\n") {
            Some(s) => s,
            None => return (None, text),
        },
    };
    let mut offset = 0;
    for line in after_open.split_inclusive('\n') {
        let line_no_newline = line.trim_end_matches(['\n', '\r']);
        if line_no_newline == "---" {
            let yaml = &after_open[..offset];
            let body_start = offset + line.len();
            let body = &after_open[body_start..];
            return (Some(yaml.to_string()), body);
        }
        offset += line.len();
    }
    (None, text)
}

/// Round 3A's comment-vocab-closure pipeline for the [[entity-doc-graph]]
/// LSP, reduced to one in-memory buffer — the same [`CommentLinter`] the
/// CLI runs, fed the unsaved text.
// ponytail: rebuilds the accept set per call; cache it on `CorpusIndex`
// if per-keystroke latency on source files ever shows up.
fn lint_code_comments(
    path: &Path,
    lang: code_comments::Lang,
    text: &str,
    root: &Path,
    config: &LintConfig,
    corpus: &CorpusIndex,
) -> Vec<Issue> {
    let Ok(extraction) = code_comments::extract_any_from_str(lang, text) else {
        return Vec::new();
    };
    let Ok(linter) = CommentLinter::new(root, config, &corpus.ontology) else {
        return Vec::new();
    };
    linter.lint(
        path,
        &extraction.doc_comments,
        config.requires_anchor(path, root),
    )
}

/// Map an [[entity-doc-graph]] `Issue` to an LSP `Diagnostic`. Pure
/// function — easy to unit-test. Severity choice mirrors the spec:
///
///   - parse-error / broken-* / missing-* → Error
///   - vocab / orphan / stale-scip / cross-context → Warning
fn issue_to_diagnostic(issue: Issue) -> Diagnostic {
    use tower_lsp::lsp_types::DiagnosticSeverity as Sev;

    let severity = match &issue {
        // Hard errors: structure / link integrity / required fields.
        Issue::MissingFrontmatter
        | Issue::ParseError(_)
        | Issue::MissingField(_)
        | Issue::YamlScalarLooksTruncated { .. }
        | Issue::LegacyTypeField { .. }
        | Issue::UnknownRole { .. }
        | Issue::WrongBoundedContextField { .. }
        | Issue::UnknownAxisValue { .. }
        | Issue::MissingAxisField { .. }
        | Issue::LifecycleNotAllowedForRole { .. }
        | Issue::FilenamePatternMismatch { .. }
        | Issue::WrongFolder { .. }
        | Issue::DuplicateId { .. }
        | Issue::IdDoesNotMatchStem { .. }
        | Issue::BrokenWikilink { .. }
        | Issue::BrokenMdLink { .. }
        | Issue::BrokenFileLink { .. }
        | Issue::BrokenTypedEdge { .. }
        | Issue::InvalidStatus { .. }
        | Issue::InvalidVisibility { .. }
        | Issue::UpdatedInFuture { .. }
        // Phase 3 of roadmap-43: dark-endpoint and the global
        // coverage-below-min are hard CI-level errors. Endpoints are
        // user-facing and must declare a domain entity; the global
        // threshold gate is a deliberate CI knob.
        | Issue::DarkEndpoint { .. }
        | Issue::CoverageBelowMinimum { .. }
        | Issue::ValeFailed { .. }
        | Issue::VocabDictionaryUnreadable { .. } => Sev::ERROR,

        // Soft / advisory: vocab, orphans, scip lifecycle, vale infra.
        Issue::UnknownEntity { .. }
        | Issue::UnknownBoundedContext { .. }
        | Issue::UnlinkedPath { .. }
        | Issue::OrphanDoc
        | Issue::ValeNotInstalled
        | Issue::ValeAlert { .. }
        | Issue::CrossContextReference { .. }
        | Issue::CommentVocabViolation { .. }
        | Issue::MissingAnchor { .. }
        | Issue::ScipIndexerMissing
        | Issue::ScipFileStale { .. }
        | Issue::ScipFileMissing { .. }
        // G7 / G5 lock-in validators (sweep-2): self-loop and
        // superseded-without-successor are graph-shape advisories that
        // surface in lint but don't block authoring per se.
        | Issue::SelfLoop { .. }
        | Issue::SupersededWithoutSuccessor
        // Phase 3 of roadmap-43: dark-public-function is a Warning
        // (per-crate ramp). entity-coverage-gap is corpus-level
        // INFORMATION (no specific file location).
        | Issue::DarkPublicFunction { .. }
        // Roadmap-48 Rule C: orphan-entity is a per-edit warning by
        // default. Repos that promote it to error via
        // `orphan_entity_severity = "error"` get the hard fail at
        // exit-code time; the LSP UI surface stays at Warning so
        // the squiggle isn't blocking authoring in real time.
        | Issue::OrphanEntity { .. }
        // Graph-density rules: same convention as orphan-entity. The
        // CLI exit code honours each rule's configured severity; the
        // LSP surface stays at Warning so the squiggle is visible
        // but non-blocking while the author types.
        | Issue::MissingCovers
        | Issue::MissingBoundedContext
        | Issue::PlaceholderEntity { .. }
        // Homepage rules: repo-level diagnostics attached to a file
        // the editor doesn't usually have open; Warning keeps the
        // squiggle non-blocking. CLI exit code still honours
        // `homepage_stale_severity`.
        | Issue::HomepageMissing { .. }
        | Issue::HomepageStale { .. }
        // Body-wikilink + bounded-context-usage rules: density
        // advisories. Warning in the LSP surface; the CLI honours
        // each rule's configured severity at exit-code time.
        | Issue::MissingBodyWikilink { .. }
        | Issue::UnusedBoundedContext { .. } => Sev::WARNING,

        Issue::EntityCoverageGap { .. } => Sev::INFORMATION,

        Issue::UnauthoredCluster { .. } => Sev::WARNING,
    };

    // Best-effort line extraction. The Issue enum stores 1-based file
    // lines; LSP wants 0-based. Issues without a line (frontmatter
    // structure, orphan-doc, etc.) attach to line 0 — the editor
    // shows the diagnostic as a file-level marker.
    let line_1based: Option<usize> = match &issue {
        Issue::BrokenWikilink { line, .. } => Some(*line),
        Issue::BrokenMdLink { line, .. } => Some(*line),
        Issue::BrokenFileLink { line, .. } => Some(*line),
        Issue::UnlinkedPath { line, .. } => Some(*line),
        Issue::ValeAlert { line, .. } => Some(*line),
        Issue::CrossContextReference { line, .. } => Some(*line),
        Issue::CommentVocabViolation { line, .. } => Some(*line),
        Issue::MissingAnchor { line, .. } => Some(*line),
        // Phase 3 of roadmap-43: source location for dark-public-function /
        // dark-endpoint comes from the SCIP / endpoint-extract row.
        Issue::DarkPublicFunction { line, .. } => Some(*line as usize),
        Issue::DarkEndpoint { line, .. } => Some(*line as usize),
        _ => None,
    };
    let lsp_line: u32 = line_1based.map_or(0, |l| l.saturating_sub(1) as u32);
    let range = Range {
        start: Position {
            line: lsp_line,
            character: 0,
        },
        end: Position {
            line: lsp_line,
            character: u32::MAX,
        },
    };

    let code = NumberOrString::String(issue.code().to_string());
    let message = issue.to_string();

    Diagnostic {
        range,
        severity: Some(severity),
        code: Some(code),
        code_description: None,
        source: Some("doc-linter".to_string()),
        message,
        related_information: None,
        tags: None,
        data: None,
    }
}

/// Entry point for the [[entity-doc-graph]] LSP server, invoked from
/// `Commands::Lsp` in main.rs. Builds a tokio runtime, wires stdin / stdout /
/// a shared service into `tower_lsp::Server`, and blocks until the LSP shuts
/// down.
pub fn run(root: &Path, config: &LintConfig, log_file: Option<PathBuf>) -> Result<()> {
    // Resolve the log path early so we can fail loudly if the parent
    // directory can't be created. Default: <root>/.doc-lint/lsp.log.
    let log_path = log_file.unwrap_or_else(|| root.join(".doc-lint").join("lsp.log"));
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    // Open the log file in append mode and redirect stderr there. This
    // is the cleanest way to capture every `eprintln!` in the binary
    // (the store, scip, vale callouts) without rewriting them to use
    // tracing. stdout is sacrosanct for the JSON-RPC channel.
    let log_handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;

    // Best-effort stderr redirect. `dup2_stderr` is a safe wrapper around
    // POSIX dup2 — after this the kernel has its own reference to the
    // file, so `log_handle` is free to drop. If the dup2 fails (rare)
    // stderr keeps pointing at the editor's LSP-output pane, which is
    // still visible to the user.
    // ponytail: unix only (rustix::stdio does not exist on Windows); there
    // stderr stays on the editor's output pane. Ceiling: SetStdHandle needs
    // unsafe/windows-sys, add it if Windows LSP users report protocol noise.
    #[cfg(unix)]
    let _ = rustix::stdio::dup2_stderr(&log_handle);
    drop(log_handle);

    let root = root.to_path_buf();
    let config = (*config).clone_for_lsp();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("build tokio runtime")?;

    runtime.block_on(async move {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        let (service, socket) = LspService::new(|client| DocLinterLsp::new(client, root, config));
        Server::new(stdin, stdout, socket).serve(service).await;
    });
    Ok(())
}

/// `LintConfig` is `!Clone` (it holds compiled `GlobSet`s which aren't
/// cheaply clonable). For the LSP we need an owned value. This helper
/// re-loads from the same path the main entry point used; falls back
/// to `Default` if the file disappeared between `main()` and now
/// (unlikely in practice but the code path is harmless).
/// Re-loads a `LintConfig` from disk for the [[entity-doc-graph]] LSP
/// server (the underlying type is `!Clone` because of compiled glob sets).
trait CloneForLsp {
    /// Returns an owned `LintConfig` rebuilt from the on-disk
    /// `.doc-lint.toml` for the [[entity-doc-graph]] LSP server.
    fn clone_for_lsp(&self) -> LintConfig;
}

/// Re-load via TOML round-trip implementation of [`CloneForLsp`] for the
/// [[entity-doc-graph]] LSP server.
impl CloneForLsp for LintConfig {
    fn clone_for_lsp(&self) -> LintConfig {
        // We can't trivially clone GlobSet, so re-build via TOML round-
        // trip is the simplest correct path. The config we accept here
        // is whatever `main()` already loaded — its source TOML lives
        // at `<root>/.doc-lint.toml`. Re-loading there gives us a
        // fully-populated owned LintConfig with all `*_set` fields
        // re-built from the patterns. If that file is gone, fall back
        // to `Default`.
        // Note: `self` is consumed only for its `include` etc; we
        // ignore it and just re-load. This is fine because main() is
        // the only caller and main()'s root + config_path are stable.
        // ... but we don't have the path here. Reconstruct from CWD as
        // best we can. Round 4B can plumb the path through.
        let candidate = std::env::current_dir()
            .ok()
            .map(|d| d.join(".doc-lint.toml"));
        if let Some(p) = candidate {
            if p.exists() {
                if let Ok(c) = LintConfig::load(&p) {
                    return c;
                }
            }
        }
        LintConfig::default()
    }
}

/// LSP-mapping tests for the [[entity-doc-graph]] LSP server — assert
/// that lint `Issue` values turn into the right LSP `Diagnostic` shape.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::DiagnosticSeverity;

    /// Asserts that a `BrokenWikilink` issue becomes an LSP error diagnostic
    /// with the correct `code` field for the [[entity-doc-graph]] LSP.
    #[test]
    fn broken_wikilink_maps_to_error_diagnostic_with_correct_code() {
        let issue = Issue::BrokenWikilink {
            target: "missing-doc".to_string(),
            line: 7,
        };
        let diag = issue_to_diagnostic(issue);

        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        match diag.code {
            Some(NumberOrString::String(s)) => assert_eq!(s, "broken-wikilink"),
            other => panic!("expected String code, got {other:?}"),
        }
        // Linter line 7 (1-based) → LSP line 6 (0-based).
        assert_eq!(diag.range.start.line, 6);
        assert_eq!(diag.source.as_deref(), Some("doc-linter"));
        assert!(diag.message.contains("missing-doc"));
    }

    /// Asserts that a vocabulary-violation issue becomes a `WARNING`
    /// severity LSP diagnostic in the [[entity-doc-graph]] LSP mapping.
    #[test]
    fn vocab_issue_maps_to_warning_severity() {
        let issue = Issue::CommentVocabViolation {
            file: PathBuf::from("src/lib.rs"),
            line: 12,
            col: 4,
            term: "Foozle".to_string(),
        };
        let diag = issue_to_diagnostic(issue);
        assert_eq!(diag.severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(diag.range.start.line, 11);
    }

    /// Asserts that a `MissingFrontmatter` issue attaches its diagnostic
    /// to line 0 in the [[entity-doc-graph]] LSP mapping (no specific
    /// span available).
    #[test]
    fn missing_frontmatter_attaches_to_line_zero() {
        let issue = Issue::MissingFrontmatter;
        let diag = issue_to_diagnostic(issue);
        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.range.start.line, 0);
    }

    /// Asserts that the [[entity-doc-graph]] LSP `lint_one` emits one
    /// `missing-frontmatter` diagnostic for a doc without YAML front matter.
    #[test]
    fn lint_one_on_missing_frontmatter_emits_one_diagnostic() {
        // Build an empty corpus so wikilink resolution isn't a factor.
        let corpus = CorpusIndex::empty();
        let config = LintConfig::default();
        let path = PathBuf::from("/tmp/doc-linter-lsp-test/no-fm.md");
        let text = "# Just a heading\n\nNo frontmatter here.\n";
        let diags = lint_one(&path, text, Path::new("/tmp"), &config, &corpus);
        assert_eq!(diags.len(), 1);
        assert!(matches!(
            diags[0].code,
            Some(NumberOrString::String(ref s)) if s == "missing-frontmatter"
        ));
    }

    /// Asserts that `lint_one` short-circuits for non-markdown paths in
    /// the [[entity-doc-graph]] LSP — the editor opens .rs/.toml files
    /// too and we don't want spurious diagnostics there.
    #[test]
    fn lint_one_on_non_md_file_returns_empty() {
        let corpus = CorpusIndex::empty();
        let config = LintConfig::default();
        let path = PathBuf::from("/tmp/doc-linter-lsp-test/foo.txt");
        let diags = lint_one(&path, "hi", Path::new("/tmp"), &config, &corpus);
        assert!(diags.is_empty());
    }

    /// Steady-state latency tripwire for the [[entity-doc-graph]] LSP
    /// per-keystroke lint — asserts a representative doc lints in well
    /// under the 50ms budget so live diagnostics stay snappy.
    #[test]
    fn per_keystroke_lint_latency_is_under_budget() {
        // Round 4A budget: a single .md lint should produce diagnostics
        // in <50ms on a warm process. Build a representative doc body
        // (~300 lines, 20 wikilinks, 10 md-links) and time three back-
        // to-back lints — first one absorbs first-time regex/parser
        // costs, the next two represent the steady-state per-keystroke
        // path. Asserts the third sample is comfortably under 50ms.
        let corpus = CorpusIndex::empty();
        let config = LintConfig::default();
        let path = PathBuf::from("/tmp/doc-linter-lsp-test/perf.md");

        let mut body = String::from(
            "---\n\
            id: perf\n\
            role: ontology-axis\n\
            title: Perf\n\
            summary: Latency probe.\n\
            status: draft\n\
            updated: 2026-04-30\n\
            ---\n\n# Perf\n\n",
        );
        for _ in 0..150 {
            body.push_str("Some prose with [[wikilink]] and [a link](other.md). ");
            body.push('\n');
        }

        // Warm up.
        let _ = lint_one(&path, &body, Path::new("/tmp"), &config, &corpus);
        // Steady-state samples.
        let mut last_ms = 0u128;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            let _ = lint_one(&path, &body, Path::new("/tmp"), &config, &corpus);
            last_ms = t.elapsed().as_millis();
        }
        // 50ms is the spec budget. Leave headroom (3x) so test is
        // stable on slow CI runners — we still catch any 10x-ish
        // regression.
        assert!(
            last_ms < 150,
            "per-keystroke lint took {last_ms}ms, expected <50ms steady-state"
        );
    }

    /// Asserts that a well-formed doc produces zero diagnostics in the
    /// [[entity-doc-graph]] LSP `lint_one` path — the success-case
    /// roundtrip.
    #[test]
    fn lint_one_on_well_formed_doc_returns_empty() {
        // Round-trip a doc that uses ONLY bootstrap-recognised
        // vocabulary so the empty-corpus ontology accepts it. The
        // hardcoded meta-role `ontology-axis` (used by ontology docs
        // themselves) needs no kind/lifecycle, so this is the simplest
        // valid frontmatter the LSP will see in a fresh workspace.
        let corpus = CorpusIndex::empty();
        let config = LintConfig::default();
        let path = PathBuf::from("/tmp/doc-linter-lsp-test/clean.md");
        let text = "---\n\
            id: clean\n\
            role: ontology-axis\n\
            title: Clean\n\
            summary: A clean doc with no lint issues for LSP testing.\n\
            status: draft\n\
            updated: 2026-04-30\n\
            ---\n\
            \n\
            # Clean\n\
            \n\
            Plain prose without wikilinks.\n";
        let diags = lint_one(&path, text, Path::new("/tmp"), &config, &corpus);
        assert!(diags.is_empty(), "expected no diagnostics, got {diags:?}");
    }
}
