//! `.doc-lint.toml` parsing and runtime predicates for the
//! [[entity-doc-graph]] linter. The on-disk schema lives across five
//! sub-modules — [`LintRules`], [`ValeConfig`], [`CoverageConfig`],
//! [`LayoutConfig`], [`OntologyConfig`] — embedded into [`LintConfig`]
//! via `#[serde(flatten)]`, so authors keep writing one flat
//! `.doc-lint.toml` even though Rust readers see the concerns grouped
//! by struct.

mod cluster_lint;
mod concepts;
mod coverage;
mod layout;
mod lint_rules;
mod ontology;
mod vale;

pub use cluster_lint::ClusterLintConfig;
pub use concepts::ConceptsConfig;
pub use coverage::CoverageConfig;
pub use layout::{LayoutConfig, LayoutRule};
pub use lint_rules::LintRules;
pub use ontology::OntologyConfig;
pub use vale::ValeConfig;

use anyhow::{anyhow, Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// On-disk `.doc-lint.toml` schema for the [[entity-doc-graph]] linter.
/// Five flattened sub-structs group the 60+ knobs by concern; the
/// walker globs and the LSP toggles stay at the top level. Loaded once
/// per run and shared by the CLI, the LSP server, and the test harness.
#[derive(Debug, Deserialize, Serialize)]
pub struct LintConfig {
    /// Repo-relative globs selecting which `.md` / `.adoc` files are docs.
    /// `[]` means no docs (a code-only graph). Docs only: code is walked
    /// regardless, so to keep a tree's docs out but its code in, leave it
    /// out of `include` (or add it to `exempt`) rather than `skip_dirs`,
    /// which drops both.
    #[serde(default = "default_include")]
    pub include: Vec<String>,

    /// Repo-relative globs the linter ignores entirely: not parsed, not
    /// indexed, not linted (third-party READMEs, generated files, agent
    /// prompt files). Setting it replaces the defaults.
    #[serde(default = "default_exempt")]
    pub exempt: Vec<String>,

    /// Directory names (not paths) never walked, at any depth: build
    /// output, dependencies, VCS and editor state. Setting it replaces the
    /// defaults, so copy them in when adding one. `build/` is also skipped
    /// next to a Gradle build script.
    #[serde(default = "default_skip_dirs")]
    pub skip_dirs: Vec<String>,

    /// Inline backtick strings that look like file paths but should not be
    /// flagged as `unlinked-path`. Used for convention references that don't
    /// correspond to a single file (e.g. `AGENTS.md` — every workspace has one).
    #[serde(default)]
    pub unlinked_path_exempt: Vec<String>,

    /// Additional vault roots to include in the path/id/stem index but NOT
    /// validate. Lets markdown links and wikilinks resolve into sibling git
    /// repos without imposing this repo's schema on them.
    ///
    /// Paths are relative to the repo root. Typical usage from a
    /// monorepo to reach client satellite repos:
    ///   cross_repo_roots = ["../clients/mayora", "../clients/ndc", ...]
    #[serde(default)]
    pub cross_repo_roots: Vec<String>,

    /// Run the embeddings pass (`*_embedding` vectors for `query_similar
    /// backend=embedding`) by default: on `check`, MCP `reingest` and the
    /// `mcp` first-start build. `--no-embeddings` / `reingest
    /// {embeddings:false}` still turn it off for one run. Off by default;
    /// warm runs are cheap because unchanged text reuses cached vectors.
    #[serde(default)]
    pub embeddings: bool,

    /// Repo-relative subdirectories to try as fallback bases when a
    /// `read_source` request's path is not found under the repo root.
    /// Needed when the SCIP code index was generated from a sub-project
    /// (e.g. a Cargo workspace under `sdks/rust/`), so the
    /// graph stores `File.path` relative to THAT dir, not the repo root.
    /// `read_source` tries the root first, then each entry in order; the
    /// resolved file must still stay within the repo root.
    /// For example `code_source_roots = ["libs/sdk"]`.
    #[serde(default)]
    pub code_source_roots: Vec<String>,

    /// Round 4A: when true (the default), the LSP server re-lints on every
    /// `did_change` event (i.e. every keystroke after debounce). Set to
    /// false to lint only on `did_save` — useful for very large files
    /// where per-keystroke latency hurts more than the missing feedback.
    #[serde(default = "default_lsp_lint_on_change")]
    pub lsp_lint_on_change: bool,

    /// Round 4A: when true (the default), `initialize` walks the entire
    /// corpus once to build the ontology and id index. Setting this to
    /// false defers the walk and resolves wikilinks lazily — drops
    /// startup latency at the cost of slightly slower first-keystroke
    /// diagnostics on a cold workspace.
    #[serde(default = "default_lsp_corpus_walk_on_init")]
    pub lsp_corpus_walk_on_init: bool,

    #[serde(default, skip)]
    pub exempt_set: Option<GlobSet>,

    #[serde(default, skip)]
    pub include_set: Option<GlobSet>,

    #[serde(default, skip)]
    pub unlinked_path_exempt_set: Option<GlobSet>,

    #[serde(flatten)]
    pub lint_rules: LintRules,

    #[serde(flatten)]
    pub vale: ValeConfig,

    #[serde(flatten)]
    pub coverage: CoverageConfig,

    #[serde(flatten)]
    pub layout: LayoutConfig,

    #[serde(flatten)]
    pub ontology: OntologyConfig,

    /// Roadmap issue #14 follow-up (self-healing ontology):
    /// gates the check-time `unauthored-cluster` lint. Nested
    /// under its own TOML table (`[cluster_lint]`) rather than
    /// flattened — the keys are namespaced so authors can read
    /// `cluster_lint.enabled = true` and know the scope at a
    /// glance.
    #[serde(default, rename = "cluster_lint")]
    pub cluster_lint: ClusterLintConfig,

    /// `[concepts]` table: the optional `namer_command` hook of
    /// `ontology propose`.
    #[serde(default)]
    pub concepts: ConceptsConfig,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            include: default_include(),
            exempt: default_exempt(),
            skip_dirs: default_skip_dirs(),
            unlinked_path_exempt: Vec::new(),
            cross_repo_roots: Vec::new(),
            embeddings: false,
            code_source_roots: Vec::new(),
            lsp_lint_on_change: default_lsp_lint_on_change(),
            lsp_corpus_walk_on_init: default_lsp_corpus_walk_on_init(),
            exempt_set: None,
            include_set: None,
            unlinked_path_exempt_set: None,
            lint_rules: LintRules::default(),
            vale: ValeConfig::default(),
            coverage: CoverageConfig::default(),
            layout: LayoutConfig::default(),
            ontology: OntologyConfig::default(),
            cluster_lint: ClusterLintConfig::default(),
            concepts: ConceptsConfig::default(),
        }
    }
}

fn default_include() -> Vec<String> {
    vec!["**/*.md".to_string(), "**/*.adoc".to_string()]
}

// AI-agent prompt files (Claude Code, opencode, Cursor) follow their own
// per-tool conventions, not the doc ontology. Default-exempt them so a
// fresh repo doesn't trip on day one.
fn default_exempt() -> Vec<String> {
    vec![
        "CLAUDE.md".to_string(),
        "**/CLAUDE.md".to_string(),
        "AGENTS.md".to_string(),
        "**/AGENTS.md".to_string(),
    ]
}

// Build output and dependency caches of the languages the code-comment
// lint reads (Cargo, npm, .NET, Flutter/Dart, Nuxt/Next). Never authored
// prose; walking them only costs time and floods the report.
fn default_skip_dirs() -> Vec<String> {
    [
        "target",
        ".git",
        ".obsidian",
        "node_modules",
        "bin",
        "obj",
        ".dart_tool",
        ".nuxt",
        ".output",
        ".next",
        "dist",
        "coverage",
    ]
    .map(String::from)
    .to_vec()
}

fn default_lsp_lint_on_change() -> bool {
    true
}

fn default_lsp_corpus_walk_on_init() -> bool {
    true
}

/// #266: keys in a `.doc-lint.toml` that no config field reads, one
/// message each. The sub-configs are `#[serde(flatten)]`ed, so serde
/// drops unknown keys silently; this diffs the file's keys against the
/// fields a default config serialises (JSON keeps `None` fields). A
/// known key misplaced under a table (`[coverage]
/// endpoint_marker_exclusive`) is pointed at its top-level home.
pub fn unknown_key_warnings(text: &str) -> Vec<String> {
    let (Ok(toml::Value::Table(user)), Ok(serde_json::Value::Object(known))) = (
        text.parse::<toml::Value>(),
        serde_json::to_value(LintConfig::default()),
    ) else {
        return Vec::new(); // a parse error is reported by the real load
    };
    let mut out = Vec::new();
    unknown_keys_in(&user, &known, "", &known, &mut out);
    out
}

fn unknown_keys_in(
    user: &toml::Table,
    known: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    top: &serde_json::Map<String, serde_json::Value>,
    out: &mut Vec<String>,
) {
    for (key, value) in user {
        // `allowed_types` is a deserialize-only alias of a retired field.
        if prefix.is_empty() && key == "allowed_types" {
            continue;
        }
        match (known.get(key), value) {
            // A struct table (`[cluster_lint]`): check its keys too. Map
            // fields default to empty and take any key.
            (Some(serde_json::Value::Object(fields)), toml::Value::Table(table))
                if !fields.is_empty() =>
            {
                unknown_keys_in(table, fields, &format!("{prefix}{key}."), top, out);
            }
            (Some(_), _) => {}
            (None, toml::Value::Table(table)) if table.keys().any(|k| top.contains_key(k)) => {
                for sub in table.keys() {
                    if top.contains_key(sub) {
                        out.push(format!(
                            "`{prefix}{key}.{sub}` is not read; did you mean the top-level key `{sub}`?"
                        ));
                    } else {
                        out.push(format!("unknown key `{prefix}{key}.{sub}`"));
                    }
                }
            }
            (None, _) => out.push(format!("unknown key `{prefix}{key}`")),
        }
    }
}

/// Loader and predicate accessors for the [[entity-doc-graph]]
/// `.doc-lint.toml` configuration.
impl LintConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let mut cfg: LintConfig = if path.exists() {
            let text =
                fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
            for warning in unknown_key_warnings(&text) {
                eprintln!("doc-linter: warning: {}: {warning}", path.display());
            }
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?
        } else {
            LintConfig::default()
        };

        // Issue #180: `auto` and `candidate` are doc-linter-managed
        // statuses emitted by `cluster --write`. They must validate
        // against `allowed_statuses` regardless of repo override, so
        // that bumping doc-linter doesn't force every consumer repo
        // to also bump their `.doc-lint.toml`. Append unconditionally
        // — the user can still extend `allowed_statuses` for their
        // own custom statuses; we just guarantee the managed ones
        // are always present.
        for managed in &["auto", "candidate"] {
            if !cfg.lint_rules.allowed_statuses.iter().any(|s| s == managed) {
                cfg.lint_rules.allowed_statuses.push((*managed).to_string());
            }
        }

        cfg.exempt_set = Some(build_glob_set(&cfg.exempt, "exempt")?);
        cfg.include_set = Some(build_glob_set(&cfg.include, "include")?);
        cfg.unlinked_path_exempt_set = Some(build_glob_set(
            &cfg.unlinked_path_exempt,
            "unlinked_path_exempt",
        )?);
        cfg.vale.code_comment_include_set = Some(build_glob_set(
            &cfg.vale.code_comment_includes,
            "code_comment_includes",
        )?);
        cfg.vale.code_comment_exclude_set = Some(build_glob_set(
            &cfg.vale.code_comment_excludes,
            "code_comment_excludes",
        )?);

        // Phase 5c: codegen exclusion globs. Compiled once at load
        // time; bail with the offending pattern so authors find typos
        // the moment the linter runs.
        cfg.coverage.coverage_codegen_exclude_set = Some(build_glob_set(
            &cfg.coverage.coverage_codegen_exclude,
            "coverage_codegen_exclude",
        )?);

        // Phase 5c: function-name exempt regexes. Same fail-fast
        // discipline — a malformed pattern in `.doc-lint.toml` should
        // halt the lint, not silently exempt nothing.
        let mut exempt_regexes: Vec<Regex> =
            Vec::with_capacity(cfg.coverage.coverage_function_exempt.len());
        for pat in &cfg.coverage.coverage_function_exempt {
            let r = Regex::new(pat)
                .with_context(|| format!("invalid coverage_function_exempt regex: {pat}"))?;
            exempt_regexes.push(r);
        }
        cfg.coverage.coverage_function_exempt_set = Some(exempt_regexes);

        // Roadmap-48 Rule A: load the anchor-exempt grandfathered
        // list. The path is repo-relative; resolve it against the
        // config file's parent directory (which is the repo root in
        // the canonical layout). Missing file is fine — the field
        // simply stays `None` and `is_function_exempt` falls back to
        // the regex-only check.
        let exempt_path = if cfg.coverage.coverage_anchor_exempt_file.is_absolute() {
            cfg.coverage.coverage_anchor_exempt_file.clone()
        } else if let Some(parent) = path.parent() {
            parent.join(&cfg.coverage.coverage_anchor_exempt_file)
        } else {
            cfg.coverage.coverage_anchor_exempt_file.clone()
        };
        if exempt_path.exists() {
            let text = fs::read_to_string(&exempt_path)
                .with_context(|| format!("reading {}", exempt_path.display()))?;
            let mut set: HashSet<String> = HashSet::new();
            for raw in text.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                set.insert(line.to_string());
            }
            cfg.coverage.coverage_anchor_exempt_set = Some(set);
        }

        // Roadmap-48 Rule C: validate severity at load time so
        // typos in `.doc-lint.toml` surface immediately, not on the
        // first orphan-entity emission.
        validate_severity(
            "orphan_entity_severity",
            &cfg.coverage.orphan_entity_severity,
        )?;
        validate_severity(
            "missing_covers_severity",
            &cfg.ontology.missing_covers_severity,
        )?;
        validate_severity(
            "missing_bounded_context_severity",
            &cfg.ontology.missing_bounded_context_severity,
        )?;
        validate_severity(
            "placeholder_entity_severity",
            &cfg.ontology.placeholder_entity_severity,
        )?;
        validate_severity(
            "homepage_stale_severity",
            &cfg.ontology.homepage_stale_severity,
        )?;
        validate_severity(
            "missing_body_wikilink_severity",
            &cfg.ontology.missing_body_wikilink_severity,
        )?;
        validate_severity(
            "unused_bounded_context_severity",
            &cfg.ontology.unused_bounded_context_severity,
        )?;

        Ok(cfg)
    }

    /// True when `path` is in scope for the [[entity-doc-graph]] code-comment
    /// vocab-closure pipeline (Round 3A) — it matches the
    /// `code_comment_includes` globs and not the excludes. A file whose
    /// extension no include pattern mentions falls through as included, so
    /// a repo whose include list predates a language (say `["**/*.rs"]`
    /// before C# support) still gets that language linted by the per-file
    /// hook rather than silently skipped.
    pub fn matches_code_comment(&self, path: &Path, root: &Path) -> bool {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let unscoped_ext = path
            .extension()
            .and_then(|s| s.to_str())
            .is_some_and(|ext| {
                let dotted = format!(".{ext}");
                !self
                    .vale
                    .code_comment_includes
                    .iter()
                    .any(|p| p.contains(&dotted))
            });
        let include = unscoped_ext
            || self
                .vale
                .code_comment_include_set
                .as_ref()
                .is_none_or(|g| g.is_match(rel));
        include
            && !self
                .vale
                .code_comment_exclude_set
                .as_ref()
                .is_some_and(|g| g.is_match(rel))
    }

    /// True if `path` (absolute) lies under one of the [[entity-doc-graph]]
    /// `anchor_required_in` crate prefixes — i.e. its public-API doc-comments
    /// must reference at least one ontology entity (Roadmap-48 Rule A).
    /// Path comparison is by repo-relative prefix match, so listing
    /// `crates/pricing-core` covers every `.rs` file in that crate.
    pub fn requires_anchor(&self, path: &Path, root: &Path) -> bool {
        if !self.coverage.require_anchor_per_pubapi {
            return false;
        }
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_str = rel.to_string_lossy();
        self.coverage
            .anchor_required_in
            .iter()
            .any(|p| rel_str.starts_with(p.as_str()))
    }

    /// True when `text` matches an `unlinked_path_exempt` glob, suppressing
    /// the bare-path warning in the [[entity-doc-graph]] validator.
    pub fn is_unlinked_path_exempt(&self, text: &str) -> bool {
        self.unlinked_path_exempt_set
            .as_ref()
            .is_some_and(|g| g.is_match(text))
    }

    /// True when `path` matches a configured exempt glob, telling the
    /// [[entity-doc-graph]] walker to skip the file entirely.
    /// True when `path` (under `root`) is a doc: a `.md` / `.adoc` file
    /// matching `include`. A config built without [`LintConfig::load`]
    /// has no compiled set and takes every such file.
    pub fn is_doc_included(&self, path: &Path, root: &Path) -> bool {
        let is_doc = matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("md" | "adoc")
        );
        is_doc
            && self
                .include_set
                .as_ref()
                .is_none_or(|globs| globs.is_match(path.strip_prefix(root).unwrap_or(path)))
    }

    pub fn is_exempt(&self, path: &Path, root: &Path) -> bool {
        let Some(globs) = self.exempt_set.as_ref() else {
            return false;
        };
        if let Ok(rel) = path.strip_prefix(root) {
            return globs.is_match(rel);
        }
        for cr in &self.cross_repo_roots {
            let candidate = root.join(cr);
            let cr_root = candidate.canonicalize().unwrap_or(candidate);
            if let Ok(rel) = path.strip_prefix(&cr_root) {
                if globs.is_match(rel) {
                    return true;
                }
            }
        }
        false
    }

    /// True when `path` matches a `skip_dirs` entry, pruning the directory
    /// from the [[entity-doc-graph]] file walker.
    pub fn should_skip_dir(&self, path: &Path, root: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        if path == root {
            return false;
        }
        // Gradle output lives in `build/`, but so does real source in other
        // languages (`src/build/`, Go `internal/build`), so only skip it
        // next to a Gradle build script.
        if name == "build" {
            let parent = path.parent().unwrap_or(root);
            if parent.join("build.gradle").is_file() || parent.join("build.gradle.kts").is_file() {
                return true;
            }
        }
        self.skip_dirs.iter().any(|d| d == name)
    }

    /// Phase 5c: true when `repo_relative_path` matches any
    /// `coverage_codegen_exclude` glob — i.e. the file is a codegen sentinel
    /// and its Function nodes should not be ingested into the
    /// [[entity-doc-graph]] code-graph at all. Falls back to `false` (no
    /// exclusion) when the glob set hasn't been compiled yet (e.g. tests
    /// that build `LintConfig::default()` directly without going through
    /// [`LintConfig::load`]).
    pub fn is_codegen_excluded(&self, repo_relative_path: &Path) -> bool {
        self.coverage
            .coverage_codegen_exclude_set
            .as_ref()
            .is_some_and(|g| g.is_match(repo_relative_path))
    }

    /// Phase 5c (header-sniff variant): true when either the file's path
    /// matches `coverage_codegen_exclude` (the explicit glob list) OR the
    /// file's first 1 KiB contains a recognised "auto-generated / do not
    /// edit" marker. See [`is_codegen_header`] for the marker list.
    pub fn is_codegen_excluded_with_header(
        &self,
        repo_relative_path: &Path,
        abs_path: &Path,
    ) -> bool {
        if self.is_codegen_excluded(repo_relative_path) {
            return true;
        }
        is_codegen_header(abs_path)
    }

    /// Phase 5c: true when `bare_fn_name` matches any
    /// `coverage_function_exempt` regex. Used at COUNT TIME (not
    /// ingest time): the Function node still exists in the graph and
    /// keeps its FUNCTION_MENTIONS edges, but it doesn't count toward
    /// the dark-function metric, the per-crate dark count, or the
    /// scaffold-coverage proposals.
    ///
    /// Roadmap-48 Rule A extends the predicate with a file-based
    /// grandfathered list (loaded from `coverage_anchor_exempt_file`).
    /// A bare name present in either source — regex match OR exact
    /// match in the loaded set — is exempt.
    pub fn is_function_exempt(&self, bare_fn_name: &str) -> bool {
        if let Some(rxs) = self.coverage.coverage_function_exempt_set.as_ref() {
            if rxs.iter().any(|r| r.is_match(bare_fn_name)) {
                return true;
            }
        }
        if let Some(set) = &self.coverage.coverage_anchor_exempt_set {
            if set.contains(bare_fn_name) {
                return true;
            }
        }
        false
    }
}

/// Compiles `patterns` into a [`GlobSet`], wrapping each parse error
/// with the offending pattern and the field name for fast triage.
fn build_glob_set(patterns: &[String], field: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        builder.add(Glob::new(pat).with_context(|| format!("invalid {field} glob: {pat}"))?);
    }
    Ok(builder.build()?)
}

/// Validates that a severity string is one of `"warning"` / `"error"`,
/// returning a uniform `anyhow` message that names the field — typos
/// in `.doc-lint.toml` surface at load time, not on the first emission.
fn validate_severity(field: &str, value: &str) -> Result<()> {
    match value {
        "warning" | "error" => Ok(()),
        other => Err(anyhow!(
            "invalid {field} `{other}` — expected \"warning\" or \"error\""
        )),
    }
}

/// Phase 5c (roadmap-43): true when the file at `abs_path` looks
/// auto-generated by reading its first 1 KiB and matching against the
/// standard "do not edit" / "@generated" markers most codegen tools
/// emit. Used by the [[entity-doc-graph]] per-file ingest to skip
/// codegen output (frb_generated.rs etc.) so vendor-generated prose
/// isn't held to the vocab-closure contract.
///
/// Recognised markers (case-insensitive substring on the first 1 KiB):
///   - `@generated` — Buck/protoc/sqlx-macro convention
///   - `Code generated` — Go convention, picked up by some Rust tools
///   - `DO NOT EDIT` — common across most tools
///   - `auto-generated` / `automatically generated`
///   - `Generated by` / `generated file`
///   - `automatically_derived` — Rust's derive-macro attribute
///
/// Returns `false` on read errors. Callers should cache per file.
pub fn is_codegen_header(abs_path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut file) = fs::File::open(abs_path) else {
        return false;
    };
    let mut buf = [0u8; 1024];
    let n = match file.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let head = String::from_utf8_lossy(&buf[..n]);
    let head_lower = head.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "@generated",
        "code generated",
        "do not edit",
        "auto-generated",
        "automatically generated",
        "automatically_derived",
        "generated by",
        "generated file",
    ];
    MARKERS.iter().any(|m| head_lower.contains(m))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod codegen_header_tests {
    use super::is_codegen_header;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_tmp(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-codegen-test-{}-{}",
            std::process::id(),
            name
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("file.rs");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    #[test]
    fn detects_at_generated_marker() {
        let p = write_tmp(
            "at-generated",
            "// @generated by some-tool 1.0\nfn foo() {}\n",
        );
        assert!(is_codegen_header(&p));
    }

    #[test]
    fn detects_do_not_edit_marker_case_insensitive() {
        let p = write_tmp("dne-mixed", "// DO NOT EDIT\nfn foo() {}\n");
        assert!(is_codegen_header(&p));
        let p2 = write_tmp("dne-lower", "// do not edit this file\nfn foo() {}\n");
        assert!(is_codegen_header(&p2));
    }

    #[test]
    fn detects_code_generated_marker() {
        let p = write_tmp(
            "code-gen",
            "// Code generated by protoc-gen-prost. DO NOT EDIT.\nfn foo() {}\n",
        );
        assert!(is_codegen_header(&p));
    }

    #[test]
    fn detects_frb_generated_header() {
        let p = write_tmp(
            "frb",
            "// This file is automatically generated, so please do not edit it.\n\
             use std::sync::Arc;\nfn foo() {}\n",
        );
        assert!(is_codegen_header(&p));
    }

    #[test]
    fn human_authored_file_returns_false() {
        let p = write_tmp(
            "human",
            "//! Pricing rule resolver.\n//!\n//! Walks the JDM graph.\n\nfn resolve() {}\n",
        );
        assert!(!is_codegen_header(&p));
    }

    #[test]
    fn missing_file_returns_false() {
        let bogus = PathBuf::from("/this/path/does/not/exist.rs");
        assert!(!is_codegen_header(&bogus));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod skip_dir_tests {
    #[test]
    fn build_dir_is_skipped_only_next_to_a_gradle_script() {
        let root =
            std::env::temp_dir().join(format!("doc-linter-skip-build-{}", std::process::id()));
        let gradle = root.join("app");
        let plain = root.join("src");
        std::fs::create_dir_all(gradle.join("build")).unwrap();
        std::fs::create_dir_all(plain.join("build")).unwrap();
        std::fs::write(gradle.join("build.gradle.kts"), "").unwrap();
        let cfg = super::LintConfig::default();
        assert!(cfg.should_skip_dir(&gradle.join("build"), &root));
        assert!(!cfg.should_skip_dir(&plain.join("build"), &root));
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(test)]
mod unknown_key_tests {
    use super::unknown_key_warnings;

    /// #266: a key no field reads is reported; a known key misplaced
    /// under a table gets a pointer to its top-level home.
    #[test]
    fn unknown_and_misplaced_keys_are_reported() {
        let text = "exempt = []\n\
                    endpoint_marker_exclusiv = true\n\
                    [coverage]\n\
                    endpoint_marker_exclusive = false\n\
                    [cluster_lint]\n\
                    enabled = true\n\
                    min_member = 3\n";
        assert_eq!(
            unknown_key_warnings(text),
            [
                "unknown key `cluster_lint.min_member`",
                "`coverage.endpoint_marker_exclusive` is not read; \
                 did you mean the top-level key `endpoint_marker_exclusive`?",
                "unknown key `endpoint_marker_exclusiv`",
            ]
        );
    }

    /// Every shipped or real-world config is free of warnings.
    #[test]
    fn known_configs_have_no_unknown_keys() {
        for text in [
            include_str!("../../templates/init/doc-lint.toml"),
            include_str!("../../.doc-lint.toml"),
            "allowed_types = []\n[[layout_rules]]\nrole = \"doc\"\nallowed_paths = []\n",
        ] {
            assert_eq!(unknown_key_warnings(text), Vec::<String>::new());
        }
    }
}
