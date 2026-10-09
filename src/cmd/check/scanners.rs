//! Corpus-level scanners that run inside [`super::run`]: orphan
//! entities, unused bounded contexts, homepage staleness. They share
//! the in-memory `Doc` map the parent already built — no fresh
//! filesystem walks, no graph round-trips.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use doc_linter::config::LintConfig;
use doc_linter::homepage;
use doc_linter::ontology::Ontology;
use doc_linter::parser::Doc;
use doc_linter::validator::Issue;

/// Roadmap-48 Rule C: scan the in-memory corpus for ontology-entity
/// docs whose id has zero inbound `covers:` references AND zero
/// `[[entity-<id>]]` wikilinks in any other doc's body. Returns a
/// `(path, Issue)` list — the path points at the entity-X.md file
/// itself so the diagnostic appears under the orphan entity's own
/// row in the report.
///
/// Detection runs against the same parsed `Doc` map the rest of
/// `cmd_check` walks; no extra IO, no extra graph round-trip. The
/// wikilink scan uses a substring search rather than the full
/// `extract_links` parser — `extract_links` returns `target` as the
/// inner content already so an exact id match is enough; we want
/// the cheaper search to keep the corpus pass O(N).
pub(crate) fn run_orphan_entity_check(
    docs: &HashMap<PathBuf, Doc>,
    require_covers: bool,
) -> Vec<(PathBuf, Issue)> {
    use std::collections::{HashMap as Map, HashSet as Set};

    // Collect every entity id declared by an `ontology-entity` doc,
    // along with the path of its defining doc so we can attach the
    // diagnostic.
    let mut entity_paths: Map<String, PathBuf> = Map::new();
    for (path, doc) in docs {
        let Some(meta) = &doc.meta else { continue };
        if meta.effective_role() != Some("ontology-entity") {
            continue;
        }
        // Issue #180: `auto`-status entities are cluster-derived stubs
        // that participate in FUNCTION_MENTIONS but are exempt from
        // narrative coverage — they exist for graph richness, not for
        // mandatory documentation. The promotion workflow (auto →
        // stable) is how a human opts an entity into orphan
        // enforcement.
        if meta.status == "auto" {
            continue;
        }
        // The entity's "id" for orphan purposes is the bare value_id
        // (e.g. `outlet`), not the prefixed doc id (`entity-outlet`).
        // covers: arrays carry the bare form.
        if let Some(value_id) = meta.extra_str("value_id") {
            let v = value_id.to_string();
            if !v.is_empty() {
                entity_paths.entry(v).or_insert_with(|| path.clone());
            }
        }
    }

    if entity_paths.is_empty() {
        return Vec::new();
    }

    // Compute every entity id that has an inbound reference. An id
    // counts as referenced if ANY other doc carries the id in its
    // `covers:` frontmatter — and, in non-strict mode, also if any
    // doc body contains `[[entity-<id>]]`. The defining entity-X.md
    // doc does NOT count as a reference to itself.
    //
    // Strict mode (`orphan_entity_requires_covers = true`): only
    // `covers:` references satisfy. The motivation is the
    // common-failure mode where ontology docs heavily wikilink each
    // other (axis-value/entity cross-refs) without any narrative
    // doc claiming to cover the entity — the graph looks dense but
    // no how-to / explanation actually explains the concept.
    let mut referenced: Set<String> = Set::new();
    for (path, doc) in docs {
        let Some(meta) = &doc.meta else { continue };
        for c in &meta.covers {
            if entity_paths.get(c).is_none_or(|p| p != path) {
                referenced.insert(c.clone());
            }
        }
        if require_covers {
            continue;
        }
        // Wikilink scan: cheap substring search for `[[entity-<id>]]`
        // bracketed forms. The entity-X.md doc itself is excluded
        // from being its own referrer.
        for id in entity_paths.keys() {
            let needle = format!("[[entity-{id}]]");
            if doc.body.contains(&needle) && entity_paths.get(id).is_none_or(|p| p != path) {
                referenced.insert(id.clone());
            }
        }
    }

    let mut out: Vec<(PathBuf, Issue)> = Vec::new();
    let mut sorted_ids: Vec<&String> = entity_paths.keys().collect();
    sorted_ids.sort();
    for id in sorted_ids {
        if referenced.contains(id) {
            continue;
        }
        // `id` came from `entity_paths.keys()` two lines up — the map
        // hasn't been mutated since.
        #[allow(
            clippy::unwrap_used,
            reason = "id is a key of entity_paths by construction"
        )]
        let path = entity_paths.get(id).unwrap().clone();
        out.push((path, Issue::OrphanEntity { id: id.clone() }));
    }
    out
}

/// Unused-bounded-context check. Walks every ontology-value doc
/// on the `bounded-context` axis and emits `UnusedBoundedContext`
/// for those with zero docs claiming them via `bounded_context:`.
/// Exemptions live in `bounded_context_usage_exempt` — useful for
/// contexts reserved for code-side classification ahead of SCIP
/// ingest landing. The diagnostic attaches to the value doc's own
/// path so the editor jumps to the source of the declaration.
pub(crate) fn run_unused_bounded_context_check(
    docs: &HashMap<PathBuf, Doc>,
    config: &LintConfig,
) -> Vec<(PathBuf, Issue)> {
    use std::collections::HashSet as Set;

    // 1. Collect every declared bounded-context value: id → path.
    let mut declared: Vec<(String, PathBuf)> = Vec::new();
    for (path, doc) in docs {
        let Some(meta) = &doc.meta else { continue };
        if meta.effective_role() != Some("ontology-value") {
            continue;
        }
        if meta.extra_str("axis_id") != Some("bounded-context") {
            continue;
        }
        if let Some(value_id) = meta.extra_str("value_id") {
            if !value_id.is_empty() {
                declared.push((value_id.to_string(), path.clone()));
            }
        }
    }
    if declared.is_empty() {
        return Vec::new();
    }

    // 2. Collect every value claimed by at least one doc.
    let mut claimed: Set<String> = Set::new();
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        if let Some(bc) = &meta.bounded_context {
            if !bc.is_empty() {
                claimed.insert(bc.clone());
            }
        }
    }

    // 3. Emit a diagnostic for every declared-but-unclaimed value
    // that isn't on the exempt list.
    let exempt: Set<&str> = config
        .ontology
        .bounded_context_usage_exempt
        .iter()
        .map(std::string::String::as_str)
        .collect();
    let mut out = Vec::new();
    declared.sort_by(|a, b| a.0.cmp(&b.0));
    for (value, path) in declared {
        if claimed.contains(&value) {
            continue;
        }
        if exempt.contains(value.as_str()) {
            continue;
        }
        out.push((path, Issue::UnusedBoundedContext { value }));
    }
    out
}

/// Homepage staleness check. Returns `Some(Issue)` when the
/// configured homepage path is missing or its contents diverge
/// from what `doc-linter homepage` would generate from the current
/// corpus; returns `None` when up-to-date. Caller is responsible
/// for attaching the issue to the homepage path in the report.
///
/// Skips the check when the homepage file path itself is exempt
/// (some adopters may want the file in a vendored dir excluded
/// from the corpus walker; we still verify it's present and
/// up-to-date).
pub(crate) fn run_homepage_check(
    root: &Path,
    docs: &HashMap<PathBuf, Doc>,
    ontology: &Ontology,
    config: &LintConfig,
) -> Option<Issue> {
    let target = homepage::homepage_file_path(root, config);
    let expected = homepage::generate(root, docs, ontology, config);
    let existing = match homepage::read_existing(&target) {
        Ok(v) => v,
        Err(_) => None, // read failure is treated as missing
    };
    let path_rel = target
        .strip_prefix(root)
        .unwrap_or(&target)
        .display()
        .to_string();
    match existing {
        None => Some(Issue::HomepageMissing { path: path_rel }),
        Some(s) if s == expected => None,
        Some(s) => Some(Issue::HomepageStale {
            path: path_rel,
            diff_summary: line_diff_summary(&s, &expected),
        }),
    }
}

/// Compact human-readable hint for the `homepage-stale`
/// diagnostic. Reports scale (line counts and the line of the
/// first divergence) without attempting a full LCS diff —
/// agents read the message to confirm "yes, regenerate" rather
/// than to reconstruct the changes line-by-line.
fn line_diff_summary(have: &str, want: &str) -> String {
    let h: Vec<&str> = have.lines().collect();
    let w: Vec<&str> = want.lines().collect();
    let first_diff = h
        .iter()
        .zip(w.iter())
        .position(|(a, b)| a != b)
        .map(|n| n + 1)
        .or(if h.len() == w.len() {
            None
        } else {
            Some(h.len().min(w.len()) + 1)
        });
    match first_diff {
        Some(line) => format!(
            "first divergence at line {line}; on-disk {} line(s), would-generate {} line(s)",
            h.len(),
            w.len()
        ),
        None => "trailing-newline difference only".to_string(),
    }
}
