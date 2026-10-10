//! Shared CLI utilities for every `cmd/*` handler in the
//! [[entity-doc-graph]] linter — output-format enum, corpus discovery
//! walkers, and the human / JSON renderers.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;
use walkdir::WalkDir;

use doc_linter::config::LintConfig;
use doc_linter::ontology::Ontology;
use doc_linter::validator::Issue;

/// Selects the rendering style — human-readable text or machine-readable
/// JSON — for [[entity-doc-graph]] check and query output.
#[derive(Clone, clap::ValueEnum)]
pub(crate) enum OutputFormat {
    Human,
    Json,
}

/// Walks `<root>` for the `.md` / `.adoc` files `include` selects, honouring the corpus skip-dirs list
/// from `LintConfig::should_skip_dir`. The primary corpus walker that
/// drives every subcommand's `all_files` argument.
pub(crate) fn discover_markdown(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
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
        if config.is_doc_included(path, root) {
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

/// Every markdown file in the corpus: `root` plus each `cross_repo_roots`
/// entry (absolute, or relative to `root`). A cross root is walked with
/// its own `.doc-lint.toml` when it has one, so the primary's `skip_dirs`
/// (e.g. `docs`) can't prune a sibling repo's tree; otherwise with
/// `config`. Missing roots are skipped. Sorted and de-duplicated.
pub(crate) fn discover_corpus(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
    let mut all = discover_markdown(root, config)?;
    for cross in cross_repo_paths(root, config) {
        let own = cross.join(".doc-lint.toml");
        if own.exists() {
            all.extend(discover_markdown(&cross, &LintConfig::load(&own)?)?);
        } else {
            all.extend(discover_markdown(&cross, config)?);
        }
    }
    all.sort();
    all.dedup();
    Ok(all)
}

/// Canonical paths of the `cross_repo_roots` entries that exist.
pub(crate) fn cross_repo_paths(root: &Path, config: &LintConfig) -> Vec<PathBuf> {
    config
        .cross_repo_roots
        .iter()
        .filter_map(|rel| root.join(rel).canonicalize().ok())
        .collect()
}

/// Walks `<root>` for source files the code-comment lint reads (any
/// [`doc_linter::code_comments::Lang`]) that match the comment-lint
/// include / exclude globs. Skip-dirs apply as for markdown discovery.
pub(crate) fn discover_code(root: &Path, config: &LintConfig) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
    {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type().is_file()
            && doc_linter::code_comments::Lang::from_path(path).is_some()
            && config.matches_code_comment(path, root)
        {
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    Ok(out)
}

/// Human-readable lint render. Streams every issue (errors and
/// warnings) per file to stderr, then prints a closing count line.
/// When nothing is an error the "all clean" line — with the warning
/// count, if any — goes to stdout so CI grep-pipelines can rely on
/// `1 file(s) scanned, all clean` landing on the success-path stream.
pub(crate) fn render_human(
    root: &Path,
    report: &[(PathBuf, Vec<Issue>)],
    scanned: usize,
    error_count: usize,
    ontology: &Ontology,
) {
    let total: usize = report.iter().map(|(_, v)| v.len()).sum();
    for (path, issues) in report {
        let rel = path.strip_prefix(root).unwrap_or(path);
        for issue in issues {
            // Roadmap-48 Rule B: render_with_stub appends the
            // copy-paste promotion stub for vocab-violation /
            // unknown-entity diagnostics. Other variants pass
            // through unchanged.
            eprintln!("{}: {}", rel.display(), issue.render_with_stub(ontology));
        }
    }
    let summary = human_summary(scanned, total, error_count, report.len());
    if error_count == 0 {
        println!("{summary}");
    } else {
        eprintln!();
        eprintln!("{summary}");
    }
}

/// Closing line of [`render_human`]: `total` issues, `errors` of them
/// errors, across `files` files.
fn human_summary(scanned: usize, total: usize, errors: usize, files: usize) -> String {
    let warnings = total - errors;
    match (errors, warnings) {
        (0, 0) => format!("doc-linter: {scanned} file(s) scanned, all clean"),
        (0, w) => format!("doc-linter: {scanned} file(s) scanned, all clean ({w} warning(s))"),
        (e, w) => format!("{e} error(s), {w} warning(s) across {files} file(s)"),
    }
}

/// Machine-readable lint render. Emits one `Entry` per file with the
/// rendered messages, extended codes, and Roadmap-48 Rule B promotion
/// stubs (suppressed when every slot is `None` for backward compat).
pub(crate) fn render_json(
    root: &Path,
    report: &[(PathBuf, Vec<Issue>)],
    ontology: &Ontology,
) -> Result<()> {
    /// Serde shape for one file-level entry in the [[entity-doc-graph]]
    /// `check --format json` output — file path, issue messages,
    /// extended codes, and (Roadmap-48 Rule B) per-issue promotion
    /// stubs for unknown-noun / unknown-entity diagnostics. The
    /// stub list is parallel to `issues` / `codes`; non-promotable
    /// diagnostics produce a `null` slot.
    #[derive(Serialize)]
    struct Entry {
        file: String,
        issues: Vec<String>,
        codes: Vec<String>,
        #[serde(skip_serializing_if = "promote_stubs_are_empty")]
        promote_stubs: Vec<Option<String>>,
    }

    let entries: Vec<Entry> = report
        .iter()
        .map(|(path, issues)| {
            let rel = path.strip_prefix(root).unwrap_or(path);
            // Render the human message WITH the appended stub so
            // JSON consumers that just want a single string can
            // grab `issues[i]` and have everything inline.
            let issue_strings: Vec<String> = issues
                .iter()
                .map(|i| i.render_with_stub(ontology))
                .collect();
            let codes: Vec<String> = issues.iter().map(Issue::extended_code).collect();
            let promote_stubs: Vec<Option<String>> =
                issues.iter().map(|i| i.promote_stub(ontology)).collect();
            Entry {
                file: rel.display().to_string(),
                issues: issue_strings,
                // Use extended_code so Vale alerts surface as
                // `vale-doclinter-ambiguousbare` etc. in JSON consumers.
                codes,
                promote_stubs,
            }
        })
        .collect();

    let out = serde_json::json!({ "report": entries });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Suppress the per-entry `promote_stubs` field when every slot is
/// `None` (no vocab-violation / unknown-entity diagnostics in this
/// file's report). Keeps the JSON shape stable for consumers that
/// pre-date Roadmap-48 Rule B.
fn promote_stubs_are_empty(v: &[Option<String>]) -> bool {
    v.iter().all(std::option::Option::is_none)
}

/// Path of the running binary, usable for a fresh `exec`/spawn.
///
/// On Linux `current_exe()` reads `/proc/self/exe`, which turns into
/// `<path> (deleted)` once the file is replaced — and `cargo install`
/// always replaces it. Strip that suffix, then fall back to `argv[0]`
/// via `PATH`. Errors instead of returning a path that won't exec.
pub(crate) fn own_exe() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let argv0 = std::env::args().next();
    pick_exe(&current, argv0.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no runnable binary at {}", current.display()))
}

fn pick_exe(current: &Path, argv0: Option<&str>) -> Option<PathBuf> {
    let s = current.to_string_lossy();
    let stripped = PathBuf::from(s.strip_suffix(" (deleted)").unwrap_or(&s));
    std::iter::once(stripped)
        .chain(argv0.and_then(|a| which::which(a).ok()))
        .find(|p| p.is_file())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Handoff cpg-os-vault #2: after `cargo install` replaces the binary,
    /// `/proc/self/exe` reads `<path> (deleted)`; exec'ing that is ENOENT.
    #[test]
    fn pick_exe_strips_deleted_suffix() {
        let dir = std::env::temp_dir().join(format!("doc-linter-own-exe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("doc-linter");
        std::fs::write(&bin, "").unwrap();
        let deleted = PathBuf::from(format!("{} (deleted)", bin.display()));
        assert_eq!(pick_exe(&deleted, None), Some(bin));
        assert_eq!(pick_exe(&dir.join("gone (deleted)"), None), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// #265: a run with only warnings (vale-missing, …) is clean but
    /// says so with the count, and is never a bare "all clean".
    #[test]
    fn human_summary_counts_warnings_apart_from_errors() {
        assert_eq!(
            human_summary(3, 0, 0, 0),
            "doc-linter: 3 file(s) scanned, all clean"
        );
        assert_eq!(
            human_summary(3, 2, 0, 1),
            "doc-linter: 3 file(s) scanned, all clean (2 warning(s))"
        );
        assert_eq!(
            human_summary(3, 3, 1, 2),
            "1 error(s), 2 warning(s) across 2 file(s)"
        );
    }
}
