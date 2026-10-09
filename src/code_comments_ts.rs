//! TypeScript / TSX source-comment extractor (roadmap-45 phase 1
//! hook-slice — TS analogue of [`crate::code_comments`]).
//!
//! Parses a `.ts` / `.cts` / `.mts` / `.tsx` file with `tree-sitter` +
//! `tree-sitter-typescript` and pulls every JSDoc / TSDoc block doc
//! comment (`/** ... */`) out of the syntax tree, attaching each one
//! to the item it documents (function / class / interface /
//! type-alias / enum / method / arrow-function-bound `const`). The
//! resulting [`CommentExtraction`] is fed through the same
//! `TermIndex` / vocab-closure pipeline that the markdown post-
//! processor and Rust per-file lint use — so prose written in TS doc
//! comments is held to the same vocab-closure rules as prose in
//! `.md` and `.rs` files.
//!
//! ## What counts as a doc comment
//!
//! Tree-sitter's TypeScript grammar exposes `comment` nodes. Within
//! those:
//!
//!   - `/** … */` (JSDoc / TSDoc) — recognised as a doc comment.
//!     Attaches to the **following** declaration.
//!   - `/* … */` plain block comments and `// …` line comments are
//!     NOT doc comments and are skipped.
//!   - Top-of-file `/** … */` blocks with no following declaration
//!     attach to the special `<module>` symbol, matching the Rust
//!     extractor's `//!` semantics.
//!
//! ## JSX text exclusion
//!
//! TSX `jsx_text` nodes (the body text inside `<div>...</div>`) are
//! NOT extracted. JSX text is end-user-facing UX copy with its own
//! i18n / copy-review pipeline; the doc-linter's vocab-closure
//! contract is internal coherence only. The comment walker below
//! never reads `jsx_text` payloads — it only collects `comment` nodes
//! whose source begins with `/**`.

use anyhow::{Context, Result};
use tree_sitter::{Node, Parser};

use crate::code_comments::{CommentExtraction, DocComment};

/// Extracts JSDoc / TSDoc doc comments from in-memory TS / TSX source for
/// the [[entity-doc-graph]] vocab-closure check. Test entry point: callers
/// pass a literal source string and choose the grammar via `is_tsx`.
pub fn extract_doc_comments_ts_from_str(content: &str, is_tsx: bool) -> Result<CommentExtraction> {
    let mut parser = Parser::new();
    let language = if is_tsx {
        tree_sitter_typescript::language_tsx()
    } else {
        tree_sitter_typescript::language_typescript()
    };
    parser
        .set_language(&language)
        .context("load tree-sitter-typescript grammar")?;
    let tree = parser
        .parse(content, None)
        .context("tree-sitter parse returned None — ungrammatical source")?;
    let bytes = content.as_bytes();

    let root = tree.root_node();
    let mut comments: Vec<DocComment> = Vec::new();
    let mut cursor = root.walk();
    walk_node(root, bytes, &mut cursor, &mut comments, /*depth=*/ 0);

    Ok(CommentExtraction {
        doc_comments: comments,
    })
}

/// Recursive descent collecting JSDoc / TSDoc comments. For each
/// container node we walk children in order; when we see a `/** … */`
/// comment, we look ahead past further comments / decorators /
/// `export` wrappers to the next named declaration and attach there.
/// Comments at the top of the `program` (file root) with nothing
/// following resolve to `<module>`.
fn walk_node<'a>(
    node: Node<'a>,
    bytes: &[u8],
    cursor: &mut tree_sitter::TreeCursor<'a>,
    out: &mut Vec<DocComment>,
    depth: usize,
) {
    let children: Vec<Node<'a>> = node.children(cursor).collect();
    let is_root = depth == 0;

    let mut i = 0;
    while i < children.len() {
        let child = children[i];
        // Never read jsx_text payloads. They are end-user UX copy and
        // explicitly out of scope for vocab-closure (they have their
        // own i18n / copy-review pipeline).
        if child.kind() == "jsx_text" {
            i += 1;
            continue;
        }
        if is_jsdoc_comment(child, bytes) {
            let raw = slice(bytes, child).unwrap_or("");
            let text = strip_jsdoc_markers(raw);
            let line = node_line(child);
            let col = node_col(child);

            // Look ahead past further comments and decorators to the
            // next "carrier" node (the named declaration we're
            // attaching to).
            let mut j = i + 1;
            let mut attached: Option<String> = None;
            while j < children.len() {
                let n = children[j];
                if is_comment(n, bytes) {
                    j += 1;
                    continue;
                }
                if n.kind() == "decorator" {
                    j += 1;
                    continue;
                }
                attached = item_name(n, bytes);
                break;
            }
            // Top-of-file JSDoc with nothing following → <module>,
            // matching the Rust extractor's inner-doc behaviour.
            if attached.is_none() && is_root {
                attached = Some("<module>".to_string());
            }

            out.push(DocComment {
                text,
                line,
                col,
                attached_to: attached,
                is_public: true,
            });
            i += 1;
            continue;
        }
        // Recurse into non-comment nodes. Methods inside classes,
        // declarations inside namespaces, etc. all live nested.
        if child.child_count() > 0 {
            let mut sub = child.walk();
            walk_node(child, bytes, &mut sub, out, depth + 1);
        }
        i += 1;
    }
}

/// True when this `comment` node's source text begins with `/**` (and is
/// not the `/**/` empty-block edge case) — gates entry into the
/// [[entity-doc-graph]] code-comment ingest. Plain `// …` and `/* … */`
/// comments return false.
fn is_jsdoc_comment(node: Node<'_>, bytes: &[u8]) -> bool {
    if node.kind() != "comment" {
        return false;
    }
    let Some(raw) = slice(bytes, node) else {
        return false;
    };
    raw.starts_with("/**") && raw != "/**/" && !raw.starts_with("/***")
}

/// True when the node is any kind of comment — used by the
/// [[entity-doc-graph]] TS extractor to skip past stray `// …` line
/// comments between a JSDoc block and its target declaration.
fn is_comment(node: Node<'_>, _bytes: &[u8]) -> bool {
    node.kind() == "comment"
}

/// Returns the best-effort name of the declaration this node
/// represents, unwrapping `export` wrappers and pulling the
/// `const Foo = …` identifier out of `lexical_declaration` nodes.
fn item_name(node: Node<'_>, bytes: &[u8]) -> Option<String> {
    match node.kind() {
        // export { ... } / export const ... / export default ...
        "export_statement" => {
            // Inspect the wrapped declaration (default-export or
            // re-export form). Try the `declaration` field first;
            // fall back to scanning children for a known declaration
            // node kind.
            if let Some(decl) = node.child_by_field_name("declaration") {
                if let Some(name) = item_name(decl, bytes) {
                    return Some(name);
                }
            }
            for c in named_children(node) {
                if let Some(name) = item_name(c, bytes) {
                    return Some(name);
                }
            }
            None
        }
        "function_declaration"
        | "generator_function_declaration"
        | "class_declaration"
        | "interface_declaration"
        | "type_alias_declaration"
        | "enum_declaration"
        | "module"
        | "internal_module"
        | "abstract_class_declaration"
        | "ambient_declaration" => {
            let name = node.child_by_field_name("name")?;
            slice(bytes, name).map(str::to_string)
        }
        // class { foo() {} }
        "method_definition" => {
            let name = node.child_by_field_name("name")?;
            slice(bytes, name).map(str::to_string)
        }
        // const Foo = () => {} / const Foo: FC = ... / let, var,
        // const exports. tree-sitter-typescript wraps these in
        // `lexical_declaration` (const/let) or `variable_declaration`
        // (var); the inner `variable_declarator` has the name.
        "lexical_declaration" | "variable_declaration" => {
            for c in named_children(node) {
                if c.kind() == "variable_declarator" {
                    if let Some(name) = c.child_by_field_name("name") {
                        if let Some(s) = slice(bytes, name) {
                            return Some(s.to_string());
                        }
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Iterator helper for the [[entity-doc-graph]] TS extractor: borrow the
/// named children of a node (skipping anonymous / punctuation nodes) so
/// callers don't need to construct their own `TreeCursor`.
fn named_children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut out = Vec::new();
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.is_named() {
            out.push(c);
        }
    }
    out
}

/// Strips the `/**` / `*/` markers from a JSDoc comment node's source text
/// and returns the body for the [[entity-doc-graph]] vocab-closure check.
/// Each line's leading ` * ` continuation marker is also trimmed so the
/// prose vocab-checker doesn't trip on the asterisk itself. Whitespace
/// inside the body (after the per-line trim) is preserved for line-level
/// diagnostics.
fn strip_jsdoc_markers(raw: &str) -> String {
    // Strip leading `/**` and trailing `*/`.
    let body = raw
        .strip_prefix("/**")
        .unwrap_or(raw)
        .strip_suffix("*/")
        .unwrap_or_else(|| raw.strip_prefix("/**").unwrap_or(raw));
    // For each line, drop any leading whitespace + `* ` continuation
    // (matches the conventional `/** \n * line \n * line \n */` shape).
    let mut out = String::new();
    let mut first = true;
    for line in body.lines() {
        if !first {
            out.push('\n');
        }
        first = false;
        let trimmed = line.trim_start();
        let stripped = if let Some(rest) = trimmed.strip_prefix("* ") {
            rest
        } else if let Some(rest) = trimmed.strip_prefix('*') {
            rest
        } else {
            trimmed
        };
        out.push_str(stripped.trim_end());
    }
    // Trim only the ends of the block, never whole lines: line `i` of
    // the text must stay source line `comment.line + i` for diagnostics.
    jsdoc_code_spans(out.trim_start_matches([' ', '\t']).trim_end())
}

/// Rewrites JSDoc markup that names code into backtick spans so vocab
/// closure skips it: `{@link X}` / `{@linkcode X}` / `{@linkplain X}`
/// and the `{Type}` annotation after a tag (`@param {MyDto} x`).
#[allow(clippy::expect_used, reason = "literal regexes; compile is infallible")]
fn jsdoc_code_spans(text: &str) -> String {
    use std::sync::OnceLock;
    static LINK: OnceLock<regex::Regex> = OnceLock::new();
    static TYPE: OnceLock<regex::Regex> = OnceLock::new();
    let link = LINK.get_or_init(|| {
        regex::Regex::new(r"\{@link(?:code|plain)?\s+([^}\s|]+)[^}]*\}").expect("link regex")
    });
    let ty = TYPE
        .get_or_init(|| regex::Regex::new(r"(@[A-Za-z]+\s+)\{([^{}\n]+)\}").expect("type regex"));
    let text = link.replace_all(text, |c: &regex::Captures<'_>| {
        format!("`{}`", c[1].trim_matches('`'))
    });
    ty.replace_all(&text, |c: &regex::Captures<'_>| {
        format!("{}`{}`", &c[1], c[2].trim_matches('`'))
    })
    .into_owned()
}

/// Borrows the UTF-8 source slice covered by a tree-sitter node for the
/// [[entity-doc-graph]] TS code-comment extractor.
fn slice<'a>(bytes: &'a [u8], node: Node<'_>) -> Option<&'a str> {
    let range = node.byte_range();
    bytes.get(range).and_then(|b| std::str::from_utf8(b).ok())
}

/// Converts a tree-sitter zero-based row to a 1-based source line for
/// [[entity-doc-graph]] diagnostics that quote line numbers.
fn node_line(node: Node<'_>) -> usize {
    node.start_position().row + 1
}

/// Converts a tree-sitter zero-based column to 1-based for
/// [[entity-doc-graph]] diagnostics that quote columns.
fn node_col(node: Node<'_>) -> usize {
    node.start_position().column + 1
}

/// Tree-sitter doc-comment extraction tests for the TypeScript / TSX
/// per-file lint hook that feeds the [[entity-doc-graph]].
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn jsdoc_links_and_types_become_code_spans() {
        let c = extract_ts(
            "/**\n * Loads a {@link BaseItem} for {@linkcode Foo.bar | the bar}.\n * @param {MyUserDto} user the caller\n */\nexport function f() {}\n",
        );
        let text = &c.doc_comments[0].text;
        assert!(text.contains("a `BaseItem` for `Foo.bar`."), "{text}");
        assert!(text.contains("@param `MyUserDto` user"), "{text}");
    }

    /// Test helper — parse a `.ts` source string through the doc-graph
    /// per-file TS lint extractor and return the JSDoc comments it
    /// found. Test cases below assert on `attached_to` to confirm the
    /// extractor binds each JSDoc to the right declaration.
    fn extract_ts(src: &str) -> CommentExtraction {
        extract_doc_comments_ts_from_str(src, /*is_tsx=*/ false).expect("parse should succeed")
    }

    /// Test helper — TSX twin of `extract_ts`, parsing the source
    /// through the doc-graph TSX grammar so JSX-bearing components
    /// don't trip the TypeScript-only parser.
    fn extract_tsx(src: &str) -> CommentExtraction {
        extract_doc_comments_ts_from_str(src, /*is_tsx=*/ true).expect("parse should succeed")
    }

    /// JSDoc above a `function foo()` declaration attaches to `foo`
    /// when the [[entity-doc-graph]] extractor walks the TS syntax tree.
    #[test]
    fn extracts_jsdoc_above_function_declaration() {
        let src = "/** doc */\nfunction foo() {}\n";
        let r = extract_ts(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "doc");
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("foo"));
    }

    /// JSDoc above `class Foo` attaches to `Foo` in the
    /// [[entity-doc-graph]] TS extractor's output.
    #[test]
    fn extracts_jsdoc_above_class_declaration() {
        let src = "/** doc */\nclass Foo {}\n";
        let r = extract_ts(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("Foo"));
    }

    /// JSDoc above `interface Foo` attaches to `Foo` in the
    /// [[entity-doc-graph]] TS extractor's output.
    #[test]
    fn extracts_jsdoc_above_interface_declaration() {
        let src = "/** doc */\ninterface Foo { x: number; }\n";
        let r = extract_ts(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("Foo"));
    }

    /// JSDoc above `const Foo: FC = () => {}` attaches to `Foo`. This
    /// is the React function-component idiom — same path through the
    /// [[entity-doc-graph]] extractor as any arrow-function-bound
    /// `const`.
    #[test]
    fn extracts_jsdoc_above_arrow_const() {
        let src = "/** doc */\nconst Foo: FC = () => null;\n";
        let r = extract_tsx(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("Foo"));
    }

    /// Plain `/* … */` block comment is not a JSDoc and is skipped by
    /// the [[entity-doc-graph]] TS extractor.
    #[test]
    fn skips_plain_block_comment() {
        let src = "/* not jsdoc */\nfunction foo() {}\n";
        let r = extract_ts(src);
        assert!(
            r.doc_comments.is_empty(),
            "expected no doc comments, got {:?}",
            r.doc_comments
        );
    }

    /// `// …` line comment is not a JSDoc and is skipped by the
    /// [[entity-doc-graph]] TS extractor.
    #[test]
    fn skips_line_comment() {
        let src = "// not jsdoc\nfunction foo() {}\n";
        let r = extract_ts(src);
        assert!(
            r.doc_comments.is_empty(),
            "expected no doc comments, got {:?}",
            r.doc_comments
        );
    }

    /// JSX text content is NOT extracted as a doc comment, but the
    /// JSDoc above the function still is. Critical contract for the
    /// [[entity-doc-graph]] hook: vocab-closure runs against `/** … */`
    /// prose only, never against UX copy living between JSX tags.
    #[test]
    fn tsx_jsx_text_is_not_extracted_as_doc_comment() {
        let src = "/** real doc */\n\
                   function App() { return <div>UI Copy</div>; }\n";
        let r = extract_tsx(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].text, "real doc");
        assert!(
            !r.doc_comments.iter().any(|c| c.text.contains("UI Copy")),
            "UI Copy from JSX text should not appear in doc comments; got {:?}",
            r.doc_comments
        );
    }

    /// A top-of-file JSDoc block with nothing following resolves to
    /// `<module>` in the [[entity-doc-graph]] extractor's output,
    /// mirroring the Rust extractor's `//!` semantics.
    #[test]
    fn inner_doc_comment_attaches_to_module() {
        let src = "/** module-level */\n";
        let r = extract_ts(src);
        assert_eq!(r.doc_comments.len(), 1, "{:?}", r.doc_comments);
        assert_eq!(r.doc_comments[0].attached_to.as_deref(), Some("<module>"));
    }

    /// Roadmap-49 phase 1c [[entity-doc-graph]]: a JSDoc with a single
    /// `@endpoint GET /x` line above a function yields one EndpointMarker.
    #[test]
    fn extract_endpoint_markers_ts_finds_get_marker() {
        let src = "/** @endpoint GET /x */\nfunction handler() {}\n";
        let r = extract_ts(src);
        let markers = crate::code_comments::extract_endpoint_markers(&r, "apps/web/src/api.ts");
        assert_eq!(markers.len(), 1, "{markers:?}");
        let m = &markers[0];
        assert_eq!(m.method, "GET");
        assert_eq!(m.path, "/x");
        assert_eq!(m.handler_symbol.as_deref(), Some("handler"));
        assert_eq!(m.source_file, "apps/web/src/api.ts");
    }

    /// Roadmap-49 phase 1c [[entity-doc-graph]]: two `@endpoint` lines in
    /// one JSDoc emit two markers attached to the same handler.
    #[test]
    fn extract_endpoint_markers_ts_handles_multiple_per_handler() {
        let src = "/**\n\
                   * @endpoint GET /x\n\
                   * @endpoint POST /x\n\
                   */\n\
                   function handler() {}\n";
        let r = extract_ts(src);
        let markers = crate::code_comments::extract_endpoint_markers(&r, "apps/web/src/api.ts");
        assert_eq!(markers.len(), 2, "{markers:?}");
        let methods: Vec<&str> = markers.iter().map(|m| m.method.as_str()).collect();
        assert!(methods.contains(&"GET"));
        assert!(methods.contains(&"POST"));
        for m in &markers {
            assert_eq!(m.handler_symbol.as_deref(), Some("handler"));
            assert_eq!(m.path, "/x");
        }
    }

    /// Roadmap-49 phase 1c [[entity-doc-graph]]: a `@endpoint` line in a
    /// plain `//` line comment (not a JSDoc) emits no markers — the
    /// extractor only reads `/** ... */` blocks.
    #[test]
    fn extract_endpoint_markers_ts_skips_text_outside_doc_comments() {
        let src = "// @endpoint GET /x\nfunction handler() {}\n";
        let r = extract_ts(src);
        let markers = crate::code_comments::extract_endpoint_markers(&r, "apps/web/src/api.ts");
        assert!(markers.is_empty(), "{markers:?}");
    }

    /// Roadmap-49 phase 1c [[entity-doc-graph]]: the path is preserved
    /// verbatim, including param colons and query strings.
    #[test]
    fn extract_endpoint_markers_ts_preserves_path_verbatim() {
        let src = "/** @endpoint POST /v1/outlets/:id/prices?foo=bar */\n\
                   function handler() {}\n";
        let r = extract_ts(src);
        let markers = crate::code_comments::extract_endpoint_markers(&r, "apps/web/src/api.ts");
        assert_eq!(markers.len(), 1, "{markers:?}");
        assert_eq!(markers[0].path, "/v1/outlets/:id/prices?foo=bar");
        assert_eq!(markers[0].method, "POST");
    }
}
