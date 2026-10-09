//! Vocabulary closure over source doc comments — the one engine behind the
//! corpus `check --lint-code-comments` pass, the per-file `check --file`
//! hook and the LSP, for every language [`crate::code_comments::Lang`]
//! extracts.
//!
//! A capitalised token (or a configured ambiguous word) in a doc comment
//! is a `comment-vocab-violation` unless the markdown path would accept
//! it too: it is on a Vale accept pack ([`crate::vale::accept_packs`] —
//! ontology terms, `vale_extra_accept`, `proper_nouns`, the EnglishCommon
//! baseline, `vale_dictionaries`), it resolves through the ontology
//! [`TermIndex`], it opens a sentence, or it sits in inline code or a
//! fenced code block. `COMMENT_STOPWORDS` and `code_comment_extra_accept`
//! add the language-plumbing names (`Vec`, `Result`, …) on top.

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;

use crate::code_comments::{DocComment, COMMENT_STOPWORDS};
use crate::config::LintConfig;
use crate::disambiguation::TermIndex;
use crate::ontology::Ontology;
use crate::vale;
use crate::validator::Issue;

/// Vocab-closure checker for doc comments, built once per run (or per
/// LSP corpus rebuild) and applied to any number of files.
pub struct CommentLinter<'a> {
    ontology: &'a Ontology,
    terms: TermIndex<'a>,
    token_re: Regex,
    accept: HashSet<String>,
}

impl<'a> CommentLinter<'a> {
    pub fn new(root: &Path, config: &LintConfig, ontology: &'a Ontology) -> Result<Self> {
        let mut alt = String::from(r"\b[A-Z][A-Za-z]+\b");
        for word in &config.vale.vale_ambiguous_words {
            alt.push_str(&format!("|\\b(?i:{})\\b", regex::escape(word)));
        }
        let mut accept = vale::accept_terms(root, ontology, config);
        accept.extend(COMMENT_STOPWORDS.iter().map(|s| (*s).to_string()));
        accept.extend(config.vale.code_comment_extra_accept.iter().cloned());
        Ok(Self {
            ontology,
            terms: TermIndex::build(ontology),
            token_re: Regex::new(&alt).context("compile comment-token regex")?,
            accept,
        })
    }

    /// Lints one file's comments. `require_anchor` turns on the
    /// `MissingAnchor` rule (callers pass `config.requires_anchor(..)`);
    /// it only fires on public, named symbols.
    pub fn lint(&self, file: &Path, comments: &[DocComment], require_anchor: bool) -> Vec<Issue> {
        let mut issues = Vec::new();
        for comment in comments {
            let base = base_indent(&comment.text);
            let (mut in_fence, mut in_indented, mut prev_blank) = (false, false, true);
            for (idx, line) in comment.text.lines().enumerate() {
                if line.trim_start().starts_with("```") {
                    in_fence = !in_fence;
                    continue;
                }
                // Markdown indented code block: 4+ columns past the
                // comment's own prose indent, opened by a blank line.
                let indent = line.len() - line.trim_start().len();
                in_indented =
                    !line.trim().is_empty() && indent >= base + 4 && (prev_blank || in_indented);
                prev_blank = line.trim().is_empty();
                if in_fence || in_indented || is_endpoint_marker_line(line) {
                    continue;
                }
                for m in self.token_re.find_iter(line) {
                    let term = m.as_str();
                    let col = line[..m.start()].chars().count() + 1;
                    if self.accept.contains(term)
                        || vale::is_sentence_start(line, 1, col)
                        || vale::alert_inside_inline_code(line, 1, col)
                        || !self.terms.lookup(term).is_empty()
                    {
                        continue;
                    }
                    issues.push(Issue::CommentVocabViolation {
                        file: file.to_path_buf(),
                        line: comment.line + idx,
                        col,
                        term: term.to_string(),
                    });
                }
            }
            if let Some(symbol) = &comment.attached_to {
                if require_anchor
                    && comment.is_public
                    && symbol != "<module>"
                    && !comment_has_anchor(&comment.text, self.ontology)
                {
                    issues.push(Issue::MissingAnchor {
                        file: file.to_path_buf(),
                        line: comment.line,
                        symbol: symbol.clone(),
                    });
                }
            }
        }
        issues
    }
}

/// Smallest indent among a comment's non-blank lines after the first —
/// its prose margin (Python docstrings keep the source indentation; the
/// first line sits on the opener's line).
fn base_indent(text: &str) -> usize {
    text.lines()
        .skip(1)
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0)
}

/// Roadmap-49: `@endpoint <METHOD> <path>` marker lines are structural
/// metadata, not prose — the verb and verbatim path skip vocab closure.
pub fn is_endpoint_marker_line(line: &str) -> bool {
    static RE: OnceLock<Regex> = OnceLock::new();
    #[allow(clippy::expect_used, reason = "literal regex; compile is infallible")]
    RE.get_or_init(|| Regex::new(r"^\s*@endpoint\s+[A-Z]+\s+\S").expect("endpoint marker regex"))
        .is_match(line)
}

/// True if `text` references any ontology entity (by id, display or
/// synonym) or a `roadmap-…` doc id — what the `MissingAnchor` rule asks
/// of a public symbol's doc comment.
pub fn comment_has_anchor(text: &str, ontology: &Ontology) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("roadmap-")
        || ontology.entities.values().any(|e| {
            std::iter::once(&e.id)
                .chain(std::iter::once(&e.display))
                .chain(&e.synonyms)
                .any(|t| lower.contains(&t.to_ascii_lowercase()))
        })
}
