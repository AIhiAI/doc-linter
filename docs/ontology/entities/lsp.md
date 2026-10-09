---
id: lsp
role: ontology-entity
title: "Entity: LSP"
summary: Editor integration via the Language Server Protocol. The `doc-linter lsp` subcommand speaks LSP over stdio so VS Code, Helix, Neovim, and any other LSP-capable editor get inline diagnostics on every keystroke.
status: stable
updated: 2026-05-30
axis_id: covers
value_id: lsp
display: Language Server Protocol
description: The stdio LSP server doc-linter exposes so editors get inline lint diagnostics in real time. Re-lints individual files on did_open / did_change / did_save; does not run Vale or full-corpus checks (the per-keystroke shell-out cost is unworkable). Workflow is "editor for fast iteration; `doc-linter check` as the canonical pre-commit gate".
synonyms:
  - language-server
  - language-server-protocol
  - lsp-server
introduced_in_version: 2
---

# Entity — Language Server Protocol

The LSP server is doc-linter's editor-side surface. It listens on stdio for the standard LSP requests (`initialize`, `textDocument/didOpen`, `textDocument/didChange`, `textDocument/didSave`, `textDocument/publishDiagnostics`), re-lints the affected file on each event, and publishes diagnostics tagged with the same codes the CLI uses.

The implementation lives under [`src/lsp.rs`](../../../src/lsp.rs) and exposes a single binary subcommand: `doc-linter lsp [--log-file <PATH>]`. Server-side debug output goes to the configured log path (a gitignored `lsp.log` file under the runtime `.doc-lint` directory).

What the LSP path covers:

- Per-file frontmatter validation, link resolution, wikilink checking
- Diagnostic publishing with severity tiers (Error / Warning / Information)
- Workspace root detection via the closest `.doc-lint.toml`

What it deliberately does NOT cover:

- **Vale vocabulary closure** — runs only on full `check`, not per-keystroke
- **Cross-file invariants** — id uniqueness, broken wikilinks pointing at unsaved files
- **SCIP refresh** — the code graph is a `check`-time artefact

See [using-doc-linter](../../using-doc-linter.md) for editor setup snippets (VS Code, Helix, Neovim).
