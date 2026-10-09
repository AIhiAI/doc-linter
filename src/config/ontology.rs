//! Graph-density rules for the [[entity-doc-graph]] ontology layer —
//! `covers:` / `bounded_context:` requirements, body-wikilink
//! enforcement, the homepage gate, and the bounded-context-usage
//! check. Embedded as `LintConfig.ontology` via `#[serde(flatten)]`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Per-doc ontology requirements: which axes must be declared, what
/// happens when they aren't, and the homepage-index gate.
#[derive(Debug, Deserialize, Serialize)]
pub struct OntologyConfig {
    /// Per-context path inference for the `bounded-context` axis (ontology v4).
    /// Keys are context ids (`pricing`, `sync`, …); values are repo-relative
    /// path prefixes that imply that context when a doc has no explicit
    /// `bounded_context:` frontmatter field. First match wins; longest prefix
    /// wins on ties (so `crates/pricing-tenant` beats `crates/pricing-`).
    ///
    /// Empty by default — repos that don't care about bounded contexts can
    /// ignore this entirely.
    ///
    /// ```toml
    /// [bounded_context_paths]
    /// pricing = ["crates/pricing-core", "crates/pricing-tenant"]
    /// sync    = ["crates/import-core"]
    /// docs    = ["crates/doc-linter", "docs/ontology"]
    /// ```
    #[serde(default)]
    pub bounded_context_paths: BTreeMap<String, Vec<String>>,

    /// Repo-wide fallback `bounded-context` for docs that have no explicit
    /// frontmatter field and don't match any `bounded_context_paths` prefix.
    /// `None` (the default) means "no implicit context" — the doc has no
    /// effective context, which is fine until the cross-context closure
    /// check (Round 2A) lands.
    #[serde(default)]
    pub default_bounded_context: Option<String>,

    /// Graph-density rule: when true, every `role: doc` (narrative)
    /// doc must declare `covers: [...]` with at least one ontology
    /// entity. Catches READMEs / explanations / how-tos that exist as
    /// prose disconnected from the ontology — the most common
    /// AI-agent-discoverability gap when a repo first adopts the
    /// linter. Default `false` for backwards-compat with existing
    /// adopting repos. Severity is configured separately via
    /// `missing_covers_severity` (default `"error"` when the rule is
    /// enabled). Skipped for `lifecycle: archived` docs.
    #[serde(default)]
    pub require_covers_per_doc: bool,

    /// Severity for the `missing-covers` diagnostic. `"warning"` keeps
    /// the build green; `"error"` (default) hard-fails when
    /// `require_covers_per_doc` is on. Validated at config load.
    #[serde(default = "default_missing_covers_severity")]
    pub missing_covers_severity: String,

    /// Graph-density rule: when true, every `role: doc` must declare
    /// `bounded_context:` in frontmatter. Pre-req: the
    /// `bounded-context` axis must be authored under
    /// `docs/ontology/axes/` with value docs under
    /// `docs/ontology/values/bounded-context/`. Default `false`.
    /// Severity follows `missing_bounded_context_severity`.
    /// Skipped for `lifecycle: archived` docs.
    #[serde(default)]
    pub require_bounded_context_per_doc: bool,

    /// Severity for the `missing-bounded-context` diagnostic.
    /// `"warning"` or `"error"` (default `"error"` when the rule is
    /// enabled). Validated at config load.
    #[serde(default = "default_missing_bounded_context_severity")]
    pub missing_bounded_context_severity: String,

    /// Strict mode for the `orphan-entity` rule: when true, an entity
    /// counts as "covered" only if some other doc declares it in its
    /// `covers:` frontmatter. `[[entity-<id>]]` wikilinks from sibling
    /// ontology docs (which form a self-referential ontology layer
    /// rather than narrative coverage) no longer satisfy. Use when
    /// you want every entity to have an explanation / how-to / README
    /// that explicitly claims to cover it. Default `false` (existing
    /// behaviour preserved).
    #[serde(default)]
    pub orphan_entity_requires_covers: bool,

    /// Severity for the `placeholder-entity` diagnostic — fires when
    /// the `example` entity scaffolded by `doc-linter init` is still
    /// present unmodified. `"warning"` (default) keeps the build
    /// green; `"error"` hard-fails until the placeholder is replaced.
    /// Validated at config load.
    #[serde(default = "default_placeholder_entity_severity")]
    pub placeholder_entity_severity: String,

    /// Repo-root index file (the "homepage") rule: when true, the
    /// linter checks that `<homepage_path>` exists and that its
    /// contents byte-match what `doc-linter homepage` would
    /// generate from the current ontology + doc set. The homepage
    /// is the AI-agent cold-start surface — a machine-shaped index
    /// of every entity, narrative doc, and bounded context — and
    /// the lint gate keeps it from drifting out of sync as the
    /// repo evolves. Default `false`; opt-in per-repo.
    #[serde(default)]
    pub require_homepage: bool,

    /// Path (repo-relative) where the auto-generated homepage
    /// lives. Default `MAP.md` at the repo root. Authors may
    /// override (e.g. `docs/MAP.md`) — the `homepage` subcommand
    /// and the lint rule both read this field.
    #[serde(default = "default_homepage_path")]
    pub homepage_path: String,

    /// Severity for `homepage-missing` and `homepage-stale`.
    /// `"warning"` keeps the build green; `"error"` (default when
    /// `require_homepage` is on) hard-fails until the homepage is
    /// regenerated. Validated at config load.
    #[serde(default = "default_homepage_stale_severity")]
    pub homepage_stale_severity: String,

    /// Optional override for the entity used in the homepage's
    /// "What this repo is" section. When unset, the generator
    /// picks the entity whose id matches the repo directory name,
    /// or — failing that — the entity with the most inbound
    /// `covers:` references. Authors override to pin a specific
    /// concept (useful when the repo dir name and the central
    /// concept differ).
    #[serde(default)]
    pub homepage_root_entity: Option<String>,

    /// Roadmap issue #13 (v0.3.0): machine-readable repo domain
    /// for cross-repo pair construction (GMN training, reference-
    /// library indexing). Suggested vocabulary: `AI` | `Backend`
    /// | `ML` | `Frontend` | `DevOps` | `Data` | `Mobile`. Free-
    /// form string — the matcher reads it; no closed validation
    /// at the linter level so adopting repos can extend without
    /// shipping a doc-linter PR.
    #[serde(default)]
    pub repo_domain: Option<String>,

    /// Roadmap issue #13 (v0.3.0): snake_case identifier for the
    /// architectural problem the repo solves (e.g.
    /// `llm_pipeline`, `rest_api_crud`, `developer_tooling`,
    /// `nlp_pipeline`, `rag_pipeline`, `auth_service`).
    /// Same free-form rule as `repo_domain`. Persisted on a
    /// singleton `RepoMeta` node so cross-repo queries can pair
    /// on `(repo.domain, repo.problem_statement)` without re-
    /// reading the config.
    #[serde(default)]
    pub repo_problem_statement: Option<String>,

    /// Graph-density rule: when true, every entity listed in a
    /// `role: doc`'s `covers:` frontmatter must also appear as a
    /// `[[entity-<id>]]` wikilink somewhere in the doc body. The
    /// covers-frontmatter declares semantic intent ("this doc
    /// explains X"); the body wikilink provides the navigational
    /// affordance ("the reader can click X to learn more"). Both
    /// matter for AI-agent discoverability and Obsidian's
    /// linked-mentions panel. Default `false` for backwards-compat
    /// with existing adopting repos. Severity follows
    /// `missing_body_wikilink_severity`.
    #[serde(default)]
    pub require_body_wikilink_per_cover: bool,

    /// Severity for the `missing-body-wikilink` diagnostic.
    /// `"warning"` (default) keeps the build green so authors can
    /// adopt incrementally; `"error"` hard-fails.
    #[serde(default = "default_missing_body_wikilink_severity")]
    pub missing_body_wikilink_severity: String,

    /// Graph-density rule: when true, every value of the
    /// `bounded-context` axis must be claimed by at least one
    /// `role: doc` via the `bounded_context:` frontmatter field.
    /// Catches speculative axis values authored ahead of time and
    /// never used. Default `false`; severity follows
    /// `unused_bounded_context_severity`.
    #[serde(default)]
    pub require_bounded_context_usage: bool,

    /// Severity for the `unused-bounded-context` diagnostic.
    /// `"warning"` (default) is the natural state — an unused
    /// context isn't broken so much as wasteful. Repos that want
    /// a hard gate flip to `"error"`.
    #[serde(default = "default_unused_bounded_context_severity")]
    pub unused_bounded_context_severity: String,

    /// Bounded-context value ids that are legitimately allowed to
    /// have zero docs claiming them (e.g. a context reserved for
    /// code-side classification that hasn't landed yet). Each
    /// entry matches the value's `value_id:` exactly. Consulted by
    /// the `unused-bounded-context` rule above.
    #[serde(default)]
    pub bounded_context_usage_exempt: Vec<String>,
}

impl Default for OntologyConfig {
    fn default() -> Self {
        Self {
            bounded_context_paths: BTreeMap::new(),
            default_bounded_context: None,
            require_covers_per_doc: false,
            missing_covers_severity: default_missing_covers_severity(),
            require_bounded_context_per_doc: false,
            missing_bounded_context_severity: default_missing_bounded_context_severity(),
            orphan_entity_requires_covers: false,
            placeholder_entity_severity: default_placeholder_entity_severity(),
            require_homepage: false,
            homepage_path: default_homepage_path(),
            homepage_stale_severity: default_homepage_stale_severity(),
            homepage_root_entity: None,
            repo_domain: None,
            repo_problem_statement: None,
            require_body_wikilink_per_cover: false,
            missing_body_wikilink_severity: default_missing_body_wikilink_severity(),
            require_bounded_context_usage: false,
            unused_bounded_context_severity: default_unused_bounded_context_severity(),
            bounded_context_usage_exempt: Vec::new(),
        }
    }
}

/// Default severity for the `missing-covers` diagnostic. `"error"`
/// — once you opt-in with `require_covers_per_doc = true` you almost
/// always want the rule to actually gate. Authors who want soft
/// rollout can override to `"warning"`.
fn default_missing_covers_severity() -> String {
    "error".to_string()
}

/// Default severity for the `missing-bounded-context` diagnostic.
/// `"error"` — same rationale as `missing-covers`: the rule is
/// opt-in via `require_bounded_context_per_doc`, and the natural
/// state once opted in is a hard gate.
fn default_missing_bounded_context_severity() -> String {
    "error".to_string()
}

/// Default severity for the `placeholder-entity` diagnostic.
/// `"warning"` — fires unconditionally on any repo that still has
/// the `doc-linter init` scaffolded `example` entity, so a soft
/// default avoids surprising existing adopters. Repos that want a
/// hard gate flip to `"error"`.
fn default_placeholder_entity_severity() -> String {
    "warning".to_string()
}

/// Default repo-relative path for the auto-generated homepage.
/// `MAP.md` at the repo root — distinct from `README.md` (which
/// stays human-authored). Authors override per-repo to e.g.
/// `docs/MAP.md` when the root is reserved for marketing assets.
fn default_homepage_path() -> String {
    "MAP.md".to_string()
}

/// Default severity for `homepage-missing` / `homepage-stale`.
/// `"error"` — once a repo opts into `require_homepage = true`,
/// the natural state is a hard gate; a stale index that an agent
/// glosses to "learn the repo" is worse than no index at all.
fn default_homepage_stale_severity() -> String {
    "error".to_string()
}

/// Default severity for the `missing-body-wikilink` diagnostic.
/// `"warning"` — adding wikilinks to existing narrative prose is
/// a non-trivial edit that should land incrementally; the soft
/// default lets repos opt-in to the rule without breaking CI on
/// day one.
fn default_missing_body_wikilink_severity() -> String {
    "warning".to_string()
}

/// Default severity for the `unused-bounded-context` diagnostic.
/// `"warning"` — an unused context isn't broken, just wasteful;
/// the soft default makes it visible without gating.
fn default_unused_bounded_context_severity() -> String {
    "warning".to_string()
}
