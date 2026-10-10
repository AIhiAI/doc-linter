//! Vale + code-comment vocab-closure settings for the
//! [[entity-doc-graph]] linter. Two pipelines share this sub-struct
//! because they share the philosophy — every domain noun in prose
//! must resolve to a registered ontology entity — and the
//! `*_extra_accept` knobs feed both.
//!
//! Embedded as `LintConfig.vale` via `#[serde(flatten)]`; on-disk
//! `.doc-lint.toml` keeps the same top-level field names.
use std::collections::BTreeMap;
use std::path::PathBuf;

use globset::GlobSet;
use serde::{Deserialize, Serialize};

/// Toggles + accept-list extensions for the markdown (Vale) and
/// source-comment (tree-sitter) vocab-closure pipelines, plus the
/// per-tool include/exclude globs for the latter.
#[derive(Debug, Deserialize, Serialize)]
pub struct ValeConfig {
    /// Whether to run the Vale glossary linter as part of `doc-linter check`.
    /// Vale provides "vocabulary closure" checking — every domain noun in a
    /// doc body must resolve to a registered ontology entity. When enabled
    /// (the default), `check` regenerates `<root>/.doc-lint/vale/` from the
    /// ontology and shells out to `vale`. If the binary isn't installed the
    /// check emits one `vale-missing` diagnostic and continues. Disable
    /// per-repo with `vale_enabled = false` or per-invocation with `--no-vale`.
    /// `.adoc` docs are checked too, through a built-in prose extractor
    /// (no `asciidoctor` needed).
    #[serde(default = "default_vale_enabled")]
    pub vale_enabled: bool,

    /// Bare ambiguous English nouns flagged by the Vale `DocLinter.AmbiguousBare`
    /// rule. Authors must qualify these (e.g. "pricing rule" not "rule") so
    /// the term resolves to a single ontology entity. Round 2A's bounded-
    /// context post-processor decides which Vale alerts to suppress vs.
    /// downgrade based on the doc's effective context — extending this list
    /// just feeds more candidate terms into that pipeline. Migration: this
    /// rule was named `FA.AmbiguousBare` before v0.3.4; JSON consumers that
    /// matched the code `vale-fa-ambiguousbare` must now match
    /// `vale-doclinter-ambiguousbare`.
    #[serde(default = "default_vale_ambiguous_words")]
    pub vale_ambiguous_words: Vec<String>,

    /// Phase 2 of the vocabulary-closure cleanup plan: extra terms that
    /// merge into Vale's `accept.txt` without becoming full ontology
    /// entities. Use for common technical terms (Postgres, JSON, HTTP,
    /// AWS, JWT, units like MB/GB) and brand acronyms that aren't
    /// domain vocabulary.
    #[serde(default)]
    pub vale_extra_accept: Vec<String>,

    /// Phase 2: proper nouns (tenant names, brand acronyms, region/people
    /// names) that vocabulary closure should accept but that aren't
    /// ontology entities. Same plumbing as `vale_extra_accept`; separate
    /// field so config files read cleanly.
    #[serde(default)]
    pub proper_nouns: Vec<String>,

    /// Phase 3-extension: external vocabulary dictionaries (cspell-style
    /// `.txt` files, one term per line). Each entry maps a Vale Vocab
    /// name (used in the generated `.vale.ini`'s `Vocab = ...` line) to
    /// a file path relative to the repo root.
    ///
    /// Each dictionary becomes its own Vocab pack under
    /// `.doc-lint/vale/styles/config/vocabularies/<name>/accept.txt`.
    /// Vale consults all listed packs case-insensitively. Keeping
    /// dictionaries in separate packs (instead of merging them into the
    /// ontology-derived accept-list) means refreshing a cspell dict
    /// is a one-shot file replace.
    ///
    /// ```toml
    /// [vale_dictionaries]
    /// AWS = "dicts/aws.txt"
    /// SoftwareTerms = "dicts/software-terms.txt"
    /// Rust = "dicts/rust.txt"
    /// ```
    #[serde(default)]
    pub vale_dictionaries: BTreeMap<String, PathBuf>,

    /// Round 3A: opt-in lint pass that extracts doc comments from `.rs`
    /// files (using tree-sitter) and runs them through the same
    /// vocab-closure resolution as markdown docs. Off by default — turn
    /// on per-repo with `lint_code_comments = true` or per-invocation
    /// with `--lint-code-comments`.
    #[serde(default)]
    pub lint_code_comments: bool,

    /// Glob patterns selecting which source files to comment-lint.
    /// Defaults to every language the extractors read. Patterns are
    /// evaluated against repo-relative paths.
    #[serde(default = "default_code_comment_includes")]
    pub code_comment_includes: Vec<String>,

    /// Glob patterns excluded from the comment lint. Defaults to
    /// `["target/**", "**/tests/**", "**/build.rs"]` — those trees are
    /// either generated, test scaffolding, or build glue and don't carry
    /// authored prose worth linting.
    #[serde(default = "default_code_comment_excludes")]
    pub code_comment_excludes: Vec<String>,

    /// Round 3A followup (task #4): extra terms that the code-comment
    /// vocab pipeline should treat as accepted, alongside the
    /// hardcoded `STOPWORDS` list. Use for tech terms (Postgres, JSON,
    /// HTTP, cloud product names, etc.) that aren't domain
    /// vocabulary but routinely appear in `///` Rust doc-comments.
    /// Mirrors `vale_extra_accept` for the markdown pipeline; the
    /// two knobs feed different pipelines but share the philosophy.
    #[serde(default)]
    pub code_comment_extra_accept: Vec<String>,

    #[serde(default, skip)]
    pub code_comment_include_set: Option<GlobSet>,

    #[serde(default, skip)]
    pub code_comment_exclude_set: Option<GlobSet>,
}

impl Default for ValeConfig {
    fn default() -> Self {
        Self {
            vale_enabled: default_vale_enabled(),
            vale_ambiguous_words: default_vale_ambiguous_words(),
            vale_extra_accept: Vec::new(),
            proper_nouns: Vec::new(),
            vale_dictionaries: BTreeMap::new(),
            lint_code_comments: false,
            code_comment_includes: default_code_comment_includes(),
            code_comment_excludes: default_code_comment_excludes(),
            code_comment_extra_accept: Vec::new(),
            code_comment_include_set: None,
            code_comment_exclude_set: None,
        }
    }
}

fn default_vale_enabled() -> bool {
    true
}

fn default_vale_ambiguous_words() -> Vec<String> {
    vec![
        "system", "service", "module", "platform", "engine", "rule", "data",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Every extension [`crate::code_comments::Lang`] reads.
fn default_code_comment_includes() -> Vec<String> {
    [
        "**/*.rs",
        "**/*.ts",
        "**/*.tsx",
        "**/*.js",
        "**/*.jsx",
        "**/*.py",
        "**/*.cs",
        "**/*.dart",
        "**/*.vue",
        "**/*.java",
    ]
    .map(String::from)
    .to_vec()
}

fn default_code_comment_excludes() -> Vec<String> {
    vec![
        "target/**".to_string(),
        "**/tests/**".to_string(),
        // Maven / Gradle test sources.
        "**/src/test/**".to_string(),
        "**/build.rs".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    /// `doc-linter init` writes the built-in `vale_ambiguous_words` list
    /// out in full; keep the two from drifting.
    #[test]
    fn init_template_ambiguous_words_match_the_default() {
        let template: crate::config::LintConfig =
            toml::from_str(include_str!("../../templates/init/doc-lint.toml")).unwrap();
        assert_eq!(
            template.vale.vale_ambiguous_words,
            super::default_vale_ambiguous_words()
        );
    }
}
