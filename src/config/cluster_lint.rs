//! Roadmap issue #14 follow-up (self-healing ontology): `.doc-lint.toml`
//! knobs for the `unauthored-cluster` check-time lint. The lint runs
//! LPA over Function+CALLS during `check`, then flags every community
//! whose `suggested_id` doesn't match any registered ontology entity
//! — pointing the contributor at `cluster --write` + `cluster --promote`
//! as the self-healing fix path.

use serde::{Deserialize, Serialize};

/// Five tunable knobs. Defaults are picked so the lint adds value
/// out of the box (on by default, warns rather than errors,
/// thresholds tight enough to skip generic `common` / `util` sprawl)
/// while still being trivially silenceable per repo by flipping
/// `enabled = false`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ClusterLintConfig {
    /// Master switch. Default `true` — on by default so a fresh
    /// `init` repo surfaces the ontology gap automatically. Flip
    /// to `false` per repo to mute the rule entirely (e.g. for
    /// repos whose code structure deliberately doesn't map to
    /// domain entities — pure tooling, libraries with no business
    /// nouns).
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Minimum cluster size that triggers a diagnostic. Default
    /// `5` — a community of 1-4 functions is usually a single
    /// utility module, not a domain concept worth promoting to
    /// an entity. Raise to suppress small clusters; lower to be
    /// more aggressive about discovery.
    #[serde(default = "default_min_members")]
    pub min_members: usize,

    /// Minimum intra-cluster density (`intra_edges / max_possible_intra_edges`).
    /// Default `0.05` — picked low so coarse modules
    /// like `common` / `code-ingest` still surface (their internal
    /// density is naturally low because they fan out widely).
    /// Raise to `0.3+` to only flag tight clusters where the
    /// community structure is obvious; lower to `0.0` to flag
    /// every cluster regardless of cohesion.
    #[serde(default = "default_min_density")]
    pub min_density: f64,

    /// Suggested ids the lint should never flag. Default covers
    /// the generic names LPA tends to produce when fall-through
    /// modules collect leftover symbols (`common`, `util`, `mod`,
    /// `tests`, `test`, `main`, `lib`). These aren't domain
    /// concepts and authoring entity files for them would be
    /// noise.
    #[serde(default = "default_ignore")]
    pub ignore: Vec<String>,

    /// Soft cap on the number of distinct clusters reported in a
    /// single `check` run. Default `10` — keeps the diagnostic
    /// list readable on a corpus where LPA finds dozens of small
    /// communities. The top clusters by `member_count` win.
    #[serde(default = "default_top_n")]
    pub top_n: usize,
}

impl Default for ClusterLintConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            min_members: default_min_members(),
            min_density: default_min_density(),
            ignore: default_ignore(),
            top_n: default_top_n(),
        }
    }
}

fn default_enabled() -> bool {
    true
}

fn default_min_members() -> usize {
    5
}

fn default_min_density() -> f64 {
    0.05
}

fn default_top_n() -> usize {
    10
}

fn default_ignore() -> Vec<String> {
    vec![
        "common".to_string(),
        "util".to_string(),
        "utils".to_string(),
        "mod".to_string(),
        "tests".to_string(),
        "test".to_string(),
        "main".to_string(),
        "lib".to_string(),
    ]
}
