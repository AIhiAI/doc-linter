//! Basic frontmatter-validity rules for the [[entity-doc-graph]]
//! linter — the schema-level "what counts as a well-formed doc"
//! axis. Embedded as `LintConfig.lint_rules` via `#[serde(flatten)]`
//! so the on-disk `.doc-lint.toml` shape stays the same: authors
//! write `required_fields = [...]` at the top level, not nested
//! under `[lint_rules]`.

use serde::{Deserialize, Serialize};

/// Allowed-value lists for the frontmatter axes the
/// [[entity-doc-graph]] validator polices (`status`, `visibility`)
/// plus the list of fields every doc must declare.
#[derive(Debug, Deserialize, Serialize)]
pub struct LintRules {
    // `allowed_types` was the pre-ontology vocabulary list. The ontology in
    // docs/ontology/ now owns role/kind/lifecycle vocabularies. The field is
    // still parsed (for old configs that haven't been pruned) but the
    // validator no longer reads it. Any value here is ignored.
    #[serde(default, alias = "allowed_types", rename = "_allowed_types_unused")]
    #[allow(dead_code)]
    pub _legacy_allowed_types: Vec<String>,

    /// Values the frontmatter `status:` field may take. `auto` and
    /// `candidate` (written by `cluster --write`) are always allowed.
    #[serde(default = "default_statuses")]
    pub allowed_statuses: Vec<String>,

    /// Frontmatter fields every doc must carry (`missing-field` otherwise).
    #[serde(default = "default_required_fields")]
    pub required_fields: Vec<String>,

    /// Allowed values for the optional `visibility:` frontmatter field.
    /// Used by `doc-linter export` (and any sibling publish pipeline) to
    /// decide which docs leave a private vault. The linter only checks
    /// the value is in the allow-list when present; it does NOT enforce
    /// visibility boundaries — that's the publish script's job.
    #[serde(default = "default_visibility")]
    pub allowed_visibility: Vec<String>,

    /// Extra filename stems exempt from the id-matches-filename check, on
    /// top of the built-in generic ones (`README`, Spec Kit's `spec` /
    /// `plan` / …). For repos that repeat one file name per folder, e.g.
    /// per-endpoint `info.md` / `testCases.md`; such docs pick a unique
    /// frontmatter `id` instead.
    #[serde(default)]
    pub generic_stems: Vec<String>,
}

impl Default for LintRules {
    fn default() -> Self {
        Self {
            _legacy_allowed_types: Vec::new(),
            allowed_statuses: default_statuses(),
            required_fields: default_required_fields(),
            allowed_visibility: default_visibility(),
            generic_stems: Vec::new(),
        }
    }
}

fn default_statuses() -> Vec<String> {
    // `candidate` is roadmap issue #14's promotion-workflow status:
    // `doc-linter cluster --write` emits draft entity stubs with this
    // status so a human can promote them to `stable` (or delete the
    // file) after review. The pattern-match validator skips candidate
    // entities — they don't carry enforcement weight until promoted.
    //
    // Issue #180: `auto` is the cluster-derived participation status —
    // entity stubs that should populate FUNCTION_MENTIONS (so they show
    // up on the graph and in `query list --ranked`) but must NOT trip
    // coverage / disambiguation lint. The pattern matcher skips them
    // for enforcement; the term index includes them.
    vec![
        "draft",
        "stable",
        "archived",
        "deprecated",
        "candidate",
        "auto",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

fn default_required_fields() -> Vec<String> {
    vec!["id", "type", "title", "summary", "status", "updated"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn default_visibility() -> Vec<String> {
    vec!["public", "internal", "private"]
        .into_iter()
        .map(String::from)
        .collect()
}
