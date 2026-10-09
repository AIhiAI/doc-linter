//! Roadmap issue #14 follow-up (self-healing ontology): check-time
//! lint that surfaces code-graph clusters not yet authored as
//! ontology entities.
//!
//! ## Why this exists
//!
//! `doc-linter cluster` discovers communities of tightly-related
//! functions via LPA over Function+CALLS. The CLI shows them on
//! demand, but nothing pushes a contributor to actually look. So a
//! repo can grow hundreds of cohesive code modules while the
//! ontology stays at one entity — the exact asymmetry this
//! self-host run uncovered.
//!
//! This lint closes that loop. It runs the same LPA pass during
//! `check`, then emits one [`Issue::UnauthoredCluster`] per
//! community whose `suggested_id` doesn't match any registered
//! entity (by id, display, or synonym) and whose member count +
//! density beat the configured thresholds. The diagnostic carries
//! the fix path — `cluster --write` to scaffold, `cluster --promote`
//! to commit — so noticing the gap and closing the gap take one
//! `check` run apiece.
//!
//! ## Off-switches
//!
//! Three layers:
//!   1. `cluster_lint.enabled = false` in `.doc-lint.toml` mutes
//!      the rule entirely.
//!   2. `cluster_lint.ignore = ["..."]` skips named clusters
//!      (defaults cover the obvious sprawl: `common`, `util`,
//!      `tests`, etc.).
//!   3. Authoring the entity (the intended fix) removes that
//!      cluster from the diagnostic stream on the next run.

use anyhow::Result;

use doc_linter::config::ClusterLintConfig;
use doc_linter::ontology::Ontology;
use doc_linter::validator::Issue;

use crate::cmd::cluster::{discover_clusters, DiscoveredCluster};

/// Roadmap issue #14 follow-up (self-healing ontology): walk the
/// discovered clusters and emit one [`Issue::UnauthoredCluster`]
/// per cluster that beats the configured size + density floor AND
/// isn't covered by an existing ontology entity or the
/// `cluster_lint.ignore` list.
///
/// Returns an empty `Vec` when the lint is disabled or the corpus
/// has no Function nodes (fresh `init` repo, SCIP ingest skipped).
pub(crate) fn run_cluster_lint(
    db: &dyn doc_linter::graph_read::GraphRead,
    config: &ClusterLintConfig,
    ontology: &Ontology,
) -> Result<Vec<Issue>> {
    // With no ontology entities there is nothing a cluster could match, so
    // every community would be reported: a code-only graph root (no
    // docs/ontology/) has no ontology by design.
    if !config.enabled || ontology.entities.is_empty() {
        return Ok(Vec::new());
    }
    let clusters = discover_clusters(db, config.min_members)?;
    if clusters.is_empty() {
        return Ok(Vec::new());
    }

    let known = build_known_slug_set(ontology, &config.ignore);

    // Dedup by `suggested_id`. LPA can produce multiple
    // communities that share the same god-node file (and therefore
    // the same derived id) — e.g. a tight inner sub-community
    // alongside a looser sprawl from the same module. Reporting
    // both confuses the fix path: "didn't I already author
    // entity-cluster?". Keep the cluster with the highest member
    // count (which is also the loudest signal that this is a real
    // concept). Discovery order is already member-count-descending,
    // so the first occurrence wins.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deduped: Vec<DiscoveredCluster> = Vec::new();
    for c in clusters {
        if !seen.insert(c.suggested_id.clone()) {
            continue;
        }
        deduped.push(c);
    }

    let issues: Vec<Issue> = deduped
        .into_iter()
        .filter(|c| c.density >= config.min_density)
        .filter(|c| !known.contains(&slug(&c.suggested_id)))
        .take(config.top_n.max(1))
        .map(into_issue)
        .collect();
    Ok(issues)
}

fn into_issue(c: DiscoveredCluster) -> Issue {
    Issue::UnauthoredCluster {
        suggested_id: c.suggested_id,
        god_node_file: c.god_node_file,
        member_count: c.member_count,
        density: c.density,
    }
}

/// Build the union of slugs the lint considers "already covered":
/// every registered entity's `id`, slugged `display`, and slugged
/// `synonyms`, plus the configured `cluster_lint.ignore` entries.
/// Slugging is lowercase + non-alphanumeric → `-` so the comparison
/// is robust to display-case drift (`MCP` vs `mcp`) and synonym
/// punctuation (`Pricing Rule` vs `pricing-rule`).
fn build_known_slug_set(
    ontology: &Ontology,
    ignore: &[String],
) -> std::collections::HashSet<String> {
    let mut set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entity in ontology.entities.values() {
        set.insert(slug(&entity.id));
        set.insert(slug(&entity.display));
        for syn in &entity.synonyms {
            set.insert(slug(syn));
        }
    }
    for ig in ignore {
        set.insert(slug(ig));
    }
    set
}

/// Lowercase, replace any run of non-alphanumerics with a single
/// `-`, trim leading/trailing `-`. Matches the slug shape that
/// `cluster`'s `suggested_id` derivation produces from a god-node
/// file path.
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = true;
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            for c in ch.to_lowercase() {
                out.push(c);
            }
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn slug_lowercases_and_collapses_non_alnum() {
        assert_eq!(slug("Doc Graph"), "doc-graph");
        assert_eq!(slug("Pricing Rule"), "pricing-rule");
        assert_eq!(slug("MCP"), "mcp");
        assert_eq!(slug("multi   space"), "multi-space");
        assert_eq!(slug("trailing-"), "trailing");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn known_slug_set_covers_id_display_and_synonyms() {
        use doc_linter::ontology::EntityDef;
        let mut ontology = Ontology::default();
        ontology.entities.insert(
            "doc-graph".to_string(),
            EntityDef {
                id: "doc-graph".to_string(),
                display: "Doc Graph".to_string(),
                synonyms: vec!["knowledge-graph".to_string(), "sql graph".to_string()],
                ..EntityDef::default()
            },
        );
        let ignore = vec!["common".to_string()];
        let set = build_known_slug_set(&ontology, &ignore);
        assert!(set.contains("doc-graph"));
        assert!(set.contains("knowledge-graph"));
        assert!(set.contains("sql-graph"));
        assert!(set.contains("common"));
    }

    /// The lint never fires when disabled, even if the discovery
    /// pass would find unauthored clusters. Guards against a future
    /// refactor that accidentally inverts the gate.
    #[test]
    fn disabled_config_returns_empty() {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-cluster-lint-disabled-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        doc_linter::store_sqlite::build_and_swap(&dir, |_| Ok(())).unwrap();
        let db = doc_linter::store_sqlite::open_ro(&dir).unwrap();

        let cfg = ClusterLintConfig {
            enabled: false,
            ..ClusterLintConfig::default()
        };
        let issues = run_cluster_lint(&db, &cfg, &Ontology::default()).unwrap();
        assert!(issues.is_empty(), "disabled lint must return zero issues");
    }

    /// Legacy gap 10: a code-only root (no ontology entities) reported every
    /// code community as `unauthored-cluster`.
    #[test]
    fn no_ontology_entities_means_no_cluster_findings() {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-cluster-no-ontology-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        doc_linter::store_sqlite::build_and_swap(&dir, |db| {
            use doc_linter::store::Store;
            // A tight ring of 6 functions: one community.
            for i in 0..6 {
                db.exec(
                    &format!(
                        "INSERT INTO Function(symbol, file) VALUES ('f{i}', 'src/billing.rs')"
                    ),
                    &[],
                )?;
            }
            for i in 0..6 {
                for j in 0..6 {
                    if i != j {
                        db.exec(
                            &format!("INSERT INTO CALLS(src, dst) VALUES ('f{i}', 'f{j}')"),
                            &[],
                        )?;
                    }
                }
            }
            Ok(())
        })
        .unwrap();
        let db = doc_linter::store_sqlite::open_ro(&dir).unwrap();
        let cfg = ClusterLintConfig {
            min_members: 2,
            ..ClusterLintConfig::default()
        };
        let issues = run_cluster_lint(&db, &cfg, &Ontology::default()).unwrap();
        assert!(issues.is_empty(), "ontology-less root got {issues:?}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
