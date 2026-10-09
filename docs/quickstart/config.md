---
id: quickstart-config
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph]
title: "Quick start: configuration"
summary: The handful of .doc-lint.toml settings most repos change, with a link to the full key reference.
status: stable
updated: 2026-10-01
tags: [quickstart, config]
---

# Quick start: configuration

`doc-linter init` writes a commented `.doc-lint.toml`. Every key, its
type and its default are in [[reference-config]]. These are the ones
repos usually touch:

| Key | Change it when |
|---|---|
| `skip_dirs` | The walk reaches build output or vendored code. It replaces the default list, so copy the defaults in and add to them. |
| `exempt` | Some files aren't yours to lint: third-party READMEs, generated docs. |
| `vale_enabled` | Vale isn't installed, or you don't want vocabulary checks yet (`false`). On by default. |
| `vale_ambiguous_words` | `init` writes seven default bare nouns (`system`, `service`, `rule`, …) that must be qualified, and a word used in the wrong bounded context is reported. Trim the list if your docs use them plainly. |
| `vale_extra_accept` | Vale flags a tech term or brand name that isn't a domain concept. |
| `lint_code_comments`, `code_comment_includes`, `code_comment_excludes` | You want doc comments in source files held to the same vocabulary as the docs. |
| `embeddings` | You want `query similar --backend embedding` (vector search) kept up to date on every `check`. |
| `cross_repo_roots` | Links should resolve into sibling repositories that you don't lint. |

A minimal start for a repo that keeps docs under `docs/` and builds
into `out/`:

```toml
skip_dirs = ["target", ".git", "node_modules", "dist", "out"]
vale_ambiguous_words = ["rule", "service"]
lint_code_comments = true
```

Run `doc-linter check` after each change; an unknown or misspelled
key is ignored rather than rejected, so compare names against
[[reference-config]].
