//! `doc-linter homepage` subcommand. Generates the repo's
//! machine-shaped homepage (`MAP.md` by default) from the assembled
//! ontology + parsed doc corpus.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};

use doc_linter::config::LintConfig;
use doc_linter::homepage;
use doc_linter::ontology::Ontology;
use doc_linter::parser::{parse_doc, Doc};

/// Generate the homepage from the current ontology + doc corpus.
/// Without `--write`, prints the canonical content to stdout for
/// preview; with `--write`, applies it to `<homepage_path>` and
/// refuses on a dirty git working tree as a safety measure.
///
/// The on-disk file is intentionally derived from the graph and
/// frontmatter — never hand-edited. The `homepage-stale` lint
/// rule (opt-in via `require_homepage = true`) gates lint on the
/// file matching this canonical output byte-for-byte, so an
/// agent that adds an entity or doc can't ship without also
/// running `homepage --write`.
pub(crate) fn run(
    root: &Path,
    config: &LintConfig,
    all_files: &[PathBuf],
    write: bool,
) -> Result<ExitCode> {
    // Parse every doc once so the generator sees the full corpus,
    // exactly as `cmd_check` would. Exempt files are excluded from
    // the homepage (they're not part of the lint surface).
    let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
    for path in all_files {
        if config.is_exempt(path, root) {
            continue;
        }
        if let Ok(doc) = parse_doc(path) {
            docs.insert(path.clone(), doc);
        }
    }
    let ontology = Ontology::load_from_docs(&docs);
    let content = homepage::generate(root, &docs, &ontology, config);

    if !write {
        print!("{content}");
        return Ok(ExitCode::SUCCESS);
    }

    if !working_tree_clean(root)? {
        eprintln!(
            "doc-linter: homepage --write requires a clean working tree; \
             commit or stash first"
        );
        return Ok(ExitCode::from(2));
    }

    let target = homepage::homepage_file_path(root, config);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent of {}", target.display()))?;
    }
    let existing = homepage::read_existing(&target)
        .with_context(|| format!("read existing {}", target.display()))?;
    if existing.as_deref() == Some(content.as_str()) {
        eprintln!(
            "doc-linter homepage: {} already up to date — no write",
            target.display()
        );
        return Ok(ExitCode::SUCCESS);
    }
    std::fs::write(&target, &content).with_context(|| format!("write {}", target.display()))?;
    eprintln!(
        "doc-linter homepage: wrote {} ({} bytes)",
        target.display(),
        content.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// True when `git status --porcelain` returns no output (working
/// tree clean) or when `root` is not inside a git repo. Mirrors
/// the helper in `scaffold.rs` so this subcommand doesn't depend
/// on the scaffold module — the safety check is small enough that
/// duplicating beats coupling. Permissive in the non-git case so
/// test fixtures don't trip on the guard.
fn working_tree_clean(root: &Path) -> Result<bool> {
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
        return Ok(true);
    }
    Ok(output.stdout.is_empty())
}
