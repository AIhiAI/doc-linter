//! Javadoc (`/** … */`) extractor for Java — the [[entity-doc-graph]]
//! code-comment lint's twin of [`crate::code_comments_slash`] for a
//! language whose doc comments are block comments directly above the
//! declaration. A line scanner covers that shape, so no grammar dependency.
//!
//! Markup that names code, not prose, is rewritten to backtick spans so
//! the vocab-closure lint skips it like any other inline code:
//!
//!   - `{@link X}`, `{@linkplain X label}`, `{@code x}`, `{@value X}`
//!     become `` `X` ``; `{@inheritDoc}` is dropped.
//!   - Block tags that name code (`@param x`, `@throws X`, `@exception X`,
//!     `@see X`) keep their prose with the name as `` `x` ``; the
//!     metadata tags (`@author`, `@since`, `@version`, `@serial…`) are
//!     blanked.
//!   - `<pre>` blocks are blanked; every other HTML tag is dropped.
//!
//! `is_public` is `true` for a `public` / `protected` declaration, and for
//! any non-`private` member of a file whose type is an interface (members
//! there are implicitly public).

use std::sync::OnceLock;

use regex::Regex;

use crate::code_comments::{CommentExtraction, DocComment};
use crate::code_comments_slash::decl_name;

/// Scans `content` for `/** … */` blocks and attaches each to the
/// declaration that follows it (skipping blank lines, `//` and `/* */`
/// comments, and annotations — including multi-line `@Foo(…)` ones).
pub(crate) fn extract(content: &str) -> CommentExtraction {
    let content = blank_text_blocks(content);
    let lines: Vec<&str> = content.lines().collect();
    let in_interface = is_interface_file(&content);
    let mut doc_comments = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(col) = doc_open(lines[i]) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut body: Vec<&str> = Vec::new();
        // First line: text after `/**`, possibly closed on the same line.
        let first = &lines[i][col - 1 + 3..];
        if let Some(end) = first.find("*/") {
            body.push(strip_star(&first[..end]));
            i += 1;
        } else {
            body.push(strip_star(first));
            i += 1;
            while let Some(l) = lines.get(i) {
                i += 1;
                if let Some(end) = l.find("*/") {
                    body.push(strip_star(&l[..end]));
                    break;
                }
                body.push(strip_star(l));
            }
        }
        let decl = next_decl(&lines, i);
        let text = normalise(&body.join("\n"));
        // A block ahead of `import` documents nothing, and one ahead of
        // `package` is the file's license header unless it is a
        // `package-info.java` package doc.
        if decl.is_some_and(|d| {
            d.starts_with("import ") || (d.starts_with("package ") && is_license(&text))
        }) {
            continue;
        }
        let name = decl.and_then(decl_name);
        let is_public = decl.is_some_and(|d| {
            let words: Vec<&str> = d.split(|c: char| !c.is_alphanumeric()).collect();
            words.iter().any(|w| *w == "public" || *w == "protected")
                || (in_interface && !words.contains(&"private"))
        });
        doc_comments.push(DocComment {
            text,
            line: start + 1,
            col,
            attached_to: name,
            is_public,
        });
    }
    CommentExtraction { doc_comments }
}

/// `Some(1-based column)` when the line opens a Javadoc block. `/**/` is an
/// empty ordinary comment and `/***` a banner, not Javadoc.
fn doc_open(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("/**")?;
    if rest.starts_with('/') || rest.starts_with('*') {
        return None;
    }
    Some(line.len() - trimmed.len() + 1)
}

/// Drops the conventional leading `*` (and one following space) of a
/// Javadoc body line.
fn strip_star(line: &str) -> &str {
    let t = line.trim_start();
    let t = t
        .strip_prefix('*')
        .map_or(t, |r| r.strip_prefix(' ').unwrap_or(r));
    t.trim_end()
}

/// The first declaration line at or after `from`, skipping blank lines,
/// comments and annotations. A multi-line annotation is skipped by
/// balancing its parentheses (string literals are skipped so a `)` in
/// `@Operation(summary = "List (all)")` does not end it early).
fn next_decl<'a>(lines: &[&'a str], from: usize) -> Option<&'a str> {
    let mut i = from;
    let mut depth: i32 = 0;
    let mut in_block_comment = false;
    while let Some(raw) = lines.get(i) {
        i += 1;
        let l = raw.trim();
        if in_block_comment {
            in_block_comment = !l.contains("*/");
            continue;
        }
        if depth > 0 {
            depth += paren_delta(l);
            continue;
        }
        if l.is_empty() || l.starts_with("//") {
            continue;
        }
        if l.starts_with("/*") {
            in_block_comment = !l.contains("*/");
            continue;
        }
        if l.starts_with('@') && !l.starts_with("@interface") {
            let delta = paren_delta(l);
            // `@GET public Response x()` — annotation and declaration on one
            // line: the declaration is what follows the balanced annotations.
            if delta == 0 {
                if let Some(rest) = strip_leading_annotations(l) {
                    return Some(rest);
                }
            }
            depth = delta;
            continue;
        }
        return Some(l);
    }
    None
}

/// Replaces the body of every Java text block (`"""…"""`) with its
/// newlines only, so line scanners that balance parentheses per line
/// don't read a multi-line `@Operation(description = """…""")` as open
/// forever. Line numbers are preserved.
pub(crate) fn blank_text_blocks(content: &str) -> std::borrow::Cow<'_, str> {
    if !content.contains("\"\"\"") {
        return std::borrow::Cow::Borrowed(content);
    }
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(open) = rest.find("\"\"\"") {
        out.push_str(&rest[..open]);
        let body = &rest[open + 3..];
        let Some(close) = body.find("\"\"\"") else {
            out.push_str(&rest[open..]);
            return std::borrow::Cow::Owned(out);
        };
        out.push_str("\"\"");
        out.push_str(&"\n".repeat(body[..close].matches('\n').count()));
        rest = &body[close + 3..];
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// Net `(` minus `)` outside string and char literals.
pub(crate) fn paren_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in line.chars() {
        if let Some(q) = quote {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => delta += 1,
            ')' => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// `Some(declaration)` when a balanced annotation line continues with a
/// declaration (`@Override public void run()` → `public void run()`);
/// `None` when the line is only annotations.
#[allow(clippy::expect_used, reason = "literal regex; compile is infallible")]
fn strip_leading_annotations(line: &str) -> Option<&str> {
    static ANN: OnceLock<Regex> = OnceLock::new();
    let ann = ANN.get_or_init(|| {
        Regex::new(r"^@[A-Za-z_][\w.]*(?:\([^()]*(?:\([^()]*\)[^()]*)*\))?\s*").expect("annotation")
    });
    let mut rest = line;
    while let Some(m) = ann.find(rest) {
        if m.end() == 0 {
            break;
        }
        rest = &rest[m.end()..];
    }
    let rest = rest.trim();
    (!rest.is_empty()).then_some(rest)
}

/// License / copyright boilerplate rather than documentation.
#[allow(clippy::expect_used, reason = "literal regex; compile is infallible")]
fn is_license(text: &str) -> bool {
    static LICENSE: OnceLock<Regex> = OnceLock::new();
    LICENSE
        .get_or_init(|| Regex::new(r"(?i)\blicen[sc]ed?\b|\bcopyright\b").expect("license"))
        .is_match(text)
}

/// True when the file's first type declaration is an interface (or an
/// annotation type), whose members are implicitly public.
#[allow(clippy::expect_used, reason = "literal regex; compile is infallible")]
fn is_interface_file(content: &str) -> bool {
    static TYPE: OnceLock<Regex> = OnceLock::new();
    let re = TYPE.get_or_init(|| {
        Regex::new(r"(?m)^\s*(?:(?:public|protected|private|abstract|static|final|sealed|non-sealed|strictfp)\s+)*(class|interface|@interface|enum|record)\s+[A-Za-z_]")
            .expect("type regex")
    });
    re.captures(content)
        .and_then(|c| c.get(1))
        .is_some_and(|m| m.as_str().ends_with("interface"))
}

/// Rewrites code-naming Javadoc markup to backtick spans and strips the
/// rest. Line count is preserved so diagnostics keep their line numbers.
#[allow(clippy::expect_used, reason = "literal regexes; compile is infallible")]
fn normalise(text: &str) -> String {
    static RULES: OnceLock<[(Regex, &'static str); 9]> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            // `<pre>` blocks: blank the content, keep the newlines.
            (Regex::new(r"(?is)<pre>.*?</pre>").expect("pre"), ""),
            (Regex::new(r"\{@inheritDoc\s*\}").expect("inheritDoc"), ""),
            // `{@link Foo#bar(int) label}` → `Foo#bar(int)`; the label is
            // prose about the same symbol and is dropped with it.
            (
                Regex::new(r"\{@(?:link|linkplain|value)\s+([^\s{}]+)[^{}]*\}").expect("link"),
                "`$1`",
            ),
            (
                Regex::new(r"\{@(?:code|literal)\s+((?:[^{}]|\{[^{}]*\})*)\}").expect("code"),
                "`$1`",
            ),
            (
                Regex::new(r"@(?:param|throws|exception|see)[ \t]+([^\s]+)").expect("block tag"),
                "`$1`",
            ),
            (
                Regex::new(
                    r"(?m)^[ \t]*@(?:author|since|version|serial\w*|deprecated|hidden)\b.*$",
                )
                .expect("metadata tag"),
                "",
            ),
            // IntelliJ's file template: `Created by Jane on 12/01/17.`
            (
                Regex::new(r"(?m)^.*\bCreated by\b.*\bon\s+\d.*$").expect("created by"),
                "",
            ),
            (Regex::new(r"@return\b").expect("return"), ""),
            (Regex::new(r"</?[A-Za-z][^>]*>").expect("tag"), ""),
        ]
    });
    let mut out = text.to_string();
    for (i, (re, rep)) in rules.iter().enumerate() {
        out = if i == 0 {
            re.replace_all(&out, |c: &regex::Captures<'_>| {
                "\n".repeat(c[0].matches('\n').count())
            })
            .into_owned()
        } else {
            re.replace_all(&out, *rep).into_owned()
        };
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&#64;", "@")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn attaches_past_multiline_annotations_and_rewrites_javadoc() {
        let src = [
            "package org.apache.fineract;",
            "",
            "public class LoansApiResource {",
            "",
            "    /**",
            "     * Lists every {@link Loan} for the {@code clientId} (see {@link LoanReadService#all() all}).",
            "     *",
            "     * @param clientId the owning client",
            "     * @return the loans",
            "     * @throws LoanNotFoundException when missing",
            "     * @since 1.2",
            "     */",
            "    @GET",
            "    @Operation(summary = \"List (all) loans\",",
            "            description = \"Example: ) paren in string\")",
            "    @Produces({ MediaType.APPLICATION_JSON })",
            "    public String retrieveAll(@Context final UriInfo uriInfo,",
            "            @QueryParam(\"clientId\") final Long clientId) {",
            "        return null;",
            "    }",
            "",
            "    /** Internal cache. */",
            "    private final Map<Long, Loan> cache = new HashMap<>();",
            "}",
        ]
        .join("\n");
        let c = extract(&src).doc_comments;
        assert_eq!(c.len(), 2);
        assert_eq!((c[0].line, c[0].col), (5, 5));
        assert_eq!(c[0].attached_to.as_deref(), Some("retrieveAll"));
        assert!(c[0].is_public);
        let lines: Vec<&str> = c[0].text.split('\n').collect();
        assert_eq!(
            lines,
            [
                "",
                "Lists every `Loan` for the `clientId` (see `LoanReadService#all()`).",
                "",
                "`clientId` the owning client",
                " the loans",
                "`LoanNotFoundException` when missing",
                "",
                "",
            ]
        );
        assert_eq!(c[1].text, "Internal cache.");
        assert_eq!(c[1].attached_to.as_deref(), Some("cache"));
        assert!(!c[1].is_public);
    }

    #[test]
    fn text_block_annotation_does_not_swallow_the_declaration() {
        let src = [
            "public class A {",
            "    /** Lists. */",
            "    @Operation(summary = \"x\", description = \"\"\"",
            "            Lists the application's tables (optional",
            "            \"\"\")",
            "    public String list() { return null; }",
            "}",
        ]
        .join("\n");
        let c = extract(&src).doc_comments;
        assert_eq!(c[0].attached_to.as_deref(), Some("list"));
        assert_eq!(blank_text_blocks(&src).lines().count(), src.lines().count());
    }

    #[test]
    fn class_level_and_single_line_annotation_with_decl() {
        let src = [
            "/**",
            " * Loan product <b>definition</b>.",
            " * <pre>",
            " *   LoanProduct p = new LoanProduct();",
            " * </pre>",
            " */",
            "@Entity",
            "@Table(name = \"m_product_loan\")",
            "public class LoanProduct extends AbstractPersistableCustom<Long> {",
            "  /** Runs it. */",
            "  @Override public void run() {}",
            "}",
        ]
        .join("\n");
        let c = extract(&src).doc_comments;
        assert_eq!(c[0].attached_to.as_deref(), Some("LoanProduct"));
        assert_eq!(c[0].text, "\nLoan product definition.\n\n\n\n");
        assert_eq!(c[1].attached_to.as_deref(), Some("run"));
        assert!(c[1].is_public);
    }

    #[test]
    fn license_header_is_not_javadoc_but_package_doc_is() {
        let src = [
            "/**",
            " * Licensed to the Apache Software Foundation (ASF) under one",
            " */",
            "/** Loan portfolio domain. */",
            "package org.apache.fineract.portfolio.loan;",
        ]
        .join("\n");
        let c = extract(&src).doc_comments;
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].text, "Loan portfolio domain.");
        let created = extract("/**\n * Created by Chirag Gupta on 12/01/17.\n */\nclass A {}\n");
        assert_eq!(created.doc_comments[0].text, "\n\n");
        let header_then_import =
            "/**\n * Licensed under X.\n */\npackage a;\n\n/** Stray. */\nimport b.C;\n";
        assert!(extract(header_then_import).doc_comments.is_empty());
    }

    #[test]
    fn interface_members_are_public_and_banners_are_not_javadoc() {
        let src = [
            "/***********************",
            " * License banner",
            " ***********************/",
            "public interface LoanReadPlatformService {",
            "    /** Finds one. */",
            "    LoanData retrieveOne(Long loanId);",
            "    /**/",
            "    void noop();",
            "}",
        ]
        .join("\n");
        let c = extract(&src).doc_comments;
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].attached_to.as_deref(), Some("retrieveOne"));
        assert!(c[0].is_public);
    }
}
