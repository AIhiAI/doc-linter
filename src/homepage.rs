//! Generator + checker for the auto-generated repo `MAP.md`.
//!
//! `MAP.md` is a machine-shaped index of the repo's knowledge graph
//! — every ontology entity, every narrative doc, every bounded
//! context, every public-API surface node. The file is regenerated
//! from the graph on demand (`doc-linter homepage --write`) and the
//! `homepage-stale` lint rule (opt-in via `require_homepage = true`
//! in `.doc-lint.toml`) gates lint on the file matching the
//! current graph state, so an agent that adds an entity or
//! narrative doc can't ship without also refreshing the index.
//!
//! Companion to `README.md`: `README.md` stays human-authored for
//! marketing / quickstart prose; `MAP.md` is the structured index
//! AI agents consume to learn the repo in <2 minutes of reading.
//! Both use Obsidian-compatible `[[wikilink]]` syntax so the
//! graph view and backlinks panel light up natively.

use crate::config::LintConfig;
use crate::ontology::Ontology;
use crate::parser::Doc;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// Generate the canonical `MAP.md` content from the parsed corpus +
/// assembled ontology. Output is deterministic — same inputs always
/// produce identical bytes — so the `homepage-stale` lint can
/// byte-compare the on-disk file against this without false
/// positives.
///
/// The body links via `[[wikilink]]` syntax (Obsidian-native) and
/// references doc ids, not paths, so it survives doc renames.
pub fn generate(
    root: &Path,
    docs: &HashMap<PathBuf, Doc>,
    ontology: &Ontology,
    config: &LintConfig,
) -> String {
    let repo_name = repo_name(root);
    let updated = freshness_date(docs);
    let title = title_from(&repo_name);

    let anchor = resolve_anchor_entity(docs, ontology, config, &repo_name);
    let mut out = String::new();
    write_frontmatter(&mut out, &title, &updated);
    write_preamble(&mut out);
    write_tldr(&mut out, ontology, anchor.as_deref());
    write_cold_start(&mut out);
    write_entity_table(&mut out, ontology, docs);
    write_narrative_index(&mut out, docs);
    write_bounded_contexts(&mut out, ontology, docs);
    write_graph_queries(&mut out, ontology, docs, anchor.as_deref());
    write_public_surface(&mut out);
    write_version_stamp(&mut out, ontology, docs);
    out
}

/// Centralised anchor-entity resolution so the TL;DR section and
/// the graph-query examples cite the same concept. Resolution
/// order matches `write_tldr`: explicit config → repo-name match
/// → highest-inbound-covers → `None`.
fn resolve_anchor_entity(
    docs: &HashMap<PathBuf, Doc>,
    ontology: &Ontology,
    config: &LintConfig,
    repo_name: &str,
) -> Option<String> {
    config
        .ontology
        .homepage_root_entity
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if ontology.entity(repo_name).is_some() {
                Some(repo_name.to_string())
            } else {
                None
            }
        })
        .or_else(|| central_entity_by_covers(docs, ontology))
}

/// Convention: the repo's display title is the directory name
/// title-cased on word boundaries. Authors can override per-section
/// by editing the underlying ontology, but the H1 / frontmatter
/// title we use the dir name as a stable best-guess. Kept private
/// because the only consumer is `generate`.
fn title_from(repo_name: &str) -> String {
    // "myapp" → "Myapp"; "my-cool-platform" → "My Cool Platform"
    repo_name
        .split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The directory name `<root>` resolves to, used as the repo's
/// canonical short id throughout the homepage.
fn repo_name(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .to_string()
}

/// Date stamp used in the homepage's frontmatter `updated:`. Picks
/// the most-recent `updated:` across every ontology-migration doc
/// in the corpus so the date moves only when the ontology itself
/// changes — keeps the homepage stable across days when nothing has
/// changed. Falls back to today only if no migration doc exists.
fn freshness_date(docs: &HashMap<PathBuf, Doc>) -> String {
    let mut best: Option<chrono::NaiveDate> = None;
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        if meta.effective_role() == Some("ontology-migration") {
            best = Some(match best {
                Some(d) if d >= meta.updated => d,
                _ => meta.updated,
            });
        }
    }
    best.unwrap_or_else(|| chrono::Local::now().date_naive())
        .to_string()
}

fn write_frontmatter(out: &mut String, title: &str, updated: &str) {
    out.push_str("---\n");
    out.push_str("id: map\n");
    out.push_str("role: index\n");
    out.push_str(&format!("title: \"{title} — Repo Map\"\n"));
    out.push_str("summary: Machine-shaped index of every ontology entity, narrative doc, and bounded context in this repo. Regenerate via `doc-linter homepage --write` whenever the underlying ontology or doc set changes.\n");
    out.push_str("status: stable\n");
    out.push_str(&format!("updated: {updated}\n"));
    out.push_str("---\n\n");
}

fn write_preamble(out: &mut String) {
    out.push_str(
        "> [!info] Auto-generated by `doc-linter homepage`\n\
         > Do not hand-edit — your changes will be overwritten on the next regeneration. \
         To change what appears here, edit the underlying frontmatter or ontology \
         and re-run `doc-linter homepage --write`.\n\n",
    );
}

/// One-line "what is this repo" TL;DR pulled from the central
/// entity's `description:`. Caller resolves the anchor id via
/// `resolve_anchor_entity`.
fn write_tldr(out: &mut String, ontology: &Ontology, anchor_id: Option<&str>) {
    out.push_str("## What this repo is\n\n");
    match anchor_id.and_then(|id| ontology.entity(id).map(|e| (id, e))) {
        Some((id, ent)) if !ent.description.is_empty() => {
            out.push_str(&format!("{}\n\n", ent.description));
            out.push_str(&format!(
                "Anchor entity: [[entity-{id}]] (override via `homepage_root_entity` in `.doc-lint.toml`).\n\n"
            ));
        }
        _ => {
            out.push_str(
                "*No anchor entity resolved.* Set `homepage_root_entity = \"<id>\"` in `.doc-lint.toml`, \
                 or rename the entity whose description summarises this repo to match the \
                 repo directory name.*\n\n",
            );
        }
    }
}

/// Inbound-covers count for every entity. Used by the homepage's
/// TL;DR resolver to pick the "most central" concept when no
/// explicit anchor is configured.
fn central_entity_by_covers(docs: &HashMap<PathBuf, Doc>, ontology: &Ontology) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        for c in &meta.covers {
            if ontology.entity(c).is_some() {
                *counts.entry(c.clone()).or_insert(0) += 1;
            }
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(id, _)| id)
}

fn write_cold_start(out: &mut String) {
    out.push_str("## Cold start for AI agents\n\n");
    out.push_str(
        "Five commands to learn the repo before touching anything:\n\n\
         ```bash\n\
         doc-linter ontology              # axes / values / entities — the repo's vocabulary\n\
         doc-linter query list            # every doc in the corpus (filter by --role/--kind/--tag)\n\
         doc-linter query coverage-report # which entities / functions have narrative cover\n\
         doc-linter query endpoints       # public HTTP / CLI / MCP surface\n\
         doc-linter check                 # baseline lint state — what's structurally wrong\n\
         ```\n\n\
         See `AGENTS.md` / `CLAUDE.md` for authoring conventions and the \
         **Graph queries** section below for navigation patterns.\n\n",
    );
}

/// Graph-query cheat sheet — surfaces the queryable side of the
/// linter to agents. Without this section MAP.md reads like a
/// static table of contents, hiding that the underlying SQLite
/// graph is callable through three families of queries:
///
///   1. Structural — neighbors / backlinks / path / subgraph /
///      context. Cheap, no SQL, fast for "what's near X?".
///   2. SQL — arbitrary read-only graph queries against the live
///      SQLite graph. Examples reference the anchor entity so the
///      snippets are immediately copy-pasteable.
///   3. Code-side — functions-mentioning / function-context /
///      explain. Cross the doc graph into the SCIP-derived
///      Function/Endpoint nodes when SCIP is ingested.
fn write_graph_queries(
    out: &mut String,
    ontology: &Ontology,
    _docs: &HashMap<PathBuf, Doc>,
    anchor_id: Option<&str>,
) {
    let anchor = anchor_id.unwrap_or("<entity-id>");
    let anchor_doc = format!("entity-{anchor}");
    let sample_other = sample_other_entity(ontology, anchor_id);
    let sample_other_doc = format!("entity-{sample_other}");

    out.push_str("## Graph queries\n\n");
    out.push_str(
        "The corpus is a live SQLite graph; the linter exposes it through three query families. \
         Run any of these from the repo root — nothing to install, the binary embeds SQLite.\n\n",
    );

    // ---- Structural queries ----
    out.push_str("### Structural (no SQL needed)\n\n");
    out.push_str(&format!(
        "```bash\n\
         doc-linter query neighbors {anchor_doc}        # outbound wikilinks from this doc\n\
         doc-linter query backlinks {anchor_doc}        # who references it\n\
         doc-linter query path {anchor_doc} {sample_other_doc}  # shortest link-path between two docs\n\
         doc-linter query subgraph {anchor_doc} --depth 2  # the doc + everything within 2 hops\n\
         doc-linter query context {anchor_doc}          # frontmatter + neighbors + backlinks in one call\n\
         ```\n\n",
    ));

    // ---- SQL queries ----
    out.push_str("### SQL (ad-hoc graph queries)\n\n");
    out.push_str(
        "Use `doc-linter query sql \"<q>\"` (one read-only `SELECT` / `WITH`) for anything the structural commands don't already cover. \
         Node tables are `Doc`, `Entity`, `Function`, ...; each edge kind is a table with `src` / `dst` columns. \
         Useful starting points:\n\n",
    );
    out.push_str(&format!(
        "```bash\n\
         # Top entities by inbound `covers:` (which concepts have the most narrative)\n\
         doc-linter query sql \"SELECT e.id, count(c.src) AS cover_count FROM Entity e LEFT JOIN COVERS c ON c.dst = e.id GROUP BY e.id ORDER BY cover_count DESC\"\n\n\
         # Entity pairs frequently co-covered in the same doc (concept clusters)\n\
         doc-linter query sql \"SELECT a.dst AS a, b.dst AS b, count(*) AS together FROM COVERS a JOIN COVERS b ON a.src = b.src AND a.dst < b.dst GROUP BY a.dst, b.dst ORDER BY together DESC LIMIT 20\"\n\n\
         # Every doc that covers a specific entity\n\
         doc-linter query sql \"SELECT d.id, d.title, d.bounded_context FROM Doc d JOIN COVERS c ON c.src = d.id WHERE c.dst = '{anchor}'\"\n\n\
         # Docs reachable from MAP.md within 3 wikilink hops (either direction)\n\
         doc-linter query sql \"WITH RECURSIVE e(a, b) AS (SELECT src, dst FROM WIKILINK UNION SELECT dst, src FROM WIKILINK), r(id, depth) AS (SELECT 'map', 0 UNION SELECT e.b, r.depth + 1 FROM r JOIN e ON e.a = r.id WHERE r.depth < 3) SELECT count(DISTINCT id) - 1 AS reach FROM r\"\n\n\
         # Schema introspection — every table\n\
         doc-linter query schema\n\
         ```\n\n",
    ));

    // ---- Code-side queries ----
    // (Placeholder for a future signal that gates the code-side block on
    // SCIP ingest having actually populated the Function / Endpoint
    // tables. Until that wiring lands, the section always renders.)
    out.push_str("### Code-side (SCIP ingest required)\n\n");
    out.push_str(
        "When `doc-linter scip-index` has populated the `Function` / `Endpoint` tables, these cross the doc graph into the codebase:\n\n",
    );
    out.push_str(&format!(
        "```bash\n\
         doc-linter query functions-mentioning {anchor}   # every fn whose doc-comment mentions this entity\n\
         doc-linter query function-context <symbol>     # full Function fact + entities + covering docs\n\
         doc-linter query coverage-report               # doc / function reach metrics\n\
         doc-linter query endpoints                     # axum / clap / MCP endpoint surface\n\
         doc-linter explain <symbol>                    # full graph walk explaining one function\n\
         ```\n\n",
    ));
    out.push_str("All queries accept `--format=json` for piping into editors / batch scripts.\n\n");
}

/// Picks any entity id distinct from the anchor for the `path`
/// example. Falls back to the anchor itself when there's only one
/// entity, then to a generic placeholder so the docs example
/// still type-checks visually.
fn sample_other_entity(ontology: &Ontology, anchor: Option<&str>) -> String {
    let mut ids: Vec<&String> = ontology.entities.keys().collect();
    ids.sort();
    for id in &ids {
        if Some(id.as_str()) != anchor {
            return (*id).clone();
        }
    }
    "<other-entity-id>".to_string()
}

/// Render every entity as a table row: id wikilink, description,
/// the docs that cover it. Sorted by entity id for deterministic
/// output.
fn write_entity_table(out: &mut String, ontology: &Ontology, docs: &HashMap<PathBuf, Doc>) {
    let mut ids: Vec<&String> = ontology.entities.keys().collect();
    ids.sort();
    out.push_str(&format!("## Domain concepts ({})\n\n", ids.len()));
    if ids.is_empty() {
        out.push_str("*No entities authored yet. Add docs under `docs/ontology/entities/`.*\n\n");
        return;
    }

    // entity-id → sorted list of covering doc ids.
    let mut coverage: HashMap<String, Vec<String>> = HashMap::new();
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        if meta.effective_role() == Some("ontology-entity") {
            continue; // entities don't cover themselves
        }
        for c in &meta.covers {
            coverage.entry(c.clone()).or_default().push(meta.id.clone());
        }
    }
    for v in coverage.values_mut() {
        v.sort();
        v.dedup();
    }

    out.push_str("| Entity | Description | Covered by |\n");
    out.push_str("|---|---|---|\n");
    for id in ids {
        let ent = &ontology.entities[id];
        let covers = coverage.get(id).map_or_else(
            || "—".to_string(),
            |v| {
                v.iter()
                    .map(|d| format!("[[{d}]]"))
                    .collect::<Vec<_>>()
                    .join(", ")
            },
        );
        let desc = escape_table_cell(&ent.description);
        let display = if ent.display.is_empty() {
            id.as_str()
        } else {
            ent.display.as_str()
        };
        out.push_str(&format!(
            "| [[entity-{id}]] {display} | {desc} | {covers} |\n"
        ));
    }
    out.push('\n');
}

/// Group narrative docs (`role: doc`) by their `kind:` axis value.
/// Each row: wikilink to the doc, its title, and its covers list.
/// `kind: ""` (unset) is reported as its own bucket so authors see
/// the gap.
fn write_narrative_index(out: &mut String, docs: &HashMap<PathBuf, Doc>) {
    let mut by_kind: BTreeMap<String, Vec<(String, String, Vec<String>)>> = BTreeMap::new();
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        if meta.effective_role() != Some("doc") {
            continue;
        }
        let kind = meta.kind.clone().unwrap_or_else(|| "(no kind)".to_string());
        by_kind.entry(kind).or_default().push((
            meta.id.clone(),
            meta.title.clone(),
            meta.covers.clone(),
        ));
    }
    let total: usize = by_kind.values().map(std::vec::Vec::len).sum();
    out.push_str(&format!("## Narrative docs ({total})\n\n"));
    if total == 0 {
        out.push_str("*No narrative docs (`role: doc`) authored yet.*\n\n");
        return;
    }
    for (kind, mut entries) in by_kind {
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        out.push_str(&format!(
            "### {} ({})\n\n",
            heading_case(&kind),
            entries.len()
        ));
        for (id, title, covers) in entries {
            let title_disp = if title.is_empty() { id.clone() } else { title };
            let covers_disp = if covers.is_empty() {
                "—".to_string()
            } else {
                covers.join(", ")
            };
            out.push_str(&format!(
                "- [[{id}]] — {title_disp} · covers: {covers_disp}\n"
            ));
        }
        out.push('\n');
    }
}

/// Bounded contexts: list every authored value of the
/// `bounded-context` axis with the number of docs in it, in
/// descending count order. Contexts with zero docs surface as a
/// distinct row so authors see the gap.
fn write_bounded_contexts(out: &mut String, ontology: &Ontology, docs: &HashMap<PathBuf, Doc>) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for id in ontology.bounded_contexts.keys() {
        counts.insert(id.clone(), 0);
    }
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        if let Some(bc) = &meta.bounded_context {
            if !bc.is_empty() {
                *counts.entry(bc.clone()).or_insert(0) += 1;
            }
        }
    }

    out.push_str(&format!("## Bounded contexts ({})\n\n", counts.len()));
    if counts.is_empty() {
        out.push_str(
            "*No `bounded-context` axis authored.* Add it under \
             `docs/ontology/axes/bounded-context.md` with values under \
             `docs/ontology/values/bounded-context/`.\n\n",
        );
        return;
    }
    // Sort by doc-count descending, then id ascending.
    let mut rows: Vec<(String, usize)> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (ctx, n) in rows {
        let suffix = if n == 0 { " — *no docs yet*" } else { "" };
        out.push_str(&format!("- **{ctx}** — {n} doc(s){suffix}\n"));
    }
    out.push('\n');
}

/// Public-API surface stub. `Endpoint` and `Function` nodes live in
/// the graph and are populated by SCIP ingest. The homepage
/// generator runs without the graph (it reads only the markdown corpus
/// + ontology), so the section names the queries that go live once
/// SCIP is active, rather than displaying counts. Counts would
/// require a graph round-trip the generator deliberately avoids.
fn write_public_surface(out: &mut String) {
    out.push_str("## Public surface\n\n");
    out.push_str(
        "`Endpoint` and `Function` nodes are sourced from the code graph \
         (`SCIP` ingest). Once `doc-linter scip-index` has run, the **Graph queries** \
         section above's code-side commands light up:\n\n\
         - `query endpoints` — every `axum` / `clap` / `MCP` route grouped by handler\n\
         - `query functions-mentioning <entity>` — code that documents the entity\n\
         - `query coverage-report` — function-to-entity reach percentages\n\
         - `explain <symbol>` — full graph walk for one function\n\n\
         Run `doc-linter scip-index` (Rust) or wait for the Python tree-sitter ingest \
         to land, then regenerate the homepage. Until then this section's queries return empty.\n\n",
    );
}

/// Footer stamp tying the homepage to a specific ontology version
/// + the migration doc that introduced it. The stamp is what makes
/// the homepage "stable across days but unstable across ontology
/// changes" — its date in frontmatter and the version reported
/// here both follow the most recent migration.
///
/// Version source preference (highest-first):
///   1. `to_version:` on the latest migration doc
///   2. `ontology_version:` (what `Ontology::version` reads)
///   3. `0` (no migration doc / no version field)
fn write_version_stamp(out: &mut String, ontology: &Ontology, docs: &HashMap<PathBuf, Doc>) {
    let latest = latest_migration(docs);
    let version = latest
        .as_ref()
        .and_then(|m| m.to_version)
        .unwrap_or(ontology.version);
    out.push_str("## Ontology version\n\n");
    out.push_str(&format!("- Ontology version: **v{version}**\n"));
    if let Some(m) = latest {
        out.push_str(&format!("- Latest migration: [[{}]]\n", m.id));
    }
    out.push('\n');
}

/// Compact tuple describing the most recent ontology-migration doc
/// in the corpus — id, the `to_version:` it bumped the ontology to
/// (if declared), and its `updated:` date (used for the homepage's
/// frontmatter freshness stamp).
struct LatestMigration {
    id: String,
    to_version: Option<u32>,
    // Parsed off frontmatter for completeness; we only stamp the id +
    // to_version in the homepage today, but the date is cheap to keep
    // and surfaces when the comparator is later upgraded to consider
    // age as a tiebreaker.
    #[allow(dead_code)] // future tiebreaker field; intentionally captured at parse time
    updated: chrono::NaiveDate,
}

fn latest_migration(docs: &HashMap<PathBuf, Doc>) -> Option<LatestMigration> {
    let mut best: Option<LatestMigration> = None;
    for doc in docs.values() {
        let Some(meta) = &doc.meta else { continue };
        if meta.effective_role() != Some("ontology-migration") {
            continue;
        }
        let to_version = meta
            .extra
            .get("to_version")
            .and_then(serde_yaml::Value::as_u64)
            .map(|n| n as u32);
        let candidate = LatestMigration {
            id: meta.id.clone(),
            to_version,
            updated: meta.updated,
        };
        best = Some(match best {
            Some(b) if b.updated >= candidate.updated => b,
            _ => candidate,
        });
    }
    best
}

/// Markdown table cells must not contain raw newlines or pipes.
/// Replace them so the table renders cleanly even when an
/// entity's description carries either.
fn escape_table_cell(s: &str) -> String {
    s.replace('\n', " ").replace('|', "\\|")
}

/// Capitalise the first letter of a heading-cased kind name.
/// `"how-to"` → `"How-to"`; `"(no kind)"` → `"(no kind)"`.
fn heading_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

/// Resolve the on-disk path the `MAP.md` file should live at,
/// per the configured `homepage_path` (relative to the repo root).
pub fn homepage_file_path(root: &Path, config: &LintConfig) -> PathBuf {
    root.join(&config.ontology.homepage_path)
}

/// Read the on-disk homepage if it exists. Returns `Ok(None)` when
/// the file is absent — callers distinguish "missing" from "stale"
/// so each gets its own diagnostic.
pub fn read_existing(path: &Path) -> std::io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}
