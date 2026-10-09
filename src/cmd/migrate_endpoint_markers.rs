//! `doc-linter migrate-endpoint-markers` subcommand. Roadmap-49 phase 2
//! — thin wrapper around [`doc_linter::endpoint_markers_migrate::run`]
//! so the subcommand dispatch reads consistently with the other
//! handlers.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Result;

use doc_linter::config::LintConfig;
use doc_linter::endpoint_markers_migrate;

/// @endpoint CLI migrate-endpoint-markers
/// Operates on [[entity-doc-graph]].
pub(crate) fn run(
    root: &Path,
    config: &LintConfig,
    dry_run: bool,
    include_typescript: bool,
) -> Result<ExitCode> {
    endpoint_markers_migrate::run(root, config, dry_run, include_typescript)
}
