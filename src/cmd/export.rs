//! `doc-linter export` subcommand. Filters the primary --root tree by
//! `visibility:` frontmatter and copies the remainder to `out_dir`,
//! preserving relative directory structure. Drives any publish
//! pipeline that ships a public subset of an internal doc vault.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{anyhow, Context, Result};
use globset::{Glob, GlobSetBuilder};
use serde::Serialize;
use walkdir::WalkDir;

use doc_linter::config::LintConfig;
use doc_linter::parser::{parse_doc, Doc};

use super::check;
use super::util::OutputFormat;

/// Default include globs when `--include` is not passed: `docs/`,
/// crate-level READMEs, and the repo-root README. Sample-workspace
/// READMEs and `target/` artefacts are excluded by the
/// `discover_markdown` skip-dirs walker (already configured).
fn default_export_includes() -> Vec<String> {
    vec![
        "docs/**/*.md".to_string(),
        "crates/*/README.md".to_string(),
        "README.md".to_string(),
    ]
}

/// Filters the primary --root tree by `visibility:` frontmatter and copies
/// the remainder to `out_dir`, preserving relative directory structure.
/// Emits a JSON manifest at `<out_dir>/.publish-manifest.json` describing
/// what was kept, what was stripped, and why.
///
/// Idempotent: re-running over an existing `out_dir` overwrites kept files
/// and removes any markdown file that would no longer be exported (e.g.
/// because its visibility flipped to `internal` since the last run).
/// Non-markdown files in `out_dir` are left alone — those are owned by the
/// publish shell script (it copies sibling tooling configs across).
/// @endpoint CLI export
pub(crate) fn run(
    root: &Path,
    config: &LintConfig,
    all_files: &[PathBuf],
    out_dir: PathBuf,
    strip: Vec<String>,
    include: Vec<String>,
    no_prelint: bool,
) -> Result<ExitCode> {
    // Pre-export lint gate. The publish pipeline must not ship a corpus
    // that fails its own validation. Skippable for local iteration only.
    if !no_prelint {
        // Pre-export lint always honours the config's `vale_enabled`. We
        // don't expose `--no-vale` to `export` because the publish gate
        // should reflect what CI runs.
        let lint_exit = check::run(
            root,
            config,
            all_files,
            None,
            OutputFormat::Human,
            false,
            false,
            None,
            false,
            false,
            false,
            false,
        )?
        .exit;
        if lint_exit != ExitCode::SUCCESS {
            eprintln!(
                "doc-linter export: pre-export `check` failed; refusing to publish a broken corpus. \
                 Fix the lint issues above, or pass --no-prelint to bypass (NOT for CI)."
            );
            return Ok(ExitCode::FAILURE);
        }
    }

    // Resolve the output directory absolute path. Create if missing.
    std::fs::create_dir_all(&out_dir)
        .with_context(|| format!("create out dir {}", out_dir.display()))?;
    let out_canon = out_dir
        .canonicalize()
        .with_context(|| format!("canonicalize {}", out_dir.display()))?;

    // Refuse to write to root or to a parent of root — the user clearly
    // didn't mean that.
    if out_canon == *root || root.starts_with(&out_canon) {
        return Err(anyhow!(
            "export --out {} would overlap the source root {}; pick a different directory",
            out_canon.display(),
            root.display()
        ));
    }

    // Build the include glob set. Patterns are matched against repo-relative
    // paths.
    let include_patterns = if include.is_empty() {
        default_export_includes()
    } else {
        include
    };
    let mut include_builder = GlobSetBuilder::new();
    for pat in &include_patterns {
        include_builder
            .add(Glob::new(pat).with_context(|| format!("invalid --include glob `{pat}`"))?);
    }
    let include_set = include_builder.build()?;

    // Strip set: --strip values, defaulting to `internal`.
    let strip_set: std::collections::HashSet<String> = if strip.is_empty() {
        std::iter::once("internal".to_string()).collect()
    } else {
        strip.into_iter().collect()
    };

    // Track decisions for the manifest.
    /// Serde shape for one kept-doc row in the [[entity-doc-graph]]
    /// export-redacted manifest.
    #[derive(Serialize)]
    struct Kept<'a> {
        path: String,
        id: &'a str,
        role: &'a str,
        visibility: &'a str,
    }
    /// Serde shape for one stripped-doc row in the [[entity-doc-graph]]
    /// export-redacted manifest.
    #[derive(Serialize)]
    struct Stripped<'a> {
        path: String,
        reason: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        visibility: Option<&'a str>,
    }
    /// Top-level serde shape for the [[entity-doc-graph]]
    /// export-redacted manifest — kept and stripped buckets.
    #[derive(Serialize)]
    struct Manifest<'a> {
        version: u32,
        source_root: String,
        kept_count: usize,
        stripped_count: usize,
        kept: Vec<Kept<'a>>,
        stripped: Vec<Stripped<'a>>,
        strip_visibility: Vec<&'a str>,
        include_globs: &'a [String],
    }

    // Track existing markdown under out_dir so we can delete any that no
    // longer belong (e.g. visibility flipped to `internal`).
    let mut existing_md: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    if out_canon.exists() {
        for entry in WalkDir::new(&out_canon).into_iter().flatten() {
            if entry.file_type().is_file()
                && entry.path().extension().and_then(|s| s.to_str()) == Some("md")
            {
                existing_md.insert(entry.path().to_path_buf());
            }
        }
    }

    // Re-parse docs (cheap; we already lint-walked once). We need
    // visibility + id + role for the manifest.
    let mut docs: BTreeMap<PathBuf, Doc> = BTreeMap::new();
    for path in all_files {
        // Restrict to the primary root. Cross-repo docs are NOT exported
        // by design — each satellite ships its own publish artefact.
        if !path.starts_with(root) {
            continue;
        }
        if config.is_exempt(path, root) {
            continue;
        }
        if let Ok(doc) = parse_doc(path) {
            docs.insert(path.clone(), doc);
        }
    }

    let mut kept: Vec<Kept> = Vec::new();
    let mut stripped: Vec<Stripped> = Vec::new();
    let mut written_targets: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    for (path, doc) in &docs {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel.to_string_lossy().to_string();

        // Include filter — pattern-based subset selection.
        if !include_set.is_match(rel) {
            continue;
        }

        // Visibility filter. Docs without frontmatter shouldn't normally
        // reach here (lint would have rejected), but guard anyway.
        let (visibility_str, id_str, role_str) = match &doc.meta {
            Some(meta) => (
                meta.visibility.as_deref().unwrap_or("public"),
                meta.id.as_str(),
                meta.role.as_deref().unwrap_or(""),
            ),
            None => ("", "", ""),
        };

        if !visibility_str.is_empty() && strip_set.contains(visibility_str) {
            stripped.push(Stripped {
                path: rel_str,
                reason: "visibility",
                visibility: Some(visibility_str),
            });
            continue;
        }

        // Copy the file over, preserving relative path. We write the raw
        // file bytes verbatim — no frontmatter rewriting. The advisory
        // `visibility:` field stays in place for downstream consumers.
        let target = out_canon.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create dir {}", parent.display()))?;
        }
        std::fs::copy(path, &target)
            .with_context(|| format!("copy {} -> {}", path.display(), target.display()))?;
        written_targets.insert(target);

        kept.push(Kept {
            path: rel.to_string_lossy().to_string(),
            id: id_str,
            role: role_str,
            visibility: visibility_str,
        });
    }

    // Prune old md files that we didn't re-write this run.
    for old in &existing_md {
        if written_targets.contains(old) {
            continue;
        }
        if old.file_name().and_then(|s| s.to_str()) == Some(".publish-manifest.json") {
            continue;
        }
        let _ = std::fs::remove_file(old);
    }

    // Sort for stable manifest output.
    kept.sort_by(|a, b| a.path.cmp(&b.path));
    stripped.sort_by(|a, b| a.path.cmp(&b.path));

    // strip_set is a HashSet — iteration order is nondeterministic.
    // Sort lexicographically so two consecutive `doc-linter export`
    // runs produce byte-identical `.publish-manifest.json` (matters
    // for the publish pipeline's idempotency check).
    let mut strip_vec: Vec<&str> = strip_set.iter().map(std::string::String::as_str).collect();
    strip_vec.sort_unstable();
    let manifest = Manifest {
        version: 1,
        source_root: root.display().to_string(),
        kept_count: kept.len(),
        stripped_count: stripped.len(),
        kept,
        stripped,
        strip_visibility: strip_vec,
        include_globs: &include_patterns,
    };

    let manifest_path = out_canon.join(".publish-manifest.json");
    let body = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, format!("{body}\n"))
        .with_context(|| format!("write {}", manifest_path.display()))?;

    eprintln!(
        "doc-linter export: kept {} file(s), stripped {} file(s) into {}",
        manifest.kept_count,
        manifest.stripped_count,
        out_canon.display()
    );

    Ok(ExitCode::SUCCESS)
}
