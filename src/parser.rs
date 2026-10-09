//! YAML frontmatter + markdown-link/wikilink extraction for the
//! [[entity-doc-graph]] linter — defines the `Doc` shape every other
//! module consumes.
use anyhow::{anyhow, Context, Result};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

/// Parsed YAML frontmatter shape used by the [[entity-doc-graph]] linter
/// — `id`, `role`, `kind`, `lifecycle`, status, dates, and free-form
/// extras.
#[derive(Debug, Deserialize)]
pub struct Frontmatter {
    pub id: String,

    // Ontology v1: `role:` is the top-level classifier (replaces legacy `type:`).
    // Kept as Option so docs in the middle of migration parse without panic;
    // the validator emits `MissingField("role")` or `LegacyTypeField` when
    // appropriate.
    #[serde(default)]
    pub role: Option<String>,

    // Legacy `type:` field. Pre-ontology docs used this. Migration 0001
    // converts every occurrence into `role:` (+ kind/lifecycle as needed).
    // Kept here so the parser doesn't fail outright on old docs; validator
    // turns presence into a LegacyTypeField issue.
    #[serde(rename = "type", default)]
    pub legacy_type: Option<String>,

    // Diátaxis quadrant — required when role is `doc`.
    #[serde(default)]
    pub kind: Option<String>,

    // Project-lifecycle stage — conditional per role (see ontology axes).
    #[serde(default)]
    pub lifecycle: Option<String>,

    // DDD bounded-context (ontology v4). Optional on every doc. Single value —
    // a doc lives in at most one context. Validated against the closed
    // per-repo vocabulary in `docs/ontology/values/bounded-context/`. The
    // doc's *effective* context is computed in
    // `crate::ontology::effective_bounded_context` (frontmatter → path map →
    // repo default).
    #[serde(default)]
    pub bounded_context: Option<String>,

    pub title: String,
    pub summary: String,
    pub status: String,
    pub updated: chrono::NaiveDate,

    // Known-optional fields — deserialize them so unknown fields remain visible via extra.
    #[serde(default)]
    pub covers: Vec<String>,

    #[serde(default)]
    pub tags: Vec<String>,

    #[serde(default)]
    pub owner: Option<String>,

    /// Optional advisory hint controlling whether `doc-linter export`
    /// (or any sibling publish pipeline) copies this doc out of a
    /// private vault. Validated against `LintConfig::allowed_visibility`
    /// when present. The publish script is the actual enforcement
    /// point — this field is a signal for the script to read.
    #[serde(default)]
    pub visibility: Option<String>,

    /// Gap-003: roadmap phase membership — typically `v1`, `v2`,
    /// `mvp`, or `deferred`. Validated against the configured
    /// vocabulary if `phase_values` is set in `.doc-lint.toml`;
    /// otherwise free-form. Surfaced via the `phasing` saved
    /// query so agents can ask "what's in v1 vs deferred?" in
    /// one MCP call instead of grepping prose.
    #[serde(default)]
    pub phase: Option<String>,

    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_yaml::Value>,
}

impl Frontmatter {
    /// Returns the effective role for this doc, or None if neither `role:`
    /// nor legacy `type:` is set. Prefers `role:` when both are present
    /// (the migration writes role first).
    pub fn effective_role(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// Reads a string from `extra` (the catch-all map for ontology-specific
    /// fields like `axis_id`, `value_id`, `requires_axes`, etc.) on a
    /// [[entity-doc-graph]] frontmatter. Returns None if absent or not a
    /// string.
    pub fn extra_str(&self, key: &str) -> Option<&str> {
        self.extra.get(key).and_then(|v| v.as_str())
    }

    /// Reads a string-list from `extra` (for `requires_axes`, `synonyms`,
    /// `allowed_lifecycle`, etc.) on a [[entity-doc-graph]] frontmatter.
    /// Returns empty Vec if absent.
    pub fn extra_str_list(&self, key: &str) -> Vec<String> {
        self.extra
            .get(key)
            .and_then(|v| v.as_sequence())
            .map(|seq| {
                seq.iter()
                    .filter_map(|item| item.as_str().map(std::string::ToString::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One parsed markdown doc — frontmatter, body, and the extracted
/// wikilink + markdown-link sets used by the [[entity-doc-graph]]
/// validator and graph builders.
pub struct Doc {
    pub meta: Option<Frontmatter>,
    pub body: String,
    /// 1-based line number in the file where `body` starts. For docs without
    /// frontmatter this is 1; for docs with frontmatter it is the line
    /// immediately after the closing `---`.
    pub body_line_offset: usize,
    pub raw_frontmatter: Option<String>,
}

impl Doc {
    /// Converts a 1-based line within `body` to a 1-based line within the
    /// file — used by [[entity-doc-graph]] diagnostics that need to point
    /// past the frontmatter into the rendered body.
    pub fn file_line(&self, body_line: usize) -> usize {
        body_line + self.body_line_offset - 1
    }
}

/// One `[[wikilink]]` occurrence in a doc body — target id and source
/// line. Consumed by the [[entity-doc-graph]] graph builders.
pub struct Wikilink {
    pub target: String,
    pub line: usize,
}

/// One markdown `[text](target)` link occurrence — target path and
/// source line. Consumed by the [[entity-doc-graph]] graph builders.
pub struct MarkdownLink {
    /// The URL/path in parentheses, stripped of any `#fragment`.
    pub target: String,
    /// The original target including fragment, used only for diagnostics.
    pub raw: String,
    pub line: usize,
}

/// One bare path occurrence (e.g. `crates/foo/src/lib.rs`) extracted
/// from prose — surfaces in [[entity-doc-graph]] unlinked-path warnings.
pub struct InlinePath {
    /// The backticked text that looks like a file path.
    pub text: String,
    pub line: usize,
}

/// One pass over the doc body extracts all three link flavors used by the
/// [[entity-doc-graph]] graph builders — wikilinks, markdown links, and
/// bare inline path mentions.
pub struct DocLinks {
    pub wikilinks: Vec<Wikilink>,
    pub md_links: Vec<MarkdownLink>,
    pub inline_paths: Vec<InlinePath>,
}

impl Frontmatter {
    /// Returns (field_name, target_id) pairs for each typed-edge entry in
    /// the [[entity-doc-graph]] frontmatter. Targets are taken verbatim —
    /// resolution to an actual doc happens in the graph builder.
    pub fn typed_edges(&self) -> Vec<(&'static str, String)> {
        // Migration 0005 removed `implements`, `blocks`, and
        // `owned-by` from this list (see [[ontology-mig-0005]]).
        const FIELDS: &[&str] = &["depends-on", "informed-by", "supersedes"];
        let mut edges = Vec::new();
        for &field in FIELDS {
            let Some(value) = self.extra.get(field) else {
                continue;
            };
            if let Some(seq) = value.as_sequence() {
                for item in seq {
                    if let Some(s) = item.as_str() {
                        let t = s.trim();
                        if !t.is_empty() {
                            edges.push((field, t.to_string()));
                        }
                    }
                }
            } else if let Some(s) = value.as_str() {
                let t = s.trim();
                if !t.is_empty() {
                    edges.push((field, t.to_string()));
                }
            }
        }
        edges
    }
}

/// Reads, splits frontmatter from body, and extracts wikilinks +
/// markdown links for one doc — the entry point every
/// [[entity-doc-graph]] pipeline calls.
pub fn parse_doc(path: &Path) -> Result<Doc> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let (fm_text, body) = split_frontmatter(&text);

    let meta = if let Some(yaml) = &fm_text {
        Some(
            serde_yaml::from_str::<Frontmatter>(yaml)
                .with_context(|| "parse frontmatter")
                .map_err(|e| anyhow!("{e}"))?,
        )
    } else if is_adoc(path) {
        Some(synthesized_frontmatter(path, body))
    } else {
        None
    };

    // Compute 1-based file line where the body starts, by counting newlines
    // in the text up to the body's byte offset. Works for docs with or
    // without frontmatter (falls back to 1 when body == text).
    let body_pos = text.len() - body.len();
    let body_line_offset = text.as_bytes()[..body_pos]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1;

    Ok(Doc {
        meta,
        body: body.to_string(),
        body_line_offset,
        raw_frontmatter: fm_text,
    })
}

/// True for AsciiDoc files (`.adoc`), which join the corpus alongside
/// markdown.
pub fn is_adoc(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()) == Some("adoc")
}

/// Frontmatter for a doc that has none, so it can still be a graph node
/// and show up in search. `parse_doc` applies it to AsciiDoc (most
/// `.adoc` corpora, like Fineract's 217 docs, carry only a `= Title`
/// header); the graph ingest applies it to markdown, which `check` still
/// flags as `missing-frontmatter`. The title is the first `= ` / `# `
/// heading. The id is the path relative
/// to the nearest `.doc-lint.toml` (slugged, so `docs/guide/index.adoc`
/// is `docs-guide-index`): stems like `index` repeat across chapters.
/// A synthesized doc is recognisable by `meta.is_some()` with
/// `raw_frontmatter.is_none()`, and `check` indexes it without
/// validating frontmatter rules it never opted into.
pub fn synthesized_frontmatter(path: &Path, body: &str) -> Frontmatter {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("doc");
    let rel = path
        .ancestors()
        .skip(1)
        .find(|dir| dir.join(".doc-lint.toml").is_file())
        .and_then(|root| path.strip_prefix(root).ok())
        .map_or_else(
            || stem.to_string(),
            |r| r.with_extension("").to_string_lossy().into_owned(),
        );
    let id = rel
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let title_prefix = if is_adoc(path) { "= " } else { "# " };
    let title = body
        .lines()
        .find_map(|l| l.strip_prefix(title_prefix))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or(stem)
        .to_string();
    let updated = fs::metadata(path).and_then(|m| m.modified()).map_or_else(
        |_| chrono::Utc::now().date_naive(),
        |t| chrono::DateTime::<chrono::Utc>::from(t).date_naive(),
    );
    Frontmatter {
        id,
        role: Some("doc".to_string()),
        legacy_type: None,
        kind: None,
        lifecycle: None,
        bounded_context: None,
        title,
        summary: first_paragraph(body),
        status: "stable".to_string(),
        updated,
        covers: Vec::new(),
        tags: Vec::new(),
        owner: None,
        visibility: None,
        phase: None,
        extra: std::collections::BTreeMap::new(),
    }
}

/// The body's first prose paragraph (headings, AsciiDoc `:attr:` lines and
/// fences skipped), capped at 300 chars: the summary search ranks a
/// synthesized doc by.
fn first_paragraph(body: &str) -> String {
    let mut para: Vec<&str> = Vec::new();
    for line in body.lines().map(str::trim) {
        let structural = ["#", "=", ":", "```", "----", "[", "<!--"]
            .iter()
            .any(|p| line.starts_with(p));
        if line.is_empty() || structural {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        para.push(line);
    }
    let text = para.join(" ");
    match text.char_indices().nth(300) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    }
}

/// Splits `---\n<yaml>\n---\n<body>` from a [[entity-doc-graph]] markdown
/// file. Returns (Some(yaml), body) if frontmatter present, else
/// (None, original text).
fn split_frontmatter(text: &str) -> (Option<String>, &str) {
    let trimmed_start = text.trim_start_matches('\u{feff}'); // BOM
    if !trimmed_start.starts_with("---") {
        return (None, text);
    }
    // Require a newline after the opening ---
    let after_open = match trimmed_start.strip_prefix("---\n") {
        Some(s) => s,
        None => match trimmed_start.strip_prefix("---\r\n") {
            Some(s) => s,
            None => return (None, text),
        },
    };
    // Find the closing ---, either at the start of a line
    let mut offset = 0;
    for (idx, line) in after_open.split_inclusive('\n').enumerate() {
        let line_no_newline = line.trim_end_matches(['\n', '\r']);
        if line_no_newline == "---" {
            let yaml = &after_open[..offset];
            let body_start = offset + line.len();
            let body = &after_open[body_start..];
            let _ = idx;
            return (Some(yaml.to_string()), body);
        }
        offset += line.len();
    }
    (None, text)
}

#[cfg(test)]
mod split_frontmatter_proptests {
    use super::split_frontmatter;
    use proptest::prelude::*;

    proptest! {
        /// For any text that does NOT begin with `---`, the splitter
        /// returns `(None, original_text)` — no frontmatter, no body
        /// rewrite. Generated input is constrained to UTF-8 strings
        /// whose first three bytes aren't `---` (and aren't BOM, which
        /// is trimmed by the splitter).
        #[test]
        fn no_dash_prefix_yields_none(text in "[^-\u{feff}].*|[^-].*\\n.*") {
            let (fm, body) = split_frontmatter(&text);
            prop_assert_eq!(fm, None);
            prop_assert_eq!(body, text.as_str());
        }

        /// For well-formed frontmatter (`---\n<yaml>\n---\n<body>`),
        /// the splitter reproduces the original by concatenation:
        /// `"---\n" + yaml + "---\n" + body == text`. Asserts the
        /// splitter doesn't drop / duplicate / mangle bytes — the
        /// kind of bug a hand-written assertion would never catch
        /// because no concrete fixture exercises every variation.
        ///
        /// YAML body is constrained to ASCII without `\n---\n` so we
        /// don't have to think about nested closing markers — those
        /// are a separate invariant.
        #[test]
        fn well_formed_input_roundtrips(
            yaml in "[a-zA-Z0-9 :_.-]{0,40}",
            body in "[a-zA-Z0-9 \n.,!?-]{0,40}",
        ) {
            // Reject yaml lines that would themselves be a closing `---`.
            prop_assume!(!yaml.lines().any(|l| l.trim_end_matches(['\r', '\n']) == "---"));

            let text = format!("---\n{yaml}\n---\n{body}");
            let (fm, got_body) = split_frontmatter(&text);
            prop_assert_eq!(fm, Some(format!("{yaml}\n")));
            prop_assert_eq!(got_body, body.as_str());
        }
    }
}

/// Single-pass extraction of wikilinks, markdown links, and inline-backtick
/// path-like tokens. Driven by pulldown-cmark events, so fenced code blocks,
/// indented code blocks, and inline code spans are filtered out by the parser
/// rather than by ad-hoc fence tracking.
///
/// - Markdown links come from `Tag::Link` events.
/// - Inline backtick path-like tokens come from `Event::Code` events that are
///   not nested inside a link label.
/// - Wikilinks (`[[target]]`) are not CommonMark; pulldown-cmark mangles them
///   into reference-link parses. We collect the byte ranges of all code
///   blocks / code spans from pulldown-cmark, then scan the raw body for
///   wikilinks and discard any match whose start falls inside one of those
///   ranges. The benefit over the legacy regex pass is that pulldown-cmark
///   knows about indented code blocks, info-string fences, nested fences,
///   and inline code — we no longer need our own fence tracker.
pub fn extract_links(body: &str) -> DocLinks {
    static WIKI: OnceLock<Regex> = OnceLock::new();
    #[allow(
        clippy::unwrap_used,
        reason = "literal regex; compile is infallible at runtime"
    )]
    let wiki = WIKI
        .get_or_init(|| Regex::new(r"\[\[([^\]\|\#]+)(?:\#[^\]\|]+)?(?:\|[^\]]+)?\]\]").unwrap());

    let mut md_links = Vec::new();
    let mut inline_paths = Vec::new();
    let mut code_ranges: Vec<(usize, usize)> = Vec::new();

    // Depth counter so nested links (rare, but possible via reference syntax)
    // don't break inline-code suppression inside link labels.
    let mut link_depth: usize = 0;

    let parser = Parser::new(body);
    for (event, range) in parser.into_offset_iter() {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                let raw = dest_url.into_string();
                let line = byte_offset_to_line(body, range.start);
                link_depth += 1;
                if is_url(&raw) || raw.starts_with('#') {
                    continue;
                }
                let target = strip_fragment(&raw).to_string();
                if target.is_empty() {
                    continue;
                }
                md_links.push(MarkdownLink { target, raw, line });
            }
            Event::End(TagEnd::Link) => {
                link_depth = link_depth.saturating_sub(1);
            }
            Event::Start(Tag::CodeBlock(_)) => {
                code_ranges.push((range.start, range.end));
            }
            Event::Code(code) => {
                code_ranges.push((range.start, range.end));
                if link_depth > 0 {
                    continue;
                }
                if is_path_like(&code) {
                    inline_paths.push(InlinePath {
                        text: code.into_string(),
                        line: byte_offset_to_line(body, range.start),
                    });
                }
            }
            _ => {}
        }
    }

    // Now scan the raw body for wikilinks, skipping anything that falls inside
    // a code block or inline code span as identified by pulldown-cmark.
    let mut wikilinks = Vec::new();
    for cap in wiki.captures_iter(body) {
        // Group 0 is the whole match; group 1 is the inner target which is
        // a non-optional capture in WIKI. Both are guaranteed present.
        #[allow(clippy::unwrap_used, reason = "group 0 and 1 are mandatory captures")]
        let whole = cap.get(0).unwrap();
        let start = whole.start();
        if code_ranges.iter().any(|&(s, e)| start >= s && start < e) {
            continue;
        }
        #[allow(clippy::unwrap_used, reason = "group 1 is a mandatory capture in WIKI")]
        let raw_target = cap.get(1).unwrap().as_str();
        let target = normalize_wikilink_target(raw_target);
        if target.is_empty() {
            continue;
        }
        wikilinks.push(Wikilink {
            target,
            line: byte_offset_to_line(body, start),
        });
    }

    DocLinks {
        wikilinks,
        md_links,
        inline_paths,
    }
}

/// Normalize a captured wikilink target so a line-wrapped wikilink
/// (`[[research-\nadaptive-rag]]`) resolves to the same target as the
/// single-line form. Per interrogation-043 Finding C: the regex
/// character class `[^\]\|\#]+` matches across newlines, so the
/// captured target string contains internal `\r` and `\n` bytes when
/// the source markdown wraps a wikilink across lines. Strip line
/// breaks, then `.trim()` to remove any whitespace at the new
/// boundaries.
fn normalize_wikilink_target(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c != '\n' && c != '\r' {
            out.push(c);
        }
    }
    out.trim().to_string()
}

/// Convert a byte offset in `body` to a 1-based line number — used by
/// [[entity-doc-graph]] link extraction to attach line numbers to
/// wikilink / md-link / inline-path occurrences.
/// AsciiDoc `xref:target[text]` links, as [`MarkdownLink`]s whose
/// target is relative to the doc (`#fragment` stripped). Antora
/// coordinates (`xref:module:page.adoc[]`) and URLs are skipped: they
/// don't name a file relative to the doc.
pub fn extract_adoc_xrefs(body: &str) -> Vec<MarkdownLink> {
    static XREF: OnceLock<Regex> = OnceLock::new();
    #[allow(
        clippy::unwrap_used,
        reason = "literal regex; compile is infallible at runtime"
    )]
    let xref = XREF.get_or_init(|| Regex::new(r"xref:([^\[\s]+)\[").unwrap());
    xref.captures_iter(body)
        .filter_map(|cap| {
            let whole = cap.get(0)?;
            let raw = cap.get(1)?.as_str();
            let target = strip_fragment(raw);
            if target.is_empty() || target.contains(':') {
                return None;
            }
            Some(MarkdownLink {
                target: target.to_string(),
                raw: raw.to_string(),
                line: byte_offset_to_line(body, whole.start()),
            })
        })
        .collect()
}

fn byte_offset_to_line(body: &str, offset: usize) -> usize {
    let cap = offset.min(body.len());
    body.as_bytes()[..cap]
        .iter()
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

/// True when a markdown link target starts with `http://` or `https://`
/// so the [[entity-doc-graph]] link extractor knows to skip URL targets.
fn is_url(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with("ftp://")
        || lower.starts_with("ftps://")
        || lower.starts_with("tel:")
}

/// Drops a `#fragment` suffix so the [[entity-doc-graph]] link
/// resolver can match the bare path component.
fn strip_fragment(s: &str) -> &str {
    s.split_once('#').map_or(s, |(head, _)| head)
}

/// Heuristic: is this backtick content a filesystem path that a reader would
/// reasonably expect to exist? We're conservative — false positives become
/// noisy lint errors. Rules:
///   - Starts with `./` or `../` → path
///   - Contains `/` AND the final component has a known extension → path
/// Skipped: URLs, glob/regex patterns, template placeholders.
fn is_path_like(text: &str) -> bool {
    let t = text.trim();
    if t.is_empty() {
        return false;
    }
    if is_url(t) {
        return false;
    }
    if t.contains('*') || t.contains('?') || t.contains('{') || t.contains('<') {
        return false;
    }
    // Whitespace suggests a shell command or prose, not a bare path.
    if t.chars().any(char::is_whitespace) {
        return false;
    }
    // User-home (`~/foo`) and absolute (`/foo`) paths can't be expressed as
    // repo-relative markdown links — flagging them produces a "fix" that
    // would just create a broken link.
    if t.starts_with("~/") || t.starts_with('/') {
        return false;
    }
    if t.starts_with("./") || t.starts_with("../") {
        return true;
    }
    if !t.contains('/') {
        return false;
    }
    // Take the path portion before any whitespace, colon (line anchor `:42`),
    // or `#` fragment.
    let path_part = t
        .split(|c: char| c.is_whitespace() || c == '#')
        .next()
        .unwrap_or(t);
    let last_segment = path_part.rsplit('/').next().unwrap_or("");
    let Some((_, ext)) = last_segment.rsplit_once('.') else {
        return false;
    };
    let ext = ext.split(':').next().unwrap_or("").to_ascii_lowercase();
    const KNOWN_EXTS: &[&str] = &[
        "md",
        "rs",
        "toml",
        "yaml",
        "yml",
        "json",
        "sh",
        "ts",
        "tsx",
        "py",
        "js",
        "jsx",
        "txt",
        "lock",
        "sql",
        "conf",
        "ini",
        "env",
        "dockerfile",
        "html",
        "css",
        "scss",
        "proto",
        "graphql",
        "mjs",
        "cjs",
        "kt",
        "java",
        "go",
        "c",
        "cpp",
        "h",
        "hpp",
    ];
    KNOWN_EXTS.iter().any(|&k| k == ext)
}

/// One heading-delimited slice of a doc body: the heading, its
/// GitHub-style anchor, and the text up to the next heading of any level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub heading: String,
    pub anchor: String,
    pub level: u8,
    /// 1-based line of the heading within `body`.
    pub line: usize,
    pub text: String,
}

/// Split a doc body on ATX headings (`#`..`######`), or AsciiDoc `=`
/// headings when `adoc`, ignoring headings inside fenced / delimited
/// blocks. Text before the first heading belongs to no section. Anchors
/// follow GitHub's slug rules, with `-1`, `-2` suffixes on repeats, so
/// `<doc-id>#<anchor>` matches the rendered link.
pub fn split_sections(body: &str, adoc: bool) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut fence: Option<&str> = None;
    let markers: &[&str] = if adoc {
        &["```", "~~~", "----", "...."]
    } else {
        &["```", "~~~"]
    };
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        let marker = markers.iter().copied().find(|m| trimmed.starts_with(m));
        match (fence, marker) {
            (None, Some(m)) => fence = Some(m),
            (Some(f), Some(m)) if f == m => fence = None,
            _ => {}
        }
        let heading = if fence.is_some() || marker.is_some() {
            None
        } else if adoc {
            adoc_heading(line)
        } else {
            atx_heading(line)
        };
        if let Some((level, heading)) = heading {
            let base = heading_slug(&heading);
            let n = seen.entry(base.clone()).or_insert(0);
            let anchor = if *n == 0 {
                base.clone()
            } else {
                format!("{base}-{n}")
            };
            *n += 1;
            out.push(Section {
                heading,
                anchor,
                level,
                line: i + 1,
                text: String::new(),
            });
        } else if let Some(cur) = out.last_mut() {
            cur.text.push_str(line);
            cur.text.push('\n');
        }
    }
    for s in &mut out {
        s.text = s.text.trim().to_string();
    }
    out
}

fn atx_heading(line: &str) -> Option<(u8, String)> {
    // CommonMark allows up to 3 spaces of indent before the `#`s.
    if line.len() - line.trim_start_matches(' ').len() > 3 {
        return None;
    }
    let t = line.trim_start_matches(' ');
    let level = t.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &t[level..];
    if !(rest.is_empty() || rest.starts_with([' ', '\t'])) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim_end();
    (!text.is_empty()).then(|| (level as u8, text.to_string()))
}

/// AsciiDoc section title: `= Title` (level 1, the document title) down
/// to `====== Title` (level 6), mirroring `#`..`######`.
fn adoc_heading(line: &str) -> Option<(u8, String)> {
    let level = line.bytes().take_while(|&b| b == b'=').count();
    if level == 0 || level > 6 {
        return None;
    }
    let text = line[level..].strip_prefix(' ')?.trim();
    (!text.is_empty()).then(|| (level as u8, text.to_string()))
}

fn heading_slug(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

/// Frontmatter + link-extraction tests for the [[entity-doc-graph]]
/// markdown parser.
#[cfg(test)]
mod tests {
    use super::*;

    /// Handoff cpg-os-vault #6: rules like T1..T12 live under headings in
    /// one long doc; each heading becomes its own retrievable section.
    #[test]
    fn split_sections_on_headings_skipping_code_fences() {
        let body = "intro\n\n## T7: Gross predicate\nBuying means gross > 0.\n\n\
                    ```md\n# not a heading\n```\n### Notes\nx\n## T7: Gross predicate\ny\n";
        let s = split_sections(body, false);
        let got: Vec<(&str, &str, u8, usize)> = s
            .iter()
            .map(|s| (s.heading.as_str(), s.anchor.as_str(), s.level, s.line))
            .collect();
        assert_eq!(
            got,
            [
                ("T7: Gross predicate", "t7-gross-predicate", 2, 3),
                ("Notes", "notes", 3, 9),
                ("T7: Gross predicate", "t7-gross-predicate-1", 2, 11),
            ]
        );
        assert_eq!(
            s[0].text,
            "Buying means gross > 0.\n\n```md\n# not a heading\n```"
        );
        assert!(split_sections("no headings\n#hashtag\n", false).is_empty());
    }

    /// AsciiDoc: `=` headings make sections, `----` blocks hide them.
    #[test]
    fn split_sections_adoc_headings() {
        let body = "= Guide\n\n== Install\nRun it.\n----\n== not a heading\n----\n=== Notes\nx\n";
        let got: Vec<(String, u8)> = split_sections(body, true)
            .into_iter()
            .map(|s| (s.heading, s.level))
            .collect();
        assert_eq!(
            got,
            [
                ("Guide".to_string(), 1),
                ("Install".to_string(), 2),
                ("Notes".to_string(), 3)
            ]
        );
        assert!(split_sections(body, false).is_empty());
    }

    #[test]
    fn adoc_xrefs_resolve_relative_targets_only() {
        let body = "Intro.\nSee xref:loans/charges.adoc#fees[Fees] and \
                    xref:ROOT:index.adoc[home].\nxref:https://x.org[x]\n";
        let links: Vec<(String, usize)> = extract_adoc_xrefs(body)
            .into_iter()
            .map(|l| (l.target, l.line))
            .collect();
        assert_eq!(links, [("loans/charges.adoc".to_string(), 2)]);
    }

    /// An `.adoc` file without YAML frontmatter still parses into a doc:
    /// title from `= Title`, id from its path under the repo root.
    #[test]
    fn adoc_without_frontmatter_gets_synthesized_meta() {
        let root = std::env::temp_dir().join(format!("doc-linter-adoc-{}", std::process::id()));
        let file = root.join("docs/guide/index.adoc");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(root.join(".doc-lint.toml"), "").unwrap();
        fs::write(&file, ":toc:\n= Loan Guide\n\n== Approve\nText.\n").unwrap();
        let doc = parse_doc(&file).unwrap();
        let meta = doc.meta.unwrap();
        assert_eq!(meta.id, "docs-guide-index");
        assert_eq!(meta.title, "Loan Guide");
        assert_eq!(meta.summary, "Text.");
        assert_eq!(meta.role.as_deref(), Some("doc"));
        assert!(doc.raw_frontmatter.is_none());
        fs::remove_dir_all(&root).ok();
    }

    /// Asserts that `split_frontmatter` separates a `---`-fenced YAML
    /// block from the body for the [[entity-doc-graph]] parser.
    #[test]
    fn splits_basic_frontmatter() {
        let input = "---\nid: foo\ntitle: Bar\n---\n# Heading\n\nBody.\n";
        let (fm, body) = split_frontmatter(input);
        assert_eq!(fm.as_deref(), Some("id: foo\ntitle: Bar\n"));
        assert_eq!(body, "# Heading\n\nBody.\n");
    }

    /// Asserts that `split_frontmatter` returns `None` when no leading
    /// `---` fence is present in the [[entity-doc-graph]] parser.
    #[test]
    fn no_frontmatter_returns_none() {
        let input = "# Heading\n\nBody.\n";
        let (fm, body) = split_frontmatter(input);
        assert!(fm.is_none());
        assert_eq!(body, input);
    }

    /// Asserts that `extract_links` recovers plain `[[target]]`,
    /// piped-alias, and `#fragment` wikilinks for the [[entity-doc-graph]]
    /// parser.
    #[test]
    fn extracts_plain_wikilinks() {
        let body = "See [[scalability]] and [[scaling-playbook|the playbook]].\n[[other#anchor]]";
        let links = extract_links(body).wikilinks;
        let targets: Vec<_> = links.iter().map(|w| w.target.as_str()).collect();
        assert_eq!(targets, vec!["scalability", "scaling-playbook", "other"]);
    }

    /// Per interrogation-043 Finding C (iter 215): a wikilink whose
    /// `[[…]]` spans a markdown line-wrap was previously captured by
    /// the regex but resolved to a target string containing `\n`, so
    /// downstream doc-id lookup silently dropped it. Verify that the
    /// iter-215 `normalize_wikilink_target` strips line breaks so the
    /// wrapped form resolves to the same target as the single-line
    /// form.
    #[test]
    fn extracts_line_wrapped_wikilinks() {
        let body =
            "Sibling: [[research-crag]] (Corrective RAG); [[research-\nadaptive-rag]] (per-query strategy selection).";
        let links = extract_links(body).wikilinks;
        let targets: Vec<_> = links.iter().map(|w| w.target.as_str()).collect();
        assert_eq!(
            targets,
            vec!["research-crag", "research-adaptive-rag"],
            "line-wrapped wikilink must normalize to single-line target",
        );
    }

    /// Per interrogation-043 Finding C: the `normalize_wikilink_target`
    /// helper handles the 4 normalization cases used by extract_links.
    #[test]
    fn normalize_wikilink_target_strips_line_breaks() {
        // Mid-string \n (the iter-215 root-cause case): collapses to
        // single-line target.
        assert_eq!(
            normalize_wikilink_target("research-\nadaptive-rag"),
            "research-adaptive-rag",
        );
        // Windows-style \r\n: both stripped.
        assert_eq!(
            normalize_wikilink_target("research-\r\nadaptive-rag"),
            "research-adaptive-rag",
        );
        // Leading + trailing whitespace: trimmed (existing behavior).
        assert_eq!(
            normalize_wikilink_target("  research-rag  "),
            "research-rag",
        );
        // Spaces preserved (no aggressive normalization beyond line
        // breaks — multi-word wikilink targets still fail downstream
        // doc-id lookup as before).
        assert_eq!(normalize_wikilink_target("some title"), "some title",);
        // Empty-after-strip returns empty (caller skips empty
        // targets).
        assert_eq!(normalize_wikilink_target("\n\r\n"), "");
    }

    /// Asserts that `extract_links` skips `[[wikilinks]]` that appear
    /// inside fenced code blocks in the [[entity-doc-graph]] parser.
    #[test]
    fn ignores_wikilinks_inside_fenced_code_blocks() {
        let body = "\
Real link: [[real-target]]

```
This is code with a [[fake-target]] that must not match.
```

After the fence: [[after]]
";
        let links = extract_links(body);
        let targets: Vec<_> = links.wikilinks.iter().map(|w| w.target.as_str()).collect();
        assert_eq!(targets, vec!["real-target", "after"]);
    }

    #[test]
    fn ignores_wikilinks_inside_inline_code_spans() {
        let body = "Real: [[real]]. Fake in code: `[[fake]]` should not be extracted.";
        let links = extract_links(body);
        let targets: Vec<_> = links.wikilinks.iter().map(|w| w.target.as_str()).collect();
        assert_eq!(targets, vec!["real"]);
    }

    /// Asserts that `extract_links` strips the optional `"title"`
    /// attribute when capturing `[text](target "title")` syntax in the
    /// [[entity-doc-graph]] parser.
    #[test]
    fn extracts_md_link_with_title_attribute() {
        let body = r#"See [the spec](spec.md "Specification details") for more."#;
        let links = extract_links(body);
        assert_eq!(links.md_links.len(), 1);
        assert_eq!(links.md_links[0].target, "spec.md");
        assert_eq!(links.md_links[0].raw, "spec.md");
    }

    #[test]
    fn home_and_absolute_paths_are_not_path_like() {
        // `~/foo` and `/foo` can't be expressed as repo-relative md links,
        // so they should not trigger `unlinked-path`.
        assert!(!is_path_like(
            "~/.ipython/profile_default/ipython_config.py"
        ));
        assert!(!is_path_like("/etc/nginx/nginx.conf"));
        // Sanity: repo-relative paths still detected.
        assert!(is_path_like("src/main.rs"));
        assert!(is_path_like("./scripts/install.sh"));
    }
}
