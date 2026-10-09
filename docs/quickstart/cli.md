---
id: quickstart-cli
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph, scip]
title: "Quick start: CLI"
summary: The six commands that take a repo from nothing to a linted, queryable doc and code graph.
status: stable
updated: 2026-10-01
tags: [quickstart, cli]
---

# Quick start: CLI

Every flag and subcommand is in [[reference-cli]]; this page is the
shortest path through them.

## 1. Install

The repo is private, so install from git with your GitHub credentials
(the `gh` credential helper or an SSH key):

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked \
  --git https://github.com/AIhiAI/doc-linter
```

The build needs a C and a C++ compiler (no cmake). Also install
[Vale](https://vale.sh) (vocabulary checks, on by default) and, for
`.adoc` files, `asciidoctor`.

## 2. Set up a repo

```bash
cd your-repo
doc-linter init         # writes .doc-lint.toml + docs/ontology/
doc-linter scip-index   # optional: code graph for every detected language
doc-linter check        # lint, then build .doc-lint/graph.sqlite
```

`check` exits `0` when clean, `1` on lint errors and `2` on an
internal error. Each code it prints is explained in [[error-codes]].

## 3. Ask the graph

```bash
doc-linter query similar "how are refunds computed"   # ranked docs
doc-linter query similar "refund" --type function     # ranked functions
doc-linter query at src/billing.rs:42                 # what's around this line
doc-linter query impact <function-symbol>             # who calls it, transitively
doc-linter query context <doc-id>                     # a doc and its links
```

Next: wire the same graph into your agent with [[quickstart-mcp]], or
tune `.doc-lint.toml` with [[quickstart-config]].
