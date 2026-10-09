//! `///` doc-comment extractor for C# and Dart — the [[entity-doc-graph]]
//! code-comment lint's twin of [`crate::code_comments`] for languages
//! whose doc comments are always a run of `///` lines directly above the
//! declaration. A line scanner covers that shape, so no grammar dependency.
//!
//! Markup that names code, not prose, is rewritten to backtick spans so
//! the vocab-closure lint skips it like any other inline code:
//!
//!   - C# XML doc: `<see cref="X"/>`, `<paramref name="x"/>`, `<c>x</c>`
//!     become `` `X` ``; `<code>` blocks are blanked; every other tag
//!     (`<summary>`, `<param …>`, `<returns>`) is dropped.
//!   - Dart: `[Symbol]` references become `` `Symbol` ``.
//!
//! `is_public` follows each language's rule: C# needs a `public` or
//! `protected` modifier on the declaration, Dart a name without a leading
//! `_`.

use std::sync::OnceLock;

use regex::Regex;

use crate::code_comments::{CommentExtraction, DocComment, Lang};

/// Scans `content` for `///` runs and attaches each to the declaration
/// that follows it (skipping blank lines, `//` comments, C# `[Attribute]`
/// and Dart `@annotation` lines).
pub(crate) fn extract(content: &str, lang: Lang) -> CommentExtraction {
    let lines: Vec<&str> = content.lines().collect();
    let mut doc_comments = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some((col, _)) = doc_line(lines[i]) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut body: Vec<&str> = Vec::new();
        while let Some((_, text)) = lines.get(i).and_then(|l| doc_line(l)) {
            body.push(text);
            i += 1;
        }
        let decl = lines[i..]
            .iter()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with("//") && !is_annotation(l, lang));
        let name = decl.and_then(decl_name);
        let is_public = match lang {
            Lang::Dart => name.as_deref().is_some_and(|n| !n.starts_with('_')),
            // ponytail: interface members carry no modifier and read as
            // private here; track the enclosing type if that matters.
            _ => decl.is_some_and(|d| {
                d.split(|c: char| !c.is_alphanumeric())
                    .any(|w| w == "public" || w == "protected")
            }),
        };
        doc_comments.push(DocComment {
            text: normalise(&body.join("\n"), lang),
            line: start + 1,
            col,
            attached_to: name,
            is_public,
        });
    }
    CommentExtraction { doc_comments }
}

/// `Some((1-based column, text after the marker))` for a `///` line; `////`
/// dividers are not doc comments.
fn doc_line(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("///")?;
    if rest.starts_with('/') {
        return None;
    }
    let col = line.len() - trimmed.len() + 1;
    Some((col, rest.strip_prefix(' ').unwrap_or(rest).trim_end()))
}

fn is_annotation(line: &str, lang: Lang) -> bool {
    match lang {
        Lang::Dart => line.starts_with('@'),
        _ => line.starts_with('['),
    }
}

/// Best-effort declared name: the identifier after a type keyword, else
/// the last identifier before the first `(`, `{`, `=`, `;` or `:` (method,
/// constructor, property, field, enum member, Dart getter).
#[allow(clippy::expect_used, reason = "literal regexes; compile is infallible")]
pub(crate) fn decl_name(decl: &str) -> Option<String> {
    static TYPE_KW: OnceLock<Regex> = OnceLock::new();
    static LAST_IDENT: OnceLock<Regex> = OnceLock::new();
    let type_kw = TYPE_KW.get_or_init(|| {
        Regex::new(r"\b(?:class|interface|struct|enum|record|mixin|extension|typedef|namespace)\s+(?:class\s+|struct\s+)?([A-Za-z_]\w*)")
            .expect("type keyword regex")
    });
    if let Some(c) = type_kw.captures(decl) {
        return c.get(1).map(|m| m.as_str().to_string());
    }
    let head = decl.split(['(', '{', '=', ';', ':']).next().unwrap_or(decl);
    // Drop a trailing generic parameter list: `Map<K, V>` → `Map`.
    let head = match head.trim_end().strip_suffix('>') {
        Some(h) => h.rsplit_once('<').map_or(h, |(name, _)| name),
        None => head,
    };
    let last_ident =
        LAST_IDENT.get_or_init(|| Regex::new(r"([A-Za-z_]\w*)\s*$").expect("identifier regex"));
    last_ident
        .captures(head.trim_end())
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Rewrites code-naming markup to backtick spans and strips the rest.
/// Line count is preserved so diagnostics keep their line numbers.
#[allow(clippy::expect_used, reason = "literal regexes; compile is infallible")]
fn normalise(text: &str, lang: Lang) -> String {
    static CS: OnceLock<[(Regex, &'static str); 4]> = OnceLock::new();
    static DART_REF: OnceLock<Regex> = OnceLock::new();
    if lang == Lang::Dart {
        // `[Ref]` outside a code span only; inside backticks it is
        // already code (`` `[BarcodeFormat.qrCode]` ``).
        let re = DART_REF.get_or_init(|| {
            Regex::new(r"(`[^`]*`)|\[([A-Za-z_][\w.]*)\]").expect("dart ref regex")
        });
        return re
            .replace_all(text, |c: &regex::Captures<'_>| match c.get(2) {
                Some(r) => format!("`{}`", r.as_str()),
                None => c[0].to_string(),
            })
            .into_owned();
    }
    let cs = CS.get_or_init(|| {
        [
            // `<code>` blocks: blank the content, keep the newlines.
            (Regex::new(r"(?s)<code>.*?</code>").expect("code"), ""),
            (
                Regex::new(r#"<(?:see|seealso|paramref|typeparamref)\s+(?:cref|name|langword|href)="(?:[A-Z]:)?([^"]*)"\s*/?>(?:</\w+>)?"#)
                    .expect("see"),
                "`$1`",
            ),
            (Regex::new(r"<c>(.*?)</c>").expect("c"), "`$1`"),
            (Regex::new(r"</?[A-Za-z][^>]*>").expect("tag"), ""),
        ]
    });
    let mut out = text.to_string();
    for (i, (re, rep)) in cs.iter().enumerate() {
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, reason = "tests")]
mod tests {
    use super::*;

    #[test]
    fn csharp_attaches_past_attributes_and_rewrites_xml_doc() {
        let src = [
            "namespace Api {",
            "/// <summary>",
            "/// Loads the <see cref=\"T:Api.Outlet\"/> for <paramref name=\"id\"/>.",
            "/// </summary>",
            "/// <code>",
            "/// var X = Foo();",
            "/// </code>",
            "[HttpGet(\"{id}\")]",
            "public async Task<IActionResult> GetOutlet(long id) { }",
            "/// <summary>Internal helper.</summary>",
            "private int Helper() => 1;",
            "/// <summary>Id.</summary>",
            "public long Id { get; set; }",
            "}",
        ]
        .join("\n");
        let c = extract(&src, Lang::CSharp).doc_comments;
        assert_eq!(c.len(), 3);
        assert_eq!((c[0].line, c[0].col), (2, 1));
        assert_eq!(c[0].attached_to.as_deref(), Some("GetOutlet"));
        assert!(c[0].is_public);
        let lines: Vec<&str> = c[0].text.split('\n').collect();
        assert_eq!(
            lines,
            ["", "Loads the `Api.Outlet` for `id`.", "", "", "", ""]
        );
        assert_eq!(c[1].attached_to.as_deref(), Some("Helper"));
        assert!(!c[1].is_public);
        assert_eq!(c[2].attached_to.as_deref(), Some("Id"));
    }

    #[test]
    fn dart_names_and_privacy() {
        let src = [
            "/// A [Outlet] cache.",
            "@immutable",
            "class OutletCache extends Base {",
            "  /// Fetches one.",
            "  Future<Outlet> fetch(int id) async {}",
            "  /// Private.",
            "  void _reset() {}",
            "  /// Name.",
            "  String get name => _name;",
            "}",
        ]
        .join("\n");
        let c = extract(&src, Lang::Dart).doc_comments;
        let names: Vec<_> = c.iter().map(|d| d.attached_to.clone().unwrap()).collect();
        assert_eq!(names, ["OutletCache", "fetch", "_reset", "name"]);
        assert_eq!(c[0].text, "A `Outlet` cache.");
        let quoted = extract(
            "/// Scans `[BarcodeFormat.qrCode]` codes.\nclass S {}\n",
            Lang::Dart,
        );
        assert_eq!(
            quoted.doc_comments[0].text,
            "Scans `[BarcodeFormat.qrCode]` codes."
        );
        assert!(c[1].is_public && !c[2].is_public);
        assert_eq!(c[1].col, 3);
    }

    #[test]
    fn divider_lines_are_not_doc_comments() {
        assert!(extract("//// ----\nclass A {}\n", Lang::CSharp)
            .doc_comments
            .is_empty());
    }
}
