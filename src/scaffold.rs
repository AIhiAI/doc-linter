//! Phase 4 of roadmap-43: `doc-linter scaffold-coverage <CRATE>`.
//!
//! Walks every dark Function node in the target crate and emits
//! proposed doc-comment templates (in the file's language) with the entity auto-inferred
//! from the function name. Three output modes:
//!
//! - default (dry-run): human-readable preview blocks per file
//! - `--json`: one JSON object per line, machine-readable
//! - `--write`: applies the edits in place, refusing to run when the
//!   working tree is dirty
//!
//! See the Phase 4 section of `docs/roadmap/43-code-doc-coverage-gap.md`
//! for the design rationale.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::code_comments::Lang;
use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::ontology::Ontology;
use crate::store::{self, DarkFunctionRow};

/// One scaffolding proposal — a dark Function node + the entity (or
/// the closest 3 candidates if no symbol token resolved).
#[derive(Debug, Clone, Serialize)]
pub struct Proposal {
    /// Repo-relative source file path (the `f.file` column).
    pub file: String,
    /// 1-based source line of the `fn` keyword (or the line just
    /// before, depending on what the SCIP indexer emitted).
    pub line: u32,
    /// Bare function name extracted from the SCIP symbol — what shows
    /// up after the last `/` and before the `()`.
    #[serde(rename = "fn")]
    pub fn_name: String,
    /// The full SCIP symbol, kept so JSON consumers (CI bots, editor
    /// extensions) can echo a stable identifier into a comment thread.
    pub symbol: String,
    /// The matched entity's bare id (e.g. `outlet`). Empty when no
    /// symbol token resolved — see `closest_candidates` for the
    /// fallback list.
    pub entity: String,
    /// `symbol-token:<token>` when matched, `none` when unmapped.
    pub matched_via: String,
    /// Up to three fallback entity ids, ranked by mention frequency
    /// in the same crate. Empty when an entity match was found.
    pub closest_candidates: Vec<String>,
    /// The pre-formatted `///` block the scaffolder proposes. Joined
    /// by `\n`; trailing newline is the caller's responsibility.
    pub proposed: String,
    /// Phase 5c of roadmap-43: true when the function name matched a
    /// `coverage_function_exempt` regex. Exempt rows are not
    /// scaffolded; they appear at the END of `--json` output with
    /// `"reason": "exempt"` so the agent driving the backfill knows
    /// why they were skipped.
    #[serde(default)]
    pub exempt: bool,
    /// Phase 5c of roadmap-43: when `exempt` is true, the reason
    /// (`"exempt"`); otherwise empty. Kept as an explicit field so
    /// future skip categories (e.g. `"codegen"`) slot in cleanly.
    #[serde(skip_serializing_if = "String::is_empty", default)]
    pub reason: String,
}

/// Build every proposal for the named crate without applying any
/// edits. Pure data — the CLI mode handlers (`render_human`,
/// `render_json`, `apply_writes`) drive the side effects.
///
/// Phase 5c of roadmap-43: rows whose bare function name matches any
/// `coverage_function_exempt` regex are emitted with `exempt: true`
/// and `reason: "exempt"`, sorted to the END of the proposal list so
/// the JSON consumer reads the actionable proposals first. The
/// `--write` path skips exempt rows; the human renderer surfaces
/// them as a final "Exempted: N functions" section.
/// Walk every dark Function in `crate_name` from the [[entity-doc-graph]]
/// code-graph and lift each into a Proposal — auto-inferred entity if the
/// SCIP symbol-token bridge resolves, fallback "TODO + 3 candidates" stub
/// otherwise. The list is sorted actionable-first then exempt-last so the
/// JSON consumer reads the actionable proposals before the trailing
/// already-exempt rows.
pub fn build_proposals(
    db: &dyn crate::graph_read::GraphRead,
    ontology: &Ontology,
    crate_name: &str,
    config: &LintConfig,
) -> Result<Vec<Proposal>> {
    let dark = db.list_dark_functions_in_crate(crate_name)?;
    let term_index = TermIndex::build(ontology);

    // Pre-compute the per-crate top entities once, used as the
    // fallback list for unmapped functions. Three is enough: more
    // than that and the author isn't really getting "the closest"
    // — they're getting a stack-ranked tour of the crate's domain
    // surface.
    let fallback = db.top_entities_in_crate(crate_name, 3).unwrap_or_default();

    let mut actionable: Vec<Proposal> = Vec::with_capacity(dark.len());
    let mut exempt: Vec<Proposal> = Vec::new();
    for row in &dark {
        let mut p = propose_for(row, &term_index, ontology, &fallback);
        if config.is_function_exempt(&p.fn_name) {
            p.exempt = true;
            p.reason = "exempt".to_string();
            exempt.push(p);
        } else {
            actionable.push(p);
        }
    }
    actionable.extend(exempt);
    Ok(actionable)
}

/// Lift a single [[entity-doc-graph]] dark-function row into a scaffold
/// Proposal. Visible for unit testing — `build_proposals` is the only
/// production caller.
fn propose_for(
    row: &DarkFunctionRow,
    term_index: &TermIndex<'_>,
    ontology: &Ontology,
    fallback: &[String],
) -> Proposal {
    let fn_name = bare_fn_name(&row.symbol).unwrap_or_else(|| row.symbol.clone());

    // Walk the SCIP-derived symbol tokens. First entity match wins —
    // deterministic order is enforced by `symbol_tokens` (insertion-
    // order on the unique tokens list) and a stable sort on the
    // returned entity ids per token.
    let tokens = store::symbol_tokens(&row.symbol);
    let mut best: Option<(String, String)> = None;
    for tok in &tokens {
        let mut entities: Vec<&str> = term_index
            .lookup(tok)
            .iter()
            .map(|e| e.id.as_str())
            .collect();
        if entities.is_empty() {
            continue;
        }
        entities.sort_unstable();
        // First entity (alphabetical by id when ambiguous).
        best = Some((entities[0].to_string(), tok.clone()));
        break;
    }

    if let Some((entity_id, token)) = best {
        let display = ontology
            .entities
            .get(&entity_id)
            .map_or_else(|| entity_id.clone(), |e| e.display.clone());
        let proposed = render_comment(
            &row.file,
            &[
                format!("TODO(roadmap-43-phase4): summarize what `{fn_name}` does for {display}."),
                String::new(),
                format!("See [`entity-{entity_id}`] for the canonical definition."),
            ],
            None,
        );
        Proposal {
            file: row.file.clone(),
            line: row.line,
            fn_name,
            symbol: row.symbol.clone(),
            entity: entity_id,
            matched_via: format!("symbol-token:{token}"),
            closest_candidates: Vec::new(),
            proposed,
            exempt: false,
            reason: String::new(),
        }
    } else {
        // No symbol-token entity match. Fall back to the three most
        // frequently-mentioned entities in the crate. If the crate
        // itself has zero mentions (corner case — no other function
        // in the crate has any FUNCTION_MENTIONS edge yet), drop down
        // to "no candidates".
        let candidates: Vec<String> = fallback.iter().take(3).cloned().collect();
        let mut body = vec![
            format!("TODO(roadmap-43-phase4): no obvious entity for `{fn_name}`."),
            String::new(),
        ];
        if !candidates.is_empty() {
            let bullets = candidates
                .iter()
                .map(|c| format!("[`entity-{c}`]"))
                .collect::<Vec<_>>()
                .join(", ");
            body.push(format!("Pick one of: {bullets}."));
        }
        let proposed = render_comment(
            &row.file,
            &body,
            Some("TODO: this function has no obvious entity match — pick one from the ontology"),
        );
        Proposal {
            file: row.file.clone(),
            line: row.line,
            fn_name,
            symbol: row.symbol.clone(),
            entity: String::new(),
            matched_via: "none".to_string(),
            closest_candidates: candidates,
            proposed,
            exempt: false,
            reason: String::new(),
        }
    }
}

/// Wrap `body` in the doc-comment syntax of `file`'s language, plus an
/// optional trailing plain line comment (`note`) that stays out of the
/// doc text: `///` for Rust / C# / Dart (and unknown extensions),
/// `/** */` for Java / TypeScript / JavaScript / Vue, a `"""` docstring
/// for Python.
fn render_comment(file: &str, body: &[String], note: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let line_prefix = match Lang::from_path(Path::new(file)) {
        Some(Lang::Java | Lang::TypeScript | Lang::Tsx | Lang::Vue) => {
            out.push("/**".to_string());
            out.extend(
                body.iter()
                    .map(|l| format!(" * {l}").trim_end().to_string()),
            );
            out.push(" */".to_string());
            "//"
        }
        Some(Lang::Python) => {
            out.push("\"\"\"".to_string());
            out.extend(body.iter().cloned());
            out.push("\"\"\"".to_string());
            "#"
        }
        Some(Lang::Rust | Lang::CSharp | Lang::Dart) | None => {
            out.extend(
                body.iter()
                    .map(|l| format!("/// {l}").trim_end().to_string()),
            );
            "//"
        }
    };
    if let Some(note) = note {
        out.push(format!("{line_prefix} {note}"));
    }
    out.join("\n")
}

/// Pull the bare function name out of a SCIP symbol descriptor for the
/// [[entity-doc-graph]] coverage exempt-pattern matcher.
///
/// SCIP symbols come in shapes like:
///   - `<scheme> <mgr> <crate> <ver> path/create_outlet().`     — fn
///   - `<scheme> <mgr> <crate> <ver> path/impl#[Foo]bar().`     — method
///   - `<scheme> <mgr> <crate> <ver> path/impl#[Foo][T]bar().`  — trait-impl
///   - `<scheme> <mgr> <crate> <ver> path/Foo#`                 — struct
///   - `<scheme> <mgr> <crate> <ver> path/tests/`               — module
///   - `<scheme> <mgr> <crate> <ver> bare_name().`              — top-level fn
///
/// We strip the four-token package prefix first (so a top-level fn
/// without a `path/` segment still gives `bare_name`), then walk
/// from the right of the descriptor.
///
/// Returns the bare identifier (`bar` for the impl-method case, etc.).
/// For module / struct shapes that have no method-tail at all, returns
/// the bracketed type name as a readable label.
pub fn bare_fn_name(symbol: &str) -> Option<String> {
    let descriptor = crate::store::strip_scip_package_prefix(symbol).unwrap_or(symbol);
    // Walk from the right, skipping empty trailing segments (a symbol
    // that ends in `/` produces an empty rsplit head).
    let last_nonempty = descriptor.rsplit('/').find(|seg| !seg.is_empty())?;
    let trimmed = last_nonempty
        .trim_end_matches('.')
        .trim_end_matches(')')
        .trim_end_matches('(')
        .trim_end_matches('#');
    if trimmed.is_empty() {
        return None;
    }
    // impl-marker segments: take the identifier after the final `]`.
    // `impl#[Foo]bar` → `bar`; `impl#[Foo][Trait]baz` → `baz`.
    if let Some(after) = trimmed.rsplit(']').next() {
        if !after.is_empty() && after != trimmed {
            return Some(after.to_string());
        }
    }
    // No impl marker tail. If the segment still contains `[`, it's a
    // type-definition / impl-without-method shape — surface the
    // bracketed type name as a readable label.
    if trimmed.contains('[') {
        if let Some(start) = trimmed.find('[') {
            let after = &trimmed[start + 1..];
            if let Some(end) = after.find(']') {
                return Some(after[..end].to_string());
            }
        }
        return None;
    }
    // Roadmap-51 Move 4: SCIP also emits a `Type#method` symbol shape
    // for inherent-impl methods AND for trait-method calls on a named
    // type (no `impl#[...]` wrapper). Example:
    // `ScopeRowSink#on_row()` → bare `on_row`. Take the segment after
    // the last `#`. (The trailing `#` is stripped above so a struct
    // marker like `Foo#` lands here as `Foo` — handled by the empty-
    // tail check.)
    if let Some(idx) = trimmed.rfind('#') {
        let after = &trimmed[idx + 1..];
        if !after.is_empty() {
            return Some(after.to_string());
        }
    }
    Some(trimmed.to_string())
}

/// Render [[entity-doc-graph]] scaffold proposals in the default (dry-run)
/// human-readable mode, grouped by file. Returns the formatted block as a
/// String for ease of testing; callers print it.
///
/// Phase 5c of roadmap-43: exempt rows (those whose name matched a
/// `coverage_function_exempt` regex) are partitioned out of the per-
/// file blocks and listed in a single "Exempted: N functions" tail
/// section so the agent driving the backfill knows they were skipped
/// by config (not missed).
pub fn render_human(proposals: &[Proposal]) -> String {
    if proposals.is_empty() {
        return "doc-linter scaffold-coverage: no dark functions in the target crate.\n"
            .to_string();
    }
    let actionable: Vec<&Proposal> = proposals.iter().filter(|p| !p.exempt).collect();
    let exempt: Vec<&Proposal> = proposals.iter().filter(|p| p.exempt).collect();

    let mut s = String::new();

    if actionable.is_empty() && !exempt.is_empty() {
        s.push_str("doc-linter scaffold-coverage: no actionable dark functions; ");
        s.push_str(&format!("{} exempted by config.\n", exempt.len()));
    } else {
        let mut by_file: BTreeMap<&str, Vec<&Proposal>> = BTreeMap::new();
        for p in &actionable {
            by_file.entry(p.file.as_str()).or_default().push(p);
        }
        for (file, ps) in &by_file {
            s.push_str(&format!("== {file} ==\n"));
            for p in ps {
                s.push_str(&format!("line {}: fn {}\n", p.line, p.fn_name));
                if !p.entity.is_empty() {
                    s.push_str(&format!(
                        "  entity-{} (matched via {})\n",
                        p.entity, p.matched_via
                    ));
                } else if !p.closest_candidates.is_empty() {
                    s.push_str(&format!(
                        "  no entity match — closest: {}\n",
                        p.closest_candidates.join(", ")
                    ));
                } else {
                    s.push_str("  no entity match — no candidates available\n");
                }
                s.push_str("  proposed:\n");
                for line in p.proposed.lines() {
                    s.push_str("  ");
                    s.push_str(line);
                    s.push('\n');
                }
                s.push('\n');
            }
        }
    }

    if !exempt.is_empty() {
        s.push_str(&format!(
            "Exempted: {} function(s) — see `coverage_function_exempt` in .doc-lint.toml\n",
            exempt.len()
        ));
        for p in &exempt {
            s.push_str(&format!("  {} ({}:{})\n", p.fn_name, p.file, p.line));
        }
    }
    s
}

/// Render [[entity-doc-graph]] scaffold proposals as JSON-Lines (one
/// object per line). Each object matches the documented contract in the
/// Phase 4 brief.
///
/// Phase 5c of roadmap-43: emits a header line first
/// (`{"exempt_function_patterns": [...]}` — copied verbatim from the
/// active config) so JSON consumers can introspect why a row is
/// flagged exempt without re-reading the config. Filtered-out exempt
/// rows already arrive at the END of `proposals` (sorted by
/// `build_proposals`) with `exempt: true` + `reason: "exempt"`.
pub fn render_json(proposals: &[Proposal], config: &LintConfig) -> Result<String> {
    let mut s = String::new();
    let header = serde_json::json!({
        "exempt_function_patterns": config.coverage.coverage_function_exempt,
    });
    s.push_str(&serde_json::to_string(&header).context("serialize exempt header")?);
    s.push('\n');
    for p in proposals {
        let body = serde_json::to_string(p).context("serialize proposal")?;
        s.push_str(&body);
        s.push('\n');
    }
    Ok(s)
}

/// Apply [[entity-doc-graph]] scaffold proposals in place. Refuses to run
/// when `git status --porcelain` shows uncommitted changes (a real safety
/// measure — `--write` modifies source code wholesale). Returns the
/// (files-edited, blocks-inserted) tuple on success.
///
/// Error path: returns Ok(ExitCode::from(2)) for the dirty-tree guard; that
/// lets the CLI produce the documented exit code without translating an
/// `anyhow::Error`.
pub fn apply_writes(root: &Path, proposals: &[Proposal]) -> Result<(usize, usize, ExitCode)> {
    if !is_working_tree_clean(root)? {
        eprintln!(
            "doc-linter: scaffold-coverage --write requires a clean working tree; \
             commit or stash first"
        );
        return Ok((0, 0, ExitCode::from(2)));
    }

    // Phase 5c of roadmap-43: never write doc-comments above functions
    // the user marked exempt by name regex. They appeared in the
    // proposal list to drive the JSON header / human tail section, but
    // the writer must not touch them.
    let mut by_file: BTreeMap<PathBuf, Vec<&Proposal>> = BTreeMap::new();
    for p in proposals {
        if p.exempt {
            continue;
        }
        let abs = root.join(&p.file);
        by_file.entry(abs).or_default().push(p);
    }

    let mut files_edited = 0usize;
    let mut blocks_inserted = 0usize;
    for (file, mut ps) in by_file {
        if !file.exists() {
            // SCIP can record a relative path that doesn't resolve against
            // root (e.g. workspace member paths). Skip silently — no edit
            // is better than an erroneous edit.
            continue;
        }
        // Sort proposals by line DESCENDING so we insert from the
        // bottom up; that keeps every prior line index stable while
        // we're editing.
        ps.sort_by(|a, b| b.line.cmp(&a.line));

        let original =
            std::fs::read_to_string(&file).with_context(|| format!("read {}", file.display()))?;
        let mut lines: Vec<String> = original
            .split('\n')
            .map(std::string::ToString::to_string)
            .collect();
        let lang = Lang::from_path(&file);
        let mut inserted_here = 0usize;
        for p in ps {
            let Some(idx) = locate_fn_line(&lines, &p.fn_name, p.line, lang) else {
                continue;
            };
            // Compute leading whitespace from the fn line so the
            // doc-comment block is indented the same way.
            let mut indent: String = lines[idx]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let insert_at = if lang == Some(Lang::Python) {
                // A docstring is the first statement of the body, so it
                // goes below a one-line `def ...:` signature. Multi-line
                // signatures are skipped rather than guessed at.
                if !lines[idx].trim_end().ends_with(':') {
                    continue;
                }
                indent.push_str("    ");
                idx + 1
            } else {
                // Walk upwards across attribute / annotation lines
                // (`#[derive(...)]`, `@Override`, C# `[Fact]`). We want
                // the doc comment to land ABOVE them, not between them
                // and the fn.
                let mut at = idx;
                while at > 0 {
                    let prev = lines[at - 1].trim_start();
                    let is_attr = prev.starts_with("#[")
                        || prev.starts_with("#![")
                        || (lang != Some(Lang::Rust) && prev.starts_with('@'))
                        || (lang == Some(Lang::CSharp) && prev.starts_with('['));
                    if !is_attr {
                        break;
                    }
                    at -= 1;
                }
                at
            };
            // Build the indented block bottom-up so reverse insertion
            // matches the natural top-down read order.
            let block_lines: Vec<String> = p
                .proposed
                .lines()
                .map(|l| {
                    if l.is_empty() {
                        String::new()
                    } else {
                        format!("{indent}{l}")
                    }
                })
                .collect();
            for (offset, bl) in block_lines.into_iter().enumerate() {
                lines.insert(insert_at + offset, bl);
            }
            inserted_here += 1;
        }
        if inserted_here > 0 {
            let new_contents = lines.join("\n");
            std::fs::write(&file, new_contents)
                .with_context(|| format!("write {}", file.display()))?;
            files_edited += 1;
            blocks_inserted += inserted_here;
        }
    }

    Ok((files_edited, blocks_inserted, ExitCode::SUCCESS))
}

/// Locate the line index (0-based) of the declaration of `fn_name`
/// within ±3 lines of the SCIP line recorded in the [[entity-doc-graph]]
/// code-graph. SCIP's `f.line` is 1-based and points at the declaration
/// *or* a line nearby (rust-analyzer's range start can be the attribute
/// line for some shapes), so we search a small window. Returns the first
/// match.
fn locate_fn_line(
    lines: &[String],
    fn_name: &str,
    scip_line: u32,
    lang: Option<Lang>,
) -> Option<usize> {
    if scip_line == 0 {
        return None;
    }
    let target = scip_line.saturating_sub(1) as usize;
    // The declaration keyword: `fn <name>` (also matches `pub fn`,
    // `async fn`), `def <name>(`. Languages without one (Java, TS, C#,
    // Dart) use `<name>(` inside the window only — the whole-file
    // fallback could land on a call site.
    let (needle, whole_file) = match lang {
        Some(Lang::Rust) | None => (format!("fn {fn_name}"), true),
        Some(Lang::Python) => (format!("def {fn_name}("), true),
        Some(_) => (format!("{fn_name}("), false),
    };
    let lo = target.saturating_sub(3);
    let hi = (target + 4).min(lines.len());
    for (i, line) in lines.iter().enumerate().take(hi).skip(lo) {
        if line.contains(&needle) {
            return Some(i);
        }
    }
    if !whole_file {
        return None;
    }
    // Fallback: scan the whole file for the first match. This makes
    // the writer resilient to SCIP-line drift in fixture-style inputs
    // where the indexer wasn't run against the same source revision.
    for (i, line) in lines.iter().enumerate() {
        if line.contains(&needle) {
            return Some(i);
        }
    }
    None
}

/// True when `git status --porcelain` returns no output (working tree
/// clean) or when `root` is not inside a git repo at all (treat
/// non-git checkouts as "clean enough" — the safety check is meant
/// for FA's actual workspace, not test fixtures).
fn is_working_tree_clean(root: &Path) -> Result<bool> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("status")
        .arg("--porcelain")
        .output();
    let Ok(output) = output else {
        // git not on PATH — be permissive.
        return Ok(true);
    };
    if !output.status.success() {
        // Not a git repo (status exits non-zero with `fatal: not a git
        // repository`). Be permissive — the user is on a non-git
        // workspace and presumably knows what they're doing.
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not a git repository") {
            return Ok(true);
        }
        return Ok(true);
    }
    Ok(output.stdout.is_empty())
}

/// CLI entry point — wires the scaffolder up against the graph that
/// `doc-linter check` built (a missing graph is an error that says so).
///
/// @endpoint CLI scaffold-coverage
/// Operates on [[entity-coverage]] and [[entity-doc-graph]].
pub fn run(
    root: &Path,
    config: &LintConfig,
    all_files: &[PathBuf],
    crate_name: &str,
    write: bool,
    json: bool,
) -> Result<ExitCode> {
    use crate::parser::parse_doc;
    use std::collections::HashMap;

    let db = crate::graph_read::ReadGraph::open(root)?;

    // Rebuild the ontology directly from disk (no need to re-walk the
    // graph for entity displays — the in-process Ontology is the
    // authoritative source for `display` strings).
    let mut docs: HashMap<PathBuf, crate::parser::Doc> = HashMap::new();
    for path in all_files {
        if let Ok(d) = parse_doc(path) {
            docs.insert(path.clone(), d);
        }
    }
    let ontology = Ontology::load_from_docs(&docs);

    let proposals = build_proposals(&*db, &ontology, crate_name, config)?;

    if write {
        let (files, blocks, exit) = apply_writes(root, &proposals)?;
        if exit != ExitCode::SUCCESS {
            return Ok(exit);
        }
        eprintln!(
            "doc-linter scaffold-coverage --write: edited {files} file(s), \
             inserted {blocks} doc-comment block(s)"
        );
        return Ok(ExitCode::SUCCESS);
    }

    if json {
        let body = render_json(&proposals, config)?;
        print!("{body}");
    } else {
        let body = render_human(&proposals);
        print!("{body}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Helper-function tests for the [[entity-coverage]] scaffolder
/// (Wave 5a/5b dark-function close-out).
#[cfg(test)]
mod tests {
    use super::*;

    /// Asserts that `bare_fn_name` strips the trailing `()` SCIP
    /// signature so the [[entity-coverage]] scaffolder can match a
    /// function name against on-disk source.
    #[test]
    fn bare_fn_name_strips_trailing_parens() {
        assert_eq!(
            bare_fn_name("rust-analyzer cargo my-crate 0.1.0 src/lib.rs/foo()."),
            Some("foo".to_string()),
        );
    }

    /// Asserts that `bare_fn_name` peels the `impl#[Type]` marker off
    /// SCIP symbols so the [[entity-coverage]] scaffolder can match
    /// methods.
    #[test]
    fn bare_fn_name_handles_impl_markers() {
        // `impl#[Foo]bar().` — we want `bar`.
        let sym = "rust-analyzer cargo my-crate 0.1.0 src/lib.rs/impl#[Foo]bar().";
        assert_eq!(bare_fn_name(sym), Some("bar".to_string()));
    }

    /// Asserts that `locate_fn_line` finds the actual `fn foo` line
    /// within a small drift window around the SCIP-reported line — the
    /// resilience the [[entity-coverage]] scaffolder needs when the
    /// source has shifted since the last SCIP index.
    #[test]
    fn locate_fn_line_finds_within_window() {
        let lines: Vec<String> = vec!["// preamble", "", "pub fn foo() {", "    // body", "}"]
            .into_iter()
            .map(String::from)
            .collect();
        // SCIP line is 3 (1-based); target idx 2.
        assert_eq!(locate_fn_line(&lines, "foo", 3, Some(Lang::Rust)), Some(2));
        // Drift: SCIP says line 5 but fn is at line 3. Outside ±3
        // window (5 - 3 = 2 → still inside). Should still find via
        // window scan.
        assert_eq!(locate_fn_line(&lines, "foo", 5, Some(Lang::Rust)), Some(2));
    }

    /// The proposed block used to be Rust `///` whatever the file was;
    /// it now follows the file's language.
    #[test]
    fn render_comment_follows_file_language() {
        let body = ["Summary.".to_string(), String::new()];
        assert_eq!(
            render_comment("src/Loan.java", &body, Some("TODO: x")),
            "/**\n * Summary.\n *\n */\n// TODO: x"
        );
        assert_eq!(
            render_comment("web/a.ts", &body, None),
            "/**\n * Summary.\n *\n */"
        );
        assert_eq!(
            render_comment("src/lib.rs", &body, None),
            "/// Summary.\n///"
        );
        assert_eq!(render_comment("src/A.cs", &body, None), "/// Summary.\n///");
        assert_eq!(
            render_comment("lib/a.dart", &body, None),
            "/// Summary.\n///"
        );
        assert_eq!(
            render_comment("pkg/a.py", &body, Some("TODO: x")),
            "\"\"\"\nSummary.\n\n\"\"\"\n# TODO: x"
        );
    }

    fn proposal(file: &str, fn_name: &str, line: u32) -> Proposal {
        Proposal {
            file: file.to_string(),
            line,
            fn_name: fn_name.to_string(),
            symbol: String::new(),
            entity: String::new(),
            matched_via: "none".to_string(),
            closest_candidates: Vec::new(),
            proposed: render_comment(file, &["Doc.".to_string()], None),
            exempt: false,
            reason: String::new(),
        }
    }

    /// `--write` only knew `fn <name>`, so Java / Python files never got
    /// an edit; Java goes above the annotations, Python inside the def.
    #[test]
    fn apply_writes_handles_java_and_python() {
        let root = std::env::temp_dir().join(format!(
            "doc-linter-scaffold-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Loan.java"),
            "class Loan {\n    @Override\n    public void approve() {\n    }\n}\n",
        )
        .unwrap();
        std::fs::write(root.join("loan.py"), "def approve(x):\n    return x\n").unwrap();
        let (files, blocks, _) = apply_writes(
            &root,
            &[
                proposal("Loan.java", "approve", 3),
                proposal("loan.py", "approve", 1),
            ],
        )
        .unwrap();
        assert_eq!((files, blocks), (2, 2));
        assert_eq!(
            std::fs::read_to_string(root.join("Loan.java")).unwrap(),
            "class Loan {\n    /**\n     * Doc.\n     */\n    @Override\n    public void approve() {\n    }\n}\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("loan.py")).unwrap(),
            "def approve(x):\n    \"\"\"\n    Doc.\n    \"\"\"\n    return x\n"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
