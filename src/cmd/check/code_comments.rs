//! Round 3A: source-comment vocab-closure pass over the whole corpus.
//! Off by default; turned on per-repo (`lint_code_comments = true`) or
//! per-invocation (`--lint-code-comments`). Skipped under `--file`
//! narrowing — that path lints one file via [`crate::cmd::check_single`].

use std::path::{Path, PathBuf};

use anyhow::Result;

use doc_linter::code_comments;
use doc_linter::comment_lint::CommentLinter;
use doc_linter::config::LintConfig;
use doc_linter::ontology;
use doc_linter::validator;

use crate::cmd::util::discover_code;

/// Extracts the doc comments of every source file matched by the
/// `code_comment_includes` / `code_comment_excludes` globs (any language
/// [`code_comments::Lang`] reads) and runs them through [`CommentLinter`].
/// Returns a flat `(path, issue)` list ready to merge into the report.
/// Unreadable or unparseable files are skipped — compile errors surface
/// elsewhere.
pub(crate) fn run_code_comment_pipeline(
    root: &Path,
    config: &LintConfig,
    ontology: &ontology::Ontology,
) -> Result<Vec<(PathBuf, validator::Issue)>> {
    let linter = CommentLinter::new(root, config, ontology)?;
    let mut out = Vec::new();
    for file in discover_code(root, config)? {
        let Ok(extraction) = code_comments::extract_any(&file) else {
            continue;
        };
        let require_anchor = config.requires_anchor(&file, root);
        for issue in linter.lint(&file, &extraction.doc_comments, require_anchor) {
            out.push((file.clone(), issue));
        }
    }
    Ok(out)
}
