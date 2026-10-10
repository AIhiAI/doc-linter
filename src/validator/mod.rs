//! Lint-rule engine for the [[entity-doc-graph]] linter.
//!
//! Module layout (#47):
//!
//! - [`issue`] — the closed-vocabulary [`Issue`] enum + its
//!   diagnostic-code / promote-stub metadata methods. Pure data.
//! - [`messages`] — the `impl fmt::Display for Issue` author-facing
//!   message catalog. Lives in its own file so the agent-nudge rewrite
//!   in #42 (retuning every diagnostic toward "grow the docs" rather
//!   than "remove the offending content") is a single-file change.
//! - This `mod.rs` — the [`validate_doc`] dispatcher + shared helpers
//!   ([`slugs_equal`], [`compile_regex`]) + the closed-vocab constant
//!   [`RELATES_TO_TYPES`].
//!
//! A per-rule split (one struct + `impl LintRule` per category, with
//! `validate_doc` reduced to iterating a `RULES` slice) is left as a
//! follow-up: it requires extracting ~350 lines of inline rule logic
//! across seven categories and is reviewable separately. The
//! `issue` / `messages` split landed here is enough to unblock #42 —
//! the explicit motivation in #47.

pub mod code_table;
mod issue;
mod messages;

pub use issue::Issue;

use crate::config::LintConfig;
use crate::ontology::Ontology;
use crate::parser::Doc;
use chrono::Local;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

/// Closed vocabulary for the `type:` field of every `relates_to:` row on
/// entity frontmatter. New types should be added deliberately — adding a
/// noun here is a public-API change for downstream consumers reading the
/// `RELATES_TO.type` column.
pub const RELATES_TO_TYPES: &[&str] = &[
    "depends_on",
    "calls",
    "feeds_into",
    "produces",
    "configured_by",
];

/// Top-level [[entity-doc-graph]] lint entry point — runs every rule
/// against a parsed `Doc` and the corpus-wide ontology + graph context,
/// returning the full `Issue` list.
pub fn validate_doc(
    path: &Path,
    root: &Path,
    doc: &Doc,
    config: &LintConfig,
    ontology: &Ontology,
    issues: &mut Vec<Issue>,
) {
    let Some(meta) = &doc.meta else {
        if doc.raw_frontmatter.is_none() {
            issues.push(Issue::MissingFrontmatter);
        }
        return;
    };

    // Interrogation-004 follow-up: catch unquoted YAML scalars that
    // contain space-preceded `#` (PR / issue references). YAML 1.2
    // treats those as comment starts, silently truncating the value
    // at the first ` #`. We detect this BEFORE downstream checks so
    // the warning lands close to the cause, not after a cascade of
    // empty-summary / unknown-axis-value issues.
    if let Some(raw) = doc.raw_frontmatter.as_deref() {
        for (idx, line) in raw.lines().enumerate() {
            if let Some((field, line_no)) = yaml_scalar_truncated_by_comment(line, idx as u32) {
                issues.push(Issue::YamlScalarLooksTruncated {
                    field,
                    line: line_no,
                });
            }
        }
    }

    // Required-field presence — id/title/summary/status/updated are still
    // structurally required. `role:` is required and ontology-validated below.
    for field in &config.lint_rules.required_fields {
        let present = match field.as_str() {
            "id" => !meta.id.is_empty(),
            "role" => meta.role.is_some(),
            "title" => !meta.title.is_empty(),
            "summary" => !meta.summary.is_empty(),
            "status" => !meta.status.is_empty(),
            "updated" => true, // already a NaiveDate — presence guaranteed by deserializer
            // Legacy holdover: if a config still lists "type" it's a hard
            // error to update the config, not a per-doc issue. Skip silently.
            "type" => true,
            _ => true,
        };
        if !present {
            // Special case: missing role + present legacy type → a more
            // specific message that points at the migration.
            if field == "role" {
                if let Some(t) = &meta.legacy_type {
                    issues.push(Issue::LegacyTypeField { got: t.clone() });
                    continue;
                }
            }
            issues.push(Issue::MissingField(field.clone()));
        }
    }

    // Ontology-driven role + axis validation.
    if let Some(role) = meta.effective_role() {
        match ontology.role(role) {
            None => {
                issues.push(Issue::UnknownRole {
                    got: role.to_string(),
                });
            }
            Some(role_def) => {
                // Required-axis enforcement.
                for axis in &role_def.requires_axes {
                    let present = match axis.as_str() {
                        "kind" => meta.kind.is_some(),
                        "lifecycle" => meta.lifecycle.is_some(),
                        _ => true,
                    };
                    if !present {
                        let axis_static: &'static str = match axis.as_str() {
                            "kind" => "kind",
                            "lifecycle" => "lifecycle",
                            _ => "unknown",
                        };
                        issues.push(Issue::MissingAxisField {
                            role: role.to_string(),
                            axis: axis_static,
                        });
                    }
                }

                // Filename pattern enforcement.
                if let Some(pattern) = &role_def.filename_pattern {
                    if let Some(filename) = path.file_name().and_then(|n| n.to_str()) {
                        let re = compile_regex(pattern);
                        if !re.is_match(filename) {
                            issues.push(Issue::FilenamePatternMismatch {
                                role: role.to_string(),
                                got: filename.to_string(),
                                pattern: pattern.clone(),
                            });
                        }
                    }
                }

                // Lifecycle restriction per role.
                if let (Some(allowed), Some(lifecycle)) =
                    (&role_def.allowed_lifecycle, &meta.lifecycle)
                {
                    if !allowed.iter().any(|a| a == lifecycle) {
                        issues.push(Issue::LifecycleNotAllowedForRole {
                            role: role.to_string(),
                            got: lifecycle.clone(),
                            allowed: allowed.clone(),
                        });
                    }
                }
            }
        }
    }

    // Roadmap-53: role↔folder convention. Walk `layout_rules` in order;
    // first rule whose role/kind/lifecycle filters all match wins. The
    // matched rule's `allowed_paths` must be a prefix of the doc's
    // repo-relative path. Archived docs are exempt. Cross-repo docs
    // (passed as absolute paths via `cross_repo_roots`) are exempt — a
    // sibling repo's layout convention is not this repo's business.
    // Layout check only applies to docs under the project root —
    // cross-repo siblings have their own conventions.
    let in_repo_path = path.strip_prefix(root).ok();
    if meta.lifecycle.as_deref() != Some("archived") {
        if let Some(rel_path) = in_repo_path {
            let path_str = rel_path.to_string_lossy().replace('\\', "/");
            let path_norm = path_str.as_str();
            for rule in &config.layout.layout_rules {
                if let Some(want) = &rule.role {
                    match &meta.role {
                        Some(have) if have == want => {}
                        _ => continue,
                    }
                }
                if let Some(want) = &rule.kind {
                    match &meta.kind {
                        Some(have) if have == want => {}
                        _ => continue,
                    }
                }
                if let Some(want) = &rule.lifecycle {
                    match &meta.lifecycle {
                        Some(have) if have == want => {}
                        _ => continue,
                    }
                }
                // Rule matched. Check the path prefix.
                let allowed = rule.allowed_paths.iter().any(|p| {
                    let prefix = if p.ends_with('/') {
                        p.clone()
                    } else {
                        format!("{p}/")
                    };
                    path_norm.starts_with(&prefix)
                });
                if !allowed {
                    issues.push(Issue::WrongFolder {
                        role: rule.role.clone(),
                        kind: rule.kind.clone(),
                        lifecycle: rule.lifecycle.clone(),
                        expected: rule.allowed_paths.clone(),
                        got: rel_path.to_path_buf(),
                    });
                }
                break;
            }
        }
    }

    // kind / lifecycle value validation against the ontology.
    if let Some(kind) = &meta.kind {
        if ontology.kind(kind).is_none() {
            issues.push(Issue::UnknownAxisValue {
                axis: "kind",
                got: kind.clone(),
            });
        }
    }
    if let Some(lifecycle) = &meta.lifecycle {
        if ontology.lifecycle(lifecycle).is_none() {
            issues.push(Issue::UnknownAxisValue {
                axis: "lifecycle",
                got: lifecycle.clone(),
            });
        }
    }

    // covers — open vocabulary; flag unknown entities so authors know to
    // either correct the id or land an entity doc.
    for entity in &meta.covers {
        if ontology.entity(entity).is_none() {
            issues.push(Issue::UnknownEntity {
                got: entity.clone(),
            });
        }
    }

    // bounded_context — closed (per-repo) vocabulary, optional on every doc.
    // When set, the value must be a registered context. Path-inference and
    // repo-default fallbacks happen in `effective_bounded_context` and are
    // NOT validated here (those are config-driven, not author-supplied).
    if let Some(bc) = &meta.bounded_context {
        if ontology.bounded_context(bc).is_none() {
            issues.push(Issue::UnknownBoundedContext { got: bc.clone() });
        }
    }

    // Entities take the `bounded_contexts:` list, every other doc the
    // `bounded_context:` string. The other spelling parses fine and is then
    // ignored, which silently makes an entity context-agnostic or a doc
    // context-less, so flag it.
    let is_entity = meta.effective_role() == Some("ontology-entity");
    let wrong_field = if is_entity {
        meta.bounded_context
            .is_some()
            .then_some(("bounded_context", "bounded_contexts"))
    } else {
        meta.extra
            .contains_key("bounded_contexts")
            .then_some(("bounded_contexts", "bounded_context"))
    };
    if let Some((got, expected)) = wrong_field {
        issues.push(Issue::WrongBoundedContextField { got, expected });
    }

    // entity-* docs may declare which bounded contexts they exist in via a
    // multi-valued `bounded_contexts:` list. Each entry must resolve. Empty
    // list (the common case) means context-agnostic — no validation needed.
    if is_entity {
        for bc in meta.extra_str_list("bounded_contexts") {
            if ontology.bounded_context(&bc).is_none() {
                issues.push(Issue::UnknownBoundedContext { got: bc });
            }
        }
        // `relates_to:` types are a closed vocabulary; validate each row's
        // `type` against the locked set so a typo gets flagged instead of
        // silently dropping the edge at ingest.
        for entry in meta
            .extra
            .get("relates_to")
            .and_then(|v| v.as_sequence())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_mapping())
        {
            let Some(ty) = entry
                .get(serde_yaml::Value::String("type".to_string()))
                .and_then(|v| v.as_str())
            else {
                continue;
            };
            if !RELATES_TO_TYPES.contains(&ty) {
                issues.push(Issue::UnknownAxisValue {
                    axis: "relates_to_type",
                    got: ty.to_string(),
                });
            }
        }
    }

    if !config
        .lint_rules
        .allowed_statuses
        .iter()
        .any(|s| s == &meta.status)
    {
        issues.push(Issue::InvalidStatus {
            got: meta.status.clone(),
        });
    }

    // Optional `visibility:` — present-only check against the publish-filter
    // allow-list. Absence is fine and means "public" (default at publish time).
    if let Some(vis) = &meta.visibility {
        if !config
            .lint_rules
            .allowed_visibility
            .iter()
            .any(|v| v == vis)
        {
            issues.push(Issue::InvalidVisibility {
                got: vis.clone(),
                allowed: config.lint_rules.allowed_visibility.clone(),
            });
        }
    }

    let today = Local::now().date_naive();
    if meta.updated > today {
        issues.push(Issue::UpdatedInFuture { date: meta.updated });
    }

    // Soft check: id should match filename stem for discoverability. Generic
    // stems are exempt — they'd collide across docs, and their id lives in
    // frontmatter instead.
    //   README       — repo / dir root doc
    //   brd          — per-tenant Business Requirements file
    //   constitution — Spec Kit constitution file (.specify/memory/)
    //   platform     — platform-scoped board / index under docs/boards/
    //   spec/plan/tasks/research — Speckit's convention: each feature dir
    //     under `.specify/specs/<feature-id>/` contains spec.md, plan.md,
    //     tasks.md, research.md. The frontmatter `id` is feature-scoped
    //     (e.g., `spec-billing-rate-limit-sync-jobs`), the filename is generic.
    // Also exempt: anything under docs/ontology/ (ids are axis-prefixed:
    // axis-role, value-role-doc, entity-outlet, etc.), anything under
    // .specify/specs/ (Speckit-managed feature dirs), and anything under
    // .sessions/ or .memories/ (roadmap 20: filenames use ISO timestamps
    // / human-readable slugs while ids are slug- or `mem-team-…`-prefixed).
    const GENERIC_STEMS: &[&str] = &[
        "README",
        "USAGE",
        "AGENTS",
        "brd",
        "constitution",
        "platform",
        "spec",
        "plan",
        "tasks",
        "research",
    ];
    let in_ontology_dir = path.components().any(|c| c.as_os_str() == "ontology");
    let in_speckit_specs = path.components().any(|c| c.as_os_str() == ".specify");
    let in_sessions_or_memories = path.components().any(|c| {
        let name = c.as_os_str();
        name == ".sessions" || name == ".memories"
    });
    if !in_ontology_dir && !in_speckit_specs && !in_sessions_or_memories {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let generic = GENERIC_STEMS.contains(&stem)
                || config.lint_rules.generic_stems.iter().any(|g| g == stem);
            if !generic && !slugs_equal(stem, &meta.id) {
                issues.push(Issue::IdDoesNotMatchStem {
                    id: meta.id.clone(),
                    stem: stem.to_string(),
                });
            }
        }
    }

    // Graph-density rules: gate on `role: doc` (narrative), skip
    // archived. Authors who opt in via `.doc-lint.toml` expect every
    // narrative doc to declare which entities and bounded-context it
    // pertains to — the two structural anchors that make the doc
    // findable in graph queries.
    let is_archived = meta.lifecycle.as_deref() == Some("archived");
    if meta.effective_role() == Some("doc") && !is_archived {
        if config.ontology.require_covers_per_doc && meta.covers.is_empty() {
            issues.push(Issue::MissingCovers);
        }
        if config.ontology.require_bounded_context_per_doc && meta.bounded_context.is_none() {
            issues.push(Issue::MissingBoundedContext);
        }

        // Body-wikilink-per-cover: for every entity declared in
        // `covers:`, the body must contain at least one
        // `[[entity-<id>]]` wikilink. Captures the gap where the
        // covers-frontmatter declares intent but the body never
        // names the concept as a navigation target. Cheap
        // substring search — the linter already does this for the
        // orphan-entity rule, and the body is held in memory.
        if config.ontology.require_body_wikilink_per_cover && !meta.covers.is_empty() {
            for entity in &meta.covers {
                let needle = format!("[[entity-{entity}]]");
                if !doc.body.contains(&needle) {
                    issues.push(Issue::MissingBodyWikilink {
                        entity: entity.clone(),
                    });
                }
            }
        }
    }

    // Placeholder-entity rule: catches the `example` entity scaffolded
    // by `doc-linter init` when it has not been replaced by a real
    // domain concept. We match on `value_id == "example"` AND the
    // exact init-template description sentence so a project that
    // genuinely has an `example` entity (renamed in body but never
    // edited from the scaffold) still trips, while one that rewrote
    // the description to describe a real concept does not.
    if meta.effective_role() == Some("ontology-entity") {
        let value_id = meta.extra_str("value_id").unwrap_or("");
        if value_id == "example" {
            let desc = meta.extra_str("description").unwrap_or("");
            if desc.contains("Placeholder domain entity") {
                issues.push(Issue::PlaceholderEntity {
                    id: value_id.to_string(),
                });
            }
        }
    }
}

// Slug-normalised comparison: lowercases, treats `_` and `-` as
// equivalent, ignores other punctuation. Lets `BUILD_SUMMARY` filename
// match id `build-summary` without losing typo detection — a missing
// word like `handoff-original-session` vs stem `HANDOFF_TO_ORIGINAL_SESSION`
// still mismatches.
fn slugs_equal(a: &str, b: &str) -> bool {
    fn norm(s: &str) -> String {
        s.chars()
            .filter_map(|c| {
                if c.is_ascii_alphanumeric() {
                    Some(c.to_ascii_lowercase())
                } else if c == '_' || c == '-' {
                    Some('-')
                } else {
                    None
                }
            })
            .collect()
    }
    norm(a) == norm(b)
}

/// Compiles a regex once and caches the result on a `OnceLock` so the
/// [[entity-doc-graph]] validator's hot path doesn't re-compile rules
/// per doc.
fn compile_regex(pattern: &str) -> &'static Regex {
    // Caches the most recent regex — fine because there are very few
    // distinct filename_pattern values across the ontology.
    use std::collections::HashMap;
    use std::sync::Mutex;
    static CACHE: OnceLock<Mutex<HashMap<String, &'static Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    // Mutex poison means a previous holder panicked while mutating the
    // map. The validator can't recover from a corrupt cache; propagate.
    #[allow(clippy::unwrap_used, reason = "mutex poison is unrecoverable here")]
    let mut guard = cache.lock().unwrap();
    if let Some(re) = guard.get(pattern) {
        return re;
    }
    // `filename_pattern` is author-supplied in the ontology — a typo
    // there is a deployment-time error surfaced via the message, not a
    // runtime condition we can paper over.
    #[allow(
        clippy::expect_used,
        reason = "ontology author error; expect message documents fix"
    )]
    let re: &'static Regex = Box::leak(Box::new(
        Regex::new(pattern).expect("invalid filename_pattern in ontology — author error"),
    ));
    guard.insert(pattern.to_string(), re);
    re
}

/// Interrogation-004 follow-up: detect a YAML scalar that's
/// silently truncated by an unquoted ` #` (comment start).
/// Returns `Some((field, 1-based line number))` on a hit.
///
/// Detection rules:
/// - Line must look like `<key>: <value>`. Indentation / list
///   prefixes (`- `) are ignored — those values aren't simple
///   scalars and have their own quoting story.
/// - Value must NOT start with `"`, `'`, `[`, `{`, or `>` /
///   `|` (block scalars are fine because `#` inside a block is
///   not a comment).
/// - Value must contain a ` #` (space-preceded `#`). `(#1)` —
///   no preceding space inside the parens — is NOT a comment
///   and is safe.
fn yaml_scalar_truncated_by_comment(line: &str, idx_zero_based: u32) -> Option<(String, u32)> {
    let trimmed = line.trim_start();
    // Skip list items, comments, dashes, empty lines.
    if trimmed.starts_with('#')
        || trimmed.starts_with('-')
        || trimmed.starts_with("---")
        || trimmed.is_empty()
    {
        return None;
    }
    let (key, value) = trimmed.split_once(':')?;
    // Key must be a plain identifier so we don't match `URL: https://example.com#frag` (no `#` rule there, but the heuristic stays tight).
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return None;
    }
    let value = value.trim_start();
    // Already quoted or a block scalar — YAML rules don't apply.
    if value.starts_with('"')
        || value.starts_with('\'')
        || value.starts_with('>')
        || value.starts_with('|')
        || value.starts_with('[')
        || value.starts_with('{')
    {
        return None;
    }
    // Flag the space-preceded `#` (comment-start trigger) OR the
    // mid-string `: ` (colon-space, which YAML 1.2 reads as the
    // start of a new mapping key — see the design-corpus iter
    // 46-47 finding where 6 audit docs were silently dropped from
    // the live DB because their unquoted summaries contained
    // markdown-style phrases like `complementarity: outbound`).
    // Both cases truncate the parsed scalar; the fix is the same
    // (quote the value), so a single Issue carries both
    // semantics.
    if value.contains(" #") || value.contains(": ") {
        return Some((key.to_string(), idx_zero_based + 1));
    }
    None
}

#[cfg(test)]
mod yaml_scalar_truncated_tests {
    use super::yaml_scalar_truncated_by_comment;

    /// Interrogation-004's exact reproduction case: an unquoted
    /// summary with a `since #225)` reference. Must flag.
    #[test]
    fn flags_unquoted_summary_with_space_hash() {
        let line = "summary: Prose linter doc-linter integrates with Vale (vale_enabled = true since #225) for in-corpus vocab-closure checks.";
        let hit = yaml_scalar_truncated_by_comment(line, 4);
        assert_eq!(hit, Some(("summary".to_string(), 5)));
    }

    /// `(#1)` (no leading space inside the parens) is NOT a YAML
    /// comment start — YAML requires whitespace before `#`. The
    /// detector must let this through to avoid spurious lints.
    #[test]
    fn passes_paren_hash_without_leading_space() {
        let line = "summary: mold linker(#1) lands the first speedup";
        let hit = yaml_scalar_truncated_by_comment(line, 0);
        assert_eq!(hit, None);
    }

    #[test]
    fn passes_quoted_value_even_with_space_hash() {
        let line = "summary: \"PR #225 is the change in question\"";
        let hit = yaml_scalar_truncated_by_comment(line, 0);
        assert_eq!(hit, None);
    }

    #[test]
    fn passes_single_quoted_value() {
        let line = "summary: 'fix in PR #225 for #226'";
        let hit = yaml_scalar_truncated_by_comment(line, 0);
        assert_eq!(hit, None);
    }

    #[test]
    fn passes_list_items_and_comments() {
        assert!(yaml_scalar_truncated_by_comment("- item with # not key", 0).is_none());
        assert!(yaml_scalar_truncated_by_comment("# a pure comment line", 0).is_none());
        assert!(yaml_scalar_truncated_by_comment("---", 0).is_none());
        assert!(yaml_scalar_truncated_by_comment("", 0).is_none());
    }

    #[test]
    fn passes_key_with_no_value_or_no_hash() {
        assert!(yaml_scalar_truncated_by_comment("summary:", 0).is_none());
        assert!(
            yaml_scalar_truncated_by_comment("summary: a normal sentence with no hash", 0)
                .is_none()
        );
    }

    /// Iter 46-47 finding: the exact reproduction case where an
    /// unquoted summary contains a markdown-style `complementarity:
    /// outbound wikilinks` phrase — YAML parses the second `: `
    /// as a new mapping key, breaking the frontmatter and silently
    /// dropping the file from ingest. Must flag.
    #[test]
    fn flags_unquoted_summary_with_colon_space() {
        let line = "summary: Edge structure on entity-vale confirms explicit complementarity: outbound wikilinks to entity-doc-linter.";
        let hit = yaml_scalar_truncated_by_comment(line, 6);
        assert_eq!(hit, Some(("summary".to_string(), 7)));
    }

    /// Markdown backticks around `role: adr` inside an unquoted
    /// scalar — YAML doesn't know about backticks; it sees the
    /// colon-space and pivots. Must flag.
    #[test]
    fn flags_unquoted_summary_with_backticked_colon_space() {
        let line = "summary: The corpus has ZERO `role: adr` instances and ZERO SUPERSEDES edges.";
        let hit = yaml_scalar_truncated_by_comment(line, 0);
        assert_eq!(hit, Some(("summary".to_string(), 1)));
    }

    /// Single-quoted summary containing `: ` is safe (the previous
    /// `passes_single_quoted_value` test only covered `#`).
    #[test]
    fn passes_single_quoted_value_with_colon_space() {
        let line = "summary: 'Edge structure on entity-vale confirms complementarity: outbound wikilinks.'";
        let hit = yaml_scalar_truncated_by_comment(line, 0);
        assert_eq!(hit, None);
    }

    /// `key: value: extra` (a value that itself contains `: `) is
    /// the trigger — but a value of just `value` is fine even if
    /// the LINE has a colon (which all key:value lines do). The
    /// detector should NOT flag the trivial `key: just-a-value`
    /// shape.
    #[test]
    fn passes_value_with_single_colon_no_space_after() {
        let line = "url: https://example.com/path:fragment";
        let hit = yaml_scalar_truncated_by_comment(line, 0);
        assert_eq!(hit, None);
    }
}

#[cfg(test)]
mod slug_tests {
    use super::slugs_equal;

    #[test]
    fn matches_case_and_separator_variants() {
        assert!(slugs_equal("BUILD_SUMMARY", "build-summary"));
        assert!(slugs_equal("SESSION_STATE", "session-state"));
        assert!(slugs_equal("Contributing", "contributing"));
        assert!(slugs_equal("my-doc", "my_doc"));
    }

    #[test]
    fn rejects_typos_and_missing_words() {
        // Missing word (the bug we want to keep catching).
        assert!(!slugs_equal(
            "HANDOFF_TO_ORIGINAL_SESSION",
            "handoff-original-session"
        ));
        assert!(!slugs_equal("foo-bar", "foo-baz"));
    }
}
