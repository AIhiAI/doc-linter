//! Roadmap-53 layout rules — role/kind/lifecycle → path-prefix
//! constraints. Lives under [[entity-doc-graph]] `LintConfig.layout`
//! (flattened so the on-disk TOML still spells the field as
//! `layout_rules = [...]` at the top level).

use serde::{Deserialize, Serialize};

/// One [`LintConfig::layout`] entry. Matches a doc by frontmatter
/// fields and constrains its filesystem location. See roadmap-53.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct LayoutRule {
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub lifecycle: Option<String>,
    pub allowed_paths: Vec<String>,
}

/// Layout-rule list for the role↔folder convention check. Single
/// field today; carved out as its own sub-struct so future layout
/// knobs (default path, severity, exempt globs) land alongside the
/// rule list without growing the top-level [`crate::config::LintConfig`].
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct LayoutConfig {
    /// Roadmap-53: role↔folder convention. Each rule matches docs by
    /// `role:` / `kind:` / `lifecycle:` (any field optional, missing =
    /// wildcard); a matched doc must have its repo-relative path begin
    /// with one of `allowed_paths`. First matching rule wins; order the
    /// array from most-specific (more matchers) to least-specific.
    /// Docs with `lifecycle: archived` skip the check unconditionally.
    /// Empty by default = no enforcement.
    #[serde(default)]
    pub layout_rules: Vec<LayoutRule>,
}
