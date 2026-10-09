//! `doc-linter lsp` subcommand. Round 4A — thin wrapper around
//! [`doc_linter::lsp::run`] so the LSP server can run on stdio for any
//! editor that speaks LSP.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;

use doc_linter::config::LintConfig;
use doc_linter::lsp;

/// Round 4A: blocks until the LSP server shuts down. The server runs
/// on stdio (LSP convention), so this command MUST be invoked by a
/// process that wires its stdin/stdout to a JSON-RPC peer (an editor's
/// LSP client). All diagnostic output flows over the protocol;
/// user-facing eprintln!s and the rest of the binary's stderr writes
/// go to `<root>/.doc-lint/lsp.log` (overridable via `--log-file`).
/// @endpoint CLI lsp
pub(crate) fn run(root: &Path, config: &LintConfig, log_file: Option<PathBuf>) -> Result<ExitCode> {
    lsp::run(root, config, log_file)?;
    Ok(ExitCode::SUCCESS)
}
