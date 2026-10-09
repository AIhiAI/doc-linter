//! Roadmap-43 / Roadmap-48 / Roadmap-49 coverage knobs — the
//! [[entity-doc-graph]] axis for "how well-documented is the code"
//! plus the anchor / endpoint / function-exempt machinery.
//!
//! Embedded as `LintConfig.coverage` via `#[serde(flatten)]`; on-disk
//! `.doc-lint.toml` keeps its top-level field names. The 28 default
//! `coverage_function_exempt` regex patterns live in
//! `src/config_data/coverage_function_exempt_defaults.txt` and are
//! loaded via `include_str!` + [`parse_pattern_list`].

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use globset::GlobSet;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// Reach thresholds, exempt lists, and the anchor-required wiring for
/// the dark-public-function / dark-endpoint / entity-coverage-gap
/// lints.
#[derive(Debug, Deserialize, Serialize)]
pub struct CoverageConfig {
    /// When true, every public-API item (pub fn / pub struct / etc.)
    /// that carries a doc comment must reference at least one ontology
    /// entity or `roadmap-…` doc id. Off by default. Only applies to
    /// crates whose path is listed in `anchor_required_in`.
    #[serde(default)]
    pub require_anchor_per_pubapi: bool,

    /// Crate paths (repo-relative, e.g. `crates/pricing-core`) for which
    /// `require_anchor_per_pubapi` enforcement is active. Empty list +
    /// `require_anchor_per_pubapi = true` = enforce nowhere (config
    /// safety net).
    #[serde(default)]
    pub anchor_required_in: Vec<String>,

    /// Round 3B: when true, `cmd_check` emits a `scip-missing` diagnostic
    /// if `<root>/.doc-lint/code.scip` doesn't exist. Default: false (the
    /// SCIP file is opportunistic; absence means the Function table stays
    /// empty but lint is still clean). Set to true in CI configs that want
    /// to gate on the code-graph being indexed.
    #[serde(default)]
    pub scip_required: bool,

    /// Phase 0 of roadmap-43: aspirational global reach target. A crate
    /// with `function_reach_pct < coverage_min_global * 100` is bucketed
    /// as `below-target` in `query coverage-report`. Default `0.0` means
    /// no enforcement (every crate reports as `tracked`); aspirational
    /// `0.95`. Read-only at this phase — Phase 3 of the roadmap promotes
    /// it into a hard lint rule.
    #[serde(default)]
    pub coverage_min_global: f32,

    /// Phase 0 of roadmap-43: per-crate override for `coverage_min_global`.
    /// Key is the crate name (matches `f.crate` in the SCIP-derived
    /// Function table). Falls back to the global value when absent.
    /// Read-only at this phase — Phase 3 turns these into the threshold
    /// for the `dark-public-function` lint per crate.
    #[serde(default)]
    pub coverage_min_per_crate: BTreeMap<String, f32>,

    /// Phase 0 of roadmap-43: minimum `doc:func` ratio per ontology
    /// entity below which the `entity-coverage-gap` lint fires. Default
    /// `0.0` means warn-only; aspirational `0.05` (one doc per 20
    /// code mentions). Read-only at this phase — Phase 3 promotes it
    /// into a real diagnostic, which is when the field gets consumed.
    /// Annotated `dead_code` because the read site is the future Phase
    /// 3 lint, not the Phase 0 reporter (the reporter uses fixed
    /// `severely-under-documented` / `under-documented` / `balanced` /
    /// `over-documented` thresholds).
    #[serde(default = "default_per_entity_doc_ratio")]
    pub coverage_min_per_entity_doc_ratio: f32,

    /// Phase 3 of roadmap-43: entities exempted from the
    /// `entity-coverage-gap` lint. Use for meta-entities whose ratio
    /// inversion is expected (e.g. `doc-graph`, `coverage` — entities
    /// that exist to *describe* the linter, not to be implemented by
    /// many code paths). Empty by default.
    #[serde(default)]
    pub coverage_entity_exempt: Vec<String>,

    /// Phase 5c of roadmap-43: glob patterns (matched against
    /// repo-relative file paths) for source files whose Function nodes
    /// should be skipped entirely from the SCIP ingest. Codegen
    /// sentinels — files like `**/frb_generated.rs`, `**/*_generated.rs`,
    /// `**/*.gen.rs`, `**/build.rs` — produce hundreds of synthetic
    /// dark functions whose authored doc-comments would be wiped on the
    /// next codegen. Defaults cover the common Rust codegen tools;
    /// extend per-repo via `.doc-lint.toml`.
    #[serde(default = "default_coverage_codegen_exclude")]
    pub coverage_codegen_exclude: Vec<String>,

    /// Also index generated code the globs above and the file-header
    /// sniff miss: files git ignores (`build/generated/`, `dist/`, ...),
    /// files under a `generated`/`__generated__` directory, build-output
    /// directories next to a build script, and `*.gen.*` files. `false`
    /// (default) keeps all of that out of the graph, so coverage, `report`
    /// and the concept work list count authored code only. To exclude
    /// more, extend `coverage_codegen_exclude`.
    #[serde(default)]
    pub include_generated: bool,

    /// Phase 5c of roadmap-43: regex patterns matched against a SCIP
    /// symbol's bare function name (the trailing `name()` after the
    /// last `/`) for functions that should NOT count toward the
    /// dark-function metric. Use for irreducibly-generic utilities —
    /// `parse_iso_8601`, `cell_as_*`, `*_fixture`, `default_*`,
    /// `format_err` — that have no plausible domain entity link.
    /// Surfaced as exempt rather than dark; the ENDPOINT_ and
    /// FUNCTION_MENTIONS edges are still produced when the function
    /// happens to mention an entity.
    #[serde(default = "default_coverage_function_exempt")]
    pub coverage_function_exempt: Vec<String>,

    /// Phase 2 of roadmap-43: list of crates whose `Subcommand`
    /// derive enums should be scanned for clap endpoints. Defaults
    /// to `["doc-linter"]` — the linter dogfoods its own subcommand
    /// extraction. Add other binary crates as they grow CLI surfaces
    /// worth tracking. Empty list disables clap extraction entirely
    /// (useful for repos that aren't binary-shaped).
    #[serde(default = "default_clap_crates")]
    pub clap_crates: Vec<String>,

    /// Phase 5c: compiled glob set for `coverage_codegen_exclude`.
    /// Built once in [`crate::config::LintConfig::load`].
    #[serde(default, skip)]
    pub coverage_codegen_exclude_set: Option<GlobSet>,

    /// Phase 5c: compiled regex list for `coverage_function_exempt`.
    /// Built once in [`crate::config::LintConfig::load`]. Each entry is
    /// the original `Regex::new` of a pattern; the loader bails on a
    /// malformed pattern so `.doc-lint.toml` typos surface immediately.
    #[serde(default, skip)]
    pub coverage_function_exempt_set: Option<Vec<Regex>>,

    /// Roadmap-48 Rule A: path (relative to repo root) to a file
    /// listing function symbols that are grandfathered into
    /// anchor-required mode. One bare function name per line; blank
    /// lines and `#` comments are ignored. Loaded once at startup
    /// into an in-memory `HashSet`, merged with
    /// `coverage_function_exempt` regex matches when the
    /// `is_function_exempt` predicate fires. Defaults to
    /// `.doc-lint/anchor-exempt.txt`. Use the
    /// `migrate-anchor-required` subcommand to seed it from the
    /// current corpus state.
    #[serde(default = "default_coverage_anchor_exempt_file")]
    pub coverage_anchor_exempt_file: PathBuf,

    /// Roadmap-48 Rule A: in-memory HashSet of bare function names
    /// loaded from `coverage_anchor_exempt_file`. Built once in
    /// [`crate::config::LintConfig::load`]; consulted from
    /// [`crate::config::LintConfig::is_function_exempt`] alongside the
    /// regex set. `None` when the file does not exist (the common case
    /// for repos that haven't run `migrate-anchor-required` yet).
    #[serde(default, skip)]
    pub coverage_anchor_exempt_set: Option<HashSet<String>>,

    /// Roadmap-48 Rule C: severity for the `orphan-entity` lint —
    /// `"warning"` (default) emits the diagnostic without changing
    /// the exit code; `"error"` promotes it to a hard fail.
    #[serde(default = "default_orphan_entity_severity")]
    pub orphan_entity_severity: String,

    /// Roadmap-49: when true, endpoint discovery uses
    /// the `@endpoint <METHOD> <path>` doc-comment markers
    /// exclusively; the syntactic axum / clap / mcp parsers are
    /// skipped. Defaults to `false`: a repo without markers (any
    /// Java/JAX-RS, FastAPI or Express code base) gets its endpoints
    /// from the syntactic extractors. Set `true` once the migration
    /// tool's run + hand-stamping landed and every handler carries a
    /// marker.
    #[serde(default = "default_endpoint_marker_exclusive")]
    pub endpoint_marker_exclusive: bool,

    /// Roadmap-49 phase 3: list of `<kind>:<METHOD>:<path>` endpoint
    /// ids that should NOT count toward the dark-endpoint metric.
    /// Use for endpoints whose handler is a third-party type with no
    /// FA-authored doc-comment we can stamp (e.g. `axum::Router::nest`
    /// onto `poem_openapi::OpenApiService` or `swagger_ui`). Each
    /// entry is matched verbatim against the `EndpointFact.id` field.
    /// Exempt endpoints are filtered BEFORE creating the Endpoint
    /// node, so they don't appear in the graph at all and the
    /// dark-endpoint lint can never fire on them.
    #[serde(default)]
    pub coverage_endpoint_exempt: Vec<String>,
}

impl Default for CoverageConfig {
    fn default() -> Self {
        Self {
            require_anchor_per_pubapi: false,
            anchor_required_in: Vec::new(),
            scip_required: false,
            coverage_min_global: 0.0,
            coverage_min_per_crate: BTreeMap::new(),
            coverage_min_per_entity_doc_ratio: default_per_entity_doc_ratio(),
            coverage_entity_exempt: Vec::new(),
            coverage_codegen_exclude: default_coverage_codegen_exclude(),
            include_generated: false,
            coverage_function_exempt: default_coverage_function_exempt(),
            clap_crates: default_clap_crates(),
            coverage_codegen_exclude_set: None,
            coverage_function_exempt_set: None,
            coverage_anchor_exempt_file: default_coverage_anchor_exempt_file(),
            coverage_anchor_exempt_set: None,
            orphan_entity_severity: default_orphan_entity_severity(),
            endpoint_marker_exclusive: default_endpoint_marker_exclusive(),
            coverage_endpoint_exempt: Vec::new(),
        }
    }
}

fn default_per_entity_doc_ratio() -> f32 {
    0.0
}

fn default_clap_crates() -> Vec<String> {
    vec!["doc-linter".to_string()]
}

/// Phase 5c of roadmap-43: starter codegen file globs. Matches the
/// `**/<name>` shape so the patterns work whether the file lives in a
/// crate's `src/` or under a generated sub-tree. Repo authors add to
/// this list via `.doc-lint.toml` when they pick up another codegen
/// tool — the defaults are deliberately conservative.
fn default_coverage_codegen_exclude() -> Vec<String> {
    vec![
        "**/frb_generated.rs".to_string(),
        "**/*_generated.rs".to_string(),
        "**/*.gen.rs".to_string(),
        "**/build.rs".to_string(),
        "**/built.rs".to_string(),
        "**/proto_*.rs".to_string(),
        "**/codegen/**".to_string(),
    ]
}

/// Roadmap-48 Rule A: default path to the auto-grandfathered anchor
/// exempt list. Repo-relative; the loader joins it against `--root`.
/// `.doc-lint/` already houses runtime artefacts (`graph.the store`,
/// `vale/`, `code.scip`) and is gitignored by default — per-repo
/// authors can move it under version control by removing the
/// gitignore line and committing the file.
fn default_coverage_anchor_exempt_file() -> PathBuf {
    PathBuf::from(".doc-lint/anchor-exempt.txt")
}

/// Roadmap-48 Rule C: default severity for the `orphan-entity`
/// lint. `"warning"` so flipping the rule on doesn't break CI for
/// repos that pre-existed the rule. Repos that want the hard-fail
/// promote per-config to `"error"`.
fn default_orphan_entity_severity() -> String {
    "warning".to_string()
}

/// Roadmap-49 phase 3: default for the [[entity-doc-graph]]
/// `endpoint_marker_exclusive` knob. `false`: the syntactic extractors
/// run unless a repo that has stamped every handler opts out.
fn default_endpoint_marker_exclusive() -> bool {
    false
}

/// Embedded default `coverage_function_exempt` patterns for the
/// [[entity-doc-graph]] dark-public-function rule. Held in a sibling
/// data file (`config_data/coverage_function_exempt_defaults.txt`) so
/// the regex list isn't 50 lines of inline `.to_string()` calls. Format
/// details — comment lines, blank-line skipping — live in the data
/// file's own header.
pub(crate) const DEFAULT_COVERAGE_FUNCTION_EXEMPT_TEXT: &str =
    include_str!("../config_data/coverage_function_exempt_defaults.txt");

/// Phase 5c of roadmap-43: starter regex patterns for function names
/// that should be exempt (not counted as dark) rather than chased with
/// a doc-comment. The patterns themselves live in
/// `config_data/coverage_function_exempt_defaults.txt` — see that file
/// for the per-section provenance comments.
pub(crate) fn default_coverage_function_exempt() -> Vec<String> {
    parse_pattern_list(DEFAULT_COVERAGE_FUNCTION_EXEMPT_TEXT)
}

/// Parses a `#`-commented pattern list (one regex per non-comment,
/// non-blank line) — shared format for any data file embedded via
/// `include_str!` from `config_data/`. Whitespace around each pattern
/// is trimmed; `#` only starts a comment when it's the first
/// non-whitespace character on a line so patterns containing `#` are
/// not split.
pub(crate) fn parse_pattern_list(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Tripwires for the `coverage_function_exempt_defaults.txt` data file
/// — guards against accidental edits that change the embedded default
/// list shape.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod default_data_tests {
    use super::{default_coverage_function_exempt, parse_pattern_list};

    #[test]
    fn parse_pattern_list_strips_comments_and_blanks() {
        let text = "# header comment\n\n^foo\n# inline note\n  ^bar  \n\n^baz#not-a-comment\n";
        let got = parse_pattern_list(text);
        assert_eq!(got, vec!["^foo", "^bar", "^baz#not-a-comment"]);
    }

    #[test]
    fn coverage_function_exempt_defaults_round_trip() {
        // Tripwire — the embedded list keeps the same 28 starter
        // patterns that landed with wave 5c. Editing the data file is
        // intentional; this test forces the editor to update the count
        // and notice if a section accidentally went missing.
        let patterns = default_coverage_function_exempt();
        assert_eq!(
            patterns.len(),
            28,
            "default_coverage_function_exempt count drifted — update data file or this assertion"
        );
        for expected in [
            "^_",
            "^main$",
            "^cmd_",
            "^deserialize_",
            "^parse_iso_",
            "^seed_",
            "^fake_",
            "^extract_axum_only$",
        ] {
            assert!(
                patterns.iter().any(|p| p == expected),
                "missing default pattern: {expected}"
            );
        }
    }
}

#[cfg(test)]
mod default_tests {
    use super::CoverageConfig;

    /// A repo with no `@endpoint` markers (JAX-RS, FastAPI, Express) must
    /// get its endpoints from the syntactic extractors out of the box.
    #[test]
    fn defaults_run_extractors_and_exclude_generated() {
        let c = CoverageConfig::default();
        assert!(!c.endpoint_marker_exclusive);
        assert!(!c.include_generated);
    }
}
