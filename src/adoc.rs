//! Built-in AsciiDoc prose extractor for the Vale vocabulary-closure pass.
//!
//! Vale renders `.adoc` by shelling out to Ruby's `asciidoctor`. Instead,
//! [`extract_prose`] turns an AsciiDoc source into plain prose that Vale
//! reads as a `.txt` file, with these guarantees:
//!
//! - **Line numbers are preserved.** The output has exactly one line per
//!   input line; non-prose lines are blanked, never deleted, so a Vale
//!   finding on line N of the extract is line N of the original.
//! - **Columns are preserved.** Markup and masked spans (inline code, URLs,
//!   attribute references, list markers) become spaces of equal character
//!   count, so `(line, col)` on the extract is `(line, col)` in the source.
//!
//! The `asciidoc-parser` crate supplies the block structure: it tells us
//! which line ranges are verbatim (`listing`, `literal`, `pass`, `stem`,
//! block media), including delimiter-less forms such as `[source]`
//! paragraphs and indented literal paragraphs, and maps them back to
//! original lines through its include/conditional source map. A small
//! line scanner then does the rest: comments, attribute entries, block
//! macros, delimiters, markers, tables, and inline masking.
//!
//! If the parser panics the scanner still runs on its own (delimited
//! blocks are tracked by the scanner too), so extraction never aborts a
//! check. Ceiling of that fallback: delimiter-less verbatim paragraphs are
//! then treated as prose.

use asciidoc_parser::blocks::{Block, FindBlocks, IsBlock, SimpleBlockStyle};
use asciidoc_parser::{HasSpan, Parser};
use regex::{Captures, Regex};
use std::sync::LazyLock;

#[allow(clippy::expect_used, reason = "pattern is a literal checked by tests")]
fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex literal")
}

struct Patterns {
    attr_entry: Regex,
    block_macro: Regex,
    block_attrs: Regex,
    block_anchor: Regex,
    delimiter: Regex,
    table_fence: Regex,
    marker: Regex,
    dlist: Regex,
    admonition: Regex,
    callout: Regex,
    passthrough: Vec<Regex>,
    literal_plus: Regex,
    attr_ref: Regex,
    xref: Regex,
    macro_text: Regex,
    footnote: Regex,
    url: Regex,
    macro_drop: Regex,
    inline_anchor: Regex,
    inline_role: Regex,
}

static P: LazyLock<Patterns> = LazyLock::new(|| Patterns {
    attr_entry: re(r"^:!?[\w][\w-]*!?:(\s.*)?$"),
    block_macro: re(r"^[A-Za-z][\w-]*::\S*\[.*\]$"),
    block_attrs: re(r"^\[[^\]]*\]$"),
    block_anchor: re(r"^\[\[[^\]]+\]\]$"),
    delimiter: re(r"^(={4,}|\*{4,}|_{4,}|--|'{3,}|<{3,}|\+)$"),
    table_fence: re(r"^[|,:!]={3,}$"),
    marker: re(r"^(\s*)(={1,6}|\*{1,5}|-|\.{1,5}|\d+\.)\s+(\[[ x*]\]\s+)?"),
    dlist: re(r"(\S)(:{2,4}|;;)(\s|$)"),
    admonition: re(r"^(NOTE|TIP|IMPORTANT|WARNING|CAUTION):(\s|$)"),
    callout: re(r"^\s*<\d+>\s"),
    passthrough: vec![
        re(r"\+\+\+.*?\+\+\+"),
        re(r"pass:\w*\[[^\]]*\]"),
        re(r"`[^`]*`"),
    ],
    literal_plus: re(r"(?:^|[^\w+])(\+[^+\s](?:[^+]*[^+\s])?\+)(?:$|[^\w+])"),
    attr_ref: re(r"\{[\w-]+\}"),
    xref: re(r"<<([^,>]+)(?:,([^>]*))?>>"),
    macro_text: re(r"(?:xref|link|mailto):[^\s\[]*\[([^\]]*)\]"),
    footnote: re(r"footnote:\w*\[([^\]]*)\]"),
    url: re(r"https?://[^\s\[<>]+(?:\[([^\]]*)\])?"),
    macro_drop: re(
        r"(?:image|icon|kbd|btn|menu|anchor|indexterm2?|stem|latexmath|asciimath|footnoteref):[^\s\[]*\[[^\]]*\]",
    ),
    inline_anchor: re(r"\[\[[^\]]+\]\]"),
    inline_role: re(r"\[[#.][^\]]*\]"),
});

/// Raw-block contexts whose content is not prose.
const VERBATIM: &[&str] = &[
    "listing",
    "literal",
    "pass",
    "stem",
    "image",
    "video",
    "audio",
    "toc",
    "thematic_break",
    "page_break",
];

/// 1-based original line ranges the parser classed as verbatim; `None` if
/// the parser panics.
fn verbatim_ranges(src: &str) -> Option<Vec<(usize, usize)>> {
    let run = || {
        let doc = Parser::default().parse(src);
        let mut out = Vec::new();
        for b in doc.descendant_blocks() {
            let literal_para = matches!(&b, Block::Simple(x) if x.style() != SimpleBlockStyle::Paragraph)
                || matches!(
                    b.declared_style(),
                    Some("pass" | "stem" | "latexmath" | "asciimath")
                );
            if literal_para || VERBATIM.contains(&b.raw_context().as_ref()) {
                let s = b.span();
                let start = doc.origin_of(s).line;
                let n = s.data().lines().count().max(1);
                out.push((start, start + n - 1));
            }
        }
        out
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(run)).ok()
}

fn spaces(s: &str) -> String {
    " ".repeat(s.chars().count())
}

/// Blank every match of `re` (same char count).
fn blank(s: &str, re: &Regex) -> String {
    re.replace_all(s, |c: &Captures<'_>| spaces(&c[0]))
        .into_owned()
}

/// Blank only capture group `g` of every match.
fn blank_group(s: &str, re: &Regex, g: usize) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for c in re.captures_iter(s) {
        if let Some(m) = c.get(g) {
            out.push_str(&s[last..m.start()]);
            out.push_str(&spaces(m.as_str()));
            last = m.end();
        }
    }
    out.push_str(&s[last..]);
    out
}

/// Keep capture group `g` (when non-empty) and blank the rest of each match.
fn keep_group(s: &str, re: &Regex, g: usize) -> String {
    re.replace_all(s, |c: &Captures<'_>| {
        let whole = &c[0];
        match c.get(g).filter(|m| !m.as_str().is_empty()) {
            Some(m) => {
                let off = m.start() - c.get(0).map_or(0, |w| w.start());
                format!(
                    "{}{}{}",
                    spaces(&whole[..off]),
                    m.as_str(),
                    spaces(&whole[off + m.as_str().len()..])
                )
            }
            None => spaces(whole),
        }
    })
    .into_owned()
}

fn mask_inline(line: &str) -> String {
    let p = &*P;
    let mut s = line.to_string();
    for r in &p.passthrough {
        s = blank(&s, r);
    }
    s = blank_group(&s, &p.literal_plus, 1);
    s = blank(&s, &p.attr_ref);
    s = keep_group(&s, &p.xref, 2);
    s = keep_group(&s, &p.footnote, 1);
    s = keep_group(&s, &p.macro_text, 1);
    s = keep_group(&s, &p.url, 1);
    s = blank(&s, &p.macro_drop);
    s = blank(&s, &p.inline_anchor);
    blank(&s, &p.inline_role)
}

/// Blank table cell separators and per-cell specs (`2+|`, `a|`).
fn mask_table_pipes(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = chars.clone();
    for (i, &c) in chars.iter().enumerate() {
        if c != '|' {
            continue;
        }
        out[i] = ' ';
        let mut j = i;
        while j > 0 && "0123456789.+*<^>adehlmsv".contains(chars[j - 1]) {
            j -= 1;
        }
        if j < i && (j == 0 || chars[j - 1].is_whitespace()) {
            out[j..i].fill(' ');
        }
    }
    out.into_iter().collect()
}

/// The closing delimiter for a verbatim/comment block opener, if `t` is one.
fn verbatim_opener(t: &str) -> Option<String> {
    let uniform = |c: char| t.len() >= 4 && t.chars().all(|x| x == c);
    if ['-', '.', '+', '/'].iter().any(|&c| uniform(c)) {
        return Some(t.to_string());
    }
    (t.starts_with("```") && !t[3..].contains(char::is_whitespace)).then(|| "```".to_string())
}

/// Extract Vale-ready prose from AsciiDoc `src`. See the module docs for
/// the line/column preservation guarantees.
pub fn extract_prose(src: &str) -> String {
    extract_prose_with_status(src).0
}

/// Like [`extract_prose`], also reporting whether the parser succeeded
/// (`false` means the scanner-only fallback produced the text).
pub fn extract_prose_with_status(src: &str) -> (String, bool) {
    let parsed = verbatim_ranges(src);
    let parser_ok = parsed.is_some();
    let ranges = parsed.unwrap_or_default();
    let p = &*P;
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<String> = None;
    let mut in_table = false;
    let mut attr_cont = false;
    let mut seen_content = false;
    let mut in_header = false;

    for (i, raw) in src.lines().enumerate() {
        let n = i + 1;
        let t = raw.trim_end();
        let blanked = || spaces(raw);

        if attr_cont {
            attr_cont = t.ends_with('\\');
            out.push(blanked());
            continue;
        }
        if let Some(f) = &fence {
            if t == f {
                fence = None;
            }
            out.push(blanked());
            continue;
        }
        if ranges.iter().any(|&(a, b)| a <= n && n <= b) {
            out.push(blanked());
            continue;
        }
        if t.is_empty() {
            in_header = false;
            out.push(String::new());
            continue;
        }
        if let Some(f) = verbatim_opener(t) {
            fence = Some(f);
            out.push(blanked());
            continue;
        }
        if t.starts_with("//") {
            out.push(blanked());
            continue;
        }
        if p.attr_entry.is_match(t) {
            attr_cont = t.ends_with('\\');
            out.push(blanked());
            continue;
        }
        // Document header: author and revision lines follow the `= Title`.
        if in_header {
            out.push(blanked());
            continue;
        }
        if !seen_content {
            seen_content = true;
            in_header = t.starts_with("= ");
        }
        if p.table_fence.is_match(t) {
            in_table = !in_table;
            out.push(blanked());
            continue;
        }
        if p.block_macro.is_match(t)
            || p.block_anchor.is_match(t)
            || p.block_attrs.is_match(t)
            || p.delimiter.is_match(t)
        {
            out.push(blanked());
            continue;
        }

        let mut s = raw.trim_end().to_string();
        if in_table {
            s = mask_table_pipes(&s);
        }
        // Block title `.Title`: drop the dot only.
        if s.starts_with('.')
            && s.chars()
                .nth(1)
                .is_some_and(|c| !c.is_whitespace() && c != '.')
        {
            s.replace_range(..1, " ");
        }
        s = blank(&s, &p.callout);
        if let Some(c) = p.marker.captures(&s) {
            let len = c.get(0).map_or(0, |m| m.end());
            let pad = spaces(&s[..len]);
            s.replace_range(..len, &pad);
        }
        if let Some(c) = p.admonition.captures(&s) {
            let len = c.get(0).map_or(0, |m| m.end());
            let pad = spaces(&s[..len]);
            s.replace_range(..len, &pad);
        }
        s = p
            .dlist
            .replace_all(&s, |c: &Captures<'_>| {
                format!("{}{}{}", &c[1], spaces(&c[2]), &c[3])
            })
            .into_owned();
        if s.ends_with(" +") {
            s.replace_range(s.len() - 1.., " ");
        }
        out.push(mask_inline(&s));
    }
    // `lines()` drops a trailing empty line; keep line count identical.
    let mut text = out.join("\n");
    if src.ends_with('\n') {
        text.push('\n');
    }
    (text, parser_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
= Loan Guide
Jane Doe <jane@example.org>
v1.0, 2026-01-01
:toc: left
:foo: bar

// a comment
Intro paragraph with `code_ident` and +lit+ and <<anchor>>.
See <<sec-two,the second section>> and xref:other.adoc#x[Other Page].

[[anchors]]
== Section One

* item one
** nested item with {foo} ref
. numbered item
continued line

[source,java]
----
class Foo { String Blorptastic; }
----

include::other.adoc[]

NOTE: Be careful here.

|===
|Head a |Head b
2+|cell spanning
a|cell a |cell b
|===

.Block title
....
literal Blorptastic
....

term:: definition text
visit https://example.org/path[the site] now
";

    fn lines(s: &str) -> Vec<&str> {
        s.lines().collect()
    }

    #[test]
    fn preserves_line_count_and_columns() {
        let out = extract_prose(SAMPLE);
        assert_eq!(out.lines().count(), SAMPLE.lines().count());
        assert!(out.ends_with('\n'));
        // Every output char sits at the same column as in the source or is a space.
        for (a, b) in SAMPLE.lines().zip(out.lines()) {
            for (x, y) in a.chars().zip(b.chars()) {
                assert!(x == y || y == ' ', "{a:?} vs {b:?}");
            }
            assert!(b.chars().count() <= a.chars().count(), "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn keeps_prose_and_drops_the_rest() {
        let out = extract_prose(SAMPLE);
        let l = lines(&out);
        assert!(l[1].trim().is_empty(), "author line");
        assert!(l[2].trim().is_empty(), "revision line");
        assert!(l[3].trim().is_empty() && l[4].trim().is_empty(), "attrs");
        assert!(l[6].trim().is_empty(), "comment");
        assert!(l[7].contains("Intro paragraph with"));
        assert!(!l[7].contains("code_ident") && !l[7].contains("lit") && !l[7].contains("anchor"));
        assert!(l[8].contains("the second section") && l[8].contains("Other Page"));
        assert!(!l[8].contains("sec-two") && !l[8].contains("other.adoc"));
        assert!(l[10].trim().is_empty(), "anchor line");
        assert_eq!(l[11].trim(), "Section One");
        assert_eq!(l[13].trim(), "item one");
        assert!(l[14].trim().starts_with("nested item with") && l[14].contains("ref"));
        assert_eq!(l[15].trim(), "numbered item");
        assert_eq!(l[16].trim(), "continued line");
        for n in [17, 18, 19, 20, 21] {
            assert!(l[n].trim().is_empty(), "source block line {}", n + 1);
        }
        assert!(l[23].trim().is_empty(), "include");
        assert_eq!(l[25].trim(), "Be careful here.");
        assert_eq!(l[28].trim(), "Head a  Head b");
        assert_eq!(l[29].trim(), "cell spanning");
        assert_eq!(l[30].trim(), "cell a  cell b");
        // the parser counts a block title as part of its (literal) block
        assert!(l[33].trim().is_empty());
        assert!(l[34].trim().is_empty() && l[35].trim().is_empty());
        assert_eq!(l[38].trim(), "term   definition text");
        assert!(l[39].contains("the site") && !l[39].contains("example.org"));
        assert!(!out.contains("Blorptastic"));
    }

    #[test]
    fn indented_literal_and_bare_source_paragraphs_come_from_the_parser() {
        let src = "Prose line.\n\n  indented Blorptastic literal\n\n[source]\nlet Blorptastic = 1;\n\nBack to prose.\n";
        let out = extract_prose(src);
        assert!(!out.contains("Blorptastic"), "{out}");
        assert!(out.contains("Prose line.") && out.contains("Back to prose."));
        assert_eq!(out.lines().count(), src.lines().count());
    }

    #[test]
    fn unterminated_block_and_garbage_do_not_panic() {
        for src in [
            "----\nnever closed\n",
            "|===\n|a\n",
            "\u{0}\u{1}[[[<<<",
            "",
            "\n\n",
        ] {
            let out = extract_prose(src);
            assert_eq!(out.lines().count(), src.lines().count());
        }
    }

    /// Corpus smoke test: `ADOC_CORPUS=<dir> cargo test --lib adoc_corpus -- --ignored --nocapture`
    /// runs the extractor over every `.adoc` below `<dir>` and fails on a
    /// parser fallback or a line-count mismatch.
    #[test]
    #[ignore = "needs ADOC_CORPUS pointing at a directory of .adoc files"]
    fn adoc_corpus_extracts_without_fallback() {
        let Some(dir) = std::env::var_os("ADOC_CORPUS") else {
            return;
        };
        let mut stack = vec![std::path::PathBuf::from(dir)];
        let (mut files, mut bad) = (0, Vec::new());
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "adoc") {
                    let src = std::fs::read_to_string(&p).unwrap();
                    let (out, ok) = extract_prose_with_status(&src);
                    files += 1;
                    if !ok || out.lines().count() != src.lines().count() {
                        bad.push(p.display().to_string());
                    }
                }
            }
        }
        eprintln!("{files} .adoc files, {} failures", bad.len());
        assert!(bad.is_empty(), "{bad:#?}");
    }

    #[test]
    fn crlf_input_keeps_line_count() {
        let src = "= T\r\n\r\nSome prose.\r\n----\r\ncode\r\n----\r\nMore.\r\n";
        let out = extract_prose(src);
        assert_eq!(out.lines().count(), src.lines().count());
        assert!(out.contains("Some prose.") && out.contains("More.") && !out.contains("code"));
    }
}
