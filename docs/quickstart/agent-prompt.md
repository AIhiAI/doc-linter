---
id: agent-prompt
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph, mcp, scip]
title: "Quick start: agent prompt"
summary: One prompt to paste into Claude Code or any coding agent in your repo; it installs doc-linter, builds the graph, verifies it, reports, and asks before changing how the agent searches.
status: stable
updated: 2026-10-01
tags: [quickstart, agent]
---

# Quick start: agent prompt

Paste everything inside the block below into Claude Code (or any coding
agent with a shell) at the root of your repository. The details behind
each step are in [[quickstart-cli]], [[quickstart-mcp]] and
[[quickstart-config]].

```text
Set up doc-linter (https://github.com/AIhiAI/doc-linter) in this repo and show me what it finds.
Work through the steps in order. Where a step says STOP, report what happened and wait for me.

1. Survey (read-only). Report briefly:
   - languages by file count: git ls-files | sed -n 's/.*\.//p' | sort | uniq -c | sort -rn | head -15
   - doc formats and where they live (*.md, *.adoc), and whether docs already use YAML frontmatter
   - monorepo or submodule layout (.gitmodules, workspace manifests), build-output dirs to skip
   - existing .doc-lint.toml, .mcp.json, CLAUDE.md, AGENTS.md (note them; change nothing yet)

2. Install.
   - doc-linter: if `command -v doc-linter` fails, check `cmake --version` and a C++17 compiler, then run
       CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/AIhiAI/doc-linter
     The repo is private: if the clone fails on auth, STOP and ask me for GitHub access (gh auth login or an SSH key).
   - Vale: if `command -v vale` fails, install it (brew install vale, or the release binary from
     https://github.com/errata-ai/vale/releases into ~/.local/bin). With .adoc docs, also check `asciidoctor`.
   - SCIP indexers, only for languages from step 1: rust-analyzer (rustup component add rust-analyzer),
     scip-typescript / scip-python (npm i -g @sourcegraph/scip-typescript @sourcegraph/scip-python),
     scip-java, scip-dotnet, scip_dart. Install what you can without sudo; list what's missing.
   If an install needs sudo, a private package feed or credentials you don't have, STOP and say what's needed.

3. Build.
   - doc-linter init. Then edit .doc-lint.toml from step 1: extend skip_dirs (keep its defaults) with build
     output, vendored code and submodules you shouldn't lint; add exempt globs for third-party docs.
     .md and .adoc are picked up automatically; leave `include` alone.
   - doc-linter scip-index (memory heavy: it refuses below 4 GiB free; if so, skip it and say so). Compare the
     languages it indexed with step 1 and say why any is missing (e.g. TypeScript needs a tsconfig.json).
   - doc-linter check --format=json > .doc-lint/check.json. Exit 1 means lint findings, not failure; exit 2 is a crash: STOP.

4. Verify with real queries. Each must return non-empty JSON of the stated shape; report any that don't.
   - doc-linter query schema                         -> node tables with row counts; Doc > 0
   - doc-linter query similar "<a topic from a doc that has frontmatter>" -> {hits: [...]} with at least one hit
   - doc-linter query context <a doc id from the hits> -> the doc plus its outbound/inbound links
   - with a code index: doc-linter query at <a source file>:<a line inside a function>, then
     doc-linter query impact <that function's symbol> -> callers grouped by depth
   - echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | doc-linter mcp -> a tools array including query_similar

5. Report to me:
   - issue counts by code from .doc-lint/check.json (jq '[.report[].codes[]] | group_by(.) | map({(.[0]): length}) | add'),
     the five most useful findings, and coverage gaps (docs with no links, code with no docs).
     Markdown docs reported `missing-frontmatter` are not in the graph or search: say how many, and offer
     (don't do it) to add frontmatter to them.
   - per language: indexed or not, and which questions it can answer (symbols, callers, file:line lookups)

6. STOP and ask me: "Make doc-linter the first search tool for agents in this repo? (yes/no)". Change nothing before I answer.
   On yes, and only then:
   - add the server to .mcp.json (merge, don't overwrite): {"mcpServers": {"doc-linter": {"command": "doc-linter", "args": ["mcp"]}}}
   - add this block to CLAUDE.md (or AGENTS.md if that's what the repo uses):
       ## Code and doc search
       Query the doc-linter MCP before Grep / git grep: query_similar for concepts, query_at for a file:line,
       function_context and query_impact for a function and its callers, read_source to read code.
       Use grep only for exact literal strings. After large edits, call reingest.
   - offer, separately, a PreToolUse hook on Grep that prints a reminder to try doc-linter first (never blocks).
   On no, leave every file as it is and tell me how to undo the install.
```

What the prompt changes without asking: it installs tools and runs
`doc-linter init`, which writes `.doc-lint.toml`, `docs/ontology/` and a
`.gitignore` line. Everything that changes how your agents search waits
for an explicit yes.
