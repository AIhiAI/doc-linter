//! Author-facing message catalog for every [`Issue`] variant.
//!
//! Co-located here (instead of inside [`super::issue`]) so the
//! agent-nudge message rewrite in #42 — which retunes every diagnostic
//! to nudge agents toward growing the docs rather than removing the
//! offending content — is a single-file change.

use super::issue::Issue;
use std::fmt;

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Issue::MissingFrontmatter => write!(
                f,
                "missing frontmatter — add `---\\n...\\n---` block at the top with id/role/title/summary/status/updated"
            ),
            Issue::ParseError(msg) => write!(f, "frontmatter parse error: {msg}"),
            Issue::MissingField(field) => write!(f, "missing required field '{field}'"),
            Issue::YamlScalarLooksTruncated { field, line } => write!(
                f,
                "frontmatter `{field}` (line {line}) looks truncated by YAML: an unquoted \
                 scalar that contains either space-preceded `#` (a PR/issue reference YAML 1.2 \
                 treats as a comment start) OR a mid-string `: ` (colon-space, which YAML reads \
                 as the start of a new mapping key) silently truncates at that position. \
                 Wrap the value in double quotes (or single quotes if the text contains `\"`); \
                 escape internal apostrophes as `''` in single-quoted strings."
            ),
            Issue::LegacyTypeField { got } => write!(
                f,
                "legacy `type: {got}` — replace with `role:` (+ `kind:`/`lifecycle:` per the ontology). See docs/ontology/migrations/0001-initial.md"
            ),
            Issue::UnknownRole { got } => write!(
                f,
                "role '{got}' is not registered in the ontology — add a value doc under docs/ontology/values/role/ or correct the role"
            ),
            Issue::UnknownAxisValue { axis, got } => write!(
                f,
                "{axis} '{got}' is not registered in the {axis} vocabulary. \
                 Preferred fix: add `docs/ontology/values/{axis}/{got}.md` and bump the \
                 ontology migration to record the addition. \
                 Only correct the value to an existing one if '{got}' was a typo — \
                 run `doc-linter ontology` to list current values."
            ),
            Issue::MissingAxisField { role, axis } => write!(
                f,
                "role '{role}' requires `{axis}:` to be set in frontmatter"
            ),
            Issue::LifecycleNotAllowedForRole { role, got, allowed } => write!(
                f,
                "role '{role}' allows lifecycle values {allowed:?}; got '{got}'"
            ),
            Issue::FilenamePatternMismatch { role, got, pattern } => write!(
                f,
                "role '{role}' requires filenames matching `{pattern}`; '{got}' does not match"
            ),
            Issue::WrongFolder { role, kind, lifecycle, expected, got } => {
                let mut who = String::new();
                if let Some(r) = role { who.push_str(&format!("role={r}")); }
                if let Some(k) = kind {
                    if !who.is_empty() { who.push_str(", "); }
                    who.push_str(&format!("kind={k}"));
                }
                if let Some(l) = lifecycle {
                    if !who.is_empty() { who.push_str(", "); }
                    who.push_str(&format!("lifecycle={l}"));
                }
                if who.is_empty() { who.push_str("layout rule"); }
                write!(
                    f,
                    "{who} requires the doc to live under one of {expected:?}; got `{}` — `git mv` to a matching path or update the rule's allowed_paths",
                    got.display()
                )
            }
            Issue::UnknownEntity { got } => write!(
                f,
                "covers '{got}' is not a registered entity. \
                 Preferred fix: add `docs/ontology/entities/{got}.md` declaring the \
                 entity, then bump the ontology migration to record the addition. \
                 Only change the id to an existing entity if '{got}' was a misspelling — \
                 run `doc-linter ontology` to see registered ids."
            ),
            Issue::UnknownBoundedContext { got } => write!(
                f,
                "bounded-context '{got}' is not registered in the ontology — add docs/ontology/values/bounded-context/{got}.md (+ migration bump) or correct the id"
            ),
            Issue::WrongBoundedContextField { got, expected } => write!(
                f,
                "`{got}:` is ignored on this doc — rename it to `{expected}:` \
                 (entities take the `bounded_contexts:` list, every other doc the `bounded_context:` string)"
            ),
            Issue::InvalidStatus { got } => write!(
                f,
                "status '{got}' is not in the allowed list (draft, stable, archived, deprecated)"
            ),
            Issue::InvalidVisibility { got, allowed } => write!(
                f,
                "visibility '{got}' is not in the allowed list {allowed:?} — see .doc-lint.toml `allowed_visibility`"
            ),
            Issue::UpdatedInFuture { date } => {
                write!(f, "updated: {date} is in the future")
            }
            Issue::DuplicateId { id, other } => {
                write!(f, "duplicate id '{id}' (also used by {})", other.display())
            }
            Issue::IdDoesNotMatchStem { id, stem } => write!(
                f,
                "id '{id}' does not match filename stem '{stem}' — rename file or update id"
            ),
            Issue::BrokenWikilink { target, line } => write!(
                f,
                "line {line}: wikilink [[{target}]] declares intent the concept should have a navigable doc. \
                 Preferred fix: create `docs/explanations/{target}.md` (or a kind that matches \
                 the concept) with frontmatter `id: {target}`. Only remove the wikilink if \
                 `{target}` genuinely shouldn't be in the doc graph."
            ),
            Issue::BrokenMdLink { target, line } => write!(
                f,
                "line {line}: markdown link to `{target}` declares intent the target should exist. \
                 Preferred fix: create `{target}` with appropriate frontmatter (a moved/renamed \
                 file is the common cause — point the link at the new path instead). Only remove \
                 the link if the material genuinely shouldn't be linked."
            ),
            Issue::BrokenFileLink { target, line } => write!(
                f,
                "line {line}: markdown link to `{target}` — file does not exist. \
                 Usually a moved/renamed file; preferred fix: update the path to the current \
                 location. Prefer creating a replacement file over silently deleting the link."
            ),
            Issue::BrokenTypedEdge { field, target } => write!(
                f,
                "frontmatter `{field}: {target}` references a doc that doesn't exist yet. \
                 Preferred fix: create the missing doc so the typed relationship resolves. \
                 Only remove the `{field}:` entry if the relationship was authored in error."
            ),
            Issue::UnlinkedPath { text, line } => write!(
                f,
                "line {line}: inline `{text}` looks like a file path but is not a markdown link — convert to `[label]({text})` so the linter can verify the target exists"
            ),
            Issue::OrphanDoc => write!(
                f,
                "orphan doc — no inbound or outbound edges; link it from a `## Related` section or frontmatter typed edge, or mark status: archived"
            ),
            Issue::ValeNotInstalled => write!(
                f,
                "vale binary not on PATH — vocabulary-closure check skipped. Install: https://vale.sh/docs/install (or set `vale_enabled = false` in .doc-lint.toml / pass --no-vale to silence this diagnostic)"
            ),
            Issue::VocabDictionaryUnreadable { name, path } => write!(
                f,
                "vale_dictionaries.{name} points at {}, which can't be read — its terms are \
                 missing from every vocabulary accept list. Fix the path or remove the entry.",
                path.display()
            ),
            Issue::AsciidoctorMissing => write!(
                f,
                "asciidoctor not on PATH — .adoc docs were left out of the Vale vocabulary-closure check. \
                 Install: https://docs.asciidoctor.org/asciidoctor/latest/install/"
            ),
            Issue::ValeFailed { message } => write!(
                f,
                "vale ran but failed — vocabulary-closure check did not run: {message}"
            ),
            Issue::ValeAlert {
                check,
                message,
                line,
                severity,
            } => write!(
                f,
                "line {line}: vale[{severity}] {check}: {message}"
            ),
            Issue::CrossContextReference {
                term,
                entity_id,
                entity_context,
                doc_context,
                line,
            } => write!(
                f,
                "line {line}: term '{term}' refers to {entity_id} (context: {entity_context}) but this doc is in context: {doc_context} — fully qualify as `[[{entity_id}]]` or move the doc"
            ),
            Issue::CommentVocabViolation {
                file,
                line,
                col,
                term,
            } => write!(
                f,
                "{}:{line}:{col}: comment vocab — term '{term}' is not in the ontology vocabulary",
                file.display()
            ),
            Issue::MissingAnchor { file, line, symbol } => write!(
                f,
                "{}:{line}: doc comment on `{symbol}` does not reference any ontology entity or roadmap-entry id — add a `[[entity-id]]` or `[[roadmap-…]]` link",
                file.display()
            ),
            Issue::ScipIndexerMissing => write!(
                f,
                "rust-analyzer (or another SCIP indexer) not on PATH — Function ingest skipped"
            ),
            Issue::ScipFileStale { path, age_days } => write!(
                f,
                "SCIP file `{path}` is {age_days} day(s) older than the source — re-run the indexer"
            ),
            Issue::ScipFileMissing { path } => write!(
                f,
                "SCIP file `{path}` is required but does not exist"
            ),
            Issue::SelfLoop { edge_kind, line } => {
                if *line > 0 {
                    write!(
                        f,
                        "line {line}: {} edge points back at this doc — drop the self-link or fix the target id",
                        edge_kind.as_str()
                    )
                } else {
                    write!(
                        f,
                        "frontmatter `{}` edge points back at this doc — remove the self-reference",
                        edge_kind.as_str()
                    )
                }
            }
            Issue::SupersededWithoutSuccessor => write!(
                f,
                "lifecycle: superseded but no doc declares `supersedes:` for this id — name the successor (or change the lifecycle, or move the doc to docs/archive/)"
            ),
            Issue::DarkPublicFunction {
                crate_name,
                file,
                line,
                symbol,
            } => write!(
                f,
                "{file}:{line}: function `{symbol}` (crate `{crate_name}`) has no entity link and no /// doc comment — add a doc comment that mentions an ontology entity, or remove `{crate_name}` from `anchor_required_in`. (anchor_required_in is currently active for this crate.)"
            ),
            Issue::DarkEndpoint {
                kind,
                method,
                path,
                file,
                line,
            } => write!(
                f,
                "{file}:{line}: endpoint `{kind}:{method}:{path}` has no ontology entity link — its handler's doc-comment must mention at least one entity, OR add a `covers:` field to the handler crate's README."
            ),
            Issue::EntityCoverageGap {
                entity,
                doc_count,
                func_count,
                ratio,
                threshold,
            } => write!(
                f,
                "entity '{entity}' has {doc_count} doc(s) covering it and {func_count} function(s) mentioning it (ratio {ratio:.4}) — under the configured threshold {threshold:.4}. Add more docs that cover '{entity}', expand its `synonyms:` list, or add '{entity}' to `coverage_entity_exempt` if this gap is intentional."
            ),
            Issue::UnauthoredCluster {
                suggested_id,
                god_node_file,
                member_count,
                density,
            } => write!(
                f,
                "code-graph community '{suggested_id}' ({member_count} functions, density {density:.2}, anchored at {god_node_file}) doesn't match any registered ontology entity. \
                 The code is organising around a concept the vocabulary hasn't captured yet. Three resolutions:\n  \
                 1. `doc-linter cluster --write` — scaffold a candidate stub at \
                 docs/ontology/entities/candidates/{suggested_id}.md, then edit summary + synonyms and run `doc-linter cluster --promote {suggested_id}` to flip to `status: stable`.\n  \
                 2. Add an existing entity's synonym that covers this cluster (e.g. if 'embedding' clusters into 'populate-embeddings', add 'populate-embeddings' as a synonym on `entity-embedding.md`).\n  \
                 3. Add '{suggested_id}' to `cluster_lint.ignore` in `.doc-lint.toml` if the cluster is genuinely a tooling concern, not a domain concept."
            ),
            Issue::CoverageBelowMinimum {
                actual_pct,
                threshold_pct,
            } => write!(
                f,
                "non-exempt function-reach is {actual_pct:.2}%, below the configured floor of {threshold_pct:.2}% — close the gap by adding entity-anchored doc-comments, growing the ontology, or adding a justified regex to `coverage_function_exempt` for genuinely-infra functions."
            ),
            Issue::OrphanEntity { id } => write!(
                f,
                "entity-{id} has no narrative doc covering it. The ontology declares the \
                 entity exists; the repo commits to maintaining a doc for it.\n\
                 Preferred fix: add a narrative doc under `docs/explanations/` or \
                 `docs/recipes/` with `covers: [{id}]`. A second-best option is to \
                 wikilink `[[entity-{id}]]` from an existing doc's body so the entity has \
                 inbound connections.\n\
                 Last resort: delete `docs/ontology/entities/{id}.md` AND bump the \
                 ontology migration to record the retirement. Without the migration bump, \
                 the deletion looks like accidental data loss to anyone reviewing the diff."
            ),
            Issue::MissingCovers => write!(
                f,
                "missing `covers:` — this `role: doc` declares no ontology entities. \
                 Preferred fix: add `covers: [<entity-id>, ...]` to frontmatter naming \
                 every entity the doc explains. Run `doc-linter ontology` to list \
                 available entity ids. \
                 Only mark `lifecycle: archived` if the doc is intentionally disconnected \
                 from the live ontology — archived is a real signal to consumers, not a \
                 way to silence the lint."
            ),
            Issue::MissingBoundedContext => write!(
                f,
                "missing `bounded_context:` — this `role: doc` has no bounded-context. \
                 Add `bounded_context: <value>` to frontmatter naming the context this \
                 doc lives in. Pre-req: a `bounded-context` axis must be authored under \
                 `docs/ontology/axes/` with value docs under \
                 `docs/ontology/values/bounded-context/`. Run `doc-linter ontology` to list available values."
            ),
            Issue::PlaceholderEntity { id } => write!(
                f,
                "placeholder entity `{id}` from `doc-linter init` has not been replaced. \
                 Either:\n\
                 \x20 (a) delete docs/ontology/entities/{id}.md if your domain has no such concept, OR\n\
                 \x20 (b) rewrite its `display:`, `description:`, `synonyms:` to reflect a real domain concept, and rename the file + `value_id:` to match. \
                 The unmodified init scaffold counts as ontology debris and should not ship."
            ),
            Issue::HomepageMissing { path } => write!(
                f,
                "homepage file `{path}` is required but does not exist. Run `doc-linter homepage --write` to generate it. The homepage is the machine-shaped index AI agents read to learn the repo's domain concepts, narrative docs, and bounded contexts in one place."
            ),
            Issue::HomepageStale { path, diff_summary } => write!(
                f,
                "homepage file `{path}` is out of sync with the current ontology / doc corpus ({diff_summary}). Run `doc-linter homepage --write` to regenerate. Do not hand-edit the file — its contents are derived from frontmatter + the ontology."
            ),
            Issue::MissingBodyWikilink { entity } => write!(
                f,
                "covers: [{entity}] declares this doc covers '{entity}' but the body has no \
                 `[[entity-{entity}]]` wikilink. \
                 Preferred fix: add `[[entity-{entity}]]` to the prose paragraph where you \
                 discuss the concept (Obsidian's linked-mentions panel + AI-agent navigation \
                 both depend on it). \
                 Only remove `{entity}` from `covers:` if the doc genuinely doesn't cover \
                 the entity — in that case the prose is the lie, not the frontmatter."
            ),
            Issue::UnusedBoundedContext { value } => write!(
                f,
                "bounded-context `{value}` is declared but no doc claims it via \
                 `bounded_context:` frontmatter. \
                 Preferred fix: assign at least one doc to this context with \
                 `bounded_context: {value}`. \
                 Only delete `docs/ontology/values/bounded-context/{value}.md` if the \
                 context was authored speculatively — bump the ontology migration if it's \
                 been used elsewhere. As a last resort, add `{value}` to \
                 `bounded_context_usage_exempt` in `.doc-lint.toml` when the value is \
                 legitimately reserved for code-side classification only."
            ),
        }
    }
}
