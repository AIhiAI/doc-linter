//! Python source-comment extractor — Python analogue of
//! [`crate::code_comments`] (Rust) and [`crate::code_comments_ts`]
//! (TypeScript / TSX).
//!
//! Parses a `.py` file with `tree-sitter` + `tree-sitter-python` and
//! pulls every PEP-257 docstring (`"""..."""` or `'''...'''` string
//! literal sitting as the first statement of a module / function /
//! class / async-function body) out of the syntax tree, attaching each
//! one to the symbol it documents. The resulting [`CommentExtraction`]
//! is fed through the same `TermIndex` / vocab-closure pipeline that
//! the markdown post-processor, Rust per-file lint, and TypeScript
//! per-file lint use — so prose written in Python docstrings is held
//! to the same vocab-closure rules as prose in `.md`, `.rs`, and `.ts`
//! files.
//!
//! ## What counts as a docstring
//!
//! Python doesn't distinguish "doc comments" from regular comments at
//! the grammar level — by convention, a bare string literal that
//! appears as the first statement of a body is the docstring (PEP 257).
//! That's exactly what we extract:
//!
//!   - Top-of-module string literal → attaches to `<module>`, matching
//!     the Rust `//!` and TS top-of-file `/** */` semantics.
//!   - First statement of a `function_definition` / `class_definition`
//!     body that is a string literal → attaches to the def's name.
//!   - `async def` is parsed as `function_definition` by the python
//!     grammar (the `async` keyword is a child token, not a separate
//!     node kind); decorated defs are wrapped in `decorated_definition`
//!     and we unwrap to the inner `definition` field.
//!   - `#` line comments are NOT docstrings — they're skipped, same as
//!     `//` line comments in TS.
//!   - Bare string literals that aren't the first statement of a body
//!     (e.g. a `"hello"` expression in the middle of a function) are
//!     NOT extracted — they're regular string literals, not docs.

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::code_comments::{CommentExtraction, DocComment};

/// Extract docstrings from in-memory Python source for the
/// [[entity-doc-graph]] vocab-closure check. Test entry point.
pub fn extract_doc_comments_py_from_str(content: &str) -> Result<CommentExtraction> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_python::language())
        .context("load tree-sitter-python grammar")?;
    let tree = parser
        .parse(content, None)
        .context("tree-sitter parse returned None — ungrammatical source")?;
    let bytes = content.as_bytes();

    let root = tree.root_node();
    let mut out: Vec<DocComment> = Vec::new();

    if let Some(s) = first_docstring_in_body(root) {
        // The module itself is the public surface; its docstring is
        // always public regardless of file name.
        push_docstring(s, bytes, Some("<module>".to_string()), true, &mut out);
    }
    // Walk top-level definitions with both visibility contexts set to
    // "public so far": each def's own name determines whether
    // anything inside it stays public.
    walk_definitions(
        root, bytes, &mut out, /*inside_function=*/ false, /*ancestor_public=*/ true,
    );

    Ok(CommentExtraction { doc_comments: out })
}

/// Recurse through `node`'s subtree collecting docstrings attached to
/// every `function_definition` / `class_definition` we encounter,
/// including nested defs and decorated wrappers. `inside_function` is
/// true when this node's lexical parent is a function body — anything
/// defined there is local-scope and therefore non-public regardless
/// of name. `ancestor_public` is false when any enclosing class or
/// function is private; it forces every descendant private as well.
fn walk_definitions(
    node: Node<'_>,
    bytes: &[u8],
    out: &mut Vec<DocComment>,
    inside_function: bool,
    ancestor_public: bool,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_definition" | "class_definition" => {
                let is_function = child.kind() == "function_definition";
                let own_public = own_name_is_public(child, bytes);
                let effective_public = ancestor_public && !inside_function && own_public;
                emit_def_docstring(child, bytes, out, effective_public);
                if let Some(body) = child.child_by_field_name("body") {
                    walk_definitions(
                        body,
                        bytes,
                        out,
                        /*inside_function=*/ is_function || inside_function,
                        /*ancestor_public=*/ effective_public,
                    );
                }
            }
            "decorated_definition" => {
                // tree-sitter-python always puts the wrapped function /
                // class node under the `definition` field. If absent
                // (which would be a grammar bug), we skip — emitting
                // nothing is safer than guessing.
                if let Some(def) = child.child_by_field_name("definition") {
                    if matches!(def.kind(), "function_definition" | "class_definition") {
                        let is_function = def.kind() == "function_definition";
                        let own_public = own_name_is_public(def, bytes);
                        let effective_public = ancestor_public && !inside_function && own_public;
                        emit_def_docstring(def, bytes, out, effective_public);
                        if let Some(body) = def.child_by_field_name("body") {
                            walk_definitions(
                                body,
                                bytes,
                                out,
                                /*inside_function=*/ is_function || inside_function,
                                /*ancestor_public=*/ effective_public,
                            );
                        }
                    }
                }
            }
            _ => {
                if child.child_count() > 0 {
                    walk_definitions(child, bytes, out, inside_function, ancestor_public);
                }
            }
        }
    }
}

/// Find the docstring (if any) of a function / class def and push it
/// onto `out` attached to the def's name and visibility.
fn emit_def_docstring(def: Node<'_>, bytes: &[u8], out: &mut Vec<DocComment>, is_public: bool) {
    let name = def
        .child_by_field_name("name")
        .and_then(|n| slice(bytes, n).map(str::to_string));
    let Some(body) = def.child_by_field_name("body") else {
        return;
    };
    if let Some(s) = first_docstring_in_body(body) {
        push_docstring(s, bytes, name, is_public, out);
    }
}

/// True when a function / class `def` node's OWN name (ignoring any
/// enclosing context) marks it as part of the public API surface
/// under PEP-8 conventions. Anonymous / un-named defs are treated
/// conservatively as public — the anchor check will key on
/// `attached_to.is_none()` separately.
fn own_name_is_public(def: Node<'_>, bytes: &[u8]) -> bool {
    def.child_by_field_name("name")
        .and_then(|n| slice(bytes, n))
        .is_none_or(is_python_public)
}

/// PEP-8 public-symbol check for a Python identifier:
///   - No leading underscore → public.
///   - Dunder (`__name__` shape: starts and ends with `__`, non-empty
///     middle) → public protocol method (`__init__`, `__call__`,
///     `__enter__`, etc.).
///   - Anything else with leading underscore → private.
pub fn is_python_public(name: &str) -> bool {
    if !name.starts_with('_') {
        return true;
    }
    if name.starts_with("__") && name.ends_with("__") && name.len() > 4 {
        return true;
    }
    false
}

/// Returns the `string` node of the first statement of `body` when it
/// is a bare-string `expression_statement` — Python's docstring shape
/// at the module / class / function level. Returns `None` for empty
/// bodies, bodies whose first statement isn't a string expression, or
/// `# ...` line comments.
fn first_docstring_in_body(body: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = body.walk();
    let first = body.named_children(&mut cursor).next()?;
    if first.kind() != "expression_statement" {
        return None;
    }
    let mut inner = first.walk();
    let str_node = first.named_children(&mut inner).next()?;
    if str_node.kind() == "string" {
        Some(str_node)
    } else {
        None
    }
}

fn push_docstring(
    string_node: Node<'_>,
    bytes: &[u8],
    attached_to: Option<String>,
    is_public: bool,
    out: &mut Vec<DocComment>,
) {
    let raw = slice(bytes, string_node).unwrap_or("");
    let text = strip_string_markers(raw);
    out.push(DocComment {
        text,
        line: node_line(string_node),
        col: node_col(string_node),
        attached_to,
        is_public,
    });
}

/// Strip the surrounding quote markers (`"""`, `'''`, `"`, `'`) and
/// optional string prefix (`r` / `R` / `b` / `B` / `u` / `U` / `f` /
/// `F`, or any 2-char combination) from a Python string literal's
/// source text, returning the docstring body for the
/// [[entity-doc-graph]] vocab-closure check. Per-line whitespace is
/// preserved (so line-level diagnostics retain offsets); only the
/// outer whitespace around the whole body is trimmed.
fn strip_string_markers(raw: &str) -> String {
    let body = raw.trim_start_matches(['r', 'R', 'b', 'B', 'u', 'U', 'f', 'F']);
    let (open, close): (&str, &str) = if body.starts_with("\"\"\"") {
        ("\"\"\"", "\"\"\"")
    } else if body.starts_with("'''") {
        ("'''", "'''")
    } else if body.starts_with('"') {
        ("\"", "\"")
    } else if body.starts_with('\'') {
        ("'", "'")
    } else {
        return body.trim_start_matches([' ', '\t']).trim_end().to_string();
    };
    let body = body.strip_prefix(open).unwrap_or(body);
    let body = body.strip_suffix(close).unwrap_or(body);
    body.trim_start_matches([' ', '\t']).trim_end().to_string()
}

fn slice<'a>(bytes: &'a [u8], node: Node<'_>) -> Option<&'a str> {
    let range = node.byte_range();
    bytes.get(range).and_then(|b| std::str::from_utf8(b).ok())
}

fn node_line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

fn node_col(node: Node<'_>) -> usize {
    node.start_position().column + 1
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    fn extract(src: &str) -> CommentExtraction {
        extract_doc_comments_py_from_str(src).expect("parse should succeed")
    }

    #[test]
    fn extracts_module_docstring() {
        let src = "\"\"\"module doc\"\"\"\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "module doc");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("<module>"));
    }

    #[test]
    fn extracts_function_docstring() {
        let src = "def foo():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "doc");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("foo"));
    }

    #[test]
    fn extracts_class_docstring() {
        let src = "class Foo:\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("Foo"));
    }

    #[test]
    fn extracts_async_function_docstring() {
        let src = "async def foo():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("foo"));
    }

    #[test]
    fn extracts_decorated_function_docstring() {
        let src = "@decorator\ndef foo():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "doc");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("foo"));
    }

    #[test]
    fn extracts_method_docstring_nested_in_class() {
        let src = "class Foo:\n    \"\"\"class doc\"\"\"\n    \
                   def bar(self):\n        \"\"\"method doc\"\"\"\n        pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 2, "{:?}", r.doc_comments);
        let by_attach: std::collections::HashMap<_, _> = r
            .doc_comments
            .iter()
            .map(|c| (c.attached_to.clone().unwrap_or_default(), c.text.clone()))
            .collect();
        assert_eq!(by_attach.get("Foo").map(String::as_str), Some("class doc"));
        assert_eq!(by_attach.get("bar").map(String::as_str), Some("method doc"));
    }

    #[test]
    fn skips_hash_line_comments() {
        let src = "# not a docstring\ndef foo():\n    return 1\n";
        let r = extract(src);
        assert!(
            r.doc_comments.is_empty(),
            "expected no docstrings, got {:?}",
            r.doc_comments
        );
    }

    #[test]
    fn skips_non_first_statement_string() {
        let src = "def foo():\n    x = 1\n    \"not a docstring\"\n    return x\n";
        let r = extract(src);
        assert!(
            r.doc_comments.is_empty(),
            "expected no docstrings, got {:?}",
            r.doc_comments
        );
    }

    #[test]
    fn handles_single_quoted_docstring() {
        let src = "def foo():\n    'doc'\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "doc");
    }

    #[test]
    fn handles_raw_string_prefix() {
        let src = "def foo():\n    r\"\"\"raw doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "raw doc");
    }

    /// `is_python_public` recognises the PEP-8 public-symbol shape:
    /// plain names public, leading-underscore private, dunders kept
    /// public (Python protocol hooks).
    #[test]
    fn is_python_public_matches_pep8_conventions() {
        assert!(is_python_public("foo"));
        assert!(is_python_public("FooBar"));
        assert!(is_python_public("foo_bar"));
        assert!(!is_python_public("_foo"));
        assert!(!is_python_public("_private_helper"));
        assert!(!is_python_public("__name_mangled"));
        assert!(is_python_public("__init__"));
        assert!(is_python_public("__call__"));
        assert!(is_python_public("__enter__"));
    }

    /// Module-level public function: docstring's `is_public` is true.
    #[test]
    fn public_top_level_function_is_public() {
        let src = "def foo():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert!(r.doc_comments[0].is_public);
    }

    /// Module-level `_private` function: docstring's `is_public` is false.
    #[test]
    fn leading_underscore_function_is_private() {
        let src = "def _helper():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert!(!r.doc_comments[0].is_public);
    }

    /// Dunder methods inside a class are treated as public — they're
    /// the Python public protocol surface (`__init__` is how callers
    /// construct an instance).
    #[test]
    fn dunder_method_in_public_class_is_public() {
        let src = "class Foo:\n    \"\"\"class doc\"\"\"\n    \
                   def __init__(self):\n        \"\"\"init doc\"\"\"\n        pass\n";
        let r = extract(src);
        let by_attach: std::collections::HashMap<_, _> = r
            .doc_comments
            .iter()
            .map(|c| (c.attached_to.clone().unwrap_or_default(), c.is_public))
            .collect();
        assert_eq!(by_attach.get("Foo"), Some(&true));
        assert_eq!(by_attach.get("__init__"), Some(&true));
    }

    /// Method on a `_PrivateClass` is non-public even if its own name
    /// has no leading underscore — the enclosing class is private,
    /// so its surface is too.
    #[test]
    fn method_on_private_class_is_private() {
        let src = "class _Internal:\n    \"\"\"class doc\"\"\"\n    \
                   def looks_public(self):\n        \"\"\"method doc\"\"\"\n        pass\n";
        let r = extract(src);
        let by_attach: std::collections::HashMap<_, _> = r
            .doc_comments
            .iter()
            .map(|c| (c.attached_to.clone().unwrap_or_default(), c.is_public))
            .collect();
        assert_eq!(by_attach.get("_Internal"), Some(&false));
        assert_eq!(by_attach.get("looks_public"), Some(&false));
    }

    /// Helper nested inside a public function body is local-scope and
    /// therefore non-public, even with a public-shaped name.
    #[test]
    fn helper_nested_in_function_is_private() {
        let src = "def public_outer():\n    \"\"\"outer doc\"\"\"\n    \
                   def helper():\n        \"\"\"helper doc\"\"\"\n        pass\n    \
                   helper()\n";
        let r = extract(src);
        let by_attach: std::collections::HashMap<_, _> = r
            .doc_comments
            .iter()
            .map(|c| (c.attached_to.clone().unwrap_or_default(), c.is_public))
            .collect();
        assert_eq!(by_attach.get("public_outer"), Some(&true));
        assert_eq!(by_attach.get("helper"), Some(&false));
    }

    /// Decorated public function preserves public visibility through
    /// the `decorated_definition` unwrap.
    #[test]
    fn decorated_public_function_is_public() {
        let src = "@decorator\ndef foo():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert!(r.doc_comments[0].is_public);
    }

    /// Decorated `_private` function is private.
    #[test]
    fn decorated_private_function_is_private() {
        let src = "@decorator\ndef _private():\n    \"\"\"doc\"\"\"\n    pass\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert!(!r.doc_comments[0].is_public);
    }

    /// Module-level docstring is always public — the module itself is
    /// the public surface.
    #[test]
    fn module_docstring_is_public() {
        let src = "\"\"\"module doc\"\"\"\n";
        let r = extract(src);
        assert_eq!(r.doc_comments.len(), 1);
        assert!(r.doc_comments[0].is_public);
    }
}
