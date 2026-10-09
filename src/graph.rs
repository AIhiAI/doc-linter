//! Builds an in-memory graph of the vault from parsed frontmatter + wikilinks +
//! markdown links + typed-edge frontmatter fields.
//! Rebuilt on every invocation — no persistent storage.

use crate::config::LintConfig;
use crate::parser::{extract_links, parse_doc};
use anyhow::Result;
use petgraph::graph::{DiGraph, NodeIndex};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// In-memory node payload for the petgraph [[entity-doc-graph]] — doc id,
/// title, role, lifecycle, status, tags, and covers list. Built from
/// frontmatter during the in-process build step.
#[derive(Debug, Clone, Serialize)]
pub struct NodeMeta {
    pub id: String,
    pub path: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub updated: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub covers: Vec<String>,
}

/// Edge kinds carried by every [[entity-doc-graph]] edge. Distinguishes
/// free-form `## Related` wikilinks from structural typed edges declared in
/// frontmatter, plus synthetic crate-ref edges synthesised from
/// `crates/<NAME>/...` markdown links.
///
/// Migration 0005 removed `Implements`, `Blocks`, and `OwnedBy`
/// (carrying-cost; see [[ontology-mig-0005]]).
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum EdgeKind {
    Wikilink,
    MdLink,
    DependsOn,
    InformedBy,
    Supersedes,
    CrateRef,
    /// Synthesised edge from a doc's `covers: [entity]` frontmatter to
    /// the corresponding `entity-<id>` glossary doc. Lets the graph reflect
    /// the DDD ubiquitous-language axis without authors having to wikilink
    /// the entity in narrative text.
    Covers,
}

impl EdgeKind {
    pub fn from_typed_field(field: &str) -> Option<Self> {
        match field {
            "depends-on" => Some(Self::DependsOn),
            "informed-by" => Some(Self::InformedBy),
            "supersedes" => Some(Self::Supersedes),
            _ => None,
        }
    }

    /// Kebab-case string form of an `EdgeKind` used by the
    /// [[entity-doc-graph]] CLI/JSON output and edge table names.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Wikilink => "wikilink",
            Self::MdLink => "md-link",
            Self::DependsOn => "depends-on",
            Self::InformedBy => "informed-by",
            Self::Supersedes => "supersedes",
            Self::CrateRef => "crate-ref",
            Self::Covers => "covers",
        }
    }

    /// Parses a CLI value (kebab-case) back to a kind. Used by query
    /// filters.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "wikilink" => Some(Self::Wikilink),
            "md-link" => Some(Self::MdLink),
            "depends-on" => Some(Self::DependsOn),
            "informed-by" => Some(Self::InformedBy),
            "supersedes" => Some(Self::Supersedes),
            "crate-ref" => Some(Self::CrateRef),
            "covers" => Some(Self::Covers),
            _ => None,
        }
    }
}

#[cfg(test)]
mod edge_kind_proptests {
    use super::EdgeKind;
    use proptest::prelude::*;

    /// Strategy: pick one of every [`EdgeKind`] variant uniformly.
    /// Listed inline (not `EdgeKind::iter()`) because the enum is
    /// `#[non_exhaustive]` and has no automatic iterator. Adding a
    /// new variant fails to compile until it's added here too —
    /// keeps the property test honest with the enum.
    fn arb_edge_kind() -> impl Strategy<Value = EdgeKind> {
        prop_oneof![
            Just(EdgeKind::Wikilink),
            Just(EdgeKind::MdLink),
            Just(EdgeKind::DependsOn),
            Just(EdgeKind::InformedBy),
            Just(EdgeKind::Supersedes),
            Just(EdgeKind::CrateRef),
            Just(EdgeKind::Covers),
        ]
    }

    proptest! {
        /// `EdgeKind::parse(k.as_str()) == Some(k)` for every variant.
        /// Round-trip invariant on the kebab-case projection used by
        /// CLI filters and edge table names.
        #[test]
        fn edge_kind_as_str_parse_roundtrip(kind in arb_edge_kind()) {
            let s = kind.as_str();
            prop_assert_eq!(EdgeKind::parse(s), Some(kind));
        }

        /// Unknown strings never accidentally parse to a real variant.
        /// Generated input is filtered to exclude the actual labels;
        /// the assertion is that `parse` returns `None`.
        #[test]
        fn edge_kind_parse_rejects_garbage(s in "[a-z][a-z0-9-]{0,30}") {
            let known = [
                "wikilink", "md-link", "depends-on", "informed-by",
                "supersedes", "crate-ref", "covers",
            ];
            prop_assume!(!known.contains(&s.as_str()));
            prop_assert_eq!(EdgeKind::parse(&s), None);
        }
    }
}

/// In-memory edge payload for the petgraph [[entity-doc-graph]] — source
/// line and edge kind. Used for query rendering and structural tests.
#[derive(Debug, Clone, Serialize)]
pub struct EdgeMeta {
    pub line: usize,
    pub kind: EdgeKind,
}

/// In-memory [[entity-doc-graph]] — a `petgraph::DiGraph` of doc nodes and
/// typed edges, plus an id-to-index map. Built once per CLI invocation
/// from the on-disk vault and discarded at exit.
pub struct DocGraph {
    pub graph: DiGraph<NodeMeta, EdgeMeta>,
    pub id_to_idx: HashMap<String, NodeIndex>,
}

/// Two-phase edge target representation used during
/// [[entity-doc-graph]] build: every link starts as a `PendingTarget` and
/// is resolved to a concrete node id only after every node has been
/// registered.
enum PendingTarget {
    /// Wikilink target — resolves as an id, or failing that, a filename stem.
    WikiOrStem(String),
    /// Markdown link target — absolute normalized path into the vault.
    Path(PathBuf),
    /// Frontmatter typed-edge target — resolves as an id only.
    Id(String),
    /// Synthetic edge to a crate's README, derived from a `crates/<name>/...`
    /// markdown link to a non-`.md` file. The string is the target id (e.g.
    /// `crate-pricing-module`).
    CrateRef(String),
}

/// Builder + lookup methods for the in-memory [[entity-doc-graph]].
impl DocGraph {
    pub fn build(root: &Path, config: &LintConfig, files: &[PathBuf]) -> Result<Self> {
        let mut graph = DiGraph::new();
        let mut id_to_idx: HashMap<String, NodeIndex> = HashMap::new();
        let mut stem_to_id: HashMap<String, String> = HashMap::new();
        let mut path_to_id: HashMap<PathBuf, String> = HashMap::new();
        let mut pending_edges: Vec<(String, PendingTarget, usize, EdgeKind)> = Vec::new();

        // Cross-repo roots to scan for `crates/<NAME>/...` synthesis. Includes
        // the primary --root plus any cross_repo_roots; covers a sibling repo
        // that ships its own crates directory (none today, but cheap to allow).
        let crate_root_candidates: Vec<PathBuf> = std::iter::once(root.to_path_buf())
            .chain(config.cross_repo_roots.iter().filter_map(|rel| {
                let candidate = root.join(rel);
                candidate.canonicalize().ok()
            }))
            .collect();

        for path in files {
            if config.is_exempt(path, root) {
                continue;
            }
            let Ok(doc) = parse_doc(path) else { continue };
            let Some(ref meta) = doc.meta else { continue };

            let rel_path = path.strip_prefix(root).unwrap_or(path);
            let node = NodeMeta {
                id: meta.id.clone(),
                path: rel_path.display().to_string(),
                role: meta.effective_role().unwrap_or("unknown").to_string(),
                kind: meta.kind.clone(),
                lifecycle: meta.lifecycle.clone(),
                title: meta.title.clone(),
                summary: meta.summary.clone(),
                status: meta.status.clone(),
                updated: meta.updated.to_string(),
                tags: meta.tags.clone(),
                covers: meta.covers.clone(),
            };
            let idx = graph.add_node(node);
            id_to_idx.insert(meta.id.clone(), idx);
            path_to_id.insert(path.clone(), meta.id.clone());

            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem != "README" {
                    stem_to_id.insert(stem.to_string(), meta.id.clone());
                }
            }

            let links = extract_links(&doc.body);
            for link in links.wikilinks {
                pending_edges.push((
                    meta.id.clone(),
                    PendingTarget::WikiOrStem(link.target),
                    doc.file_line(link.line),
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
                        doc.file_line(link.line),
                        EdgeKind::MdLink,
                    ));
                } else if let Some(crate_id) =
                    synthesize_crate_id(&abs, &crate_root_candidates, &meta.id)
                {
                    pending_edges.push((
                        meta.id.clone(),
                        PendingTarget::CrateRef(crate_id),
                        doc.file_line(link.line),
                        EdgeKind::CrateRef,
                    ));
                }
            }
            for (field, target) in meta.typed_edges() {
                let kind = EdgeKind::from_typed_field(field).unwrap_or(EdgeKind::Wikilink);
                pending_edges.push((meta.id.clone(), PendingTarget::Id(target), 0, kind));
            }

            // covers: [entity, ...] synthesises edges to entity-<id> docs.
            // The validator separately confirms each entity exists; the graph
            // builder just emits the edge when the target doc is present.
            for entity_id in &meta.covers {
                pending_edges.push((
                    meta.id.clone(),
                    PendingTarget::Id(format!("entity-{entity_id}")),
                    0,
                    EdgeKind::Covers,
                ));
            }
        }

        // Resolve edges.
        for (from_id, target, line, kind) in pending_edges {
            let resolved_id: Option<String> = match target {
                PendingTarget::WikiOrStem(t) => {
                    if id_to_idx.contains_key(&t) {
                        Some(t)
                    } else {
                        stem_to_id.get(&t).cloned()
                    }
                }
                PendingTarget::Path(p) => path_to_id.get(&p).cloned(),
                PendingTarget::Id(t) | PendingTarget::CrateRef(t) => {
                    if id_to_idx.contains_key(&t) {
                        Some(t)
                    } else {
                        None
                    }
                }
            };
            let Some(to_id) = resolved_id else { continue };
            let Some(&from_idx) = id_to_idx.get(&from_id) else {
                continue;
            };
            let Some(&to_idx) = id_to_idx.get(&to_id) else {
                continue;
            };
            graph.add_edge(from_idx, to_idx, EdgeMeta { line, kind });
        }

        Ok(DocGraph { graph, id_to_idx })
    }

    /// Looks up a node index by its doc id in the [[entity-doc-graph]].
    pub fn find(&self, id: &str) -> Option<NodeIndex> {
        self.id_to_idx.get(id).copied()
    }
}

/// If `abs` is a path of the shape `<repo-root>/crates/<NAME>/...`, returns
/// the conventional [[entity-doc-graph]] crate-doc id `crate-<NAME>`.
/// Returns `None` otherwise, or when the synthesised id equals `source_id`
/// — that case happens when a crate's own README references its own
/// `src/foo.rs` files, which would otherwise wire up as a self-loop edge
/// (G7 regression vector).
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
        if id == source_id {
            return None;
        }
        return Some(id);
    }
    None
}

/// Collapses `.` and `..` components without touching the filesystem for
/// the [[entity-doc-graph]] path-resolution helpers. Unlike `canonicalize`,
/// works for paths that may not yet exist.
pub fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod normalize_path_tests {
    use super::*;

    #[test]
    fn no_op_on_already_canonical() {
        assert_eq!(
            normalize_path(Path::new("docs/ontology/entities/outlet.md")),
            PathBuf::from("docs/ontology/entities/outlet.md")
        );
    }

    #[test]
    fn drops_cur_dir_segments() {
        assert_eq!(
            normalize_path(Path::new("./docs/./entities/./outlet.md")),
            PathBuf::from("docs/entities/outlet.md")
        );
    }

    #[test]
    fn resolves_parent_dir_segments() {
        assert_eq!(
            normalize_path(Path::new("docs/entities/../entities/outlet.md")),
            PathBuf::from("docs/entities/outlet.md")
        );
        assert_eq!(normalize_path(Path::new("a/b/../../c")), PathBuf::from("c"));
    }

    #[test]
    fn parent_dir_underflow_is_lossy_not_panic() {
        // A `..` against an empty path is a no-op (pop on empty), so the
        // helper stays infallible.
        assert_eq!(normalize_path(Path::new("..")), PathBuf::new());
        assert_eq!(
            normalize_path(Path::new("../../docs")),
            PathBuf::from("docs")
        );
    }

    #[test]
    fn keeps_absolute_root() {
        assert_eq!(normalize_path(Path::new("/")), PathBuf::from("/"));
        assert_eq!(
            normalize_path(Path::new("/var/log/./app/../app.log")),
            PathBuf::from("/var/log/app.log")
        );
    }

    #[test]
    fn empty_path_returns_empty() {
        assert_eq!(normalize_path(Path::new("")), PathBuf::new());
    }
}
