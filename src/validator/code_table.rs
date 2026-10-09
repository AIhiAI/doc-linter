//! Canonical (code → meaning → fix) table for every [`super::Issue`]
//! variant — the single source of truth that backs the
//! `doc-linter codes --markdown` subcommand (#58) and the generated
//! `docs/error-codes.md`. Pulling these strings out of README / USAGE
//! / AGENTS into one Rust-side constant means the four-way drift
//! flagged in #58 cannot happen — `Issue::code()` and
//! [`ERROR_CODE_TABLE`] are kept in lock-step by the unit test below
//! (every variant code must have a row; no row may reference a code
//! that isn't a real variant).
//!
//! The `fix:` column is deliberately a single sentence — the per-
//! instance Display impl in [`super::messages`] already produces a
//! site-specific suggestion; the table value is the variant-level
//! one-liner that goes into the docs.

/// One row in the canonical error-code table.
///
/// `code` matches [`super::Issue::code()`]; `meaning` is the
/// variant-level description (the "what just fired"); `fix` is the
/// short imperative hint (the "what to do next"). Both strings are
/// `'static` so the table is a `const` slice.
#[derive(Debug, Clone, Copy)]
pub struct ErrorCodeEntry {
    pub code: &'static str,
    pub meaning: &'static str,
    pub fix: &'static str,
}

/// Every [`super::Issue`] variant, in code order. Used by
/// `doc-linter codes --markdown` to emit `docs/error-codes.md` and by
/// the [`code_table_matches_issue_variants`] tripwire to prevent
/// drift between [`super::Issue::code()`] and this list.
pub const ERROR_CODE_TABLE: &[ErrorCodeEntry] = &[
    ErrorCodeEntry {
        code: "missing-frontmatter",
        meaning: "The file has no `---\\n…\\n---` YAML block.",
        fix: "Add a frontmatter block with `id`, `role`, `title`, `summary`, `status`, `updated`.",
    },
    ErrorCodeEntry {
        code: "parse-error",
        meaning: "The frontmatter YAML failed to parse.",
        fix: "Inspect the YAML for indentation, unquoted special characters, or a missing colon.",
    },
    ErrorCodeEntry {
        code: "missing-field",
        meaning: "A field listed in `required_fields` is absent from this doc's frontmatter.",
        fix: "Add the named field, or remove it from `required_fields` if the doc-set no longer needs it.",
    },
    ErrorCodeEntry {
        code: "yaml-scalar-truncated-by-comment",
        meaning: "An unquoted YAML scalar contains space-preceded `#`, which YAML 1.2 parses as a comment start — the value is silently truncated at that position.",
        fix: "Wrap the value in double quotes (or single quotes if the text contains `\"`). Common trigger: PR / issue references like `since #225` in unquoted summaries.",
    },
    ErrorCodeEntry {
        code: "legacy-type-field",
        meaning: "The doc uses the pre-ontology `type:` field instead of `role:` / `kind:` / `lifecycle:`.",
        fix: "Run `doc-linter migrate-ontology` or rewrite the frontmatter to use the current axes.",
    },
    ErrorCodeEntry {
        code: "unknown-role",
        meaning: "The `role:` value doesn't match any value doc under `docs/ontology/values/role/`.",
        fix: "Either add a value doc at `docs/ontology/values/role/<name>.md` or correct the role to one of the registered values.",
    },
    ErrorCodeEntry {
        code: "unknown-axis-value",
        meaning: "An axis value (e.g. `kind:` / `lifecycle:`) doesn't match its axis vocabulary.",
        fix: "Add a value doc for the new axis term, or pick one of the existing values.",
    },
    ErrorCodeEntry {
        code: "missing-axis-field",
        meaning: "The doc's role requires an axis (e.g. `kind` for `role: doc`) that isn't declared.",
        fix: "Add the missing axis to the frontmatter, or change `role:` to one that doesn't require it.",
    },
    ErrorCodeEntry {
        code: "lifecycle-not-allowed-for-role",
        meaning: "The combination of `role:` and `lifecycle:` is rejected by the ontology.",
        fix: "Pick a lifecycle value the role allows, or extend the axis vocabulary to permit this pairing.",
    },
    ErrorCodeEntry {
        code: "filename-pattern-mismatch",
        meaning: "The filename doesn't match the role's required regex (e.g. `roadmap-entry` requires `^\\d+-.*\\.md$`).",
        fix: "Rename the file to match.",
    },
    ErrorCodeEntry {
        code: "wrong-folder",
        meaning: "The doc matched a `[[layout_rules]]` entry but lives outside its `allowed_paths`.",
        fix: "`git mv` the file to the suggested path, or extend the layout entry's `allowed_paths`.",
    },
    ErrorCodeEntry {
        code: "unknown-entity",
        meaning: "A `covers:` value isn't a registered entity.",
        fix: "Add `docs/ontology/entities/<id>.md`, or correct the spelling to an existing entity id.",
    },
    ErrorCodeEntry {
        code: "unknown-bounded-context",
        meaning: "The `bounded_context:` value isn't a registered value doc under `docs/ontology/values/bounded-context/`.",
        fix: "Add a value doc for the new context, or pick one of the existing values.",
    },
    ErrorCodeEntry {
        code: "wrong-bounded-context-field",
        meaning: "An entity wrote `bounded_context:` or a non-entity doc wrote `bounded_contexts:`; the value is ignored.",
        fix: "Entities use the `bounded_contexts: [..]` list, every other doc the `bounded_context: <value>` string. Rename the field.",
    },
    ErrorCodeEntry {
        code: "invalid-status",
        meaning: "`status:` isn't in `allowed_statuses` (default: draft / stable / archived / deprecated / candidate).",
        fix: "Pick an allowed status, or extend `allowed_statuses` in `.doc-lint.toml`.",
    },
    ErrorCodeEntry {
        code: "invalid-visibility",
        meaning: "`visibility:` isn't in `allowed_visibility` (default: public / internal / private).",
        fix: "Pick an allowed visibility, or extend `allowed_visibility` in `.doc-lint.toml`.",
    },
    ErrorCodeEntry {
        code: "updated-in-future",
        meaning: "`updated:` is a date strictly later than today.",
        fix: "Set `updated:` to today or earlier — future-dating defeats staleness queries.",
    },
    ErrorCodeEntry {
        code: "duplicate-id",
        meaning: "Two docs declare the same `id:`.",
        fix: "Rename one of the docs' `id:` fields; the linker resolves wikilinks by id and a collision breaks every reference.",
    },
    ErrorCodeEntry {
        code: "id-mismatch",
        meaning: "`id:` doesn't match the doc's filename stem.",
        fix: "Rename the file or change `id:` so the two agree.",
    },
    ErrorCodeEntry {
        code: "broken-wikilink",
        meaning: "`[[target]]` doesn't resolve to a doc id or filename stem.",
        fix: "Fix the target id, or add the missing doc.",
    },
    ErrorCodeEntry {
        code: "broken-md-link",
        meaning: "A markdown link `[label](path.md)` points at a non-existent doc.",
        fix: "Fix the path — common cause: file moved and the link wasn't updated.",
    },
    ErrorCodeEntry {
        code: "broken-file-link",
        meaning: "A non-`.md` link target doesn't exist on disk.",
        fix: "Fix the path, or add the file the link references.",
    },
    ErrorCodeEntry {
        code: "broken-typed-edge",
        meaning: "A typed-edge frontmatter field (e.g. `depends_on:`, `informed_by:`, `supersedes:`) references an unknown doc id.",
        fix: "Correct the target id, or remove the typed edge if the relationship no longer holds.",
    },
    ErrorCodeEntry {
        code: "unlinked-path",
        meaning: "Inline backtick text looks like a file path but isn't a markdown link.",
        fix: "Convert to `[label](path)`, or add the text to `unlinked_path_exempt` if it's a convention reference.",
    },
    ErrorCodeEntry {
        code: "orphan-doc",
        meaning: "A doc with frontmatter has no inbound or outbound graph edges.",
        fix: "Link to/from at least one other doc, or set `lifecycle: archived` if it's truly standalone.",
    },
    ErrorCodeEntry {
        code: "vale-missing",
        meaning: "The `vale` binary isn't on PATH; vocab-closure was skipped for this run.",
        fix: "Install Vale (`brew install vale` / `cargo install vale`), or set `vale_enabled = false` to silence.",
    },
    ErrorCodeEntry {
        code: "vocab-dictionary-unreadable",
        meaning: "A `vale_dictionaries` entry points at a file that can't be read; its terms are missing from every vocab accept list.",
        fix: "Fix the path in `.doc-lint.toml` (it's relative to the repo root) or remove the entry.",
    },
    ErrorCodeEntry {
        code: "asciidoctor-missing",
        meaning: "The corpus has `.adoc` docs but `asciidoctor` isn't on PATH; Vale needs it to read AsciiDoc, so those docs skipped vocab-closure.",
        fix: "Install Asciidoctor (`gem install asciidoctor` / `brew install asciidoctor`), or pass --no-vale.",
    },
    ErrorCodeEntry {
        code: "vale-failed",
        meaning: "`vale` is installed but exited with a runtime error, so vocab-closure did not run.",
        fix: "Run the `vale` command from the message by hand and fix what it reports (bad style, unreadable file), or pass --no-vale.",
    },
    ErrorCodeEntry {
        code: "vale-alert",
        meaning: "A Vale check fired (one of `Vocabulary.*`, `Style.*`, etc.) that the post-processor didn't downgrade.",
        fix: "Fix the prose, add the term to `vale_extra_accept`, or extend Vale's vocab dictionaries.",
    },
    ErrorCodeEntry {
        code: "cross-context-reference",
        meaning: "A bare noun resolved to an entity in a different bounded context than this doc's.",
        fix: "Fully qualify with `[[entity-id]]`, rephrase to a non-colliding term, or move the doc to the matching context.",
    },
    ErrorCodeEntry {
        code: "comment-vocab-violation",
        meaning: "A capitalised token in a `///` doc-comment doesn't resolve to a known entity.",
        fix: "Add the entity, rephrase the comment, or add the term to `code_comment_extra_accept`.",
    },
    ErrorCodeEntry {
        code: "missing-anchor",
        meaning: "A public-API doc-comment in an `anchor_required_in` crate doesn't reference any ontology entity or roadmap id.",
        fix: "Add a `[[entity-foo]]` (or `[[roadmap-N]]`) wikilink to the comment.",
    },
    ErrorCodeEntry {
        code: "scip-indexer-missing",
        meaning: "`rust-analyzer` isn't on PATH; the SCIP index step was skipped.",
        fix: "Install `rust-analyzer`, or set `scip_required = false` to silence.",
    },
    ErrorCodeEntry {
        code: "scip-stale",
        meaning: "`<root>/.doc-lint/code.scip` is older than the newest `.rs` file.",
        fix: "Run `doc-linter scip-index` (or rebuild the index manually).",
    },
    ErrorCodeEntry {
        code: "scip-missing",
        meaning: "`scip_required = true` but `<root>/.doc-lint/code.scip` is absent.",
        fix: "Run `doc-linter scip-index`, or set `scip_required = false`.",
    },
    ErrorCodeEntry {
        code: "self-loop",
        meaning: "A doc declares a typed-edge frontmatter field that points at itself.",
        fix: "Remove the self-reference — typed edges always cross between docs.",
    },
    ErrorCodeEntry {
        code: "superseded-without-successor",
        meaning: "A doc with `lifecycle: superseded` has no `supersedes:` field pointing at its replacement.",
        fix: "Add `supersedes: [<successor-id>]`, or change the lifecycle to `deprecated` / `archived`.",
    },
    ErrorCodeEntry {
        code: "dark-public-function",
        meaning: "A `Function` whose crate is in `anchor_required_in` has neither a `///` doc-comment nor any `FUNCTION_MENTIONS` edge.",
        fix: "Add a doc-comment that mentions an ontology entity, or move the function out of the listed crate.",
    },
    ErrorCodeEntry {
        code: "dark-endpoint",
        meaning: "An `Endpoint` (axum / clap / mcp) has no `ENDPOINT_TOUCHES_ENTITY` edge — its handler doc-comment doesn't reach the ontology.",
        fix: "Add an entity reference to the handler's doc-comment, or set `covers:` on the handler crate's README.",
    },
    ErrorCodeEntry {
        code: "entity-coverage-gap",
        meaning: "An entity's `doc:func` ratio is strictly below `coverage_min_per_entity_doc_ratio`.",
        fix: "Author more covering docs, expand the entity's `synonyms:` list, or add the id to `coverage_entity_exempt`.",
    },
    ErrorCodeEntry {
        code: "unauthored-cluster",
        meaning: "LPA over Function+CALLS discovered a community of related functions whose `suggested_id` doesn't match any registered ontology entity. The code is organising around a concept the vocabulary hasn't captured yet.",
        fix: "`doc-linter cluster --write` scaffolds a candidate stub; review summary + synonyms, then `doc-linter cluster --promote <id>` flips it to `status: stable`. Or add the id as a synonym on an existing entity. Or add it to `cluster_lint.ignore`.",
    },
    ErrorCodeEntry {
        code: "coverage-below-min",
        meaning: "The corpus-wide function-reach percentage is below the `--coverage-min-global` flag (or `coverage_min_global` knob).",
        fix: "Close the gap by anchoring more functions to entities, or lower the threshold.",
    },
    ErrorCodeEntry {
        code: "orphan-entity",
        meaning: "An entity doc has zero inbound `covers:` or `[[entity-<id>]]` references (Roadmap-48 Rule C). Strict mode (`orphan_entity_requires_covers = true`) rejects wikilinks.",
        fix: "Author a narrative doc whose `covers:` includes this entity, or remove the entity if it's no longer used.",
    },
    ErrorCodeEntry {
        code: "missing-covers",
        meaning: "A `role: doc` declares no `covers:`. Opt-in via `require_covers_per_doc`.",
        fix: "Add `covers: [<entity-id>]` listing the entities this doc explains, or set `lifecycle: archived`.",
    },
    ErrorCodeEntry {
        code: "missing-bounded-context",
        meaning: "A `role: doc` has no `bounded_context:`. Opt-in via `require_bounded_context_per_doc`.",
        fix: "Add `bounded_context: <value>` to the frontmatter, or extend `bounded_context_paths` so it's inferred by path prefix.",
    },
    ErrorCodeEntry {
        code: "placeholder-entity",
        meaning: "The `example` entity scaffolded by `doc-linter init` is still present verbatim.",
        fix: "Replace the placeholder with a real entity, or delete `docs/ontology/entities/example.md` once the ontology has at least one authored entity.",
    },
    ErrorCodeEntry {
        code: "homepage-missing",
        meaning: "The configured `homepage_path` (default `MAP.md`) doesn't exist. Opt-in via `require_homepage`.",
        fix: "Run `doc-linter homepage --write` to generate it.",
    },
    ErrorCodeEntry {
        code: "homepage-stale",
        meaning: "The on-disk homepage file diverges from what `doc-linter homepage` would produce.",
        fix: "Run `doc-linter homepage --write`.",
    },
    ErrorCodeEntry {
        code: "missing-body-wikilink",
        meaning: "An entity declared in a doc's `covers:` isn't cited as `[[entity-<id>]]` in the body. Opt-in via `require_body_wikilink_per_cover`.",
        fix: "Add a `[[entity-<id>]]` mention to the body where the concept is first discussed.",
    },
    ErrorCodeEntry {
        code: "unused-bounded-context",
        meaning: "A `bounded-context` axis value is declared but no doc claims it. Opt-in via `require_bounded_context_usage`.",
        fix: "Author a doc with `bounded_context: <value>`, add the value to `bounded_context_usage_exempt`, or remove the value doc.",
    },
];

/// Renders [`ERROR_CODE_TABLE`] as a GitHub-flavored Markdown table.
/// Used by the `doc-linter codes --markdown` subcommand to (re)generate
/// `docs/error-codes.md`. The exact output is byte-stable so the file
/// stays diff-clean when nothing has changed.
///
/// Emits doc-linter-shaped YAML frontmatter so the regenerated file
/// passes the linter unchanged — without it, every regeneration would
/// strip the frontmatter and trip `missing-frontmatter` on the next
/// `check`. The `updated:` date is a fixed bootstrap value (callers
/// who care about staleness should bump it in a follow-up commit, the
/// same way they would for a hand-edited reference doc).
pub fn render_markdown() -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str("id: error-codes\n");
    s.push_str("role: doc\n");
    s.push_str("kind: reference\n");
    s.push_str("lifecycle: stable\n");
    s.push_str("covers: [doc-graph, endpoint]\n");
    s.push_str("title: Diagnostic codes\n");
    s.push_str(
        "summary: Generated reference table mapping every doc-linter diagnostic code to its \
         meaning and canonical fix. Source of truth is `src/validator/code_table.rs`; regenerate \
         via `doc-linter codes --markdown > docs/error-codes.md`.\n",
    );
    s.push_str("status: stable\n");
    s.push_str("updated: 2026-05-30\n");
    s.push_str("tags: [reference, diagnostics]\n");
    s.push_str("---\n\n");
    s.push_str("<!-- This file is generated by `doc-linter codes --markdown`. -->\n");
    s.push_str("<!-- Edit src/validator/code_table.rs and re-run, not this file. -->\n\n");
    s.push_str("# Diagnostic codes\n\n");
    s.push_str(
        "Every code the linter emits, paired with what it means and the canonical fix. \
         The CLI / LSP / JSON output uses the `Code` column verbatim; consumers can \
         match on these stable identifiers (with a `vale-` prefix on Vale-check alerts, \
         e.g. `vale-vocabulary-ambiguousbare`).\n\n",
    );
    s.push_str("| Code | Meaning | Fix |\n");
    s.push_str("|---|---|---|\n");
    for e in ERROR_CODE_TABLE {
        s.push_str(&format!("| `{}` | {} | {} |\n", e.code, e.meaning, e.fix));
    }
    s
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::ERROR_CODE_TABLE;
    use std::collections::HashSet;

    /// Tripwire: every code in [`ERROR_CODE_TABLE`] must be unique, and
    /// the full set must line up 1:1 with the [`super::super::Issue`]
    /// variants surfaced via [`super::super::Issue::code()`]. A new
    /// variant added without a table row (or a row added without a
    /// matching variant) trips this test.
    #[test]
    fn code_table_matches_issue_variants() {
        // 1. No duplicates.
        let mut seen: HashSet<&str> = HashSet::new();
        for e in ERROR_CODE_TABLE {
            assert!(
                seen.insert(e.code),
                "duplicate code in ERROR_CODE_TABLE: {}",
                e.code
            );
        }

        // 2. The set matches the codes the `Issue::code()` match-arm
        // produces. We extract the codes by parsing the source file —
        // there's no enumeration helper on `Issue` itself, and
        // hand-rolling one would be its own drift risk. Failures here
        // mean either:
        //   - a new variant was added with `code()` returning a new
        //     value but no ERROR_CODE_TABLE row;
        //   - or a row was removed without removing the variant
        //     (would be caught by `Issue::code()` exhaustiveness anyway).
        let issue_src = include_str!("issue.rs");
        let mut issue_codes: HashSet<String> = HashSet::new();
        for line in issue_src.lines() {
            let trimmed = line.trim();
            // Match lines like: `Issue::Foo { .. } => "kebab-name",`
            if let Some(rest) = trimmed.strip_prefix("Issue::") {
                if let Some(arrow_pos) = rest.find("=>") {
                    let after_arrow = rest[arrow_pos + 2..].trim();
                    if let Some(s) = after_arrow.strip_prefix('"') {
                        if let Some(end) = s.find('"') {
                            issue_codes.insert(s[..end].to_string());
                        }
                    }
                }
            }
        }
        assert!(
            !issue_codes.is_empty(),
            "failed to extract any Issue::code() values from issue.rs — parser regression"
        );

        let table_codes: HashSet<String> = ERROR_CODE_TABLE
            .iter()
            .map(|e| e.code.to_string())
            .collect();

        let missing_in_table: Vec<&String> = issue_codes.difference(&table_codes).collect();
        let unknown_in_table: Vec<&String> = table_codes.difference(&issue_codes).collect();

        assert!(
            missing_in_table.is_empty(),
            "Issue variants missing from ERROR_CODE_TABLE: {missing_in_table:?}"
        );
        assert!(
            unknown_in_table.is_empty(),
            "ERROR_CODE_TABLE rows reference unknown codes: {unknown_in_table:?}"
        );
    }

    #[test]
    fn render_markdown_includes_header_and_row_count() {
        let md = super::render_markdown();
        assert!(md.contains("# Diagnostic codes"));
        assert!(md.contains("| Code | Meaning | Fix |"));
        // One row per table entry.
        let body_rows = md.lines().filter(|l| l.starts_with("| `")).count();
        assert_eq!(body_rows, ERROR_CODE_TABLE.len());
    }

    /// The rendered file is checked into `docs/error-codes.md`, so the
    /// renderer has to emit doc-linter-shaped frontmatter — otherwise
    /// every regeneration would strip it and trip `missing-frontmatter`.
    #[test]
    fn render_markdown_emits_frontmatter() {
        let md = super::render_markdown();
        assert!(
            md.starts_with("---\n"),
            "render output must open with frontmatter"
        );
        assert!(md.contains("\nid: error-codes\n"));
        assert!(md.contains("\nrole: doc\n"));
        assert!(md.contains("\nkind: reference\n"));
        // Frontmatter terminator precedes the body header.
        let fm_end = md.find("\n---\n").expect("frontmatter must close");
        let header_pos = md
            .find("# Diagnostic codes")
            .expect("body header must appear");
        assert!(fm_end < header_pos);
    }
}
