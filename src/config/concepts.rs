//! `[concepts]` table of `.doc-lint.toml`: knobs for `doc-linter ontology
//! propose` (docs/design/local-concept-proposer.md).

use serde::{Deserialize, Serialize};

/// `namer_command` is an optional local program (llama.cpp wrapper, Ollama
/// script, anything) that names a candidate. `propose` writes one JSON
/// object to its stdin and reads `{"name","description"}` from stdout; any
/// failure or timeout falls back to the TF-IDF name. doc-linter itself
/// contains no network code.
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
pub struct ConceptsConfig {
    #[serde(default)]
    pub namer_command: Option<String>,
}
