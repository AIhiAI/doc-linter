//! Vale (https://vale.sh) integration for doc-linter (Round 2A).
//!
//! Vale is a configurable command-line prose linter. We use it as the
//! glossary engine for "vocabulary closure" — every domain noun in a doc
//! body must resolve to a registered ontology entity, or be an explicitly
//! ambiguous English noun that the post-processor (see
//! [`crate::disambiguation`]) clears using bounded-context rules.
//!
//! This module owns:
//!   1. Generating a self-contained Vale config tree under
//!      `<root>/.doc-lint/vale/` from the live ontology on every `check`
//!      run. The directory is gitignored and idempotent — files only get
//!      rewritten when their content changes, so back-to-back runs don't
//!      churn mtimes (handy for editor watchers).
//!   2. Detecting the `vale` binary on `PATH`. Missing binary is a soft
//!      failure: we emit a single `vale-missing` diagnostic and continue.
//!   3. Shelling out to `vale --output=JSON` and parsing the resulting
//!      `{ "<file>": [Alert, ...] }` map into typed `ValeAlert` records.
//!
//! ## Vale rule shape
//!
//! Vale's primitives don't natively express "this bare token, when not
//! preceded by an ontology-vocab qualifier, is an error" — `existence` /
//! `substitution` look at single tokens, not lookbehind-conditional ones.
//! Rather than wrestle Vale into doing context-sensitive matching, we use
//! the simpler approach: the `FA.AmbiguousBare` rule fires on every
//! occurrence of an ambiguous English noun, and the Rust post-processor
//! decides per-alert whether the surrounding context excused it. The
//! generated `Vocabulary/Vocabulary.yml` rule covers the closure side —
//! Vale will flag any capitalised non-vocab term itself.

use crate::config::LintConfig;
use crate::ontology::Ontology;
use crate::validator::Issue;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Roadmap-51 Move 1: baked-in baseline of common English words (lowercase,
/// 3–15 chars, alphabetic only) sourced from `/usr/share/dict/american-english`
/// and committed at `data/english-common.txt`. Gets
/// expanded to case variants and written to a dedicated `EnglishCommon`
/// Vocab pack so Vale's `Vocabulary.Vocabulary` rule stops flagging
/// ordinary tech-prose words like `Lever`, `Reach`, `Tightening`.
/// Domain nouns still need to be in the project ontology — `EnglishCommon`
/// only silences plain English.
const ENGLISH_COMMON: &str = include_str!("../data/english-common.txt");

/// The ontology's axis names — accepted as vocabulary alongside the
/// values each axis declares.
const ONTOLOGY_AXES: &[&str] = &["role", "kind", "lifecycle", "covers", "bounded-context"];

/// One alert returned by `vale --output=JSON`. Vale's schema includes
/// several more fields (Action, Description, Link, ...); we deserialize
/// the ones the post-processor needs.
///
/// `Span` is `[start_col, end_col]` — 1-indexed column offsets within
/// the (1-indexed) `Line`. The Phase 1 sentence-start filter uses the
/// start column to skip alerts that fire on the first capitalised word
/// after a `.`, `!`, `?`, `;`, or `:` boundary.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ValeAlert {
    #[serde(rename = "Check")]
    pub check: String,
    #[serde(rename = "Match")]
    pub match_text: String,
    #[serde(rename = "Line")]
    pub line: usize,
    #[serde(rename = "Message")]
    pub message: String,
    #[serde(rename = "Severity")]
    pub severity: String,
    /// Vale's `[start_col, end_col]` 1-indexed column span. Default is
    /// `[1, 1]` for older Vale versions that may omit it.
    #[serde(rename = "Span", default = "default_span")]
    pub span: [usize; 2],
}

fn default_span() -> [usize; 2] {
    [1, 1]
}

/// Top-level Vale JSON shape used by the [[entity-doc-graph]] vocab-closure
/// pipeline: a map from absolute file path → list of alerts. Files with no
/// alerts are still present with an empty list, but older Vale versions
/// omit them — both shapes deserialize fine.
pub type ValeOutput = BTreeMap<String, Vec<ValeAlert>>;

/// Resolves the directory tree we own under `<root>/.doc-lint/vale/`. The
/// caller is expected to have created (or be about to create) it.
pub fn vale_dir(root: &Path) -> PathBuf {
    root.join(".doc-lint").join("vale")
}

/// Writes (idempotently) the entire generated Vale config tree from the
/// live ontology. Files are only rewritten when the on-disk content differs
/// from what we'd generate — repeated `check` runs leave timestamps alone.
///
/// Layout produced:
/// ```text
/// .doc-lint/vale/
///   .vale.ini
///   styles/
///     Vocabulary/Vocabulary.yml
///     config/vocabularies/FA/accept.txt
///     config/vocabularies/FA/reject.txt
///     FA/AmbiguousBare.yml
/// ```
pub fn generate_config(root: &Path, ontology: &Ontology, config: &LintConfig) -> Result<()> {
    let base = vale_dir(root);
    fs::create_dir_all(&base).with_context(|| format!("create {}", base.display()))?;

    // Phase 3-extension: external dictionaries (cspell-style .txt files,
    // one term per line) get their own Vocab pack each. Vale natively
    // supports multiple Vocab names in `.vale.ini` — `Vocab = FA, AWS,
    // SoftwareTerms`. Each name maps to a sibling dir under
    // `styles/config/vocabularies/`. Keeping them in separate packs
    // means the ontology-derived FA list stays small and pure; updates
    // to the cspell dicts are one-shot file refreshes.
    let packs = accept_packs(root, ontology, config);
    // Every pack after `FA` joins the `Vocab =` line; Vale unions them.
    let extra_vocab_names: Vec<String> = packs.iter().skip(1).map(|(n, _)| n.clone()).collect();
    write_idempotent(&base.join(".vale.ini"), &build_vale_ini(&extra_vocab_names))?;

    let voc_dir = base.join("styles/Vocabulary");
    fs::create_dir_all(&voc_dir)?;
    write_idempotent(&voc_dir.join("Vocabulary.yml"), VOCAB_RULE_YML)?;

    for (name, accept) in &packs {
        let pack_dir = base.join("styles/config/vocabularies").join(name);
        fs::create_dir_all(&pack_dir)?;
        write_idempotent(&pack_dir.join("accept.txt"), accept)?;
        write_idempotent(&pack_dir.join("reject.txt"), "")?;
    }

    let fa_rules_dir = base.join("styles/FA");
    fs::create_dir_all(&fa_rules_dir)?;
    let ambiguous_path = fa_rules_dir.join("AmbiguousBare.yml");
    if config.vale.vale_ambiguous_words.is_empty() {
        // Empty list means the repo doesn't want the bare-ambiguous-noun
        // rule. Vale's `existence` check with an empty `tokens:` list
        // pathologically matches every line (issuing thousands of empty
        // `''` alerts), so we must remove the rule file entirely rather
        // than write a zero-token YAML. Stale file from a prior run with
        // a non-empty list gets cleaned up here too.
        if ambiguous_path.exists() {
            fs::remove_file(&ambiguous_path)
                .with_context(|| format!("remove stale {}", ambiguous_path.display()))?;
        }
    } else {
        write_idempotent(
            &ambiguous_path,
            &build_ambiguous_rule(&config.vale.vale_ambiguous_words),
        )?;
    }

    Ok(())
}

/// The accept-list packs Vale reads, as `(pack name, accept.txt body)`:
/// the ontology-derived `FA` pack first, then the baked-in
/// `EnglishCommon` baseline, then one pack per `vale_dictionaries` entry.
/// Single source for both the generated Vale tree and the in-process
/// code-comment lint ([`accept_terms`]), so a term accepted in markdown
/// is accepted in a doc comment too.
///
/// `vale_ambiguous_words` are excluded from every pack: Vale's accept
/// list outranks rule firing, so an ambiguous word left in any pack would
/// silently suppress `FA.AmbiguousBare`.
pub fn accept_packs(
    root: &Path,
    ontology: &Ontology,
    config: &LintConfig,
) -> Vec<(String, String)> {
    let ambiguous = &config.vale.vale_ambiguous_words;
    let mut packs = vec![
        (
            "FA".to_string(),
            build_accept(
                ontology,
                &config.vale.vale_extra_accept,
                &config.vale.proper_nouns,
                ambiguous,
            ),
        ),
        (
            "EnglishCommon".to_string(),
            expand_dict_variants(ENGLISH_COMMON, ambiguous),
        ),
    ];
    // External cspell-style dictionaries, repo-relative. Vale matches
    // vocab case-sensitively, so each entry is expanded into its case
    // variants by `expand_dict_variants`. An unreadable one is skipped
    // here and reported by `check` ([`unreadable_dictionaries`]) as
    // `vocab-dictionary-unreadable`, so the other packs still apply.
    for (name, rel_path) in &config.vale.vale_dictionaries {
        if let Ok(body) = fs::read_to_string(root.join(rel_path)) {
            packs.push((name.clone(), expand_dict_variants(&body, ambiguous)));
        }
    }
    packs
}

/// `vale_dictionaries` entries whose file can't be read, as
/// `(pack name, resolved path)`. [`accept_packs`] skips them.
pub fn unreadable_dictionaries(root: &Path, config: &LintConfig) -> Vec<(String, PathBuf)> {
    config
        .vale
        .vale_dictionaries
        .iter()
        .map(|(name, rel)| (name.clone(), root.join(rel)))
        .filter(|(_, path)| fs::read_to_string(path).is_err())
        .collect()
}

/// Union of every [`accept_packs`] entry — the exact-match accept set the
/// code-comment lint checks a capitalised token against before calling it
/// a vocabulary violation.
pub fn accept_terms(
    root: &Path,
    ontology: &Ontology,
    config: &LintConfig,
) -> std::collections::HashSet<String> {
    accept_packs(root, ontology, config)
        .iter()
        .flat_map(|(_, body)| body.lines().map(str::to_string))
        .collect()
}

/// Builds the `.vale.ini` content. The `Vocab = ...` line lists every
/// vocabulary pack Vale should consult: the ontology-derived `FA` pack
/// always plus any extra packs from `vale_dictionaries`. Order matters
/// only for human readability — Vale's accept-list lookup is unordered.
fn build_vale_ini(extra_vocabs: &[String]) -> String {
    let mut vocab_names = vec!["FA".to_string()];
    vocab_names.extend(extra_vocabs.iter().cloned());
    let vocab_line = vocab_names.join(", ");
    format!(
        "# GENERATED by doc-linter — do not edit by hand. Regenerated on every\n\
         # `doc-linter check` run from the live ontology and .doc-lint.toml.\n\
         StylesPath = styles\n\
         MinAlertLevel = warning\n\
         Vocab = {vocab_line}\n\
         \n\
         # Vale renders AsciiDoc with `asciidoctor`. A rendered `:toc:` repeats\n\
         # the headings and Vale then drops every alert in the file.\n\
         [asciidoctor]\n\
         toc = NO\n\
         \n\
         [*.md]\n\
         # Phase 1 (Fix B): skip H1-H6 markdown headers wholesale. Header words\n\
         # like `Related`, `Status`, `How`, `Why` aren't prose — they're section\n\
         # labels and don't need to resolve to an ontology entity.\n\
         BlockIgnores = (?m)^#+\\s.*$\n\
         BasedOnStyles = Vocabulary, FA\n\
         \n\
         # AsciiDoc: Vale renders it with `asciidoctor` (must be on PATH).\n\
         [*.adoc]\n\
         BasedOnStyles = Vocabulary, FA\n",
    )
}

/// Checks whether the [[entity-doc-graph]] vocabulary-closure tool `vale` is
/// on PATH. Returns the resolved path on success.
pub fn detect() -> Option<PathBuf> {
    which::which("vale").ok()
}

/// Runs the Vale binary over `files` for [[entity-doc-graph]] vocabulary
/// closure and returns the parsed alert map. Vale gets the explicit file
/// list rather than `<root>`: walking the root lints files `check` never
/// targets, and one unreadable entry anywhere under it (a dangling
/// symlink) aborts the whole run.
///
/// Vale exits 0 (clean) or 1 (error-level alerts) with its alert JSON on
/// stdout. Any other exit is a runtime failure (`E100` on stderr, empty
/// stdout) and is returned as an error: reading it as "no alerts" would
/// report every file clean.
pub fn run(root: &Path, vale_bin: &Path, files: &[PathBuf]) -> Result<ValeOutput> {
    let cfg = vale_dir(root).join(".vale.ini");
    let mut all = BTreeMap::new();
    // Batched so a large corpus stays well under ARG_MAX.
    for batch in files.chunks(VALE_BATCH) {
        let output = Command::new(vale_bin)
            .arg("--output=JSON")
            .arg(format!("--config={}", cfg.display()))
            .args(batch)
            .output()
            .with_context(|| format!("invoke {}", vale_bin.display()))?;

        if !matches!(output.status.code(), Some(0 | 1)) {
            anyhow::bail!(
                "vale exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let trimmed = stdout.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: ValeOutput = serde_json::from_str(trimmed)
            .with_context(|| format!("parse vale --output=JSON output:\n{trimmed}"))?;
        all.extend(parsed);
    }
    Ok(all)
}

/// Files per `vale` invocation in [`run`].
const VALE_BATCH: usize = 500;

/// Phase 1 (Fix A) helper: returns true if `(line, col)` (both 1-indexed,
/// matching Vale's `Line` and `Span[0]`) sits at a sentence-start position
/// in `body`.
///
/// A position is sentence-start when:
///   - it's the first non-whitespace token on its line (col 1 or only
///     whitespace before col), OR
///   - the token is preceded (with optional whitespace between) by one
///     of `.`, `!`, `?`, `;`, `:`, or the `|` opening a table cell, on the same line.
///
/// Vale's `\b[A-Z][A-Za-z]+\b` rule fires on every capitalised word,
/// including the first word of every sentence — `When the sky is blue`
/// yields a `When` alert. Filtering at sentence-start positions clears
/// the bulk of this noise without paying for full prose parsing.
pub fn is_sentence_start(body: &str, line: usize, col: usize) -> bool {
    if line == 0 || col == 0 {
        return false;
    }
    // Lines are 1-indexed; collect the relevant line.
    let target_line = match body.lines().nth(line - 1) {
        Some(l) => l,
        None => return false,
    };
    // Inspect the bytes/chars before col on the same line.
    let prefix_len = col.saturating_sub(1).min(target_line.chars().count());
    let prefix: String = target_line.chars().take(prefix_len).collect();

    // Trim trailing whitespace; the alert's column may sit several spaces
    // after the punctuation (`.  When` → prefix is `.  `, trimmed `.`).
    let trimmed = prefix.trim_end();
    if trimmed.is_empty() {
        // First non-whitespace token on the line — column 1 or after
        // leading whitespace. Either way it's a sentence start.
        return true;
    }
    // Look at the last char of the trimmed prefix.
    #[allow(
        clippy::unwrap_used,
        reason = "is_empty() check above guarantees at least one char"
    )]
    let last = trimmed.chars().last().unwrap();
    // `|` starts a markdown table cell, which reads as a new sentence.
    matches!(last, '.' | '!' | '?' | ';' | ':' | '|')
}

/// Returns true if `(line, col)` (1-indexed, matching Vale's `Line` and
/// `Span[0]`) sits inside an instance of `entity_id` on the same line.
///
/// Vale's `\brule\b` regex tokenises on `-`, so the bare 'rule' token in
/// `pricing-rule` (or `[[pricing-rule]]`) fires the AmbiguousBare rule
/// even though the author already wrote the qualified form. This helper
/// recognises that and lets the post-processor suppress the redundant
/// cross-context-reference downgrade — the qualifier is right there.
///
/// Match is ASCII case-insensitive (entity ids are kebab-case ASCII;
/// authors may write `Pricing-rule` mid-sentence).
///
/// A single-word id (`settlement`) has no qualifier: the bare word *is*
/// the id, so every bare use would sit "inside" it. Those never count as
/// already qualified — otherwise each cross-context use of a one-word
/// concept is silently suppressed.
pub fn alert_inside_entity_id(body: &str, line: usize, col: usize, entity_id: &str) -> bool {
    if line == 0 || col == 0 || entity_id.is_empty() {
        return false;
    }
    if entity_id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let Some(target_line) = body.lines().nth(line - 1) else {
        return false;
    };
    let line_lower = target_line.to_ascii_lowercase();
    let id_lower = entity_id.to_ascii_lowercase();
    // Vale's col is 1-indexed chars; entity ids are ASCII so byte and char
    // offsets within the id are identical, but `target_line` may contain
    // multi-byte chars, so map the alert col to a byte offset.
    let target_byte = match target_line.char_indices().nth(col - 1) {
        Some((b, _)) => b,
        None => return false,
    };
    let mut start = 0usize;
    while let Some(found) = line_lower[start..].find(&id_lower) {
        let abs_start = start + found;
        let abs_end = abs_start + id_lower.len();
        if abs_start <= target_byte && target_byte < abs_end {
            return true;
        }
        start = abs_start + 1;
    }
    false
}

/// Returns true if `(line, col)` sits inside a `[[...]]` Obsidian
/// wikilink on the same line. The wikilink target IS the disambiguation,
/// so a bare-noun token Vale matched inside it should not downgrade to a
/// cross-context-reference.
pub fn alert_inside_wikilink(body: &str, line: usize, col: usize) -> bool {
    if line == 0 || col == 0 {
        return false;
    }
    let Some(target_line) = body.lines().nth(line - 1) else {
        return false;
    };
    let target_byte = match target_line.char_indices().nth(col - 1) {
        Some((b, _)) => b,
        None => return false,
    };
    let bytes = target_line.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let open_end = i + 2;
            // Find the matching ]] on the same line.
            let mut j = open_end;
            while j + 1 < bytes.len() {
                if bytes[j] == b']' && bytes[j + 1] == b']' {
                    if open_end <= target_byte && target_byte < j {
                        return true;
                    }
                    i = j + 2;
                    break;
                }
                j += 1;
            }
            if j + 1 >= bytes.len() {
                return false; // unterminated; conservative
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Returns true if `(line, col)` sits inside a backtick code span on the
/// same line. Inline code is a reference / mention, not a prose claim, so
/// a bare-noun token Vale matched inside backticks should not downgrade
/// to a cross-context-reference. Counts backticks pairwise; supports the
/// common `…` form (single-backtick spans).
pub fn alert_inside_inline_code(body: &str, line: usize, col: usize) -> bool {
    if line == 0 || col == 0 {
        return false;
    }
    let Some(target_line) = body.lines().nth(line - 1) else {
        return false;
    };
    let target_byte = match target_line.char_indices().nth(col - 1) {
        Some((b, _)) => b,
        None => return false,
    };
    let bytes = target_line.as_bytes();
    let mut inside = false;
    let mut span_start = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            if inside {
                if span_start <= target_byte && target_byte < i {
                    return true;
                }
                inside = false;
            } else {
                inside = true;
                span_start = i + 1;
            }
        }
        i += 1;
    }
    false
}

/// Maps a Vale alert into the [[entity-doc-graph]] `Issue::ValeAlert`
/// variant. Severities are passed through verbatim (Vale uses "warning"
/// / "error" / "suggestion"); the validator's `Display` impl renders them.
pub fn alert_to_issue(alert: &ValeAlert) -> Issue {
    Issue::ValeAlert {
        check: alert.check.clone(),
        message: alert.message.clone(),
        line: alert.line,
        severity: alert.severity.clone(),
    }
}

// ---------------------------------------------------------------------------
// Static templates (no string interpolation needed for these).
// ---------------------------------------------------------------------------

const VOCAB_RULE_YML: &str = "\
# GENERATED — vocabulary-closure rule. Vale reports any capitalised token
# in prose that's not on the project accept-list or in the (empty) reject list.
# The reject list is reserved for future blacklists (e.g. discouraged terms).
#
# NOTE: no `action:` block — the Vale 2.x `{ name: suggest, params: [spellings] }`
# shape was rejected by Vale 3.9.1 with `unknown check 'Vocabulary.Vocabulary'`,
# silently disabling the entire vocabulary-closure pass. The rule still detects
# violations without an action; --fix-mode auto-suggestions are not used by
# the post-processor anyway.
extends: existence
message: \"Term '%s' is not in the ontology vocabulary\"
ignorecase: false
level: warning
tokens:
  - '\\b[A-Z][A-Za-z]+\\b'
";

/// Builds `FA/AmbiguousBare.yml` from the configured ambiguous-words list.
/// Vale's `existence` rule fires on every occurrence of any of the listed
/// tokens as an isolated word (case-insensitive). Bounded-context-aware
/// suppression happens AFTER Vale runs — see `crate::disambiguation`. We
/// noted this design choice in the module-level doc comment above.
fn build_ambiguous_rule(words: &[String]) -> String {
    let mut tokens = String::new();
    for w in words {
        // Word-boundary anchored, matched case-insensitively (level set
        // below). Regex-escape just in case a future config slips a metachar
        // through; the default list is plain ASCII.
        let escaped: String = w
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c.to_string()
                } else {
                    format!("\\{c}")
                }
            })
            .collect();
        tokens.push_str(&format!("  - '\\b{escaped}\\b'\n"));
    }
    format!(
        "# GENERATED — bare-ambiguous-noun rule. Fires on every occurrence; the\n\
         # doc-linter Rust post-processor decides which alerts are actually\n\
         # errors based on the doc's bounded context. See vale.rs for why we\n\
         # don't try to express the lookbehind in Vale's primitives directly.\n\
         extends: existence\n\
         message: \"Bare ambiguous noun '%s' — qualify it with a term from the ontology vocabulary\"\n\
         ignorecase: true\n\
         level: warning\n\
         tokens:\n{tokens}"
    )
}

/// Builds the FA accept-list. Includes, for every ontology entity:
///   - `entity.id`
///   - `entity.display`
///   - every entry of `entity.synonyms`
///   - the bare-acronym lead of `entity.display` if it carries a
///     parenthetical (Phase 1 Fix C — e.g. `MCP (Model Context Protocol)`
///     also accepts bare `MCP` and plural `MCPs`)
///   - the simple plural of each of the above
///
/// `extra_accept` and `proper_nouns` (Phase 2) merge in additional
/// terms that aren't ontology entities — populated from `.doc-lint.toml`.
///
/// `ambiguous_words` (G1 followup) holds terms that the
/// `FA.AmbiguousBare` Vale rule fires on — these get EXCLUDED from
/// accept.txt so Vale's accept-list precedence doesn't silently
/// suppress them. The bounded-context post-processor then resolves
/// each alert per-doc-context. Exclusion is per-case-variant on the
/// bare term only; plurals stay in accept.txt because Vale's
/// `\brule\b` regex doesn't match "rules".
fn build_accept(
    ontology: &Ontology,
    extra: &[String],
    proper_nouns: &[String],
    ambiguous_words: &[String],
) -> String {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for entity in ontology.entities.values() {
        insert_with_plural(&mut set, &entity.id);
        insert_with_plural(&mut set, &entity.display);
        // Fix C: bare acronym lead from a parenthetical display.
        if let Some(bare) = bare_lead(&entity.display) {
            insert_with_plural(&mut set, bare);
        }
        for s in &entity.synonyms {
            insert_with_plural(&mut set, s);
        }
    }
    // The ontology's own axes and values are vocabulary too: a doc that
    // says "ADR" or "Lifecycle" names the `adr` role / `lifecycle` axis.
    for id in ontology
        .roles
        .keys()
        .chain(ontology.kinds.keys())
        .chain(ontology.lifecycles.keys())
        .chain(ontology.bounded_contexts.keys())
        .map(String::as_str)
        .chain(ONTOLOGY_AXES.iter().copied())
    {
        insert_with_plural(&mut set, id);
    }
    for term in extra {
        insert_with_plural(&mut set, term);
    }
    for term in proper_nouns {
        insert_with_plural(&mut set, term);
    }

    // Build the exclusion set from ambiguous_words — every bare-term
    // case variant gets dropped so Vale's FA.AmbiguousBare rule can
    // tokenize the term. The plural forms stay (Vale's `\b<word>\b`
    // doesn't match "<word>s").
    if !ambiguous_words.is_empty() {
        let mut exclude: BTreeSet<String> = BTreeSet::new();
        for word in ambiguous_words {
            let trimmed = word.trim();
            if trimmed.is_empty() {
                continue;
            }
            for variant in case_variants(trimmed) {
                exclude.insert(variant);
            }
        }
        set.retain(|term| !exclude.contains(term));
    }

    let mut out = String::with_capacity(set.len() * 16);
    for term in set {
        out.push_str(&term);
        out.push('\n');
    }
    out
}

/// Phase 3-extension: expand each line of a cspell-style `.txt`
/// dictionary into three case variants — original, TitleCase, and
/// ALL-CAPS — so Vale's Vocab match (which is empirically case-
/// sensitive against capitalised prose despite documentation to the
/// contrary) suppresses the term regardless of how authors write it.
///
/// Comment lines (starting `#`) and blank lines are stripped. Within
/// each line, only the first whitespace-delimited token is taken
/// (cspell allows `# directives` on the term line).
///
/// Output is sorted + de-duplicated via `BTreeSet` so two source
/// entries that collapse to the same UPPERCASE form (e.g. `vpc` and
/// `VPC`) don't bloat the accept-list.
///
/// G1 followup: `ambiguous_words` are excluded — same reason as
/// build_accept. Vale's accept-list is consulted across ALL Vocab
/// packs, so an ambiguous term sitting in any dict's accept.txt
/// silently suppresses FA.AmbiguousBare just like it would in FA's
/// own accept.txt. Apply the same exclusion here.
fn expand_dict_variants(body: &str, ambiguous_words: &[String]) -> String {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let token = line.split_whitespace().next().unwrap_or("");
        insert_case_variants(&mut set, token);
    }

    if !ambiguous_words.is_empty() {
        let mut exclude: BTreeSet<String> = BTreeSet::new();
        for word in ambiguous_words {
            let trimmed = word.trim();
            if trimmed.is_empty() {
                continue;
            }
            for variant in case_variants(trimmed) {
                exclude.insert(variant);
            }
        }
        set.retain(|term| !exclude.contains(term));
    }

    let mut out = String::with_capacity(set.len() * 8);
    for term in set {
        out.push_str(&term);
        out.push('\n');
    }
    out
}

/// Phase 5 followup: returns the canonical case variants for a
/// single token: the term itself, plus a TitleCase form when the
/// lead char is lowercase, plus an ALL-CAPS form when the term is
/// short (≤6 chars — the "likely acronym" heuristic).
///
/// Used by both `expand_dict_variants` (cspell dicts) and
/// `insert_with_plural` (ontology terms). Vale's Vocab match is
/// empirically case-sensitive against capitalised prose, so any
/// term we want suppressed needs all reasonable case forms in
/// `accept.txt` explicitly.
fn case_variants(term: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(3);
    out.push(term.to_string());
    // TitleCase: first char upper, rest unchanged. Only meaningful
    // when the lead char is lowercase — `Vpc` from `VPC` is just noise.
    if let Some(first) = term.chars().next() {
        if first.is_lowercase() {
            let mut tc = String::with_capacity(term.len());
            tc.extend(first.to_uppercase());
            tc.push_str(&term[first.len_utf8()..]);
            out.push(tc);
        }
    }
    // ALL-CAPS form, any length: prose markers, section labels and
    // headings (`SESSION`, `CRITICAL`, `TECHNICAL IMPLEMENTATION`). A
    // length cap (≤12 chars) only produced false positives on longer
    // words; admitting the capitals of an accepted word costs nothing.
    out.push(term.to_uppercase());
    out
}

/// Insert `term`'s case variants into the [[entity-doc-graph]] vocab
/// accept-list `set`. Wrapper used by `expand_dict_variants` for cspell
/// dicts (one-variant-pass, no plural — cspell entries are already in
/// their canonical form).
fn insert_case_variants(set: &mut BTreeSet<String>, term: &str) {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return;
    }
    for v in case_variants(trimmed) {
        set.insert(v);
    }
}

/// Phase 1 (Fix C): pulls the bare acronym lead from an entity display
/// like `"MCP (Model Context Protocol)"` → `Some("MCP")`. Returns
/// `None` when the display has no parenthetical, when the lead is
/// empty, or when the lead contains a space (we only want
/// single-token bare acronyms — `"Pricing Rule"` shouldn't match).
fn bare_lead(display: &str) -> Option<&str> {
    let idx = display.find('(')?;
    let lead = display[..idx].trim();
    if lead.is_empty() {
        return None;
    }
    if lead.contains(char::is_whitespace) {
        return None;
    }
    Some(lead)
}

/// Insert `term`'s case variants AND the plural of each variant into
/// `set`. Used by `build_accept` for ontology terms (entity ids,
/// displays, synonyms, bare acronym leads) and the manual
/// `vale_extra_accept` / `proper_nouns` lists from `.doc-lint.toml`.
///
/// Phase 5 followup: pluralise each case variant individually rather
/// than once on the base, then case-expand the plural. The order
/// matters for acronyms — English convention is `SKUs` (capital +
/// lowercase s), and `naive_plural("SKU")` correctly yields `SKUs`.
/// Pluralising the lowercase base first then case-expanding would
/// produce `SKUS` (all caps) instead, which is wrong.
fn insert_with_plural(set: &mut BTreeSet<String>, term: &str) {
    let trimmed = term.trim();
    if trimmed.is_empty() {
        return;
    }
    for variant in case_variants(trimmed) {
        let pl = naive_plural(&variant);
        set.insert(variant);
        set.insert(pl);
    }
}

/// Naive English pluralization. Sufficient for our vocabulary — entity
/// displays are short noun phrases like "Outlet" or "Pricing Rule". Cases:
///   - already ends in `s` / `S` → unchanged
///   - ends in consonant + `y` → `ies` (rule → rules… wait no, `rule` is
///     vowel+y, so this only kicks in for words like `family` → `families`)
///   - default → append `s`
fn naive_plural(term: &str) -> String {
    if term.ends_with('s') || term.ends_with('S') {
        return term.to_string();
    }
    let chars: Vec<char> = term.chars().collect();
    if chars.len() >= 2 {
        let last = chars[chars.len() - 1];
        let prev = chars[chars.len() - 2];
        if (last == 'y' || last == 'Y') && !is_vowel(prev) {
            let mut s: String = chars[..chars.len() - 1].iter().collect();
            s.push_str("ies");
            return s;
        }
    }
    let mut s = term.to_string();
    s.push('s');
    s
}

/// True for ASCII vowels — used by the [[entity-doc-graph]] Vale
/// pluraliser to pick `consonant+y -> ies` vs `vowel+y -> ys`.
fn is_vowel(c: char) -> bool {
    matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'A' | 'E' | 'I' | 'O' | 'U')
}

/// Writes `content` to `path` only if the existing content differs. Keeps
/// mtimes stable for editor watchers and avoids spurious git diffs even
/// inside the gitignored tree (we still don't want false "changed" signals
/// in IDEs).
fn write_idempotent(path: &Path, content: &str) -> Result<()> {
    if let Ok(existing) = fs::read_to_string(path) {
        if existing == content {
            return Ok(());
        }
    }
    fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Tests for the [[entity-doc-graph]] Vale config generator and the
/// vocab-closure post-processor.
#[cfg(test)]
mod tests {
    use super::*;

    /// `run` splits the file list into `VALE_BATCH`-sized invocations and
    /// merges their output. A fake `vale` counts its calls and echoes an
    /// empty alert list per file.
    #[cfg(unix)]
    #[test]
    fn run_batches_files_and_merges_output() {
        use std::os::unix::fs::PermissionsExt;
        let dir =
            std::env::temp_dir().join(format!("doc-linter-vale-batch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("vale");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\necho x >> {calls}\nshift 2\nprintf '{{'\nsep=''\n\
                 for f in \"$@\"; do printf '%s\"%s\": []' \"$sep\" \"$f\"; sep=','; done\necho '}}'\n",
                calls = dir.join("calls").display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        let files: Vec<PathBuf> = (0..=VALE_BATCH * 2)
            .map(|i| dir.join(format!("{i}.md")))
            .collect();
        let out = run(&dir, &fake, &files).unwrap();
        assert_eq!(out.len(), files.len());
        let calls = fs::read_to_string(dir.join("calls")).unwrap();
        assert_eq!(calls.lines().count(), 3);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn naive_plural_rules() {
        assert_eq!(naive_plural("Outlet"), "Outlets");
        assert_eq!(naive_plural("Pricing Rule"), "Pricing Rules");
        // already plural
        assert_eq!(naive_plural("rules"), "rules");
        // consonant+y → ies
        assert_eq!(naive_plural("family"), "families");
        // vowel+y → just s
        assert_eq!(naive_plural("key"), "keys");
    }

    /// Asserts that the [[entity-doc-graph]] Vale "accept" word list
    /// contains every entity display name plus its synonyms and naive
    /// plurals so legitimate prose passes vocab closure.
    #[test]
    fn accept_includes_synonyms_and_plurals() {
        use crate::ontology::EntityDef;
        let mut ont = Ontology::bootstrap();
        ont.entities.insert(
            "pricing-rule".to_string(),
            EntityDef {
                id: "pricing-rule".to_string(),
                display: "Pricing Rule".to_string(),
                description: "test".to_string(),
                synonyms: vec!["rule".to_string()],
                bounded_contexts: vec!["pricing".to_string()],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        let accept = build_accept(&ont, &[], &[], &[]);
        assert!(accept.contains("Pricing Rule\n"));
        assert!(accept.contains("Pricing Rules\n"));
        assert!(accept.contains("rule\n"));
        assert!(accept.contains("rules\n"));
        assert!(accept.contains("pricing-rule\n"));
        assert!(accept.contains("pricing-rules\n"));
    }

    /// Phase 5 followup: a lowercase synonym like `[sku]` should
    /// produce all case forms AND their plurals in accept.txt because
    /// Vale's Vocab match is case-sensitive against capitalised prose.
    /// Author writes the synonym once; the linter fans it out — and
    /// crucially, the plural of an ALL-CAPS variant is `<term>s`
    /// (capital + lowercase s), the natural English acronym plural.
    #[test]
    fn accept_expands_case_variants_from_lowercase_synonym() {
        use crate::ontology::EntityDef;
        let mut ont = Ontology::bootstrap();
        ont.entities.insert(
            "product".to_string(),
            EntityDef {
                id: "product".to_string(),
                display: "Product".to_string(),
                description: "test".to_string(),
                synonyms: vec!["sku".to_string()],
                bounded_contexts: vec![],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        let accept = build_accept(&ont, &[], &[], &[]);
        // Bases — every case form
        assert!(accept.contains("sku\n"));
        assert!(accept.contains("Sku\n"));
        assert!(accept.contains("SKU\n"));
        // Plurals — pluralise each case variant individually so the
        // acronym's natural plural `SKUs` lands (NOT `SKUS`).
        assert!(accept.contains("skus\n"));
        assert!(accept.contains("Skus\n"));
        assert!(accept.contains("SKUs\n"));
    }

    /// `extra_accept` and `proper_nouns` lists from .doc-lint.toml go
    /// through the same [[entity-doc-graph]] case-variant + plural
    /// expansion so authors don't have to enumerate every form manually.
    #[test]
    fn accept_expands_case_variants_for_extra_accept_terms() {
        let ont = Ontology::bootstrap();
        let extra = vec!["json".to_string()];
        let accept = build_accept(&ont, &extra, &[], &[]);
        assert!(accept.contains("json\n"));
        assert!(accept.contains("Json\n"));
        assert!(accept.contains("JSON\n"));
    }

    /// G1 followup: `vale_ambiguous_words` terms get EXCLUDED from
    /// accept.txt so Vale's `FA.AmbiguousBare` rule can tokenize them
    /// instead of being silently suppressed by accept-list precedence.
    /// The exclusion drops every case variant of the bare term (rule,
    /// Rule, RULE) but leaves the plurals (rules, Rules, RULES) — Vale's
    /// `\brule\b` regex matches only the bare word, not its plural form.
    #[test]
    fn accept_excludes_ambiguous_words() {
        use crate::ontology::EntityDef;
        let mut ont = Ontology::bootstrap();
        ont.entities.insert(
            "pricing-rule".to_string(),
            EntityDef {
                id: "pricing-rule".to_string(),
                display: "Pricing Rule".to_string(),
                description: "test".to_string(),
                synonyms: vec!["rule".to_string()],
                bounded_contexts: vec!["pricing".to_string()],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        let ambiguous = vec!["rule".to_string()];
        let accept = build_accept(&ont, &[], &[], &ambiguous);
        // Bare-term case variants are excluded — AmbiguousBare can fire.
        assert!(!accept.contains("\nrule\n") && !accept.starts_with("rule\n"));
        assert!(!accept.contains("\nRule\n") && !accept.starts_with("Rule\n"));
        assert!(!accept.contains("\nRULE\n") && !accept.starts_with("RULE\n"));
        // Plurals stay — Vale's `\brule\b` doesn't match "rules".
        // Note `RULEs` (lowercase `s`) is the natural acronym plural,
        // matching the SKUs/URLs/VMs convention.
        assert!(accept.contains("rules\n"));
        assert!(accept.contains("Rules\n"));
        assert!(accept.contains("RULEs\n"));
        // Other entity terms unaffected.
        assert!(accept.contains("pricing-rule\n"));
        assert!(accept.contains("Pricing Rule\n"));
        assert!(accept.contains("Pricing Rules\n"));
    }

    /// Phase 1 (Fix C): an entity whose display has a parenthetical
    /// emits both the full display AND the bare lead. `MCP (Model
    /// Context Protocol)` → `MCP` and `MCPs` (the plural) land in
    /// accept.txt alongside the full display.
    #[test]
    fn accept_extracts_bare_acronym_lead() {
        use crate::ontology::EntityDef;
        let mut ont = Ontology::bootstrap();
        ont.entities.insert(
            "mcp".to_string(),
            EntityDef {
                id: "mcp".to_string(),
                display: "MCP (Model Context Protocol)".to_string(),
                description: "test".to_string(),
                synonyms: vec![],
                bounded_contexts: vec![],
                scanner_coverage: vec![],
                source_modules: vec![],
                relates_to: vec![],
                status: "stable".to_string(),
                entity_class: None,
                attributes: Vec::new(),
                code_terms: Vec::new(),
                code_veto: Vec::new(),
            },
        );
        let accept = build_accept(&ont, &[], &[], &[]);
        assert!(
            accept.contains("MCP\n"),
            "bare acronym 'MCP' missing:\n{accept}"
        );
        assert!(
            accept.contains("MCPs\n"),
            "bare acronym plural 'MCPs' missing:\n{accept}"
        );
        assert!(accept.contains("MCP (Model Context Protocol)\n"));
    }

    /// `bare_lead` returns `None` when there's no parenthetical, when
    /// the lead is multiple words, or when the lead is empty. Single-
    /// word leads with a parenthetical extract cleanly into the
    /// [[entity-doc-graph]] vocab accept-list.
    #[test]
    fn bare_lead_extraction() {
        assert_eq!(bare_lead("MCP (Model Context Protocol)"), Some("MCP"));
        assert_eq!(bare_lead("JDM (Decision Model)"), Some("JDM"));
        assert_eq!(bare_lead("Pricing Rule"), None);
        assert_eq!(bare_lead("(empty lead)"), None);
        // Multi-word leads aren't extracted — `Pricing Rule (...)` stays
        // as the full display only; the lead `Pricing Rule` already
        // covers the bare form.
        assert_eq!(bare_lead("Pricing Rule (PR)"), None);
    }

    /// Phase 1 (Fix A): [[entity-doc-graph]] sentence-start detection
    /// covers column 1, the position right after sentence-ending
    /// punctuation, and the soft-boundary punctuation (`;`, `:`).
    #[test]
    fn sentence_start_positions() {
        // Column 1 of any line is sentence start.
        assert!(is_sentence_start("When the sky is blue.\n", 1, 1));
        // After `.` + space.
        assert!(is_sentence_start(
            "Hello world. When the sky is blue.\n",
            1,
            14
        ));
        // After `! `.
        assert!(is_sentence_start("Wow! When indeed.\n", 1, 6));
        // Mid-sentence: NOT a sentence start.
        assert!(!is_sentence_start("the When clause matters.\n", 1, 5));
        // After leading whitespace on the line.
        assert!(is_sentence_start("    When indented.\n", 1, 5));
        // Multi-line body, second line column 1.
        assert!(is_sentence_start("first line.\nWhen blue.\n", 2, 1));
        // A table cell starts a sentence.
        assert!(is_sentence_start(
            "| `key` | Repo-relative globs |\n",
            1,
            11
        ));
    }

    /// #259: a single-word id is never "already qualified" by itself.
    #[test]
    fn alert_inside_entity_id_never_excuses_a_single_word_id() {
        // `settlement` owned by payments, used bare in a lending doc: the
        // bare word is the whole id, so it is not "already qualified".
        let body = "the loan reaches settlement and the account is closed";
        assert!(!alert_inside_entity_id(body, 1, 18, "settlement"));
        assert!(!alert_inside_entity_id(
            "Settlement date",
            1,
            1,
            "settlement"
        ));
    }

    /// G8 fix #1: when the entity_id (e.g. `pricing-rule`) appears on
    /// the same line as the alert, the bare token Vale matched is part
    /// of that qualified form and should not downgrade. Hyphens count
    /// as word boundaries, so `\brule\b` fires inside `pricing-rule`.
    #[test]
    fn alert_inside_entity_id_recognises_qualified_form() {
        // Bare 'rule' inside `pricing-rule` — col points at the 'r' of
        // the inner `rule` (1-indexed; offset 9 in `pricing-rule`).
        let body = "see pricing-rule shape\n";
        assert!(alert_inside_entity_id(body, 1, 13, "pricing-rule"));
        // Bare 'rule' OUTSIDE the qualified form — same body, but
        // hypothetical alert at a position where there's no entity-id
        // span (col 1 = 's').
        assert!(!alert_inside_entity_id(body, 1, 1, "pricing-rule"));
        // Case-insensitive (author wrote `Pricing-Rule`).
        let body2 = "See Pricing-Rule shape\n";
        assert!(alert_inside_entity_id(body2, 1, 13, "pricing-rule"));
        // Multi-line body — alert on line 2 inside the qualified form.
        let body3 = "header\nfoo pricing-rule bar\n";
        assert!(alert_inside_entity_id(body3, 2, 13, "pricing-rule"));
    }

    /// G8 fix #2: a bare token inside `[[wikilink]]` is already
    /// disambiguated by the wikilink target. Vale's `\b...\b` match
    /// fires inside the brackets; the post-processor should suppress.
    #[test]
    fn alert_inside_wikilink_recognises_brackets() {
        let body = "fully qualify as [[pricing-rule]] please\n";
        // 'rule' inside [[pricing-rule]] — chars 28-31.
        assert!(alert_inside_wikilink(body, 1, 28));
        // 'qualify' is OUTSIDE any wikilink.
        assert!(!alert_inside_wikilink(body, 1, 7));
        // Unterminated `[[` — conservative: do not suppress.
        assert!(!alert_inside_wikilink(
            "see [[pricing-rule and more\n",
            1,
            8
        ));
    }

    /// G8 fix #3: a bare token inside `\`code\`` is a mention, not a
    /// prose claim, and should not downgrade. Backticks are paired
    /// left-to-right.
    #[test]
    fn alert_inside_inline_code_recognises_backticks() {
        // `rule` inside backticks — chars 6-9.
        let body = "see `rule` mentioned\n";
        assert!(alert_inside_inline_code(body, 1, 6));
        // `mentioned` is OUTSIDE the backticks.
        assert!(!alert_inside_inline_code(body, 1, 12));
        // Two separate code spans on one line — both detected.
        let body2 = "either `pricing rule` or `rule` qualified\n";
        assert!(alert_inside_inline_code(body2, 1, 9)); // first span
        assert!(alert_inside_inline_code(body2, 1, 27)); // second span
                                                         // Bare 'or' between the two spans — NOT inside code.
        assert!(!alert_inside_inline_code(body2, 1, 23));
    }

    #[test]
    fn ambiguous_rule_renders_yaml_tokens() {
        let yml = build_ambiguous_rule(&["rule".to_string(), "system".to_string()]);
        assert!(yml.contains("extends: existence"));
        assert!(yml.contains("\\brule\\b"));
        assert!(yml.contains("\\bsystem\\b"));
        assert!(yml.contains("ignorecase: true"));
    }
}
