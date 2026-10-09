---
id: doc-linter-usage
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph]
title: Using doc-linter in your repo
summary: Task-oriented guide for installing doc-linter in a fresh repo, authoring docs that pass the lint, configuring the role↔folder convention to match your project's layout, and integrating with CI / editor / agent workflows. Pairs with the comprehensive reference in [doc-linter README](README.md) and the agent prompt at `AGENTS.md`.
status: stable
updated: 2026-05-09
tags: [docs, lint, getting-started]
---

# Using doc-linter

A task-oriented guide. For the comprehensive reference (every
subcommand, every issue code, the graph schema, …) see the
[doc-linter README](README.md). For an agent-runtime prompt that
teaches Claude Code / opencode / Cursor how to author docs against
your repo's conventions see `AGENTS.md`.

## Quickstart (5 minutes)

```bash
# 1. Install
cargo install --git https://github.com/AIhiAI/doc-linter.git

# 2. Bootstrap — writes .doc-lint.toml, docs/ontology/, .doc-lint/
cd /path/to/your/repo
doc-linter init

# 3. Lint
doc-linter check
```

A fresh `init` lints clean out of the box. Add real content, run
`check` again, fix what surfaces.

`init` writes:

- `.doc-lint.toml` — every config option, commented inline.
- `docs/ontology/` — starter four-axis ontology (role, kind,
  lifecycle, covers) plus a placeholder entity. Replace the
  placeholder with one entity doc per real domain concept in your
  codebase.
- `.doc-lint/` — gitignored cache directory (graph DB, generated
  Vale config, optional SCIP file). The init step appends
  `.doc-lint/` to your `.gitignore`.

## Build requirements

The graph is an embedded SQLite file (bundled SQLite, built by the
`cc` crate). Hosts need a C compiler and a C++ compiler on `PATH` (the
usearch vector index is C++); no cmake. On Debian/Ubuntu:
`sudo apt install build-essential`. After the first build the dev cycle
is sub-second.

## Authoring a new doc

1. **Pick a role** by reading
   `docs/ontology/values/role/*.md`. Common starters: `doc`,
   `roadmap-entry`, `index`.
2. **Pick a folder** that matches the role per
   `[[layout_rules]]` in `.doc-lint.toml` (see [layout rules](#layout-rules-role-folder-convention)).
3. **Author the frontmatter** — the canonical six required fields
   are `id`, `role`, `title`, `summary`, `status`, `updated`. Some
   roles also require `kind` and/or `lifecycle` per the ontology.
4. **Use wikilinks** `[[some-id]]` to reference other docs by their
   frontmatter `id:` (NOT by filename).
5. **Run `doc-linter check --file path/to/your-doc.md`** and fix
   anything it flags.

Example frontmatter:

```yaml
---
id: my-thing
role: doc
kind: explanation
lifecycle: stable
covers: [some-entity]
title: How my thing works
summary: One sentence describing this doc; the linter pulls this into the sitemap and shows it in tooltips.
status: stable
updated: 2026-05-09
tags: [docs]
---
```

## Layout rules (role↔folder convention)

doc-linter enforces that a doc's role/kind/lifecycle dictates which
directory it lives under. Configured via `[[layout_rules]]` blocks
in `.doc-lint.toml`.

A starter set, adapt to your repo's conventions:

```toml
[[layout_rules]]
role = "roadmap-entry"
allowed_paths = ["docs/roadmap/"]

# More-specific entries first: explanation+lifecycle:planning sits in
# its own folder, while plain explanation goes under explanations/.
[[layout_rules]]
role = "doc"
kind = "explanation"
lifecycle = "planning"
allowed_paths = ["docs/planning/"]

[[layout_rules]]
role = "doc"
kind = "explanation"
allowed_paths = ["docs/explanations/", "docs/planning/"]

[[layout_rules]]
role = "doc"
kind = "how-to"
allowed_paths = ["docs/recipes/", "docs/explanations/"]

# Reference docs cover narrative refs under docs/ AND READMEs that
# live next to code packages. List every parent dir where you keep
# code packages with READMEs.
[[layout_rules]]
role = "doc"
kind = "reference"
allowed_paths = [
    "docs/explanations/",
    "src/",
    "packages/",
    "apps/",
]

[[layout_rules]]
role = "ontology-axis"
allowed_paths = ["docs/ontology/axes/"]

[[layout_rules]]
role = "ontology-value"
allowed_paths = ["docs/ontology/values/"]

[[layout_rules]]
role = "ontology-entity"
allowed_paths = ["docs/ontology/"]
```

**Three knobs you actually tune:**

1. **Which roles you constrain.** Omit an entry and that role is
   unconstrained — handy for `index` (nav hubs) and `status-report`
   (generated dashboards) which are best left free.
2. **Folder names.** If your project calls it `docs/guides/`
   instead of `docs/recipes/`, just change the `allowed_paths`
   value.
3. **Where READMEs live.** The `doc/reference` entry's
   `allowed_paths` should cover every parent directory where you
   keep code packages with READMEs.

**Order matters** — first match wins. Put more-specific entries
(those with more matchers like `lifecycle = "..."`) before less-
specific ones.

**Built-in escape hatches:**

- `lifecycle: archived` exempts the doc from layout enforcement
  (use for retired material under `docs/archive/`).
- A glob in the `exempt = [...]` list skips a file from all linting,
  not just layout.

## Common workflows

### Run the lint on every save (Claude Code)

Add a PostToolUse hook to your project's Claude Code settings file
(usually under the `.claude/` directory at the repo root):

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write",
        "hooks": [
          {
            "type": "command",
            "command": "doc-linter check --file $CLAUDE_FILE_PATHS"
          }
        ]
      }
    ]
  }
}
```

The hook runs after every Edit / Write to a `.md` file. The agent
sees the failure and fixes it before claiming the task is done.

### CI gate

```yaml
# .github/workflows/doc-lint.yml
name: doc-lint
on: [push, pull_request]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - run: true  # the runner already has a C/C++ compiler
      - run: cargo install --git https://github.com/AIhiAI/doc-linter.git
      - run: doc-linter check
```

`check` exits 0 on a clean corpus, 1 on lint violations, 2 on
internal errors.

### LSP — inline diagnostics in your editor

```bash
# VS Code (with the vscode-glspc generic-LSP extension)
# .vscode/settings.json:
{
  "glspc.serverCommand": "doc-linter",
  "glspc.serverCommandArguments": ["lsp"],
  "glspc.languageId": "markdown",
  "glspc.enabledLanguageIds": ["markdown", "rust"]
}
```

Helix and Neovim wire-up snippets are in the
[doc-linter README](README.md#editor-wiring-lsp).

### Per-edit hook for non-Claude-Code agents

Most agent runtimes (opencode, Cursor) look for `AGENTS.md` and
load it as context. Drop `AGENTS.md` into your repo
root or symlink it from `crates/doc-linter/AGENTS.md`. The agent
reads it, learns to call `doc-linter check --file <path>` after
every edit, and fixes errors before claiming success.

## Common errors and how to fix them

See [`docs/error-codes.md`](docs/error-codes.md) for the full (code →
meaning → fix) table. It's generated from
[`src/validator/code_table.rs`](src/validator/code_table.rs) and stays
in sync with the actual diagnostics the linter can emit — no parallel
hand-maintained copy here to drift.

## Configuration cheatsheet

`.doc-lint.toml` knobs you'll touch most often:

```toml
# Glob list of files the linter should ignore entirely.
exempt = ["**/CHANGELOG.md", "third-party/**"]

# Frontmatter fields every non-exempt doc must have.
required_fields = ["id", "role", "title", "summary", "status", "updated"]

# Ambiguous English nouns that must be qualified with a wikilink
# (e.g. "module" must become "[[some-module]]" or backticked).
vale_ambiguous_words = ["service", "module", "engine"]

# Extra accept-list entries that bypass vocabulary closure WITHOUT
# becoming entities. Use for tech terms (Postgres, JSON, JWT, AWS).
vale_extra_accept = ["Postgres", "JSON", "JWT"]

# Vocabulary closure on doc comments inside .rs files (opt-in).
lint_code_comments = false

# Per-crate enforcement: every public-API doc comment must reference
# at least one ontology entity in the listed crates.
anchor_required_in = ["src/core"]
```

The full set with defaults is in
[`templates/init/doc-lint.toml`](templates/init/doc-lint.toml) — what
`doc-linter init` writes for you.

## When to add an entity vs. when to add a vocabulary term

A common authoring decision:

- **Add an entity** (`docs/ontology/entities/<id>.md`) when the noun
  represents a domain concept the codebase reasons about — something
  you'd build a chapter of documentation around. Entities get a
  graph node, can be `covers:`-targeted, and can absorb wikilinks.

- **Add to `vale_extra_accept`** when the noun is a technical term,
  brand, acronym, or proper noun that's just *vocabulary* — it
  doesn't deserve a doc page of its own. Examples: `Postgres`,
  `JWT`, `AWS`, library names, file format names.

If you're not sure, start with `vale_extra_accept`. Promote to an
entity later if it grows into something documentation-worthy.

## Pulling in updates

`cargo install --force --git https://github.com/AIhiAI/doc-linter.git`
to grab the latest. The mirror is private; expect the maintainer to
push snapshot commits periodically. Tag-based pinning is a planned
follow-up; for now `--rev <commit>` works.

## Related

- [doc-linter README](README.md) — comprehensive reference
- `AGENTS.md` — prompt for AI coding agents
