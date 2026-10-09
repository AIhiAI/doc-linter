//! Corpus-walk → ingest-plan → the store writer for the [[entity-doc-graph]]
//! linter.
//!
//! `build_plan` mirrors the resolver logic in `graph::DocGraph::build`
//! so the in-memory `petgraph` view and the persisted the store view stay
//! in lockstep. `IngestPlan::write_to` drains the plan into the store in
//! one transaction using one prepared statement per edge kind.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::config::LintConfig;
use crate::graph::{normalize_path, EdgeKind};
use crate::ontology::{effective_bounded_context, Ontology};
use crate::parser::{extract_adoc_xrefs, extract_links, parse_doc, split_sections, Section};
use crate::validator::RELATES_TO_TYPES;

/// Holds every node/edge we discovered during the corpus walk.
/// Built once by `build_plan`, then drained into the store by
/// [`IngestPlan::write_to`].
#[derive(Default)]
pub(crate) struct IngestPlan {
    /// Gap-010 phase 2: canonical absolute path of the primary
    /// `--root`. Stamped onto every Doc / Entity / Function ingested
    /// from this --root so cross-repo queries can filter by source.
    /// Matches the `id` of the corresponding Repo node written by
    /// `store::ingest_repos`.
    pub(crate) repo_id: String,
    pub(crate) docs: Vec<DocNode>,
    pub(crate) entities: Vec<EntityNode>,
    pub(crate) edges: Vec<DocEdge>,
    pub(crate) covers_edges: Vec<CoversEdge>,
    pub(crate) relates_to_edges: Vec<RelatesToEdge>,
    /// `(doc_id, section)` with `section.line` already file-relative.
    pub(crate) sections: Vec<(String, Section)>,
}

/// Row shape for a `Doc` node insert into the [[entity-doc-graph]]
/// store — flattens frontmatter fields the linter cares about.
pub(crate) struct DocNode {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) role: String,
    pub(crate) kind: String,
    pub(crate) lifecycle: String,
    pub(crate) bounded_context: String,
    pub(crate) title: String,
    pub(crate) summary: String,
    pub(crate) status: String,
    pub(crate) updated: String,
    pub(crate) tags: Vec<String>,
    pub(crate) covers: Vec<String>,
    /// Gap-003: roadmap-phase label (e.g. `v1`, `v2`, `mvp`,
    /// `deferred`). Empty string when the frontmatter doesn't
    /// declare `phase:`. Drives the `phasing` saved query.
    pub(crate) phase: String,
    /// Non-standard frontmatter keys as `key=value` (see
    /// [`frontmatter_attributes`]), same shape as `Entity.attributes`.
    pub(crate) attributes: Vec<String>,
    /// `Repo.id` of the repo the file lives in: `--root` or the
    /// `cross_repo_roots` entry containing it.
    pub(crate) repo_id: String,
}

/// Row shape for an `Entity` node insert into the [[entity-doc-graph]]
/// SQLite store — projection of one ontology entry.
pub(crate) struct EntityNode {
    pub(crate) id: String,
    pub(crate) display: String,
    pub(crate) description: String,
    pub(crate) synonyms: Vec<String>,
    pub(crate) bounded_contexts: Vec<String>,
    pub(crate) scanner_coverage: Vec<String>,
    pub(crate) source_modules: Vec<String>,
    /// Bug #150: optional ontology classification. `None` (default)
    /// behaves like a regular domain entity; `Some("language")` or
    /// `Some("infrastructure")` excludes the entity from god-node
    /// ranking in `code_ingest::derive_god_nodes`.
    pub(crate) entity_class: Option<String>,
    /// Gap-001: free-form `key=value` attribute rows lifted from
    /// the entity's frontmatter `attributes:` mapping. Stored as
    /// STRING[] in the store so an agent can return a competitor-tool
    /// feature matrix in one query without a schema migration per
    /// new attribute.
    pub(crate) attributes: Vec<String>,
}

/// Row shape for a typed `Doc -> Doc` edge insert into the
/// [[entity-doc-graph]] SQLite store. The `kind` field selects the the store
/// rel-table (`Wikilink`, `DependsOn`, ...).
pub(crate) struct DocEdge {
    pub(crate) from_id: String,
    pub(crate) to_id: String,
    pub(crate) line: u32,
    pub(crate) kind: EdgeKind,
}

/// Row shape for an `Entity -> Entity` directed relationship insert
/// into the [[entity-doc-graph]] SQLite store, derived from each entity's
/// `relates_to:` frontmatter. The `relation_type` is validated against
/// the closed set in `crate::validator::RELATES_TO_TYPES`; an unknown
/// type produces a validator diagnostic *and* is silently dropped here
/// to avoid persisting noise.
pub(crate) struct RelatesToEdge {
    pub(crate) from_id: String,
    pub(crate) to_id: String,
    pub(crate) relation_type: String,
}

/// Row shape for a `Doc -> Entity` covers-edge insert into the
/// [[entity-doc-graph]] SQLite store, derived from a doc's `covers:`
/// frontmatter.
pub(crate) struct CoversEdge {
    pub(crate) doc_id: String,
    pub(crate) entity_id: String,
    /// Inferred from the title and headings of a doc that declares no
    /// frontmatter (#262), rather than declared in `covers:`.
    pub(crate) inferred: bool,
}

/// Walks the corpus, resolves wikilinks/typed-edges/covers, and produces
/// the [[entity-doc-graph]] `IngestPlan` that `ingest` writes to the store in
/// one transaction.
pub(crate) fn build_plan(
    root: &Path,
    config: &LintConfig,
    files: &[PathBuf],
    ontology: &Ontology,
) -> Result<IngestPlan> {
    let mut plan = IngestPlan::default();
    plan.repo_id = root.canonicalize().map_or_else(
        |_| root.to_string_lossy().to_string(),
        |p| p.to_string_lossy().to_string(),
    );

    let mut id_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut stem_to_id: HashMap<String, String> = HashMap::new();
    let mut path_to_id: HashMap<PathBuf, String> = HashMap::new();
    let mut pending_edges: Vec<(String, PendingTarget, u32, EdgeKind)> = Vec::new();

    let crate_root_candidates: Vec<PathBuf> = std::iter::once(root.to_path_buf())
        .chain(config.cross_repo_roots.iter().filter_map(|rel| {
            let candidate = root.join(rel);
            candidate.canonicalize().ok()
        }))
        .collect();

    // Longest cross root first so a nested root wins over its parent.
    let mut cross_roots: Vec<PathBuf> = crate_root_candidates[1..].to_vec();
    cross_roots.sort_by_key(|p| std::cmp::Reverse(p.as_os_str().len()));
    let repo_of = |path: &Path| -> String {
        cross_roots
            .iter()
            .find(|r| path.starts_with(r))
            .map_or_else(|| plan.repo_id.clone(), |r| r.to_string_lossy().to_string())
    };

    // Track which path "owns" each id so duplicate-id collisions emit a
    // clear, actionable warning naming both files. Without this dedup,
    // the primary-key constraint blows up mid-transaction on the
    // second `CREATE (:Doc {id: ...})` — and the rest of the ingest
    // (edges, entities, COVERS) never lands, leaving every downstream
    // `query` returning empty `outbound`/`inbound` arrays. The fix is
    // defensive: first writer wins, the second is logged + skipped, and
    // ingest continues so the graph stays whole.
    let mut id_to_path: HashMap<String, PathBuf> = HashMap::new();
    let term_index = crate::disambiguation::TermIndex::build(ontology);
    for path in files {
        if config.is_exempt(path, root) {
            continue;
        }
        let Ok(doc) = parse_doc(path) else { continue };
        // A markdown doc without frontmatter is still indexed (and still
        // linted as `missing-frontmatter`), so search can find it.
        let synthesized;
        let meta = if let Some(meta) = &doc.meta {
            meta
        } else {
            synthesized = crate::parser::synthesized_frontmatter(path, &doc.body);
            &synthesized
        };

        if let Some(prev) = id_to_path.get(&meta.id) {
            let rel_prev = prev.strip_prefix(root).unwrap_or(prev);
            let rel_dup = path.strip_prefix(root).unwrap_or(path);
            eprintln!(
                "doc-linter: duplicate doc id `{}` — keeping `{}`, skipping `{}` \
                 (rename the dupe's frontmatter `id:` or add the path to `exempt`)",
                meta.id,
                rel_prev.display(),
                rel_dup.display(),
            );
            continue;
        }

        let rel_path = path.strip_prefix(root).unwrap_or(path);
        // G1: store the resolved bounded context (frontmatter ∪
        // path-inference ∪ default) so SQL consumers see the same
        // value the Rust-side resolver uses. Without this, 87% of docs
        // showed as empty and `WHERE d.bounded_context = 'X'` queries
        // silently missed most of the corpus.
        let resolved_bc = effective_bounded_context(path, meta, config, root).unwrap_or_default();
        let node = DocNode {
            id: meta.id.clone(),
            path: rel_path.display().to_string(),
            role: meta.effective_role().unwrap_or("unknown").to_string(),
            kind: meta.kind.clone().unwrap_or_default(),
            lifecycle: meta.lifecycle.clone().unwrap_or_default(),
            bounded_context: resolved_bc,
            title: meta.title.clone(),
            summary: meta.summary.clone(),
            status: meta.status.clone(),
            updated: meta.updated.to_string(),
            tags: meta.tags.clone(),
            covers: meta.covers.clone(),
            phase: meta.phase.clone().unwrap_or_default(),
            attributes: frontmatter_attributes(&meta.extra),
            repo_id: repo_of(path),
        };
        id_set.insert(meta.id.clone());
        id_to_path.insert(meta.id.clone(), path.clone());
        path_to_id.insert(path.clone(), meta.id.clone());
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if stem != "README" {
                stem_to_id.insert(stem.to_string(), meta.id.clone());
            }
        }

        // Cover edges: doc -> entity (separate table; entity ids may not be
        // doc ids, even though the legacy petgraph used `entity-<id>` doc
        // sentinels — the store has a real Entity node table).
        for entity_id in &meta.covers {
            plan.covers_edges.push(CoversEdge {
                doc_id: meta.id.clone(),
                entity_id: entity_id.clone(),
                inferred: false,
            });
        }
        let is_adoc = crate::parser::is_adoc(path);
        let sections = split_sections(&doc.body, is_adoc);
        // #262: a doc with no frontmatter (most `.adoc` corpora) can't
        // declare `covers:`, so infer it from the concepts its title and
        // headings name — the same term scan that binds doc comments to
        // concepts. Marked `inferred` on the edge; lint rules still read
        // only the declared list.
        if doc.raw_frontmatter.is_none() {
            let headings = std::iter::once(meta.title.as_str())
                .chain(sections.iter().map(|s| s.heading.as_str()))
                .collect::<Vec<_>>()
                .join("\n");
            for entity_id in super::scan_doc_comment_for_entities(&headings, &term_index) {
                plan.covers_edges.push(CoversEdge {
                    doc_id: meta.id.clone(),
                    entity_id,
                    inferred: true,
                });
            }
        }

        let is_ontology_doc = node.role.starts_with("ontology-");
        plan.docs.push(node);
        for mut sec in sections {
            // A heading with no text of its own (`# Traps` straight into
            // `## T1`) is just a title. An ontology doc's H1 section
            // restates the Doc row (the entity definition) and crowds
            // rule-level sections out of `type=section` rankings.
            if sec.text.is_empty() || (is_ontology_doc && sec.level == 1) {
                continue;
            }
            sec.line = doc.file_line(sec.line);
            // Lead with the heading so the embedded text says which rule
            // or topic this is ("T7: Gross predicate"), not just its body.
            sec.text = format!("{}\n\n{}", sec.heading, sec.text);
            truncate_at_char_boundary(&mut sec.text, MAX_SECTION_CHARS);
            plan.sections.push((meta.id.clone(), sec));
        }

        let links = extract_links(&doc.body);
        if is_adoc {
            for link in extract_adoc_xrefs(&doc.body) {
                let doc_dir = path.parent().unwrap_or(root);
                pending_edges.push((
                    meta.id.clone(),
                    PendingTarget::Path(normalize_path(&doc_dir.join(&link.target))),
                    doc.file_line(link.line) as u32,
                    EdgeKind::MdLink,
                ));
            }
        }
        for link in links.wikilinks {
            pending_edges.push((
                meta.id.clone(),
                PendingTarget::WikiOrStem(link.target),
                doc.file_line(link.line) as u32,
                EdgeKind::Wikilink,
            ));
        }
        for link in links.md_links {
            let doc_dir = path.parent().unwrap_or(root);
            let abs = normalize_path(&doc_dir.join(&link.target));
            let is_md = link.target.to_ascii_lowercase().ends_with(".md");
            if is_md {
                pending_edges.push((
                    meta.id.clone(),
                    PendingTarget::Path(abs),
                    doc.file_line(link.line) as u32,
                    EdgeKind::MdLink,
                ));
            } else if let Some(crate_id) =
                synthesize_crate_id(&abs, &crate_root_candidates, &meta.id)
            {
                pending_edges.push((
                    meta.id.clone(),
                    PendingTarget::Id(crate_id),
                    doc.file_line(link.line) as u32,
                    EdgeKind::CrateRef,
                ));
            }
        }
        for (field, target) in meta.typed_edges() {
            let kind = EdgeKind::from_typed_field(field).unwrap_or(EdgeKind::Wikilink);
            pending_edges.push((meta.id.clone(), PendingTarget::Id(target), 0, kind));
        }
    }

    // Resolve and emit Doc->Doc edges.
    for (from_id, target, line, kind) in pending_edges {
        let resolved: Option<String> = match target {
            PendingTarget::WikiOrStem(t) => {
                if id_set.contains(&t) {
                    Some(t)
                } else {
                    stem_to_id.get(&t).cloned()
                }
            }
            PendingTarget::Path(p) => path_to_id.get(&p).cloned(),
            PendingTarget::Id(t) => {
                if id_set.contains(&t) {
                    Some(t)
                } else {
                    None
                }
            }
        };
        let Some(to_id) = resolved else { continue };
        if !id_set.contains(&from_id) || !id_set.contains(&to_id) {
            continue;
        }
        plan.edges.push(DocEdge {
            from_id,
            to_id,
            line,
            kind,
        });
    }

    // Entities from ontology.
    for ent in ontology.entities.values() {
        plan.entities.push(EntityNode {
            id: ent.id.clone(),
            display: ent.display.clone(),
            description: ent.description.clone(),
            synonyms: ent.synonyms.clone(),
            bounded_contexts: ent.bounded_contexts.clone(),
            scanner_coverage: ent.scanner_coverage.clone(),
            source_modules: ent.source_modules.clone(),
            entity_class: ent.entity_class.clone(),
            attributes: ent.attributes.clone(),
        });
    }

    // Entity → entity relates_to edges. Drop rows whose target isn't a
    // registered entity or whose type isn't in the closed vocabulary —
    // both are validator-diagnosed conditions, so keeping them out of
    // the store prevents persisting noise authors will see flagged anyway.
    for ent in ontology.entities.values() {
        for rel in &ent.relates_to {
            if !ontology.entities.contains_key(&rel.target) {
                continue;
            }
            if !RELATES_TO_TYPES.contains(&rel.relation_type.as_str()) {
                continue;
            }
            plan.relates_to_edges.push(RelatesToEdge {
                from_id: ent.id.clone(),
                to_id: rel.target.clone(),
                relation_type: rel.relation_type.clone(),
            });
        }
    }

    // Drop covers edges whose entity isn't in the ontology — keeps the store's
    // referential constraint happy. The petgraph version silently dropped
    // them too (they didn't resolve to a doc id either).
    let entity_ids: HashSet<&str> = plan.entities.iter().map(|e| e.id.as_str()).collect();
    plan.covers_edges
        .retain(|c| entity_ids.contains(c.entity_id.as_str()));

    Ok(plan)
}

/// Two-phase edge target representation used during the
/// [[entity-doc-graph]] ingest — every link is collected as a
/// `PendingTarget` first and resolved to a doc id only after all nodes
/// have been registered.
enum PendingTarget {
    WikiOrStem(String),
    Path(PathBuf),
    Id(String),
}

/// Mints a synthetic `crate-<name>` doc id when a markdown link points
/// into `crates/<name>/` but the target file isn't itself indexed — lets
/// the [[entity-doc-graph]] still record a `crate-ref` edge.
fn synthesize_crate_id(abs: &Path, repo_roots: &[PathBuf], source_id: &str) -> Option<String> {
    for repo in repo_roots {
        let Ok(rel) = abs.strip_prefix(repo) else {
            continue;
        };
        let mut comps = rel.components();
        let first = comps.next()?;
        if first.as_os_str() != "crates" {
            continue;
        }
        let name = comps.next()?.as_os_str().to_str()?;
        if name.is_empty() {
            continue;
        }
        let id = format!("crate-{name}");
        // Skip self-references — a crate's own README markdown-linking
        // its `src/foo.rs` would otherwise resolve back to itself and
        // wire up a self-loop edge (G7 regression vector).
        if id == source_id {
            return None;
        }
        return Some(id);
    }
    None
}

impl IngestPlan {}

/// Section bodies are stored for BM25 and embedding text; past this a
/// section is a chapter and the head is what retrieval needs.
const MAX_SECTION_CHARS: usize = 8_000;

fn truncate_at_char_boundary(s: &mut String, max: usize) {
    if s.len() > max {
        let mut cut = max;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
    }
}

/// Frontmatter keys the Doc schema has no column for (`company`,
/// `window`, `findings` on an RCA) as `key=value` strings: a scalar gives
/// one entry, a list one entry per scalar item; nested maps and nulls are
/// skipped. Queryable as `WHERE 'company=234032' IN d.attributes`.
fn frontmatter_attributes(
    extra: &std::collections::BTreeMap<String, serde_yaml::Value>,
) -> Vec<String> {
    fn scalar(v: &serde_yaml::Value) -> Option<String> {
        match v {
            serde_yaml::Value::String(s) => Some(s.clone()),
            serde_yaml::Value::Number(n) => Some(n.to_string()),
            serde_yaml::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
    let mut out = Vec::new();
    for (key, value) in extra {
        match value {
            serde_yaml::Value::Sequence(items) => {
                out.extend(
                    items
                        .iter()
                        .filter_map(scalar)
                        .map(|v| format!("{key}={v}")),
                );
            }
            v => out.extend(scalar(v).map(|v| format!("{key}={v}"))),
        }
    }
    out
}
