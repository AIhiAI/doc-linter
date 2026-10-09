//! `doc-linter ontology` subcommand. Loads the vault's ontology from
//! `docs/ontology/` and emits it as JSON. Cold-start endpoint for AI
//! agents discovering the vocabulary they should target.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use serde::Serialize;

use doc_linter::config::LintConfig;
use doc_linter::ontology::Ontology;
use doc_linter::parser::{parse_doc, Doc};

/// The ontology assembled from the vault's `docs/ontology/` files.
pub(crate) fn load(all_files: &[PathBuf]) -> Ontology {
    let mut docs: HashMap<PathBuf, Doc> = HashMap::new();
    for path in all_files {
        if let Ok(doc) = parse_doc(path) {
            docs.insert(path.clone(), doc);
        }
    }
    Ontology::load_from_docs(&docs)
}

/// Output shape:
///
/// ```json
/// {
///   "version": 1,
///   "axes": {
///     "role": {
///       "values": [{"id": "doc", "display": "...", "requires_axes": ["kind"], ...}, ...]
///     },
///     "kind":      {"values": [...]},
///     "lifecycle": {"values": [...]},
///     "covers":    {"open": true, "values": [{"id": "outlet", "synonyms": [...], ...}, ...]}
///   },
///   "migrations": [{"from": 0, "to": 1, "doc_id": "ontology-mig-0001"}, ...]
/// }
/// ```
/// @endpoint CLI ontology
pub(crate) fn run(_root: &Path, _config: &LintConfig, all_files: &[PathBuf]) -> Result<ExitCode> {
    let ontology = load(all_files);

    #[derive(Serialize)]
    struct RoleOut<'a> {
        id: &'a str,
        display: &'a str,
        description: &'a str,
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        requires_axes: &'a [String],
        #[serde(skip_serializing_if = "Option::is_none")]
        allowed_lifecycle: Option<&'a [String]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename_pattern: Option<&'a str>,
        /// True when this role is hardcoded into the linter (a meta-role
        /// like `ontology-axis` / `ontology-value`). False when the role
        /// was loaded from a `value-role-*` doc and can be redefined per
        /// repo. AI agents use this to know which roles are extensible.
        bootstrap: bool,
    }
    /// Serde shape for one ontology value row in the
    /// [[entity-doc-graph]] `ontology` JSON dump.
    #[derive(Serialize)]
    struct ValueOut<'a> {
        id: &'a str,
        display: &'a str,
        description: &'a str,
    }
    /// Serde shape for one entity row (with synonyms + bounded contexts)
    /// in the [[entity-doc-graph]] `ontology` JSON dump.
    #[derive(Serialize)]
    struct EntityWithCtxOut<'a> {
        id: &'a str,
        display: &'a str,
        description: &'a str,
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        synonyms: &'a [String],
        #[serde(skip_serializing_if = "<[String]>::is_empty")]
        bounded_contexts: &'a [String],
    }
    /// Top-level serde shape for the [[entity-doc-graph]] `ontology`
    /// subcommand JSON output — version, roles, kinds, lifecycles, and
    /// bounded contexts.
    #[derive(Serialize)]
    struct OntologyOut<'a> {
        version: u32,
        roles: Vec<RoleOut<'a>>,
        kinds: Vec<ValueOut<'a>>,
        lifecycles: Vec<ValueOut<'a>>,
        #[serde(rename = "bounded-contexts")]
        bounded_contexts: Vec<ValueOut<'a>>,
        entities: Vec<EntityWithCtxOut<'a>>,
    }

    let mut roles: Vec<RoleOut> = ontology
        .roles
        .values()
        .map(|r| RoleOut {
            id: &r.id,
            display: &r.display,
            description: &r.description,
            requires_axes: &r.requires_axes,
            allowed_lifecycle: r.allowed_lifecycle.as_deref(),
            filename_pattern: r.filename_pattern.as_deref(),
            bootstrap: r.bootstrap,
        })
        .collect();
    roles.sort_by(|a, b| a.id.cmp(b.id));

    let mut kinds: Vec<ValueOut> = ontology
        .kinds
        .values()
        .map(|v| ValueOut {
            id: &v.id,
            display: &v.display,
            description: &v.description,
        })
        .collect();
    kinds.sort_by(|a, b| a.id.cmp(b.id));

    let mut lifecycles: Vec<ValueOut> = ontology
        .lifecycles
        .values()
        .map(|v| ValueOut {
            id: &v.id,
            display: &v.display,
            description: &v.description,
        })
        .collect();
    lifecycles.sort_by(|a, b| a.id.cmp(b.id));

    let mut bounded_contexts: Vec<ValueOut> = ontology
        .bounded_contexts
        .values()
        .map(|v| ValueOut {
            id: &v.id,
            display: &v.display,
            description: &v.description,
        })
        .collect();
    bounded_contexts.sort_by(|a, b| a.id.cmp(b.id));

    let mut entities: Vec<EntityWithCtxOut> = ontology
        .entities
        .values()
        .map(|e| EntityWithCtxOut {
            id: &e.id,
            display: &e.display,
            description: &e.description,
            synonyms: &e.synonyms,
            bounded_contexts: &e.bounded_contexts,
        })
        .collect();
    entities.sort_by(|a, b| a.id.cmp(b.id));

    let out = OntologyOut {
        version: ontology.version,
        roles,
        kinds,
        lifecycles,
        bounded_contexts,
        entities,
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(ExitCode::SUCCESS)
}
