//! Roadmap-49 phase 2: one-shot migration tool that walks the
//! syntactic endpoint extractor's output and stamps `@endpoint
//! <METHOD> <path>` lines on each handler's doc-comment. After the
//! run, [`crate::endpoint_extract::extract_endpoint_markers_workspace`]
//! becomes a complete substitute for the regex extractor — at which
//! point flipping `endpoint_marker_exclusive = true` is safe (a
//! follow-up commit, not this one).
//!
//! ## Algorithm
//!
//! 1. Refuse to run on a dirty git tree (`git status --porcelain`
//!    non-empty → exit 2). Override only via `--dry-run`.
//! 2. Run the existing `extract_endpoints` to get every
//!    [`crate::endpoint_extract::EndpointFact`] whose `handler_local`
//!    resolved.
//! 3. For each fact, look up the handler's source location via SCIP
//!    (`FunctionIndex::build` → `function.symbol → file:line`). When
//!    SCIP can't resolve, queue the fact for the manual report.
//! 4. Read the resolved source file. Locate the `///` block above
//!    the handler's `fn` line (or the handler's struct definition
//!    for MCP `HandlerStruct` cases). Append the marker line at the
//!    end of the existing block; create a minimal `///` block when
//!    none exists.
//! 5. Write a "manual" report to `/tmp/endpoint-marker-manual.txt`
//!    with one line per unresolved endpoint plus a hint at where
//!    the handler probably lives.
//!
//! ## Constraints
//!
//! - Refuses to run on a dirty git tree; `--dry-run` is always
//!   allowed (no writes).
//! - Skips facts whose marker would already exist (the file's
//!   doc-comment already carries the `@endpoint <METHOD> <path>`
//!   line — re-running is a no-op).
//! - Inserts each new marker on a single new `///` line; multi-fact
//!   handlers (a struct serving both GET and POST) get one line per
//!   fact.

use crate::config::LintConfig;
use crate::endpoint_extract::{extract_endpoints, EndpointFact, EndpointKind};
use crate::scip_ingest;
use crate::store::FunctionIndex;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// One proposed `@endpoint` stamp targeting a handler in the
/// [[entity-doc-graph]]. The migration tool collects these for every
/// EndpointFact whose handler resolved via SCIP, applies them in
/// `--write` mode, or just prints them in `--dry-run`.
#[derive(Debug, Clone)]
pub struct MarkerProposal {
    /// Repo-relative path to the source file that will be edited.
    pub file: String,
    /// 1-based line of the handler's `fn` (or `struct` for MCP
    /// HandlerStruct) keyword. The marker block is inserted ABOVE
    /// this line.
    pub handler_line: u32,
    /// Marker method as it should appear in the stamped comment
    /// (`GET`/`POST`/`CLI`/`MCP`/...).
    pub method: String,
    /// Marker path verbatim as the migrate tool found it in the
    /// EndpointFact (route path, kebab-cased subcommand, MCP tool
    /// name).
    pub path: String,
    /// Best-effort name of the handler — for reporting only; the
    /// writer locates the `fn`/`struct` line via `handler_line` +
    /// nearby search so a slightly-wrong name doesn't stop the
    /// stamp from landing.
    pub handler_name: String,
}

/// One unresolved endpoint that didn't make it into a [[entity-doc-graph]]
/// `@endpoint` proposal — emitted to the manual report so a human / agent
/// can stamp the marker in source by hand.
#[derive(Debug, Clone)]
pub struct ManualEntry {
    pub kind: EndpointKind,
    pub method: String,
    pub path: String,
    pub reason: String,
    pub hint: Option<String>,
}

/// CLI entry point for the [[entity-doc-graph]] @endpoint-marker migrator —
/// wires the migrator up against a fresh SCIP load. Mirrors `scaffold::run`'s
/// shape; converts the regex extractor's output into stamped @endpoint
/// doc-comment lines on each handler.
pub fn run(
    root: &Path,
    config: &LintConfig,
    dry_run: bool,
    include_typescript: bool,
) -> Result<ExitCode> {
    // 1. Refuse on dirty tree (unless dry-run, which never writes).
    if !dry_run && !is_working_tree_clean(root)? {
        eprintln!(
            "doc-linter: migrate-endpoint-markers requires a clean working tree; \
             commit or stash first (use --dry-run to preview proposals on a dirty tree)"
        );
        return Ok(ExitCode::from(2));
    }

    // 2. Run the regex extractor.
    let regex_facts = extract_endpoints(root, config).context("extract_endpoints for migration")?;

    // 3. Load SCIP for handler resolution. SCIP is optional — when
    //    absent, every fact goes to the manual report.
    let scip_path = scip_ingest::default_scip_path(root);
    let func_index: FunctionIndex = if scip_path.exists() {
        match scip_ingest::parse_scip(&scip_path, root) {
            Ok(facts) => FunctionIndex::build(&facts),
            Err(e) => {
                eprintln!(
                    "doc-linter: migrate-endpoint-markers: SCIP load failed ({e:#}); \
                     all endpoints will go to the manual report"
                );
                FunctionIndex::default()
            }
        }
    } else {
        eprintln!(
            "doc-linter: migrate-endpoint-markers: no SCIP file at {} — \
             all endpoints will go to the manual report (run `doc-linter scip-index` first)",
            scip_path.display()
        );
        FunctionIndex::default()
    };

    // 4. Plan proposals + collect manual entries.
    let mut proposals: Vec<MarkerProposal> = Vec::new();
    let mut manual: Vec<ManualEntry> = Vec::new();

    for fact in &regex_facts {
        match plan_marker(fact, &func_index) {
            PlanOutcome::Proposal(p) => proposals.push(p),
            PlanOutcome::Manual(m) => manual.push(m),
        }
    }

    let _ = include_typescript; // reserved — TS handler resolution lands in a follow-up.

    // 5. Apply or preview.
    let mut stamped = 0usize;
    if dry_run {
        for p in &proposals {
            println!(
                "would stamp {}:{} above {}:{} (handler {})",
                p.method, p.path, p.file, p.handler_line, p.handler_name
            );
        }
    } else {
        stamped = apply_proposals(root, &proposals)?;
    }

    // 6. Always write the manual report. Even on dry-run we want
    //    the operator to see the unresolved set so the next run
    //    addresses it.
    let manual_path = manual_report_path();
    if manual.is_empty() {
        // Clean up a stale manual report so re-runs after a fix
        // don't leave the operator chasing yesterday's gaps.
        let _ = std::fs::remove_file(&manual_path);
    } else {
        write_manual_report(&manual_path, &manual)?;
    }

    eprintln!(
        "migrate-endpoint-markers: {} {} handlers, {} manual (see {})",
        if dry_run { "would stamp" } else { "stamped" },
        if dry_run { proposals.len() } else { stamped },
        manual.len(),
        manual_path.display()
    );

    Ok(ExitCode::SUCCESS)
}

/// Outcome of planning a single [[entity-doc-graph]] @endpoint marker — we
/// either have everything needed to stamp (a `Proposal`) or we don't (a
/// `Manual` entry with a reason + best-guess hint for hand-stamping).
enum PlanOutcome {
    Proposal(MarkerProposal),
    Manual(ManualEntry),
}

/// Decide whether `fact` can produce a stamp-ready proposal. Falls
/// back to a `Manual` entry when:
///   - `handler_local` is `None` (axum closure, MCP tool from a
///     macro the tree-sitter walker couldn't reach into).
///   - The handler's local name doesn't resolve to any SCIP symbol
///     (clap dispatcher whose function follows a non-`cmd_*`
///     convention; macro-generated handler).
///   - The disambiguator can't pick a single SCIP entry from
///     multiple matches (rare; happens when two crates define
///     same-named handlers).
fn plan_marker(fact: &EndpointFact, index: &FunctionIndex) -> PlanOutcome {
    let marker_method = match fact.kind {
        EndpointKind::Clap => "CLI".to_string(),
        EndpointKind::Mcp => "MCP".to_string(),
        EndpointKind::Axum
        | EndpointKind::Fastapi
        | EndpointKind::Flask
        | EndpointKind::Express
        | EndpointKind::Jaxrs => fact.method.to_ascii_uppercase(),
    };

    let Some(local) = fact.handler_local.as_deref() else {
        return PlanOutcome::Manual(ManualEntry {
            kind: fact.kind,
            method: marker_method,
            path: fact.path.clone(),
            reason: "handler symbol unknown (closure / macro-opaque)".to_string(),
            hint: Some(format!(
                "registration at {}:{}",
                fact.source_file, fact.source_line
            )),
        });
    };

    let Some(candidates) = index.by_local_name.get(local) else {
        return PlanOutcome::Manual(ManualEntry {
            kind: fact.kind,
            method: marker_method,
            path: fact.path.clone(),
            reason: format!("SCIP symbol for handler `{local}` not found"),
            hint: Some(format!(
                "registration at {}:{}; handler probably lives near the registration site",
                fact.source_file, fact.source_line
            )),
        });
    };
    if candidates.is_empty() {
        return PlanOutcome::Manual(ManualEntry {
            kind: fact.kind,
            method: marker_method,
            path: fact.path.clone(),
            reason: format!("SCIP symbol for handler `{local}` not found"),
            hint: Some(format!(
                "registration at {}:{}",
                fact.source_file, fact.source_line
            )),
        });
    }

    // Resolve to a single entry: same-file colocation wins, then
    // same-crate fallback. Mirrors `store::resolve_handler_symbol`.
    let chosen = if candidates.len() == 1 {
        Some(&candidates[0])
    } else {
        let same_file: Vec<_> = candidates
            .iter()
            .filter(|e| e.file == fact.source_file)
            .collect();
        if same_file.len() == 1 {
            Some(same_file[0])
        } else {
            let crate_name = source_file_crate(&fact.source_file);
            if let Some(cn) = crate_name.as_deref() {
                let same_crate: Vec<_> = candidates.iter().filter(|e| e.crate_name == cn).collect();
                if same_crate.len() == 1 {
                    Some(same_crate[0])
                } else {
                    None
                }
            } else {
                None
            }
        }
    };

    let Some(entry) = chosen else {
        return PlanOutcome::Manual(ManualEntry {
            kind: fact.kind,
            method: marker_method,
            path: fact.path.clone(),
            reason: format!(
                "SCIP symbol for handler `{local}` is ambiguous ({} candidates)",
                candidates.len()
            ),
            hint: Some(format!(
                "registration at {}:{}",
                fact.source_file, fact.source_line
            )),
        });
    };

    if entry.line == 0 || entry.file.is_empty() {
        return PlanOutcome::Manual(ManualEntry {
            kind: fact.kind,
            method: marker_method,
            path: fact.path.clone(),
            reason: format!("SCIP entry for `{local}` lacks a definition line"),
            hint: Some(format!("symbol = {}", entry.symbol)),
        });
    }

    PlanOutcome::Proposal(MarkerProposal {
        file: entry.file.clone(),
        handler_line: entry.line,
        method: marker_method,
        path: fact.path.clone(),
        handler_name: local.to_string(),
    })
}

/// Apply every [[entity-doc-graph]] @endpoint proposal in place. Groups
/// proposals by file, sorts by line descending, and edits each file once.
/// Returns the number of proposals successfully stamped.
///
/// Idempotent: a proposal whose marker already exists in the target file's
/// doc-comment is silently skipped (no-op re-runs).
fn apply_proposals(root: &Path, proposals: &[MarkerProposal]) -> Result<usize> {
    let mut by_file: BTreeMap<String, Vec<&MarkerProposal>> = BTreeMap::new();
    for p in proposals {
        by_file.entry(p.file.clone()).or_default().push(p);
    }

    let mut stamped = 0usize;
    for (rel_file, mut ps) in by_file {
        let abs = root.join(&rel_file);
        if !abs.exists() {
            eprintln!("migrate-endpoint-markers: skip {rel_file} — resolved path does not exist");
            continue;
        }
        // Bottom-up so prior insertions don't shift later line numbers.
        ps.sort_by(|a, b| b.handler_line.cmp(&a.handler_line));

        let original =
            std::fs::read_to_string(&abs).with_context(|| format!("read {}", abs.display()))?;
        let mut lines: Vec<String> = original
            .split('\n')
            .map(std::string::ToString::to_string)
            .collect();
        let mut local_stamped = 0usize;

        for p in ps {
            // Locate the `fn <handler_name>` (or `struct <handler_name>`)
            // line within ±3 of the SCIP-recorded line.
            let Some(idx) = locate_handler_line(&lines, &p.handler_name, p.handler_line) else {
                eprintln!(
                    "migrate-endpoint-markers: could not locate `{}` in {} near line {}",
                    p.handler_name, rel_file, p.handler_line
                );
                continue;
            };

            // Walk upward across attribute lines so the marker block
            // sits ABOVE the attributes (rustdoc convention).
            let mut anchor = idx;
            while anchor > 0 {
                let prev = lines[anchor - 1].trim_start();
                if prev.starts_with("#[") || prev.starts_with("#![") {
                    anchor -= 1;
                } else {
                    break;
                }
            }
            // Indent the marker line to match the handler's leading
            // whitespace.
            let indent: String = lines[idx]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let marker_line = format!("{indent}/// @endpoint {} {}", p.method, p.path);

            // Find an existing `///` doc-comment block immediately
            // above `anchor`. If one exists, append our marker at its
            // end (after the last `///` line). If it doesn't, create
            // a minimal block: a single `///` line.
            let block_end = anchor; // exclusive: range of doc lines is [block_start, block_end)
            let mut block_start = block_end;
            while block_start > 0 {
                let trimmed = lines[block_start - 1].trim_start();
                if trimmed.starts_with("///") && !trimmed.starts_with("////") {
                    block_start -= 1;
                } else {
                    break;
                }
            }
            // Idempotency — skip when the marker line already exists
            // anywhere in the existing block.
            let already = (block_start..block_end).any(|i| {
                let t = lines[i].trim_start();
                t == format!("/// @endpoint {} {}", p.method, p.path)
            });
            if already {
                continue;
            }

            if block_start < block_end {
                // Insert the new marker line at block_end (after the
                // last `///` line of the existing block).
                lines.insert(block_end, marker_line);
            } else {
                // No existing block — insert a single `///` line above
                // the anchor.
                lines.insert(anchor, marker_line);
            }
            local_stamped += 1;
        }

        if local_stamped > 0 {
            let new_contents = lines.join("\n");
            std::fs::write(&abs, new_contents)
                .with_context(|| format!("write {}", abs.display()))?;
            stamped += local_stamped;
        }
    }
    Ok(stamped)
}

/// Locate the line index (0-based) of `fn <name>` or `struct <name>` for
/// the [[entity-doc-graph]] @endpoint stamper, within ±3 lines of
/// `scip_line`. Falls back to a whole-file scan if the local window
/// doesn't match.
fn locate_handler_line(lines: &[String], handler_name: &str, scip_line: u32) -> Option<usize> {
    if scip_line == 0 {
        return None;
    }
    let target = scip_line.saturating_sub(1) as usize;
    let needles = [
        format!("fn {handler_name}"),
        format!("struct {handler_name}"),
    ];
    let lo = target.saturating_sub(3);
    let hi = (target + 4).min(lines.len());
    for (i, line) in lines.iter().enumerate().take(hi).skip(lo) {
        if needles.iter().any(|n| line.contains(n.as_str())) {
            return Some(i);
        }
    }
    for (i, line) in lines.iter().enumerate() {
        if needles.iter().any(|n| line.contains(n.as_str())) {
            return Some(i);
        }
    }
    None
}

/// Write the unresolved-endpoints report for the [[entity-doc-graph]]
/// @endpoint migrator. Format matches the brief:
///   `<kind>:<method>:<path>  reason: <why>`
///   `  hint: <best-guess>`
fn write_manual_report(path: &Path, entries: &[ManualEntry]) -> Result<()> {
    use std::fmt::Write as _;
    let mut out = String::new();
    out.push_str(
        "# doc-linter migrate-endpoint-markers manual report\n\
         #\n\
         # One entry per endpoint the migration tool could not stamp\n\
         # automatically. Author the marker by hand: locate the handler\n\
         # in source and add `/// @endpoint <METHOD> <PATH>` to its\n\
         # doc-comment.\n\n",
    );
    for e in entries {
        let _ = writeln!(
            out,
            "{}:{}:{}  reason: {}",
            e.kind, e.method, e.path, e.reason
        );
        if let Some(h) = &e.hint {
            let _ = writeln!(out, "  hint: {h}");
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, out).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Default location for the [[entity-doc-graph]] @endpoint manual report.
/// `/tmp` matches the brief; the path is overridable in tests via the
/// `DOC_LINTER_ENDPOINT_MARKER_MANUAL_PATH` env var so concurrent test
/// runs don't clobber each other.
fn manual_report_path() -> PathBuf {
    if let Ok(p) = std::env::var("DOC_LINTER_ENDPOINT_MARKER_MANUAL_PATH") {
        return PathBuf::from(p);
    }
    PathBuf::from("/tmp/endpoint-marker-manual.txt")
}

/// True when `git status --porcelain` is empty or `root` is not
/// inside a git repo. Mirrors `scaffold::is_working_tree_clean`.
fn is_working_tree_clean(root: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .output();
    let Ok(output) = output else {
        return Ok(true);
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(true);
        }
        return Ok(true);
    }
    Ok(output.stdout.is_empty())
}

/// Recover the crate name from a `crates/<name>/...` path; mirror
/// of `store::source_file_crate` so the migrator's
/// disambiguator behaves the same as the live ingest's.
fn source_file_crate(rel_path: &str) -> Option<String> {
    let p = rel_path.replace('\\', "/");
    let mut parts = p.split('/');
    let first = parts.next()?;
    if first != "crates" {
        return None;
    }
    let name = parts.next()?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}
