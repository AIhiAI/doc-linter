---
id: language-support
role: doc
kind: explanation
lifecycle: planning
title: Language support evidence (C6)
summary: Per-language, code-backed account of what doc-linter gets natively versus from an external SCIP indexer for Rust, TypeScript/JavaScript, Python, Java, C#, Dart and Vue, with file and line references. Backs the README language table.
status: draft
updated: 2026-10-08
covers: [scip]
tags: [launch, languages]
---

# Language support evidence

Line numbers refer to the tree at the commit that added this page; re-check after edits.

## Matrix

| Capability | Rust | TS/JS | Python | Java | C# | Dart | Vue |
|---|---|---|---|---|---|---|---|
| Functions, types, fields | SCIP | SCIP | SCIP | SCIP | SCIP | SCIP | none |
| Call edges, implements/extends | SCIP | SCIP | SCIP | SCIP | SCIP | SCIP | none |
| Doc comment on a graph function | SCIP `documentation` | same | same | same | same | same | none |
| `File` / `Module` rows | native | native | native | native | native | native | File only |
| Doc-comment lint (`--lint-code-comments`) | native, tree-sitter | native, tree-sitter | native, tree-sitter | native, line scanner | native, line scanner | native, line scanner | native, TS extractor on `<script>` |
| Endpoint extraction | axum/clap/MCP | express | fastapi/flask | JAX-RS | none | none | none |

"SCIP" means the item exists only if the external indexer is installed and succeeds. Without it `scip-index` prints a missing-indexer message and the language contributes no functions (`src/cmd/scip_index.rs:268-277`).

## Evidence

- Indexer table, markers and source extensions: `src/cmd/scip_index.rs:109-219` (`INDEXERS`). Per language: rust 111, typescript 124, csharp 140, dart 166, java 181, python 203. No entry has `vue` in `sources`.
- Detection requires a marker file within three directory levels (`MARKER_DEPTH`, `src/cmd/scip_index.rs:399-428`) and a source file with a listed extension (`source_extensions`, line 616). Markers starting with `.` match by suffix, so C# means any `*.csproj`.
- Java markers are Gradle and Maven files only (lines 183-189). The comment at line 179 mentions sbt but no sbt marker is listed, so sbt repos are not detected.
- Java runs the Gradle or Maven build with the SemanticDB plugin, so the repo must compile; Gradle test source sets are excluded (`java_trailing`, line 694). C# runs the `dotnet` CLI to generate one solution (`dotnet_solution`, line 716). Dart runs once per `pubspec.yaml` and needs `dart pub get` (lines 163-175).
- Missing indexer: `src/cmd/scip_index.rs:268-277` reports it and fails the command; `check` still lints docs.
- Functions, types, calls come only from the SCIP file: `src/scip_ingest.rs` (`parse_scip` around 430-600, `derive_calls_for_document` 827, `add_dispatch_calls` 873). The graph function's `doc_comment` is `join_doc(&sym.documentation)` (`src/scip_ingest.rs:545,579`), so Java/C#/Dart doc text lands in the graph only when the indexer emits SCIP documentation. The repo tests assert this shape for rust-analyzer-like fixtures; no test here asserts that scip-java, scip-dotnet or scip_dart emit documentation.
- Language-specific ingest handling that exists: Java compiler-generated member demotion (`src/scip_ingest.rs:1168`, tests ~1795), C# and Dart field/property symbols (tests 2527-2650), Dart relative-path rewrite (`src/cmd/scip_index.rs:533`, test 870).
- Extension to language label: `src/scip_ingest.rs:1543-1560` (adds `vue`, `cs`, `dart`, `java`); walker equivalent `src/kuzu_graph/file_module_ingest.rs:200-215`. This is why `.vue` files appear as `File` rows with `language = vue` while no function ever has that language from SCIP.
- Comment lint dispatch: `Lang` enum and `from_path` at `src/code_comments.rs:1387-1415`; `extract_any_from_str` 1432-1445; C#/Dart `///` scanner `src/code_comments_slash.rs:27`; Javadoc scanner `src/code_comments_java.rs:31`; Vue `<script>` extraction `src/code_comments.rs:1448`.
- Tree-sitter dependencies are rust, typescript (also JS/TSX) and python only (`Cargo.toml:70-81`).
- Staleness hint scans `.rs .py .ts .cs .dart .java` but not `.js`, `.tsx`, `.vue` (`src/scip_ingest.rs:1698`); a minor gap.

## Not verified

No Java, C# or Dart repository is exercised end to end in this repository's tests; only synthetic SCIP documents are. Plan item C6 still needs one real small repo per language run through `scip-index` then `check`.
