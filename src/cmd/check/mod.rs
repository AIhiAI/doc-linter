//! `doc-linter check` subcommand — the primary CLI endpoint. Walks
//! every doc, parses frontmatter, ingests the doc-graph corpus into
//! the SQLite graph, runs SCIP ingest if a `.doc-lint/code.scip` is present, then
//! emits every validator's `Issue` set (frontmatter, wikilinks, vale
//! alerts, dark-public-function, dark-endpoint,
//! entity-coverage-gap).
//!
//! Every other subcommand reads from the doc-graph that this one
//! populates, so `check` is also the canonical "warm the cache" call.
//! Operates on the whole vault as a knowledge-graph, not a single
//! doc-graph node.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use doc_linter::config::LintConfig;
use doc_linter::graph::{normalize_path, DocGraph, EdgeKind};
use doc_linter::homepage;
use doc_linter::ontology::Ontology;
use doc_linter::parser::{extract_links, parse_doc, Doc};
use doc_linter::validator::{validate_doc, Issue};

use super::check_single;
use super::util::{render_human, render_json, OutputFormat};

mod cluster_lint;
mod code_comments;
mod contradictions;
mod coverage;
mod scanners;
mod sitemap;
mod vale;

use cluster_lint::run_cluster_lint;
use code_comments::run_code_comment_pipeline;
use contradictions::run_contradiction_check;
use coverage::run_coverage_lint;
use scanners::{run_homepage_check, run_orphan_entity_check, run_unused_bounded_context_check};
use sitemap::write_sitemap;
use vale::run_vale_pipeline;

/// Walk the corpus, populate the [[entity-doc-graph]] SQLite graph, and
/// emit lint diagnostics for every doc / code-comment / endpoint /
/// coverage rule. The default subcommand when no other is given.
///
/// `silent` suppresses the human/json render so the MCP JSON-RPC stream
/// over stdout stays uncorrupted (gap-009: the `reingest` tool runs this
/// in-process); CLI callers pass `false`.
///
/// @endpoint CLI check
#[allow(clippy::too_many_arguments)] // grouping into a struct goes with the cmd/* split in #49
pub(crate) fn run(
    root: &Path,
    config: &LintConfig,
    all_files: &[PathBuf],
    only_file: Option<PathBuf>,
    format: OutputFormat,
    cli_no_vale: bool,
    cli_lint_code_comments: bool,
    cli_coverage_min_global: Option<f32>,
    cli_rebuild: bool,
    cli_embeddings: bool,
    cli_contradictions: bool,
    silent: bool,
) -> Result<CheckOutcome> {
    let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
    let mut parse_issues: Vec<(PathBuf, Issue)> = Vec::new();

    for path in all_files {
        // Exempt files (e.g. doc-linter's own scaffold templates under
        // crates/doc-linter/templates/, tool-managed .specify/templates/,
        // .claude/commands/speckit.*) carry their own non-vault frontmatter
        // schema. Skip them entirely so they don't pollute id_to_path with
        // duplicate-id collisions against the real vault. This makes the
        // exempt list semantically "the linter ignores these files" rather
        // than "the linter parses these but doesn't report on them".
        if config.is_exempt(path, root) {
            continue;
        }
        match parse_doc(path) {
            Ok(doc) => {
                docs.insert(path.clone(), doc);
            }
            Err(e) => {
                parse_issues.push((path.clone(), Issue::ParseError(e.to_string())));
            }
        }
    }

    let mut id_to_path: HashMap<String, PathBuf> = HashMap::new();
    let mut duplicate_issues: Vec<(PathBuf, Issue)> = Vec::new();
    // Walk in `all_files` order, not HashMap order, so the file we flag as
    // the dupe is the one graph ingest skips (first writer wins there too).
    for path in all_files {
        let Some(doc) = docs.get(path) else { continue };
        if let Some(meta) = &doc.meta {
            // First writer keeps the id (as in graph ingest); every later
            // dupe points at that survivor, not at the previous dupe.
            match id_to_path.entry(meta.id.clone()) {
                std::collections::hash_map::Entry::Occupied(kept) => duplicate_issues.push((
                    path.clone(),
                    Issue::DuplicateId {
                        id: meta.id.clone(),
                        other: kept.get().clone(),
                    },
                )),
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(path.clone());
                }
            }
        }
    }

    let stem_to_path: HashMap<String, PathBuf> = all_files
        .iter()
        .filter_map(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| (s.to_string(), p.clone()))
        })
        .collect();

    // Absolute path → id, used to resolve markdown `.md` links into the vault.
    let path_to_id: HashMap<PathBuf, String> = docs
        .iter()
        .filter_map(|(p, d)| d.meta.as_ref().map(|m| (p.clone(), m.id.clone())))
        .collect();

    // Graph is rebuilt for orphan detection. Only non-exempt docs with
    // frontmatter become nodes; edges combine wikilinks, `.md` markdown
    // links, and frontmatter typed-edge fields.
    let graph = DocGraph::build(root, config, all_files)?;

    // Ontology is assembled from docs with meta-roles (ontology-axis,
    // ontology-value, ontology-entity, ontology-migration). Bootstrap
    // meta-roles are recognised even when no ontology docs are present.
    let ontology = Ontology::load_from_docs(&docs);

    // Apply the exempt filter once when building target_files so every
    // downstream consumer (per-file validators, Vale post-processor,
    // render counts) sees the same set. Without this, Vale alerts on
    // exempt-listed files (e.g. `.claude/skills/**`, `.opencode/skills/**`)
    // bubble up because Vale walks the entire root and the post-processor
    // only filtered against `target_files`.
    let target_files: Vec<PathBuf> = if let Some(ref f) = only_file {
        let abs = if f.is_absolute() {
            f.clone()
        } else {
            root.join(f)
        };
        // Source files take the doc-comment vocab-closure path (Round
        // 3A's `lint_code_comments` engine, narrowed to one file) so the
        // PostToolUse hook gives the same immediate feedback on a code
        // edit as on a markdown edit. The corpus-wide passes (frontmatter,
        // wikilinks, SCIP, endpoints) stay with the full `check` run.
        if doc_linter::code_comments::Lang::from_path(&abs).is_some() {
            return check_single::code(root, config, &abs, format).map(Into::into);
        }
        let ext = abs.extension().and_then(|s| s.to_str());
        if !matches!(ext, Some("md" | "adoc")) {
            return Ok(ExitCode::SUCCESS.into());
        }
        let canon = match abs.canonicalize() {
            Ok(c) => c,
            Err(_) => return Ok(ExitCode::SUCCESS.into()),
        };
        if !all_files.iter().any(|p| p == &canon) {
            return Ok(ExitCode::SUCCESS.into());
        }
        if config.is_exempt(&canon, root) {
            // Exempt file passed via --file: silent no-op, matches the
            // "linter ignores these files entirely" semantics.
            return Ok(ExitCode::SUCCESS.into());
        }
        vec![canon]
    } else {
        // Docs from `cross_repo_roots` are indexed (graph, links, search)
        // but not validated: a sibling repo's schema isn't this repo's.
        all_files
            .iter()
            .filter(|p| p.starts_with(root) && !config.is_exempt(p, root))
            .cloned()
            .collect()
    };

    let mut report: Vec<(PathBuf, Vec<Issue>)> = Vec::new();
    for path in &target_files {
        let mut issues = Vec::new();
        for (p, issue) in &parse_issues {
            if p == path {
                issues.push(issue.clone());
            }
        }
        for (p, issue) in &duplicate_issues {
            if p == path {
                issues.push(issue.clone());
            }
        }
        if let Some(doc) = docs.get(path) {
            // Issue #180: `auto`-status docs are cluster-derived entity
            // stubs whose body is machine-extracted (SCIP symbols,
            // raw file paths in `## Top members`). Skip the per-doc
            // validation pass — broken-link, inline-path, orphan-doc,
            // self-loop checks would all fire on auto-generated text
            // that a human review will rewrite during promotion.
            let is_auto = doc.meta.as_ref().is_some_and(|m| m.status == "auto");
            // AsciiDoc without frontmatter: metadata was synthesized for
            // the graph, so there is no authored frontmatter to validate,
            // and AsciiDoc's `[[anchor]]` isn't a wikilink.
            let is_synthesized = doc.meta.is_some() && doc.raw_frontmatter.is_none();
            if is_auto || is_synthesized {
                continue;
            }
            if !config.is_exempt(path, root) {
                validate_doc(path, root, doc, config, &ontology, &mut issues);

                let links = extract_links(&doc.body);
                let doc_dir = path.parent().unwrap_or(root);

                for wl in &links.wikilinks {
                    if !id_to_path.contains_key(&wl.target)
                        && !stem_to_path.contains_key(&wl.target)
                    {
                        issues.push(Issue::BrokenWikilink {
                            target: wl.target.clone(),
                            line: doc.file_line(wl.line),
                        });
                    }
                }

                for link in &links.md_links {
                    let abs = normalize_path(&doc_dir.join(&link.target));
                    let is_md = link.target.to_ascii_lowercase().ends_with(".md");
                    if is_md {
                        // A doc outside the graph (exempt, e.g. CLAUDE.md)
                        // is still a valid link target when it exists.
                        if !path_to_id.contains_key(&abs) && !abs.exists() {
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
                        if !id_to_path.contains_key(&target) {
                            issues.push(Issue::BrokenTypedEdge {
                                field: field.to_string(),
                                target,
                            });
                        }
                    }

                    if let Some(idx) = graph.find(&meta.id) {
                        let in_count = graph.graph.edges_directed(idx, Direction::Incoming).count();
                        let out_count =
                            graph.graph.edges_directed(idx, Direction::Outgoing).count();
                        if in_count == 0 && out_count == 0 {
                            issues.push(Issue::OrphanDoc);
                        }

                        // G7 regression guard: any outbound edge that
                        // resolves back to this same doc. Almost always a
                        // copy-paste mistake. Covers the wikilink, md-link,
                        // typed-edge, and crate-ref kinds — `synthesize_crate_id`
                        // already screens its own self-references at the
                        // synthesis step, but this catches authored ones too.
                        for edge in graph.graph.edges_directed(idx, Direction::Outgoing) {
                            if edge.target() == idx {
                                issues.push(Issue::SelfLoop {
                                    edge_kind: edge.weight().kind,
                                    line: edge.weight().line,
                                });
                            }
                        }

                        // G5 regression guard: `lifecycle: superseded` must
                        // have an inbound `:SUPERSEDES` from the successor.
                        // No successor ⇒ either name one (`supersedes:` on
                        // the successor's frontmatter), change the lifecycle,
                        // or move the file under docs/archive/ (vault-exempt).
                        if meta.lifecycle.as_deref() == Some("superseded") {
                            let has_successor = graph
                                .graph
                                .edges_directed(idx, Direction::Incoming)
                                .any(|e| matches!(e.weight().kind, EdgeKind::Supersedes));
                            if !has_successor {
                                issues.push(Issue::SupersededWithoutSuccessor);
                            }
                        }
                    }
                }
            }
        }
        if !issues.is_empty() {
            report.push((path.clone(), issues));
        }
    }

    // Round 2A: Vale vocabulary-closure integration. Generates the per-repo
    // Vale config tree, shells out to `vale`, then post-processes alerts
    // through bounded-context resolution. Soft-fails when the binary is
    // missing — the rest of `check` keeps working. Skipped entirely when
    // `--no-vale` or `vale_enabled = false`.
    // A missing `vale_dictionaries` file only drops its pack, so say so
    // whether or not Vale runs: the code-comment lint and the
    // unstubbed-concept pass read the same packs.
    let dict_issues: Vec<Issue> = doc_linter::vale::unreadable_dictionaries(root, config)
        .into_iter()
        .map(|(name, path)| Issue::VocabDictionaryUnreadable { name, path })
        .collect();
    if !dict_issues.is_empty() {
        report.push((root.join(".doc-lint.toml"), dict_issues));
    }

    let vale_active = config.vale.vale_enabled && !cli_no_vale;
    if vale_active {
        let vale_issues = run_vale_pipeline(root, config, &ontology, &docs, &target_files)?;
        for (path, issue) in vale_issues {
            // Merge into the existing report — preserve per-file grouping
            // so the human/json renderers stay tidy.
            if let Some(slot) = report.iter_mut().find(|(p, _)| p == &path) {
                slot.1.push(issue);
            } else {
                report.push((path, vec![issue]));
            }
        }
    }

    // Round 3A: Rust source-comment vocab-closure pass. Off by default;
    // turned on per-repo (`lint_code_comments = true`) or per-invocation
    // (`--lint-code-comments`). Skipped under `--file` narrowing — the
    // pass walks `.rs`, not `.md`, and the narrow scope is markdown-only.
    let code_comment_active =
        (config.vale.lint_code_comments || cli_lint_code_comments) && only_file.is_none();
    if code_comment_active {
        let cc_issues = run_code_comment_pipeline(root, config, &ontology)?;
        for (path, issue) in cc_issues {
            if let Some(slot) = report.iter_mut().find(|(p, _)| p == &path) {
                slot.1.push(issue);
            } else {
                report.push((path, vec![issue]));
            }
        }
    }

    // Round 2B: refresh the SQLite graph, then re-derive sitemap.json from
    // it. Both are full-corpus operations, so we skip when --file narrows
    // the scope (the PostToolUse hook fires on every Edit/Write — partial
    // re-ingest would be a footgun).
    //
    // The petgraph above is *not* discarded — it's still the source of
    // truth for the orphan check (already run). The graph is the queryable
    // export; the two graphs are derived from the same corpus walk so
    // they stay in lockstep by construction.
    //
    // Phase 3 of roadmap-43 reorders this: the coverage-driven lint
    // (run_coverage_lint) runs AFTER scip+endpoint ingest, so the
    // primary human/json render is deferred until those diagnostics are
    // folded into `report`. In `--file` mode we skip the graph rebuild
    // and the coverage diagnostics entirely (the hook fires per-keystroke;
    // global metrics belong in CI, not the editor).
    let mut coverage_diagnostics: Vec<Issue> = Vec::new();
    if only_file.is_none() {
        // The graph is `.doc-lint/graph.sqlite`, built beside the live file
        // and swapped in; the post-ingest steps (contradictions, sitemap,
        // coverage and cluster lint) read it back through `GraphRead`.
        if !silent {
            if let Some(notice) = doc_linter::store_sqlite::legacy_graph_notice(root) {
                eprintln!("{notice}");
            }
        }
        let embedder = cli_embeddings.then(doc_linter::embeddings::default_embedder);
        let mut scip_snapshot = None;
        doc_linter::store_sqlite::build_and_swap(root, |db| {
            scip_snapshot = doc_linter::store_sqlite::ingest::pipeline::ingest_corpus(
                db,
                root,
                config,
                all_files,
                &ontology,
                embedder.as_deref(),
                cli_rebuild,
            )?;
            if cli_contradictions {
                match run_contradiction_check(db) {
                    Ok(stats) => eprintln!(
                        "doc-linter: contradictions check \
                         — {} pair(s) evaluated, {} finding(s)",
                        stats.pairs_evaluated, stats.findings,
                    ),
                    Err(e) => eprintln!("doc-linter: contradictions check failed: {e:#}"),
                }
            }
            if let Err(e) = write_sitemap(root, db) {
                eprintln!("doc-linter: failed to write sitemap.json: {e:#}");
            }
            match run_coverage_lint(db, config, cli_coverage_min_global, &ontology) {
                Ok(diags) => coverage_diagnostics = diags,
                Err(e) => eprintln!("doc-linter: coverage lint failed: {e:#}"),
            }
            match run_cluster_lint(db, &config.cluster_lint, &ontology) {
                Ok(mut diags) => coverage_diagnostics.append(&mut diags),
                Err(e) => eprintln!("doc-linter: cluster lint failed: {e:#}"),
            }
            Ok(())
        })
        .context("update the sqlite doc graph (index NOT updated)")?;
        // Only now that the swap committed: the next run may trust
        // the code rows this snapshot describes.
        if let Some(snap) = scip_snapshot {
            let path = doc_linter::scip_ingest_cache::sqlite_cache_path(root);
            if let Err(e) = doc_linter::scip_ingest_cache::write_at(&path, &snap) {
                eprintln!("doc-linter: scip ingest — failed to write cache: {e:#}");
            }
        }
    }

    // Coverage diagnostics are not associated with a single doc path,
    // so we attach them to the repo root as a synthetic file. The human
    // renderer prints `<root>: <message>`; the JSON renderer surfaces
    // them under a `file: ""` (or repo path) entry. Keep them separate
    // from per-file `report` slots until just before render so the
    // existing per-file ordering stays stable.
    if !coverage_diagnostics.is_empty() {
        report.push((root.to_path_buf(), coverage_diagnostics));
    }

    // Roadmap-48 Rule C: orphan-entity check. Walk every doc with
    // `role: ontology-entity` and verify at least one OTHER doc in
    // the corpus references the entity (via a `covers:` array entry
    // OR a `[[entity-<id>]]` wikilink in the body). In `--file`
    // mode we still run the full check (corpus walk has already
    // built the `docs` map) but filter the emitted issues to the
    // target path — so editing an entity-X.md gets per-edit
    // feedback without flooding the hook output with unrelated
    // orphan diagnostics from across the corpus.
    let orphan_issues =
        run_orphan_entity_check(&docs, config.ontology.orphan_entity_requires_covers);
    for (path, issue) in orphan_issues {
        if only_file.is_some() && !target_files.iter().any(|p| p == &path) {
            continue;
        }
        if let Some(slot) = report.iter_mut().find(|(p, _)| p == &path) {
            slot.1.push(issue);
        } else {
            report.push((path, vec![issue]));
        }
    }

    // Unused-bounded-context check. Gated by
    // `require_bounded_context_usage = true`. Walks every value
    // doc under the `bounded-context` axis and flags those with
    // zero docs claiming them, minus any listed in
    // `bounded_context_usage_exempt`. Skipped in `--file` mode —
    // it's a corpus-level property the per-file hook can't compute
    // on its own.
    if config.ontology.require_bounded_context_usage && only_file.is_none() {
        for (path, issue) in run_unused_bounded_context_check(&docs, config) {
            if let Some(slot) = report.iter_mut().find(|(p, _)| p == &path) {
                slot.1.push(issue);
            } else {
                report.push((path, vec![issue]));
            }
        }
    }

    // Homepage staleness check. Skipped in `--file` mode (the
    // per-file hook fires too quickly to do a meaningful corpus
    // walk; agents see the diagnostic on the next full `check`).
    // Gated on `require_homepage = true` so it's silent for repos
    // that haven't opted in.
    if config.ontology.require_homepage && only_file.is_none() {
        if let Some(issue) = run_homepage_check(root, &docs, &ontology, config) {
            let path = homepage::homepage_file_path(root, config);
            if let Some(slot) = report.iter_mut().find(|(p, _)| p == &path) {
                slot.1.push(issue);
            } else {
                report.push((path, vec![issue]));
            }
        }
    }

    // Roadmap-48 Rule C: severity-aware error count. `OrphanEntity`
    // (and only that variant) reads `orphan_entity_severity` —
    // `"warning"` keeps the diagnostic visible without breaking the
    // build, `"error"` promotes it to a hard fail. Every other
    // diagnostic is always an error.
    let error_count: usize = report
        .iter()
        .flat_map(|(_, v)| v.iter())
        .filter(|i| i.is_error(config))
        .count();

    // Free coverage-history snapshot: one row per full check, appended to
    // the freshly swapped graph (`--file` runs do not rebuild it).
    if only_file.is_none() {
        let total: usize = report.iter().map(|(_, v)| v.len()).sum();
        if let Err(e) = doc_linter::store_sqlite::history::record(
            root,
            config,
            error_count,
            total - error_count,
        ) {
            eprintln!("doc-linter: failed to record history snapshot: {e:#}");
        }
    }

    if !silent {
        match format {
            OutputFormat::Human => {
                render_human(root, &report, target_files.len(), error_count, &ontology);
            }
            OutputFormat::Json => render_json(root, &report, &ontology)?,
        }
    }

    let exit = if error_count > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    };
    Ok(CheckOutcome { exit, report })
}

/// What [`run`] found: the process exit code plus the per-file issue list,
/// so silent callers (MCP `reingest`) can surface what the render would
/// have printed.
pub(crate) struct CheckOutcome {
    pub(crate) exit: ExitCode,
    pub(crate) report: Vec<(PathBuf, Vec<Issue>)>,
}

impl From<ExitCode> for CheckOutcome {
    fn from(exit: ExitCode) -> Self {
        Self {
            exit,
            report: Vec::new(),
        }
    }
}
