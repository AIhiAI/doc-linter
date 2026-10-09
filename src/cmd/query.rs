//! `doc-linter query` subcommand. Thin wrapper around
//! [`doc_linter::query::run`] that opens the SQLite graph read-only and
//! dispatches to the read-side query helpers.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;

use doc_linter::config::LintConfig;
use doc_linter::graph_read::ReadGraph;
use doc_linter::query::{self, QueryKind};

/// Query the persistent graph at `<root>/.doc-lint/graph.sqlite`.
///
/// The DB is only refreshed by `cmd_check`; query is read-only — calling
/// `query` against a stale corpus returns stale results. Run `doc-linter
/// check` first if the corpus has changed since the last lint pass; a
/// missing graph is an error that says so.
/// @endpoint CLI query
pub(crate) fn run(
    root: &Path,
    config: &LintConfig,
    _all_files: &[PathBuf],
    kind: QueryKind,
) -> Result<ExitCode> {
    // A missing graph is an error from the open ("run `doc-linter check` first").
    let db = ReadGraph::open(root)?;
    let out = query::run(&*db, root, config, kind)?;
    println!("{out}");
    Ok(ExitCode::SUCCESS)
}
