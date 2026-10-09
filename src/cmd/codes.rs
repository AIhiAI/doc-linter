//! `doc-linter codes` — emits the canonical (code → meaning → fix)
//! table that backs `docs/error-codes.md`. Source-of-truth lives in
//! [`doc_linter::validator::code_table::ERROR_CODE_TABLE`]; this
//! command just renders.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

use doc_linter::validator::code_table;

/// Print the diagnostic-code table to stdout. Today only one
/// rendering — Markdown — is supported, gated by `--markdown` so the
/// flag is reserved for future JSON / YAML outputs. Without
/// `--markdown` we print the same Markdown (the flag is a no-op
/// today but documents the intent).
pub(crate) fn run(_root: &Path, markdown: bool) -> Result<ExitCode> {
    // `markdown` is the only output format today; reserved as a flag
    // so a future `--json` / `--yaml` variant lands without shifting
    // the default behaviour.
    let _ = markdown;
    print!("{}", code_table::render_markdown());
    Ok(ExitCode::SUCCESS)
}
