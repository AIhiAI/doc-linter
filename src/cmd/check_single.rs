//! Per-file doc-comment lint for any source file
//! [`doc_linter::code_comments::Lang`] reads. Driven by the `PostToolUse`
//! hook when an agent edits a single file — sub-second turnaround, no
//! the store, no SCIP, no corpus-wide passes. The corpus-wide pass lives in
//! [`super::check`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use walkdir::WalkDir;

use doc_linter::code_comments;
use doc_linter::comment_lint::CommentLinter;
use doc_linter::config::LintConfig;
use doc_linter::ontology::Ontology;
use doc_linter::parser::{parse_doc, Doc};
use doc_linter::validator::Issue;

use super::util::{render_human, render_json, OutputFormat};

/// Lints the doc comments of one edited source file with the same
/// [`CommentLinter`] the corpus pass uses, and renders the result like
/// `cmd_check` renders markdown. Codegen output and files outside the
/// `code_comment_includes` / `code_comment_excludes` globs are a silent
/// no-op, as is a file the grammar can't parse (compile errors surface
/// elsewhere; the doc-linter doesn't gate on them).
pub(crate) fn code(
    root: &Path,
    config: &LintConfig,
    abs_path: &Path,
    format: OutputFormat,
) -> Result<ExitCode> {
    let rel = abs_path.strip_prefix(root).unwrap_or(abs_path);
    if config.is_codegen_excluded_with_header(rel, abs_path)
        || !config.matches_code_comment(abs_path, root)
    {
        return Ok(ExitCode::SUCCESS);
    }
    let Ok(extraction) = code_comments::extract_any(abs_path) else {
        return Ok(ExitCode::SUCCESS);
    };
    if extraction.doc_comments.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let ontology = load_ontology_dir(root);
    let linter = CommentLinter::new(root, config, &ontology)?;
    let issues = linter.lint(
        abs_path,
        &extraction.doc_comments,
        config.requires_anchor(abs_path, root),
    );
    render_and_exit(root, abs_path.to_path_buf(), issues, format, &ontology)
}

/// Build the minimum ontology + TermIndex needed for per-file vocab
/// closure. We load just `docs/ontology/` rather than parsing every
/// doc in the corpus because (a) a single edit can't have changed the
/// ontology, (b) the tree is small, and (c) the hook wants
/// sub-second turnaround.
fn load_ontology_dir(root: &Path) -> Ontology {
    let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
    let ontology_dir = root.join("docs").join("ontology");
    if ontology_dir.is_dir() {
        for entry in WalkDir::new(&ontology_dir).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            if entry.path().extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            if let Ok(doc) = parse_doc(entry.path()) {
                docs.insert(entry.path().to_path_buf(), doc);
            }
        }
    }
    Ontology::load_from_docs(&docs)
}

/// Final stage of every per-file path — render the assembled
/// [`Issue`] list under the appropriate format and translate the
/// error count to a process exit code.
fn render_and_exit(
    root: &Path,
    target_path: PathBuf,
    issues: Vec<Issue>,
    format: OutputFormat,
    ontology: &Ontology,
) -> Result<ExitCode> {
    let mut report: Vec<(PathBuf, Vec<Issue>)> = Vec::new();
    if !issues.is_empty() {
        report.push((target_path, issues));
    }
    let error_count: usize = report.iter().map(|(_, v)| v.len()).sum();

    match format {
        OutputFormat::Human => render_human(root, &report, 1, error_count, ontology),
        OutputFormat::Json => render_json(root, &report, ontology)?,
    }

    if error_count > 0 {
        Ok(ExitCode::FAILURE)
    } else {
        Ok(ExitCode::SUCCESS)
    }
}
