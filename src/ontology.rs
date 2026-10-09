//! Doc-graph ontology assembled from the ontology docs in the vault.
//!
//! The ontology defines what `role:` / `kind:` / `lifecycle:` values are
//! allowed and which domain entities exist for `covers:`. It's discovered
//! at lint time by walking docs whose role is one of four meta-roles:
//!
//!   - `ontology-axis`     — defines an axis (role/kind/lifecycle/covers)
//!   - `ontology-value`    — defines one allowed value of a closed axis
//!   - `ontology-entity`   — defines one domain entity (open vocab for `covers`)
//!   - `ontology-migration` — records an ontology version bump
//!
//! Bootstrap: the four meta-roles are recognised even when the ontology
//! docs aren't present (or aren't loaded yet), so the ontology can describe
//! itself without circular dependency. Every other role must come from an
//! `ontology-value` doc.

use crate::config::LintConfig;
use crate::parser::{Doc, Frontmatter};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Hardcoded meta-roles. These are recognised by the validator even when
/// no ontology docs exist (bootstrap). See `docs/ontology/values/role/` for
/// the doc-side definitions.
pub const META_ROLES: &[&str] = &[
    "ontology-axis",
    "ontology-value",
    "ontology-entity",
    "ontology-migration",
];

#[derive(Debug, Clone, Default)]
pub struct RoleDef {
    pub id: String,
    pub display: String,
    pub description: String,
    /// Other axes this role mandates (e.g. `["kind"]` for role=doc).
    pub requires_axes: Vec<String>,
    /// If set, only these lifecycle values are allowed for this role.
    pub allowed_lifecycle: Option<Vec<String>>,
    /// If set, the doc's filename (basename) must match this regex.
    pub filename_pattern: Option<String>,
    /// True when this role is hardcoded (bootstrap meta-role) rather than
    /// loaded from an ontology-value doc.
    pub bootstrap: bool,
}

/// One ontology value entry — id, display name, and description. Used for
/// the kind/lifecycle/bounded-context axes of the [[entity-doc-graph]]
/// vocabulary.
#[derive(Debug, Clone, Default)]
pub struct ValueDef {
    pub id: String,
    pub display: String,
    pub description: String,
}

/// One ontology entity entry — id, display, description, surface-form
/// synonyms, and the bounded contexts in which the entity is defined.
/// Lookup keys for the [[entity-doc-graph]] term index.
#[derive(Debug, Clone, Default)]
pub struct EntityDef {
    pub id: String,
    pub display: String,
    pub description: String,
    pub synonyms: Vec<String>,
    /// Bounded contexts this entity exists in (ontology v4). Empty means
    /// the entity is context-agnostic — references resolve regardless of
    /// the doc's effective context. Non-empty means the entity is only
    /// considered defined inside one of these contexts.
    pub bounded_contexts: Vec<String>,
    /// Scanners (language indexers) that *can* see code mentions of this
    /// entity — e.g. `["python", "typescript"]`. Author-declared, free-form
    /// strings. The linter doesn't validate against a closed set; downstream
    /// consumers use it to distinguish "entity has zero
    /// function mentions because no scanner covers its implementation
    /// language" (dark, expected) from "entity has zero function mentions
    /// because the developer hasn't built it yet" (genuinely missing).
    pub scanner_coverage: Vec<String>,
    /// File globs whose implementation files belong to this entity —
    /// e.g. `["backend/app/api/routes/*", "backend/app/core/security.py"]`.
    /// Lets the SCIP ingest emit `FUNCTION_BELONGS_TO` edges from every
    /// matching Function to this Entity, which collapses the module-import
    /// graph into entity-level directed edges. A downstream consumer uses those to
    /// derive "backend-api depends_on auth" from code evidence rather than
    /// from human-authored `relates_to:` declarations.
    pub source_modules: Vec<String>,
    /// Author-declared directed relationships to other entities. The YAML
    /// shape is a list of `{target: <entity-id>, type: <relation>}` rows;
    /// `type` is validated against the closed set in
    /// `crate::validator::RELATES_TO_TYPES`. Mirrored into the store as
    /// `RELATES_TO(FROM Entity TO Entity, type STRING)` edges so a downstream consumer
    /// can query `MATCH (a)-[r:RELATES_TO]->(b) RETURN a, r.type, b`
    /// directly.
    pub relates_to: Vec<EntityRelation>,
    /// Roadmap issue #14 (v0.3.0): doc frontmatter `status:` copied
    /// through so the pattern matcher can skip entities still in
    /// review. The `cluster --write` flow emits entities with
    /// `status: candidate`; the [[entity-doc-graph]] term index
    /// filters those out so they don't fire mention-coverage lint
    /// errors until a human promotes them.
    pub status: String,
    /// Bug #150: optional classification used to gate god-node
    /// ranking. Free-form string in the frontmatter; the well-known
    /// values are `language` (implementation language, e.g. `python`),
    /// `infrastructure` (runtime/platform, e.g. `kubernetes`), and
    /// `domain` (default architectural entity). Entities with
    /// `entity_class` in {`language`, `infrastructure`} are excluded
    /// from `is_god_node` assignment in `derive_god_nodes` — their
    /// mention count is still recorded, just not used to dominate
    /// architectural ranking. `None` means "domain" by default.
    pub entity_class: Option<String>,
    /// Gap-001: free-form key/value attributes for an entity.
    /// Serialised as STRING[] of "key=value" rows so an agent
    /// can return e.g. a competitor-tool feature matrix in one
    /// query. The loader treats every scalar `attr_<name>:` or
    /// every `attributes.<name>:` frontmatter entry as one row.
    /// Empty when the doc doesn't declare attributes.
    pub attributes: Vec<String>,
    /// #269: compound terms (`business-date`) that bind code symbols
    /// to this entity on adjacent identifier tokens (`BusinessDate`,
    /// `isBusinessDate`). When declared, they replace the single-token
    /// id / display / synonym match for symbols, narrowing an ambiguous
    /// one-word id. Doc-comment scanning is unaffected.
    pub code_terms: Vec<String>,
    /// #269: veto phrases (`feign-client`); a symbol containing one, as
    /// adjacent identifier tokens, never binds to this entity.
    pub code_veto: Vec<String>,
}

/// One entity → entity directed relationship declared via `relates_to:`
/// on entity frontmatter. Parsed from the YAML mapping `{target, type}`;
/// the `type` field is validated against the closed vocabulary in
/// `crate::validator::RELATES_TO_TYPES`.
#[derive(Debug, Clone)]
pub struct EntityRelation {
    pub target: String,
    /// Named `relation_type` because `type` is a Rust keyword. The
    /// YAML-side field stays `type`, parsed via the loader below.
    pub relation_type: String,
}

/// In-memory ontology — roles, kinds, lifecycles, entities, and bounded
/// contexts loaded from a vault's `docs/ontology/` tree. The vocabulary
/// the [[entity-doc-graph]] linter validates every doc against.
#[derive(Debug, Default)]
pub struct Ontology {
    pub version: u32,
    pub roles: HashMap<String, RoleDef>,
    pub kinds: HashMap<String, ValueDef>,
    pub lifecycles: HashMap<String, ValueDef>,
    pub entities: HashMap<String, EntityDef>,
    /// DDD bounded contexts (ontology v4). Closed but per-repo —
    /// each repo's `docs/ontology/values/bounded-context/` directory
    /// declares its own set.
    pub bounded_contexts: HashMap<String, ValueDef>,
}

/// Loader, lookup, and merge methods for the [[entity-doc-graph]]
/// `Ontology` — the vocabulary loaded from `docs/ontology/`.
impl Ontology {
    /// Build the bootstrap ontology containing only the four meta-roles.
    /// Used as a starting point before walking ontology docs.
    pub fn bootstrap() -> Self {
        let mut roles = HashMap::new();
        for &mr in META_ROLES {
            roles.insert(
                mr.to_string(),
                RoleDef {
                    id: mr.to_string(),
                    display: mr.to_string(),
                    description: "Hardcoded meta-role used by docs in docs/ontology/.".to_string(),
                    requires_axes: Vec::new(),
                    allowed_lifecycle: None,
                    filename_pattern: None,
                    bootstrap: true,
                },
            );
        }
        Ontology {
            version: 0,
            roles,
            kinds: HashMap::new(),
            lifecycles: HashMap::new(),
            entities: HashMap::new(),
            bounded_contexts: HashMap::new(),
        }
    }

    /// Assemble the ontology from a vault's parsed docs. Looks for any doc
    /// with a meta-role and registers its contribution. Bootstrap meta-roles
    /// are always present even if the ontology docs are missing.
    pub fn load_from_docs(docs: &HashMap<PathBuf, Doc>) -> Self {
        let mut ont = Self::bootstrap();

        for doc in docs.values() {
            let Some(meta) = &doc.meta else { continue };
            let Some(role) = meta.effective_role() else {
                continue;
            };
            match role {
                "ontology-axis" => {
                    // Axis docs declare metadata (multiplicity, openness) we
                    // don't currently enforce in code — but reading them lets
                    // future expansions reference them.
                    let _ = meta.extra_str("axis_id");
                }
                "ontology-value" => {
                    let axis = meta.extra_str("axis_id").unwrap_or("");
                    let value_id = meta.extra_str("value_id").unwrap_or("");
                    if value_id.is_empty() {
                        continue;
                    }
                    match axis {
                        "role" => {
                            ont.roles.insert(
                                value_id.to_string(),
                                RoleDef {
                                    id: value_id.to_string(),
                                    display: meta
                                        .extra_str("display")
                                        .unwrap_or(value_id)
                                        .to_string(),
                                    description: meta
                                        .extra_str("description")
                                        .unwrap_or("")
                                        .to_string(),
                                    requires_axes: meta.extra_str_list("requires_axes"),
                                    allowed_lifecycle: {
                                        let v = meta.extra_str_list("allowed_lifecycle");
                                        if v.is_empty() {
                                            None
                                        } else {
                                            Some(v)
                                        }
                                    },
                                    filename_pattern: meta
                                        .extra_str("filename_pattern")
                                        .map(std::string::ToString::to_string),
                                    // Preserve the meta-role marker even when
                                    // a value-role-<name>.md doc shadows the
                                    // bootstrap entry — the role is still one
                                    // of the linter's intrinsics, just now
                                    // with author-supplied metadata.
                                    bootstrap: META_ROLES.contains(&value_id),
                                },
                            );
                        }
                        "kind" => {
                            ont.kinds.insert(
                                value_id.to_string(),
                                ValueDef {
                                    id: value_id.to_string(),
                                    display: meta
                                        .extra_str("display")
                                        .unwrap_or(value_id)
                                        .to_string(),
                                    description: meta
                                        .extra_str("description")
                                        .unwrap_or("")
                                        .to_string(),
                                },
                            );
                        }
                        "lifecycle" => {
                            ont.lifecycles.insert(
                                value_id.to_string(),
                                ValueDef {
                                    id: value_id.to_string(),
                                    display: meta
                                        .extra_str("display")
                                        .unwrap_or(value_id)
                                        .to_string(),
                                    description: meta
                                        .extra_str("description")
                                        .unwrap_or("")
                                        .to_string(),
                                },
                            );
                        }
                        "bounded-context" => {
                            ont.bounded_contexts.insert(
                                value_id.to_string(),
                                ValueDef {
                                    id: value_id.to_string(),
                                    display: meta
                                        .extra_str("display")
                                        .unwrap_or(value_id)
                                        .to_string(),
                                    description: meta
                                        .extra_str("description")
                                        .unwrap_or("")
                                        .to_string(),
                                },
                            );
                        }
                        _ => { /* unknown axis — silently skip; ontology evolves */ }
                    }
                }
                "ontology-entity" => {
                    let value_id = meta.extra_str("value_id").unwrap_or("");
                    if value_id.is_empty() {
                        continue;
                    }
                    ont.entities.insert(
                        value_id.to_string(),
                        EntityDef {
                            id: value_id.to_string(),
                            display: meta.extra_str("display").unwrap_or(value_id).to_string(),
                            description: meta.extra_str("description").unwrap_or("").to_string(),
                            synonyms: meta.extra_str_list("synonyms"),
                            bounded_contexts: meta.extra_str_list("bounded_contexts"),
                            scanner_coverage: meta.extra_str_list("scanner_coverage"),
                            source_modules: meta.extra_str_list("source_modules"),
                            relates_to: parse_relates_to(meta),
                            status: meta.status.clone(),
                            entity_class: meta
                                .extra_str("entity_class")
                                .filter(|s| !s.is_empty())
                                .map(std::string::ToString::to_string),
                            attributes: parse_entity_attributes(meta),
                            code_terms: meta.extra_str_list("code_terms"),
                            code_veto: meta.extra_str_list("code_veto"),
                        },
                    );
                }
                _ => {}
            }
        }

        // Pick the highest declared `ontology_version:` from any ontology doc.
        let max_version = docs
            .values()
            .filter_map(|d| d.meta.as_ref())
            .filter(|m| {
                matches!(
                    m.effective_role(),
                    Some(
                        "ontology-axis"
                            | "ontology-value"
                            | "ontology-entity"
                            | "ontology-migration"
                    )
                )
            })
            .filter_map(|m| m.extra.get("ontology_version"))
            .filter_map(serde_yaml::Value::as_u64)
            .max()
            .unwrap_or(0) as u32;
        ont.version = max_version;

        ont
    }

    pub fn role(&self, id: &str) -> Option<&RoleDef> {
        self.roles.get(id)
    }

    /// Looks up a kind value by id in the [[entity-doc-graph]] ontology.
    pub fn kind(&self, id: &str) -> Option<&ValueDef> {
        self.kinds.get(id)
    }

    /// Looks up a lifecycle value by id in the [[entity-doc-graph]] ontology.
    pub fn lifecycle(&self, id: &str) -> Option<&ValueDef> {
        self.lifecycles.get(id)
    }

    /// Looks up an entity by id in the [[entity-doc-graph]] ontology.
    pub fn entity(&self, id: &str) -> Option<&EntityDef> {
        self.entities.get(id)
    }

    /// Looks up a bounded-context value by id in the [[entity-doc-graph]]
    /// ontology.
    pub fn bounded_context(&self, id: &str) -> Option<&ValueDef> {
        self.bounded_contexts.get(id)
    }
}

/// Compute a doc's effective bounded-context, in precedence order:
///
///   1. The frontmatter `bounded_context:` field, if set.
///   2. Path inference: the longest matching prefix in
///      `config.bounded_context_paths` wins (so `crates/pricing-tenant`
///      beats `crates/pricing-` when both are present).
///   3. The repo-wide `config.default_bounded_context`.
///
/// `path` is the doc's absolute path; `root` is the repo root used to
/// strip into a repo-relative form for path matching. Returns `None`
/// when no rule applies — that's fine; bounded-context is optional in
/// Round 1B and only enforced in later rounds.
pub fn effective_bounded_context(
    path: &Path,
    frontmatter: &Frontmatter,
    config: &LintConfig,
    root: &Path,
) -> Option<String> {
    if let Some(bc) = frontmatter.bounded_context.as_ref() {
        return Some(bc.clone());
    }

    let rel = path.strip_prefix(root).unwrap_or(path);
    let rel_str = rel.to_string_lossy();

    // Find the longest matching prefix across all contexts.
    let mut best: Option<(usize, &str)> = None;
    for (ctx, prefixes) in &config.ontology.bounded_context_paths {
        for prefix in prefixes {
            if rel_str.starts_with(prefix.as_str()) {
                let len = prefix.len();
                if best.is_none_or(|(b, _)| len > b) {
                    best = Some((len, ctx.as_str()));
                }
            }
        }
    }
    if let Some((_, ctx)) = best {
        return Some(ctx.to_string());
    }

    config.ontology.default_bounded_context.clone()
}

/// Parse `relates_to:` from entity frontmatter — a YAML sequence whose
/// items are `{target: <entity-id>, type: <relation>}` mappings. Returns
/// an empty Vec when the field is absent or malformed; per-row issues
/// (unknown type, missing target) surface as validator diagnostics, not
/// silent drops.
/// Gap-001: parse entity attributes from frontmatter. Accepts the
/// YAML shape `attributes:` as either a flat string-to-scalar
/// mapping (`{cross_repo: true, target: code}`) or a list of
/// `{key, value}` entries. Emits a `Vec<String>` of `key=value`
/// rows for storage in the the store `Entity.attributes STRING[]`
/// column. Bool / number scalars are stringified; non-scalar
/// values are skipped.
fn parse_entity_attributes(meta: &crate::parser::Frontmatter) -> Vec<String> {
    let Some(value) = meta.extra.get("attributes") else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    if let Some(map) = value.as_mapping() {
        for (k, v) in map {
            let Some(key) = k.as_str() else {
                continue;
            };
            let val_str = match v {
                serde_yaml::Value::String(s) => s.clone(),
                serde_yaml::Value::Bool(b) => b.to_string(),
                serde_yaml::Value::Number(n) => n.to_string(),
                _ => continue,
            };
            out.push(format!("{}={}", key.trim(), val_str.trim()));
        }
    } else if let Some(seq) = value.as_sequence() {
        for item in seq {
            let Some(item_map) = item.as_mapping() else {
                continue;
            };
            let key = item_map
                .get(serde_yaml::Value::String("key".to_string()))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let val = item_map
                .get(serde_yaml::Value::String("value".to_string()))
                .and_then(|v| match v {
                    serde_yaml::Value::String(s) => Some(s.clone()),
                    serde_yaml::Value::Bool(b) => Some(b.to_string()),
                    serde_yaml::Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .unwrap_or_default();
            if key.is_empty() {
                continue;
            }
            out.push(format!("{}={}", key.trim(), val.trim()));
        }
    }
    out.sort();
    out
}

fn parse_relates_to(meta: &crate::parser::Frontmatter) -> Vec<EntityRelation> {
    let Some(value) = meta.extra.get("relates_to") else {
        return Vec::new();
    };
    let Some(seq) = value.as_sequence() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in seq {
        let Some(map) = item.as_mapping() else {
            continue;
        };
        let target = map
            .get(serde_yaml::Value::String("target".to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let relation_type = map
            .get(serde_yaml::Value::String("type".to_string()))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        if target.is_empty() || relation_type.is_empty() {
            continue;
        }
        out.push(EntityRelation {
            target,
            relation_type,
        });
    }
    out
}

/// Tests for the [[entity-doc-graph]] ontology loader, value-doc walker,
/// and bounded-context resolver.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;
    use crate::parser::{parse_doc, Doc};
    use std::collections::HashMap;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Allocates a per-test temp directory under the system temp dir for
    /// the [[entity-doc-graph]] ontology tests, without requiring the
    /// `tempfile` crate (kept dep-free to match the existing crate's lean
    /// dependency surface). Caller is responsible for not relying on
    /// cleanup; CI runners reap their tmpdirs.
    fn tmp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!("doc-linter-test-{nanos}-{n}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// Helper for the [[entity-doc-graph]] ontology tests: write a temp
    /// markdown file with the given frontmatter+body and parse it into a
    /// Doc. Returns the parsed Doc and its temp path so the caller can
    /// build the docs HashMap.
    fn make_doc(
        dir: &std::path::Path,
        name: &str,
        frontmatter: &str,
        body: &str,
    ) -> (PathBuf, Doc) {
        let path = dir.join(name);
        let content = format!("---\n{frontmatter}\n---\n\n{body}");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let doc = parse_doc(&path).unwrap();
        (path, doc)
    }

    #[test]
    fn bootstrap_has_meta_roles_only() {
        let ont = Ontology::bootstrap();
        for &mr in META_ROLES {
            assert!(ont.role(mr).is_some(), "missing meta-role {mr}");
        }
        assert!(ont.role("doc").is_none());
        assert!(ont.role("spec").is_none());
        assert_eq!(ont.version, 0);
    }

    #[test]
    fn loads_speckit_roles_from_value_docs() {
        // Roadmap 19 added three new closed-vocab role values: spec, plan,
        // tasks. (research already existed.) This test exercises the load
        // path so a future ontology change that breaks the loader fails fast.
        let dir = tmp_dir();

        let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
        for (name, value_id) in [
            ("spec.md", "spec"),
            ("plan.md", "plan"),
            ("tasks.md", "tasks"),
        ] {
            let frontmatter = format!(
                "id: value-role-{value_id}\n\
                 role: ontology-value\n\
                 title: \"Role: {value_id}\"\n\
                 summary: test\n\
                 status: stable\n\
                 updated: 2026-04-29\n\
                 axis_id: role\n\
                 value_id: {value_id}\n\
                 display: {value_id}\n\
                 description: test\n\
                 requires_axes: [lifecycle]\n\
                 introduced_in_version: 2"
            );
            let (p, d) = make_doc(&dir, name, &frontmatter, "body");
            docs.insert(p, d);
        }
        // Add a migration doc to bump version to 2.
        let mig_fm = "id: ontology-mig-test\n\
                      role: ontology-migration\n\
                      title: test\n\
                      summary: test\n\
                      status: stable\n\
                      updated: 2026-04-29\n\
                      from_version: 1\n\
                      to_version: 2\n\
                      ontology_version: 2";
        let (p, d) = make_doc(&dir, "mig.md", mig_fm, "body");
        docs.insert(p, d);

        let ont = Ontology::load_from_docs(&docs);
        assert_eq!(ont.version, 2, "ontology version should be 2");
        for v in ["spec", "plan", "tasks"] {
            let r = ont.role(v).unwrap_or_else(|| panic!("missing role {v}"));
            assert_eq!(
                r.requires_axes,
                vec!["lifecycle"],
                "role {v} should require lifecycle"
            );
            assert!(!r.bootstrap, "role {v} should not be bootstrap");
        }
        // Meta-roles still present after loading.
        assert!(ont.role("ontology-value").unwrap().bootstrap);
    }

    #[test]
    fn loads_session_memory_roles_from_value_docs() {
        // Roadmap 20 added two new closed-vocab role values: session, memory.
        // Mirrors `loads_speckit_roles_from_value_docs` (roadmap 19) so a
        // future ontology change that breaks the loader for v3-introduced
        // values fails fast.
        let dir = tmp_dir();

        let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
        for (name, value_id) in [("session.md", "session"), ("memory.md", "memory")] {
            let frontmatter = format!(
                "id: value-role-{value_id}\n\
                 role: ontology-value\n\
                 title: \"Role: {value_id}\"\n\
                 summary: test\n\
                 status: stable\n\
                 updated: 2026-04-29\n\
                 axis_id: role\n\
                 value_id: {value_id}\n\
                 display: {value_id}\n\
                 description: test\n\
                 requires_axes: [lifecycle]\n\
                 introduced_in_version: 3"
            );
            let (p, d) = make_doc(&dir, name, &frontmatter, "body");
            docs.insert(p, d);
        }
        // Migration doc bumping version to 3.
        let mig_fm = "id: ontology-mig-test-3\n\
                      role: ontology-migration\n\
                      title: test\n\
                      summary: test\n\
                      status: stable\n\
                      updated: 2026-04-29\n\
                      from_version: 2\n\
                      to_version: 3\n\
                      ontology_version: 3";
        let (p, d) = make_doc(&dir, "mig.md", mig_fm, "body");
        docs.insert(p, d);

        let ont = Ontology::load_from_docs(&docs);
        assert_eq!(ont.version, 3, "ontology version should be 3");
        for v in ["session", "memory"] {
            let r = ont.role(v).unwrap_or_else(|| panic!("missing role {v}"));
            assert_eq!(
                r.requires_axes,
                vec!["lifecycle"],
                "role {v} should require lifecycle"
            );
            assert!(!r.bootstrap, "role {v} should not be bootstrap");
        }
        // Meta-roles still present after loading.
        assert!(ont.role("ontology-value").unwrap().bootstrap);
    }

    /// Asserts that the [[entity-doc-graph]] ontology loader picks up
    /// `bounded-context` axis values declared in `value-bounded-context-*`
    /// docs.
    #[test]
    fn loads_bounded_context_values_from_value_docs() {
        // Round 1B / ontology v4: closed per-repo `bounded-context` axis.
        // Exercise the loader so a future change that drops the axis arm
        // in `Ontology::load_from_docs` fails fast.
        let dir = tmp_dir();

        let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
        for (name, value_id) in [
            ("pricing.md", "pricing"),
            ("sync.md", "sync"),
            ("docs.md", "docs"),
        ] {
            let frontmatter = format!(
                "id: value-bounded-context-{value_id}\n\
                 role: ontology-value\n\
                 title: \"Bounded Context: {value_id}\"\n\
                 summary: test\n\
                 status: stable\n\
                 updated: 2026-04-30\n\
                 axis_id: bounded-context\n\
                 value_id: {value_id}\n\
                 display: {value_id}\n\
                 description: test\n\
                 introduced_in_version: 4"
            );
            let (p, d) = make_doc(&dir, name, &frontmatter, "body");
            docs.insert(p, d);
        }
        // Entity doc with bounded_contexts list — exercise EntityDef field.
        // Also pins scanner_coverage so a regression on the lenient-extra
        // plumbing fails here before it reaches the downstream dark-entity
        // classifier.
        let entity_fm = "id: entity-order\n\
                         role: ontology-entity\n\
                         title: \"Entity: Order\"\n\
                         summary: test\n\
                         status: stable\n\
                         updated: 2026-04-30\n\
                         axis_id: covers\n\
                         value_id: order\n\
                         display: Order\n\
                         description: test\n\
                         bounded_contexts: [pricing, sync]\n\
                         scanner_coverage: [python, typescript]\n\
                         source_modules: [\"backend/order/**/*.py\", \"backend/order/queue.py\"]\n\
                         code_terms: [order-line]\n\
                         code_veto: [order-by]\n\
                         relates_to:\n  \
                         - target: pricing-rule\n    \
                         type: depends_on\n  \
                         - target: shipment\n    \
                         type: feeds_into";
        let (p, d) = make_doc(&dir, "order.md", entity_fm, "body");
        docs.insert(p, d);

        let mig_fm = "id: ontology-mig-test-4\n\
                      role: ontology-migration\n\
                      title: test\n\
                      summary: test\n\
                      status: stable\n\
                      updated: 2026-04-30\n\
                      from_version: 3\n\
                      to_version: 4\n\
                      ontology_version: 4";
        let (p, d) = make_doc(&dir, "mig.md", mig_fm, "body");
        docs.insert(p, d);

        let ont = Ontology::load_from_docs(&docs);
        assert_eq!(ont.version, 4);
        for v in ["pricing", "sync", "docs"] {
            assert!(
                ont.bounded_context(v).is_some(),
                "missing bounded-context {v}"
            );
        }
        let order = ont.entity("order").expect("entity-order should load");
        assert_eq!(order.bounded_contexts, vec!["pricing", "sync"]);
        assert_eq!(order.scanner_coverage, vec!["python", "typescript"]);
        assert_eq!(
            order.source_modules,
            vec!["backend/order/**/*.py", "backend/order/queue.py"]
        );
        assert_eq!(order.code_terms, vec!["order-line"]);
        assert_eq!(order.code_veto, vec!["order-by"]);
        let rels: Vec<(&str, &str)> = order
            .relates_to
            .iter()
            .map(|r| (r.target.as_str(), r.relation_type.as_str()))
            .collect();
        assert_eq!(
            rels,
            vec![("pricing-rule", "depends_on"), ("shipment", "feeds_into"),]
        );
    }

    /// Asserts the precedence order — frontmatter, then path map, then
    /// default — used by the [[entity-doc-graph]] effective-bounded-context
    /// resolver.
    #[test]
    fn effective_bounded_context_precedence() {
        use crate::config::LintConfig;
        use std::collections::BTreeMap;

        let dir = tmp_dir();
        let root = dir.clone();

        // Build a config with a path map.
        let mut paths: BTreeMap<String, Vec<String>> = BTreeMap::new();
        paths.insert(
            "pricing".to_string(),
            vec!["crates/pricing-core".to_string()],
        );
        paths.insert("sync".to_string(), vec!["crates/import-core".to_string()]);
        let mut cfg = LintConfig::default();
        cfg.ontology.bounded_context_paths = paths;
        cfg.ontology.default_bounded_context = Some("docs".to_string());

        // 1. Frontmatter wins.
        let fm_explicit = "id: x\n\
                           role: doc\n\
                           kind: reference\n\
                           title: t\n\
                           summary: s\n\
                           status: stable\n\
                           updated: 2026-04-30\n\
                           bounded_context: mcp";
        std::fs::create_dir_all(dir.join("crates/pricing-core")).unwrap();
        let (p, d) = make_doc(
            &dir.join("crates/pricing-core"),
            "x.md",
            fm_explicit,
            "body",
        );
        let meta = d.meta.unwrap();
        assert_eq!(
            effective_bounded_context(&p, &meta, &cfg, &root),
            Some("mcp".to_string())
        );

        // 2. Path inference when no frontmatter field.
        let fm_implicit = "id: y\n\
                           role: doc\n\
                           kind: reference\n\
                           title: t\n\
                           summary: s\n\
                           status: stable\n\
                           updated: 2026-04-30";
        let (p, d) = make_doc(
            &dir.join("crates/pricing-core"),
            "y.md",
            fm_implicit,
            "body",
        );
        let meta = d.meta.unwrap();
        assert_eq!(
            effective_bounded_context(&p, &meta, &cfg, &root),
            Some("pricing".to_string())
        );

        // 3. Default fallback when path doesn't match.
        std::fs::create_dir_all(dir.join("misc")).unwrap();
        let (p, d) = make_doc(&dir.join("misc"), "z.md", fm_implicit, "body");
        let meta = d.meta.unwrap();
        assert_eq!(
            effective_bounded_context(&p, &meta, &cfg, &root),
            Some("docs".to_string())
        );

        // 4. None when no default and no match.
        let mut cfg2 = LintConfig::default();
        cfg2.ontology.bounded_context_paths = BTreeMap::new();
        cfg2.ontology.default_bounded_context = None;
        assert_eq!(effective_bounded_context(&p, &meta, &cfg2, &root), None);
    }

    /// Helper: parse just the `relates_to:` portion of a frontmatter
    /// block and return the resulting [`EntityRelation`] list. Lets
    /// the direct unit tests below feed `parse_relates_to` table-style
    /// inputs without re-implementing the make_doc dance every time.
    fn relates_to_from_yaml(relates_to_block: &str) -> Vec<EntityRelation> {
        let dir = tmp_dir();
        let fm = format!(
            "id: e\n\
             role: ontology-entity\n\
             title: t\n\
             summary: s\n\
             status: stable\n\
             updated: 2026-04-30\n\
             {relates_to_block}"
        );
        let (_p, d) = make_doc(&dir, "e.md", &fm, "body");
        parse_relates_to(d.meta.as_ref().unwrap())
    }

    #[test]
    fn relates_to_absent_returns_empty() {
        assert!(relates_to_from_yaml("").is_empty());
    }

    #[test]
    fn relates_to_single_well_formed_row() {
        let out = relates_to_from_yaml("relates_to:\n  - target: x\n    type: depends_on");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "x");
        assert_eq!(out[0].relation_type, "depends_on");
    }

    #[test]
    fn relates_to_multiple_rows_preserve_order() {
        let out = relates_to_from_yaml(
            "relates_to:\n  \
             - target: a\n    \
             type: depends_on\n  \
             - target: b\n    \
             type: feeds_into\n  \
             - target: c\n    \
             type: calls",
        );
        let rels: Vec<(&str, &str)> = out
            .iter()
            .map(|r| (r.target.as_str(), r.relation_type.as_str()))
            .collect();
        assert_eq!(
            rels,
            vec![("a", "depends_on"), ("b", "feeds_into"), ("c", "calls")]
        );
    }

    #[test]
    fn relates_to_skips_row_missing_target() {
        let out = relates_to_from_yaml(
            "relates_to:\n  - type: depends_on\n  - target: real\n    type: feeds_into",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "real");
    }

    #[test]
    fn relates_to_skips_row_missing_type() {
        let out = relates_to_from_yaml(
            "relates_to:\n  - target: orphan\n  - target: real\n    type: feeds_into",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "real");
    }

    #[test]
    fn relates_to_skips_row_with_empty_strings() {
        // Empty target / empty type both disqualify the row — the
        // validator surfaces the malformed row separately.
        let out = relates_to_from_yaml(
            "relates_to:\n  \
             - target: \"\"\n    \
             type: depends_on\n  \
             - target: real\n    \
             type: \"\"\n  \
             - target: kept\n    \
             type: depends_on",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "kept");
    }

    #[test]
    fn relates_to_trims_whitespace() {
        let out = relates_to_from_yaml(
            "relates_to:\n  - target: \"  spaced  \"\n    type: \"  depends_on  \"",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "spaced");
        assert_eq!(out[0].relation_type, "depends_on");
    }

    #[test]
    fn relates_to_non_sequence_returns_empty() {
        // String instead of a sequence — silently drops, validator
        // emits no row-level diagnostic for a malformed scalar.
        let out = relates_to_from_yaml("relates_to: not-a-list");
        assert!(out.is_empty());
    }

    #[test]
    fn relates_to_non_mapping_items_are_skipped() {
        // A scalar inside the sequence — skipped, neighbouring rows
        // still parse.
        let out = relates_to_from_yaml(
            "relates_to:\n  - bare-string\n  - target: real\n    type: depends_on",
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].target, "real");
    }
}
