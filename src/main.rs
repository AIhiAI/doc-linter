//! `doc-linter` CLI entry point. Parses argv, walks the markdown
//! corpus, then dispatches to one of the [`cmd`] subcommand handlers.
//!
//! The full subcommand catalogue lives in [`cmd::Commands`]; adding a
//! new subcommand is a two-file change — the new `cmd/<name>.rs` plus
//! one variant in the enum and one match-arm in `cmd::run_command`
//! (#49).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use doc_linter::config::LintConfig;
use doc_linter::init;

mod cmd;

use cmd::{CheckArgs, Commands, OutputFormat};

/// Top-level clap CLI for the [[entity-doc-graph]] linter — root path,
/// config override, output format, and the `Commands` subcommand enum.
#[derive(Parser)]
#[command(name = "doc-linter")]
#[command(about = "Validates and queries project documentation")]
struct Cli {
    /// Repo root (defaults to current working directory)
    #[arg(long, default_value = ".", global = true)]
    root: PathBuf,

    /// Config path (defaults to <root>/.doc-lint.toml)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    // Backward-compat flags that the pre-existing `check` behavior exposed
    // at the top level. If present and no subcommand is given, we run `check`.
    /// (check only) Lint a single file
    #[arg(long, global = true)]
    file: Option<PathBuf>,

    /// (check only) Output format for lint results
    #[arg(long, global = true, default_value = "human")]
    format: OutputFormat,

    /// (check only) Skip Vale vocabulary-closure integration entirely.
    /// Equivalent to `vale_enabled = false` in `.doc-lint.toml`. Has no
    /// effect on other subcommands.
    #[arg(long, global = true, default_value_t = false)]
    no_vale: bool,

    /// (check only) Enable the Round 3A Rust source-comment vocab-closure
    /// lint. Walks every `.rs` file matching `code_comment_includes`,
    /// extracts `///` / `//!` / `/** */` / `/*! */` doc comments, and
    /// runs their prose through the same `TermIndex` used for markdown.
    /// Equivalent to `lint_code_comments = true` in `.doc-lint.toml`.
    #[arg(long, global = true, default_value_t = false)]
    lint_code_comments: bool,

    /// (check only) Phase 3 of roadmap-43 — global function-reach floor
    /// expressed as a percentage. When the corpus's global reach
    /// (`functions reaching an Entity` / `total functions`) is below
    /// this value, `check` emits a single `coverage-below-min`
    /// diagnostic. Overrides the `coverage_min_global` knob in
    /// `.doc-lint.toml`. Useful in CI: `doc-linter check --coverage-min-global=70`.
    #[arg(long, global = true)]
    coverage_min_global: Option<f32>,

    /// (check only) Roadmap issue #33 (v0.3.0): force a full SCIP
    /// ingest regardless of cache state. The default fast-path
    /// (cache-aware ingest) is being built incrementally — for v1
    /// the cache is informational and `--rebuild` only suppresses
    /// the "cache hit" stderr line.
    #[arg(long, global = true, default_value_t = false)]
    rebuild: bool,

    /// (check only) Roadmap issue #28 v3 follow-up (perf): opt
    /// into the post-ingest pass that populates the FLOAT[384]
    /// `*_embedding` columns powering `query similar --backend
    /// embedding`. Default off — the populate pass runs an ONNX
    /// forward over every Doc / Entity / Function / Type row and
    /// dominates `--rebuild` wall-clock when nothing in the
    /// corpus has changed. `query similar` defaults to
    /// `--backend bm25` which doesn't need the column, so most
    /// users never need to flip this on. Hash-skip means
    /// re-running with `--embeddings` after a small edit only
    /// re-embeds the rows that actually changed.
    #[arg(long, global = true, default_value_t = false)]
    embeddings: bool,

    /// (check only) Skip the embeddings pass even when `.doc-lint.toml`
    /// sets `embeddings = true`. Cached vectors are still restored.
    #[arg(
        long,
        global = true,
        default_value_t = false,
        conflicts_with = "embeddings"
    )]
    no_embeddings: bool,

    /// Gap-005 deferred slice: opt-in LLM pairwise contradiction
    /// check across `tags=[design]` Docs. Requires the binary
    /// built with `--features llm` AND `ANTHROPIC_API_KEY` set.
    /// O(N^2) per design-tagged doc pair — only viable for small
    /// corpora. NoOp (with stderr note) when the Judge isn't
    /// available.
    #[arg(long, global = true, default_value_t = false)]
    contradictions: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

fn main() -> ExitCode {
    match run() {
        Ok(exit) => exit,
        Err(e) => {
            eprintln!("doc-linter: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Parses argv, dispatches to the matching [[entity-doc-graph]]
/// subcommand handler, and returns the process exit code.
fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    // `init` is the only subcommand that may run against a directory
    // that doesn't exist yet (or that has no `.doc-lint.toml`). Handle
    // it up-front so the canonicalize-root path below doesn't fail on
    // a fresh target. We do still create the dir + canonicalize it
    // inside `init_at` so the summary prints an absolute path.
    if matches!(cli.command, Some(Commands::Init)) {
        std::fs::create_dir_all(&cli.root)
            .with_context(|| format!("create init root {}", cli.root.display()))?;
        let root = cli
            .root
            .canonicalize()
            .context("canonicalize --root for init")?;
        let summary = init::init_at(&root)?;
        summary.render();
        return Ok(ExitCode::SUCCESS);
    }

    let root = cli.root.canonicalize().context("canonicalize --root")?;
    let config_path = cli.config.unwrap_or_else(|| root.join(".doc-lint.toml"));
    let config = LintConfig::load(&config_path).context("load config")?;

    // The repo itself plus any cross_repo_roots (indexed, not validated).
    let all_files = cmd::util::discover_corpus(&root, &config)?;

    let check_args = CheckArgs {
        only_file: cli.file,
        format: cli.format,
        no_vale: cli.no_vale,
        lint_code_comments: cli.lint_code_comments,
        coverage_min_global: cli.coverage_min_global,
        rebuild: cli.rebuild,
        embeddings: !cli.no_embeddings && (cli.embeddings || config.embeddings),
        contradictions: cli.contradictions,
    };
    cmd::run_command(cli.command, &root, &config, &all_files, check_args)
}

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::{CommandFactory, Parser};

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// clap skips propagating a global arg into a subcommand that defines
    /// the same id, then panics ("Mismatch between definition and access")
    /// when the two types differ. `debug_assert` doesn't catch that, so
    /// check every subcommand for a local arg shadowing a global one.
    #[test]
    fn no_subcommand_arg_shadows_a_global_arg() {
        fn walk(cmd: &clap::Command, globals: &[String], path: &str, bad: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                let here = format!("{path} {}", sub.get_name());
                for arg in sub.get_arguments() {
                    if !arg.is_global_set() && globals.iter().any(|g| g == arg.get_id().as_str()) {
                        bad.push(format!("{here} --{}", arg.get_id()));
                    }
                }
                walk(sub, globals, &here, bad);
            }
        }
        let cmd = Cli::command();
        let globals: Vec<String> = cmd
            .get_arguments()
            .filter(|a| a.is_global_set())
            .map(|a| a.get_id().to_string())
            .collect();
        let mut bad = Vec::new();
        walk(&cmd, &globals, "doc-linter", &mut bad);
        assert!(bad.is_empty(), "subcommand args shadow globals: {bad:?}");
    }

    #[test]
    fn query_map_parses() {
        let cli = Cli::try_parse_from(["doc-linter", "query", "map", "--name", "example"]);
        assert!(cli.is_ok());
        let cli = Cli::try_parse_from([
            "doc-linter",
            "query",
            "map",
            "--name",
            "example",
            "--format",
            "json",
        ]);
        assert!(cli.is_ok());
    }
}
