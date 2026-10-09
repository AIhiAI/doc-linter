//! The closed-vocabulary [`Issue`] enum + its diagnostic-code /
//! promote-stub metadata methods.
//!
//! Author-facing message strings live in [`super::messages`] so the
//! agent-nudge rewrite in #42 is a single-file change.

use crate::config::LintConfig;
use crate::disambiguation::closest_entities_for_token;
use crate::endpoint_extract::EndpointKind;
use crate::graph::EdgeKind;
use crate::ontology::Ontology;
use crate::promote::format_promote_stub;
use chrono::NaiveDate;
use std::path::PathBuf;

/// Every diagnostic the [[entity-doc-graph]] linter can emit — broken
/// wikilinks, missing frontmatter fields, vocabulary violations,
/// cross-context references, coverage shortfalls, and so on.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Issue {
    MissingFrontmatter,
    ParseError(String),
    MissingField(String),
    /// Surfaced by interrogation-004 in the design corpus: an
    /// unquoted YAML scalar contains space-preceded `#` (a PR
    /// or issue reference), which YAML 1.2 treats as comment
    /// start. The parser silently truncates the value at that
    /// position. Quote the value (single or double) to fix.
    YamlScalarLooksTruncated {
        field: String,
        line: u32,
    },
    /// Legacy `type:` is set but new `role:` isn't. Migration 0001 should
    /// have handled this — flag it loudly so it gets fixed.
    LegacyTypeField {
        got: String,
    },
    /// `role:` value isn't registered in the ontology. Either the role
    /// is misspelled or the ontology doc defining it is missing.
    UnknownRole {
        got: String,
    },
    /// `kind:` / `lifecycle:` value isn't a registered value of that axis.
    UnknownAxisValue {
        axis: &'static str,
        got: String,
    },
    /// Role demanded an axis (e.g. role=doc requires kind) and it's missing.
    MissingAxisField {
        role: String,
        axis: &'static str,
    },
    /// Role restricts allowed lifecycle values; the doc declared one outside
    /// the allowed set.
    LifecycleNotAllowedForRole {
        role: String,
        got: String,
        allowed: Vec<String>,
    },
    /// Role mandates a filename pattern and the doc's filename doesn't match.
    FilenamePatternMismatch {
        role: String,
        got: String,
        pattern: String,
    },
    /// Roadmap-53: a `[[layout_rules]]` entry matched the doc's
    /// frontmatter (role/kind/lifecycle) but the doc's path doesn't
    /// start with any of the rule's `allowed_paths`. Move the file or
    /// add the path to the rule. Skipped for `lifecycle: archived`.
    WrongFolder {
        role: Option<String>,
        kind: Option<String>,
        lifecycle: Option<String>,
        expected: Vec<String>,
        got: PathBuf,
    },
    /// `covers:` references a domain entity not registered in the ontology.
    /// Open vocabulary — adding the entity doc is the fix.
    UnknownEntity {
        got: String,
    },
    /// `bounded_context:` (on any doc) or one entry of `bounded_contexts:`
    /// (on an entity-* doc) names a context that has no value doc registered
    /// under `docs/ontology/values/bounded-context/`. Closed vocabulary —
    /// either correct the id or land a value doc + migration bump.
    UnknownBoundedContext {
        got: String,
    },
    /// The bounded-context field is spelled for the wrong role: an entity
    /// wrote `bounded_context:` (entities take the `bounded_contexts:`
    /// list) or a non-entity doc wrote `bounded_contexts:` (docs take the
    /// `bounded_context:` string). Either way the value was ignored.
    WrongBoundedContextField {
        got: &'static str,
        expected: &'static str,
    },
    InvalidStatus {
        got: String,
    },
    /// `visibility:` is set but its value is not in `allowed_visibility`.
    /// Used by `doc-linter export` (and any sibling publish pipeline) to
    /// filter what leaves a private vault.
    InvalidVisibility {
        got: String,
        allowed: Vec<String>,
    },
    UpdatedInFuture {
        date: NaiveDate,
    },
    DuplicateId {
        id: String,
        other: PathBuf,
    },
    IdDoesNotMatchStem {
        id: String,
        stem: String,
    },
    BrokenWikilink {
        target: String,
        line: usize,
    },
    BrokenMdLink {
        target: String,
        line: usize,
    },
    BrokenFileLink {
        target: String,
        line: usize,
    },
    BrokenTypedEdge {
        field: String,
        target: String,
    },
    UnlinkedPath {
        text: String,
        line: usize,
    },
    OrphanDoc,
    /// `vale` is not on PATH — vocabulary-closure (Round 2A) is degraded.
    /// Emitted at most once per `check` run as a repo-level diagnostic.
    /// Install instructions live at https://vale.sh/docs/install.
    ValeNotInstalled,
    /// `vale` is installed but the run itself failed (non-zero runtime
    /// exit, unparseable output). Vocabulary closure did not happen for
    /// this run, so the report must not read as clean.
    ValeFailed {
        message: String,
    },
    /// The corpus has `.adoc` docs but `asciidoctor` (which Vale needs to
    /// read AsciiDoc) is not on PATH, so they were left out of the Vale run.
    AsciidoctorMissing,
    /// A `vale_dictionaries` file can't be read. Its pack is left out of
    /// every accept list (Vale, code-comment lint, unstubbed-concept), so
    /// its terms start firing as unknown vocabulary.
    VocabDictionaryUnreadable {
        name: String,
        path: std::path::PathBuf,
    },
    /// One alert from the Vale binary, mapped to a doc-linter Issue. `check`
    /// is the Vale rule name (e.g. `FA.AmbiguousBare`); `code()` derives a
    /// `vale-<rule>` namespace from it.
    ValeAlert {
        check: String,
        message: String,
        line: usize,
        severity: String,
    },
    /// A bare term in the doc body resolves to an entity that lives in a
    /// DIFFERENT bounded context than the doc itself. Either fully-qualify
    /// the reference (`[[entity-id]]`) or move the doc.
    CrossContextReference {
        term: String,
        entity_id: String,
        entity_context: String,
        doc_context: String,
        line: usize,
    },
    /// Round 3A: a noun-like token inside a Rust source-doc comment
    /// (`///` / `//!` / `/** */` / `/*! */`) doesn't resolve to any
    /// registered ontology entity. Fires only when
    /// `lint_code_comments = true` is set in `.doc-lint.toml` (or
    /// `--lint-code-comments` is passed). The diagnostic carries
    /// `file:line:col` for editor jump-to.
    CommentVocabViolation {
        file: PathBuf,
        line: usize,
        col: usize,
        term: String,
    },
    /// Round 3A: a public-API item carries a doc comment but the comment
    /// doesn't reference any ontology entity or roadmap-entry id —
    /// orphaned vocabulary. Fires only for crates listed in
    /// `anchor_required_in` AND when `require_anchor_per_pubapi = true`.
    MissingAnchor {
        file: PathBuf,
        line: usize,
        symbol: String,
    },
    /// Round 3B: `rust-analyzer` (or any other SCIP indexer) is not on
    /// PATH, so `scip-index` couldn't auto-produce a SCIP file. The
    /// SCIP-driven Function table simply stays empty — non-fatal.
    /// Emitted only when the user explicitly runs `doc-linter scip-index`.
    ScipIndexerMissing,
    /// Round 3B: `<root>/.doc-lint/code.scip` is older than the newest
    /// `.rs` source file by more than a trivial margin — the ingested
    /// Function graph is stale. Re-run the indexer.
    ScipFileStale {
        path: String,
        age_days: u64,
    },
    /// Round 3B: SCIP file is required (LintConfig.scip_required = true)
    /// but doesn't exist. By default the file is optional and absence is
    /// silent.
    ScipFileMissing {
        path: String,
    },
    /// G7 regression guard: an edge whose source and target ids are the
    /// same (a doc wikilinking or typed-edging itself). Almost always a
    /// copy-paste mistake. Emitted post-graph-build in `cmd_check`.
    SelfLoop {
        edge_kind: EdgeKind,
        line: usize,
    },
    /// G5 regression guard: a doc declares `lifecycle: superseded` but no
    /// other doc has a `supersedes:` (or successor `superseded-by:`) edge
    /// pointing at it. Either declare the successor or change the
    /// lifecycle.
    SupersededWithoutSuccessor,
    /// Phase 3 of roadmap-43: a `pub fn` (best-effort: any Function) that
    /// has no FUNCTION_MENTIONS edge AND no `///` doc-comment text. Only
    /// fires for functions in crates listed in `anchor_required_in`.
    /// Emitted post-ingest by querying the store after SCIP is loaded; never
    /// fires in `--file` (single-doc) mode.
    DarkPublicFunction {
        crate_name: String,
        file: String,
        line: u32,
        symbol: String,
    },
    /// Phase 3 of roadmap-43: an Endpoint (axum/clap/mcp) with no
    /// ENDPOINT_TOUCHES_ENTITY edge. Hard-error from day 1 — endpoints
    /// are user-facing surfaces and must declare which entity they serve.
    /// Skipped in `--file` mode (the hook fires per-doc save).
    DarkEndpoint {
        kind: EndpointKind,
        method: String,
        path: String,
        file: String,
        line: u32,
    },
    /// Roadmap issue #14 follow-up (self-healing ontology): a
    /// community discovered by LPA over Function+CALLS doesn't
    /// match any registered ontology entity. Surfaces the
    /// code-vs-ontology asymmetry as a soft warning so contributors
    /// notice that real domain concepts are flowing through the code
    /// graph without being authored as entities. Fix paths in the
    /// message: `cluster --write` scaffolds a candidate stub,
    /// `cluster --promote <id>` flips it to `status: stable`.
    /// Severity is `warning` by default (configured in
    /// `[cluster_lint]` along with `min_members` / `min_density`
    /// thresholds and the `ignore` list for generic clusters like
    /// `common` / `util`).
    UnauthoredCluster {
        suggested_id: String,
        god_node_file: String,
        member_count: usize,
        density: f64,
    },
    /// Phase 3 of roadmap-43: an ontology Entity whose `doc:func` ratio is
    /// strictly below `coverage_min_per_entity_doc_ratio`. Default
    /// threshold is 0.0 (warn-only); raise per-repo. Per-entity
    /// exemptions live in `coverage_entity_exempt` for meta-entities
    /// where ratio inversion is expected (`doc-graph`, `coverage`, …).
    EntityCoverageGap {
        entity: String,
        doc_count: u64,
        func_count: u64,
        ratio: f32,
        threshold: f32,
    },
    /// Phase 3 of roadmap-43 + Roadmap-51 Move 2: the global *non-exempt*
    /// function-reach percentage is below the requested floor. The
    /// metric is `reach / (total - exempt)` — exempt functions
    /// (parser internals, derive-impls, FFI shims matched by
    /// `coverage_function_exempt`) are excluded from the denominator
    /// because they don't map to a domain entity by design, so 100%
    /// is achievable as a target. Triggered only by the
    /// `--coverage-min-global <FLOAT>` CLI flag (or a non-zero
    /// `coverage_min_global` config value). Single repo-level diagnostic.
    CoverageBelowMinimum {
        actual_pct: f32,
        threshold_pct: f32,
    },
    /// Roadmap-48 Rule C: an ontology-entity doc whose id has zero
    /// inbound `covers:` references from any other doc in the
    /// corpus and zero `[[entity-<id>]]` wikilinks in any other
    /// doc's body. Fires immediately (no time-lag) — the author who
    /// introduced the entity must, in the same PR, also write at
    /// least one narrative doc that references it. Severity is
    /// controlled by `orphan_entity_severity` in `.doc-lint.toml`
    /// (default `warning`).
    OrphanEntity {
        id: String,
    },
    /// Graph-density Rule: a `role: doc` (narrative) doc that
    /// declares an empty (or absent) `covers:` frontmatter. Fires
    /// only when `require_covers_per_doc = true`. Severity follows
    /// `missing_covers_severity` (default `error` when enabled).
    /// Skipped for `lifecycle: archived` docs.
    MissingCovers,
    /// Graph-density Rule: a `role: doc` (narrative) doc with no
    /// `bounded_context:` set in frontmatter. Fires only when
    /// `require_bounded_context_per_doc = true`. Severity follows
    /// `missing_bounded_context_severity` (default `error` when
    /// enabled). Skipped for `lifecycle: archived` docs.
    MissingBoundedContext,
    /// Graph-density Rule: an ontology-entity doc whose `value_id`
    /// is `example` AND whose `description:` still carries the
    /// `doc-linter init` scaffold sentence ("Placeholder domain
    /// entity. Replace with concepts from your own domain"). Signals
    /// the user hasn't authored a real ontology yet — the entity
    /// exists only as init debris. Severity is controlled by
    /// `placeholder_entity_severity` (default `warning`).
    PlaceholderEntity {
        id: String,
    },
    /// Homepage rule: the configured `homepage_path` file is
    /// missing. Fires only when `require_homepage = true`. Fix is
    /// `doc-linter homepage --write`. Severity follows
    /// `homepage_stale_severity` (default `error`).
    HomepageMissing {
        path: String,
    },
    /// Homepage rule: the on-disk `homepage_path` file's contents
    /// don't byte-match what `doc-linter homepage` would generate
    /// from the current ontology + doc corpus. Fires only when
    /// `require_homepage = true`. Fix is `doc-linter homepage
    /// --write`. Severity follows `homepage_stale_severity`
    /// (default `error`). `diff_summary` is a human-readable hint
    /// ("12 lines differ") so the diagnostic is informative on the
    /// CLI without dumping the full diff.
    HomepageStale {
        path: String,
        diff_summary: String,
    },
    /// Graph-density Rule: an entity listed in a doc's `covers:`
    /// frontmatter doesn't appear as a `[[entity-<id>]]` wikilink
    /// anywhere in the doc body. Fires only when
    /// `require_body_wikilink_per_cover = true`. The
    /// covers-frontmatter declares semantic intent; the body
    /// wikilink provides the navigational affordance — both are
    /// load-bearing for AI agents and Obsidian's
    /// linked-mentions panel.
    MissingBodyWikilink {
        entity: String,
    },
    /// Graph-density Rule: a bounded-context value is declared in
    /// `docs/ontology/values/bounded-context/` but no doc in the
    /// corpus claims it via `bounded_context:` frontmatter. Fires
    /// only when `require_bounded_context_usage = true`. Attaches
    /// to the value doc itself so the diagnostic points at the
    /// authored-but-unused declaration. Severity follows
    /// `unused_bounded_context_severity` (default `warning`).
    UnusedBoundedContext {
        value: String,
    },
}

impl Issue {
    /// Stable kebab-case diagnostic code for a single [[entity-doc-graph]]
    /// `Issue` variant — used as the lint code in CLI / LSP / JSON output.
    pub fn code(&self) -> &'static str {
        match self {
            Issue::MissingFrontmatter => "missing-frontmatter",
            Issue::ParseError(_) => "parse-error",
            Issue::MissingField(_) => "missing-field",
            Issue::YamlScalarLooksTruncated { .. } => "yaml-scalar-truncated-by-comment",
            Issue::LegacyTypeField { .. } => "legacy-type-field",
            Issue::UnknownRole { .. } => "unknown-role",
            Issue::UnknownAxisValue { .. } => "unknown-axis-value",
            Issue::MissingAxisField { .. } => "missing-axis-field",
            Issue::LifecycleNotAllowedForRole { .. } => "lifecycle-not-allowed-for-role",
            Issue::FilenamePatternMismatch { .. } => "filename-pattern-mismatch",
            Issue::WrongFolder { .. } => "wrong-folder",
            Issue::UnknownEntity { .. } => "unknown-entity",
            Issue::UnknownBoundedContext { .. } => "unknown-bounded-context",
            Issue::WrongBoundedContextField { .. } => "wrong-bounded-context-field",
            Issue::InvalidStatus { .. } => "invalid-status",
            Issue::InvalidVisibility { .. } => "invalid-visibility",
            Issue::UpdatedInFuture { .. } => "updated-in-future",
            Issue::DuplicateId { .. } => "duplicate-id",
            Issue::IdDoesNotMatchStem { .. } => "id-mismatch",
            Issue::BrokenWikilink { .. } => "broken-wikilink",
            Issue::BrokenMdLink { .. } => "broken-md-link",
            Issue::BrokenFileLink { .. } => "broken-file-link",
            Issue::BrokenTypedEdge { .. } => "broken-typed-edge",
            Issue::UnlinkedPath { .. } => "unlinked-path",
            Issue::OrphanDoc => "orphan-doc",
            Issue::ValeNotInstalled => "vale-missing",
            Issue::ValeFailed { .. } => "vale-failed",
            Issue::AsciidoctorMissing => "asciidoctor-missing",
            Issue::VocabDictionaryUnreadable { .. } => "vocab-dictionary-unreadable",
            Issue::ValeAlert { .. } => "vale-alert",
            Issue::CrossContextReference { .. } => "cross-context-reference",
            Issue::CommentVocabViolation { .. } => "comment-vocab-violation",
            Issue::MissingAnchor { .. } => "missing-anchor",
            Issue::ScipIndexerMissing => "scip-indexer-missing",
            Issue::ScipFileStale { .. } => "scip-stale",
            Issue::ScipFileMissing { .. } => "scip-missing",
            Issue::SelfLoop { .. } => "self-loop",
            Issue::SupersededWithoutSuccessor => "superseded-without-successor",
            Issue::DarkPublicFunction { .. } => "dark-public-function",
            Issue::DarkEndpoint { .. } => "dark-endpoint",
            Issue::EntityCoverageGap { .. } => "entity-coverage-gap",
            Issue::UnauthoredCluster { .. } => "unauthored-cluster",
            Issue::CoverageBelowMinimum { .. } => "coverage-below-min",
            Issue::OrphanEntity { .. } => "orphan-entity",
            Issue::MissingCovers => "missing-covers",
            Issue::MissingBoundedContext => "missing-bounded-context",
            Issue::PlaceholderEntity { .. } => "placeholder-entity",
            Issue::HomepageMissing { .. } => "homepage-missing",
            Issue::HomepageStale { .. } => "homepage-stale",
            Issue::MissingBodyWikilink { .. } => "missing-body-wikilink",
            Issue::UnusedBoundedContext { .. } => "unused-bounded-context",
        }
    }

    /// Roadmap-48 Rule C: severity tier. Most diagnostics are
    /// errors that gate the build; `OrphanEntity` is the only one
    /// whose severity is config-driven (`orphan_entity_severity` in
    /// `.doc-lint.toml`, default `"warning"`). Pass the live
    /// `LintConfig` so the predicate stays a pure function of
    /// configured state.
    pub fn is_error(&self, config: &LintConfig) -> bool {
        match self {
            Issue::OrphanEntity { .. } => config.coverage.orphan_entity_severity == "error",
            Issue::MissingCovers => config.ontology.missing_covers_severity == "error",
            Issue::MissingBoundedContext => {
                config.ontology.missing_bounded_context_severity == "error"
            }
            Issue::PlaceholderEntity { .. } => {
                config.ontology.placeholder_entity_severity == "error"
            }
            Issue::HomepageMissing { .. } | Issue::HomepageStale { .. } => {
                config.ontology.homepage_stale_severity == "error"
            }
            Issue::MissingBodyWikilink { .. } => {
                config.ontology.missing_body_wikilink_severity == "error"
            }
            Issue::UnusedBoundedContext { .. } => {
                config.ontology.unused_bounded_context_severity == "error"
            }
            Issue::UnauthoredCluster { .. }
            | Issue::ValeNotInstalled
            | Issue::AsciidoctorMissing => false,
            _ => true,
        }
    }

    /// Roadmap-48 Rule B: if this issue is an unknown-noun /
    /// unknown-entity diagnostic, return the structured promotion
    /// stub (raw frontmatter + body) suitable for a JSON consumer
    /// to surface as an actionable proposal. Other issue variants
    /// return `None`. The closest-by-token-similarity candidate
    /// list is computed here off the live ontology.
    pub fn promote_stub(&self, ontology: &Ontology) -> Option<String> {
        let term = match self {
            Issue::CommentVocabViolation { term, .. } => term.as_str(),
            Issue::UnknownEntity { got } => got.as_str(),
            _ => return None,
        };
        let closest = closest_entities_for_token(term, ontology, 3);
        Some(format_promote_stub(term, &closest))
    }

    /// Roadmap-48 Rule B: render the diagnostic as it appears in
    /// human / JSON output, with the actionable promotion stub
    /// appended for unknown-noun / unknown-entity issues. Other
    /// variants render as plain `Display`. Callers pass the live
    /// ontology so the closest-candidate list is up-to-date with
    /// the corpus on this run.
    pub fn render_with_stub(&self, ontology: &Ontology) -> String {
        let mut s = self.to_string();
        if let Some(stub) = self.promote_stub(ontology) {
            s.push_str(&stub);
        }
        s
    }

    /// Vale-rule-aware code variant. For `Issue::ValeAlert`, returns
    /// `"vale-<rule>"` derived from the check name (lowercased, dotted-path
    /// normalised to dashes). Other variants delegate to `code()`. Used by
    /// JSON output so consumers can filter by Vale rule.
    pub fn extended_code(&self) -> String {
        match self {
            Issue::ValeAlert { check, .. } => {
                let normalised = check.replace('.', "-").to_ascii_lowercase();
                format!("vale-{normalised}")
            }
            other => other.code().to_string(),
        }
    }
}
