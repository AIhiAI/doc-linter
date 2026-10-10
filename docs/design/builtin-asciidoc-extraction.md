---
id: builtin-asciidoc-extraction
role: doc
kind: explanation
lifecycle: planning
title: Built-in AsciiDoc prose extraction
summary: Why the Vale vocabulary pass reads .adoc through an in-process extractor built on the asciidoc-parser crate instead of shelling out to Ruby asciidoctor, which crate was chosen, and what was rejected.
status: draft
updated: 2026-10-10
covers: [doc-graph]
tags: [vale, asciidoc]
---

# Built-in AsciiDoc prose extraction

Vale renders `.adoc` by running Ruby `asciidoctor`. Without it every AsciiDoc doc was skipped by the vocabulary-closure check, which left most of a corpus like Apache Fineract unchecked. [`src/adoc.rs`](../../src/adoc.rs) now turns each `.adoc` into plain prose with the same line and column layout and hands Vale that text as a `.txt` mirror. Findings are mapped back to the original `.adoc` path and line.

## Crate choice: asciidoc-parser 0.31.2

- Licence `MIT OR Apache-2.0` (both already on the `deny.toml` allow list) and minimum supported Rust 1.88, equal to our `rust-version`.
- Pure Rust. The only new crates in the tree are `asciidoc-parser`, `bytecount` and `self_cell`; `regex`, `memchr`, `thiserror` and the rest were already present.
- Maintained: the latest release is from 2026-09, with an extensive test suite against the Asciidoctor behaviour.
- Exposes source positions and an include and conditional source map (`Document::origin_of`), so a block span maps to the original line even after preprocessing.
- Distinguishes verbatim blocks, including delimiter-less forms (`[source]` paragraphs, indented literal paragraphs) that a line scanner cannot find reliably.

## Rejected

| Candidate | Why not |
|---|---|
| `asciidork-parser` | MIT, but no declared minimum Rust version and it pulls `jiff`, `bumpalo` and `lazy_static` into the tree; a heavier tree for the same facts. |
| `acdc-parser` | Dual licensed, Rust 1.88, but pulls `chrono`, `peg`, `serde_json`, `url`, `csv`, `encoding_rs` and `evalexpr`; 0.x with few users (about 750 downloads). |
| `asciidocr` | A converter CLI, not a parser API with spans. |
| Hand-written line scanner only | Cannot see delimiter-less verbatim paragraphs. Kept as the fallback if the parser panics. |
| Keep Ruby `asciidoctor` | Output differs between machines, `:toc:` hides every finding in a file, and Windows CI needs a Ruby install. |

## How it works

The parser supplies the line ranges of verbatim blocks. A line scanner then blanks comments, attribute entries, block macros, delimiters, the document header (author and revision lines) and callouts; it strips list, heading and admonition markers and table pipes to spaces; and it masks inline code, passthroughs, URLs, attribute references and cross-reference targets while keeping link text. Blanked text becomes spaces, never deleted, so columns and line numbers survive.

## Ceiling

- If the parser panics the scanner runs alone; delimiter-less verbatim paragraphs then count as prose.
- Includes are not followed, so included files are checked as their own docs.
- Heading words are kept as prose, unlike Markdown headings, which Vale skips.
- A block title on a verbatim block is blanked together with that block.
