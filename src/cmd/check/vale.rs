//! Round 2A: Vale vocabulary-closure integration. Generates the
//! per-repo Vale config tree, shells out to `vale`, then
//! post-processes alerts through the bounded-context disambiguator.
//! Soft-fails when the binary is missing — the rest of `check` keeps
//! working. Skipped entirely when `--no-vale` or
//! `vale_enabled = false`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use doc_linter::config::LintConfig;
use doc_linter::disambiguation::{is_vocab_closure_rule, resolve_alert, Resolution, TermIndex};
use doc_linter::ontology::{self, effective_bounded_context};
use doc_linter::parser::Doc;
use doc_linter::vale;
use doc_linter::validator;

/// Run the full Vale pipeline (config-gen → shell-out → resolve →
/// downgrade) over the corpus and return a flat `(path, issue)` list
/// ready to merge into the per-file report. Vale's `--output=JSON`
/// keys alerts by absolute path; we map back to the in-memory
/// `target_files` list so cross-repo docs and `--file` narrow scopes
/// both behave correctly.
pub(crate) fn run_vale_pipeline(
    root: &Path,
    config: &LintConfig,
    ontology: &ontology::Ontology,
    docs: &HashMap<PathBuf, Doc>,
    target_files: &[PathBuf],
) -> Result<Vec<(PathBuf, validator::Issue)>> {
    use std::collections::HashSet;

    // Pre-build the lowercase set of vale_ambiguous_words for quick
    // O(1) lookup in the alert loop. Used by the duplicate-alert
    // filter (task #12): a Vocabulary.Vocabulary alert on a term
    // that's also an ambiguous word duplicates the FA.AmbiguousBare
    // path, which the post-processor handles via cross-context-
    // reference. Drop the closure-rule duplicate.
    let ambiguous_lower: HashSet<String> = config
        .vale
        .vale_ambiguous_words
        .iter()
        .map(|w| w.to_lowercase())
        .collect();

    // 1. Regenerate the Vale config tree (idempotent — only writes files
    //    whose content changed).
    vale::generate_config(root, ontology, config).context("generate vale config")?;

    // 2. Detect the binary. Missing binary is a single repo-level
    //    diagnostic, attached to the first target file (or the root if
    //    target list is empty for some reason).
    let Some(vale_bin) = vale::detect() else {
        let attach_to = target_files
            .first()
            .cloned()
            .unwrap_or_else(|| root.to_path_buf());
        return Ok(vec![(attach_to, validator::Issue::ValeNotInstalled)]);
    };

    // Vale shells out to `asciidoctor` for `.adoc` and fails the whole run
    // without it, so leave AsciiDoc out (with one warning) when it's
    // missing rather than lose the markdown alerts too.
    let mut issues: Vec<(PathBuf, validator::Issue)> = Vec::new();
    let mut vale_files: Vec<PathBuf> = target_files.to_vec();
    if let Some(first_adoc) = target_files.iter().find(|p| doc_linter::parser::is_adoc(p)) {
        if which::which("asciidoctor").is_err() {
            issues.push((first_adoc.clone(), validator::Issue::AsciidoctorMissing));
            vale_files.retain(|p| !doc_linter::parser::is_adoc(p));
        }
    }

    // 3. Run Vale. A failure here (runtime error, parse error) becomes a
    //    single diagnostic so we don't silently lose vocab-closure coverage.
    let vale_output = match vale::run(root, &vale_bin, &vale_files) {
        Ok(o) => o,
        Err(e) => {
            let attach_to = target_files
                .first()
                .cloned()
                .unwrap_or_else(|| root.to_path_buf());
            issues.push((
                attach_to,
                validator::Issue::ValeFailed {
                    message: format!("{e:#}"),
                },
            ));
            return Ok(issues);
        }
    };

    // 4. Build the term index once for this run.
    let term_index = TermIndex::build(ontology);
    let target_set: std::collections::HashSet<&PathBuf> = target_files.iter().collect();

    let mut out = issues;
    for (file_str, alerts) in &vale_output {
        let file_path = PathBuf::from(file_str);
        let canon = file_path.canonicalize().unwrap_or(file_path.clone());
        // Vale only sees `target_files`; the check guards against it
        // reporting a path in a different spelling.
        if !target_set.contains(&canon) {
            continue;
        }

        // Issue #180: `auto`-status entity docs are cluster-derived
        // stubs whose body contains machine-extracted SCIP symbols
        // (e.g. `TestGitRevertSignal`, raw class names) that aren't
        // in the project vocabulary by design. Skip Vale processing for
        // these files entirely — a human review promotes them to
        // `stable` and rewrites the body in proper prose before the
        // vocab-closure lint should fire.
        let is_auto_entity = docs
            .get(&canon)
            .and_then(|d| d.meta.as_ref())
            .is_some_and(|m| m.status == "auto");
        if is_auto_entity {
            continue;
        }

        // Resolve the doc's effective bounded context for this file.
        let doc_context = docs
            .get(&canon)
            .and_then(|d| d.meta.as_ref())
            .and_then(|meta| effective_bounded_context(&canon, meta, config, root));

        // Phase 1 (Fix A): read the doc body once per file so we can
        // skip alerts whose `(line, col)` lands at a sentence-start
        // position. Vale's `\b[A-Z][A-Za-z]+\b` rule fires on every
        // capitalised word — including the first word of every
        // sentence (`When the sky is blue` → flags `When`). The body
        // read is cheap (already on disk) and amortised over every
        // alert in the file.
        let body = std::fs::read_to_string(&canon).unwrap_or_default();
        // Frontmatter is metadata doc-linter validates itself. Vale lints
        // its string values (title, summary) as prose and no BlockIgnores
        // reaches them, so alerts above the body are dropped here.
        let body_start = docs.get(&canon).map_or(1, |d| d.body_line_offset);

        for alert in alerts {
            if alert.line < body_start {
                continue;
            }
            // Closure rule (`Vocabulary.Vocabulary`): drop the alert
            // when it sits at a sentence-start position. Bare-ambiguous
            // alerts (`FA.AmbiguousBare`) keep their original behaviour
            // — those words are case-insensitive prose nouns where
            // sentence-start doesn't excuse the lack of qualifier.
            if alert.check == "Vocabulary.Vocabulary"
                && vale::is_sentence_start(&body, alert.line, alert.span[0])
            {
                continue;
            }
            // Task #12: drop closure-rule alerts that duplicate the
            // bare-ambiguous-noun path. When `vale_ambiguous_words`
            // contains the prose token (case-insensitive), the
            // FA.AmbiguousBare rule already fires on the same prose,
            // and the post-processor below resolves it via
            // cross-context-reference. Letting the Vocabulary rule
            // also fire would emit a duplicate "Term not in vocab"
            // warning on the same character offset.
            if alert.check == "Vocabulary.Vocabulary"
                && ambiguous_lower.contains(&alert.match_text.to_lowercase())
            {
                continue;
            }
            // Roadmap-51 Move 1: a `Vocabulary.Vocabulary` alert inside
            // a `[[wikilink]]`, an inline-code span (backticks), or
            // already part of a fully-qualified entity id is not a
            // closure violation — the prose has already disambiguated.
            // Mirrors the existing G8 suppressions for AmbiguousBare
            // downgrades but applies them at alert-routing time so
            // code-literal references like `MockEmitter` don't surface
            // as Vale warnings on every check.
            if alert.check == "Vocabulary.Vocabulary" {
                let col = alert.span[0];
                if vale::alert_inside_wikilink(&body, alert.line, col)
                    || vale::alert_inside_inline_code(&body, alert.line, col)
                {
                    continue;
                }
            }
            if is_vocab_closure_rule(&alert.check) {
                match resolve_alert(alert, doc_context.as_deref(), &term_index) {
                    Resolution::Suppress => {}
                    Resolution::Downgrade(issue) => {
                        // Three quality-improving suppressions: the
                        // author has ALREADY disambiguated and Vale's
                        // `\b` tokenisation just split the qualifier.
                        // Skipping these tightens the cross-context
                        // signal without losing it.
                        let suppress =
                            if let validator::Issue::CrossContextReference {
                                entity_id, line, ..
                            } = &issue
                            {
                                let col = alert.span[0];
                                vale::alert_inside_entity_id(&body, *line, col, entity_id)
                                    || vale::alert_inside_wikilink(&body, *line, col)
                                    || vale::alert_inside_inline_code(&body, *line, col)
                            } else {
                                false
                            };
                        if !suppress {
                            out.push((canon.clone(), issue));
                        }
                    }
                    Resolution::Keep => out.push((canon.clone(), vale::alert_to_issue(alert))),
                }
            } else {
                // Non-closure rules pass straight through as ValeAlert.
                out.push((canon.clone(), vale::alert_to_issue(alert)));
            }
        }
    }

    Ok(out)
}
