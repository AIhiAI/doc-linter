---
id: crate-doc-linter
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph]
title: doc-linter
summary: Validates project documentation frontmatter and inter-doc links, ingests SCIP code-graph + semantic embeddings, and exposes the unified knowledge graph as MCP tools for AI agents. Drop-in for any codebase via `cargo install --git` and `doc-linter init`.
status: stable
updated: 2026-10-01
tags: [crates, docs, platform, rust, mcp, embeddings]
---

# doc-linter

> **You are an AI coding agent reading this README** because you've been
> asked to integrate doc-linter into a repository, or because you're
> being asked questions about a codebase that already has it installed.
> The rest of this document is structured as your operating manual:
> install steps, authoring conventions, MCP tool catalog, schema
> reference, recipes. Skim the headings, jump to the section that
> matches the task you're on.

## Supported languages

Docs (markdown) work for every repo. The **code graph** (functions, types, call edges) comes from `doc-linter scip-index`, which runs one external [SCIP](https://github.com/sourcegraph/scip) indexer per language it detects (marker file plus a source file) and merges the results. doc-linter does not bundle the indexers. A detected language whose indexer is not on PATH emits `scip-indexer-missing`; docs linting still runs.

| Language | Detected by | Indexer binary | Install | Doc comments linted natively |
|---|---|---|---|---|
| Rust | `Cargo.toml` + `.rs` | `rust-analyzer` | `rustup component add rust-analyzer` | yes (tree-sitter) |
| TypeScript / JavaScript | `tsconfig.json` or `package.json` + `.ts .tsx .js .jsx .mjs .cjs` | `scip-typescript` | `npm i -g @sourcegraph/scip-typescript` | yes (tree-sitter) |
| Python | `pyproject.toml` or `setup.py` + `.py` | `scip-python` | `npm i -g @sourcegraph/scip-python` | yes (tree-sitter) |
| Java | `settings.gradle[.kts]`, `build.gradle[.kts]` or `pom.xml` + `.java` | `scip-java` | see [scip-java](https://github.com/sourcegraph/scip-java); needs a JDK and a repo that builds (sbt is not detected) | yes (Javadoc line scanner) |
| C# | a `*.csproj` + `.cs` | `scip-dotnet` | see [scip-dotnet](https://github.com/sourcegraph/scip-dotnet); needs the `dotnet` CLI | yes (`///` line scanner) |
| Dart / Flutter | `pubspec.yaml` + `.dart` | `scip_dart` | see [scip_dart](https://pub.dev/packages/scip_dart); run `dart pub get` first | yes (`///` line scanner) |
| Vue | none | none wired in | n/a | `<script>` blocks only (TypeScript extractor) |

What you get without an indexer, for every language above: `File` and `Module` rows (the walker recognises `.rs .py .ts .tsx .js .vue .cs .dart .java` and `Cargo.toml`, `*.csproj`, `pubspec.yaml`, `package.json` modules), and the doc-comment lint. **Functions, types, call edges and the doc comment shown on a function all come from the SCIP indexer**; no language gets them natively. Tree-sitter is used only for the comment lint (Rust, TypeScript/JavaScript, Python) and endpoint extraction. `.vue` files get File rows and comment linting but no functions or calls, because no indexer is wired in for them (`scip-typescript` is not asked to index `.vue`). Per-cell evidence with file and line references is in [docs/launch/language-support.md](docs/launch/language-support.md). Java, C# and Dart have not been verified end to end in this repository's CI; see that page for what the tests do and do not cover.

Other languages (Go, Kotlin, C++, and so on) have no indexer wired in, so they get no code graph. Their docs are still linted.

Separately from SCIP, `check --lint-code-comments` reads doc comments directly from `.rs`, TypeScript and JavaScript files (`.ts`, `.tsx`, `.js`, `.jsx`) and `.py` (tree-sitter), and `.java .cs .dart .vue` (lighter-weight line scanners, no tree-sitter grammar).

---

`doc-linter` is a single Rust binary that:

1. **Lints** project documentation frontmatter + inter-doc links
   against a per-repo ontology (roles / kinds / lifecycles / entities)
   the project authors define.
2. **Ingests** the docs + a SCIP code-graph + per-row sentence
   embeddings into an embedded [SQLite](https://sqlite.org) graph
   (node and edge tables, FTS5 text search, a usearch HNSW vector
   index) at `<root>/.doc-lint/graph.sqlite`.
3. **Exposes** that graph via a CLI (`doc-linter query …`), an LSP
   server (`doc-linter lsp` for editor diagnostics), and an MCP
   server (`doc-linter mcp` for agent-callable tools).

Use it when you want to answer questions like _"which docs cover this
entity?"_, _"what's the test for this function?"_, _"what types
implement Validate?"_, or _"semantically: what's the code that
handles long-document sharding?"_ — without ad-hoc grep, without
maintaining a separate knowledge base, and without coupling to a
specific language.

---

## Quick start

- **Let your agent do it:** paste [the agent prompt](docs/quickstart/agent-prompt.md)
  into Claude Code (or any coding agent) at your repo root. It installs
  doc-linter, builds the graph, checks that queries answer, reports what
  it found, and asks before changing how agents search.
- **By hand:** [CLI](docs/quickstart/cli.md) ·
  [MCP server](docs/quickstart/mcp.md) ·
  [configuration](docs/quickstart/config.md).
- **Reference** (generated from the code by `doc-linter gen-docs`, so it
  can't drift): [every CLI flag](docs/reference/cli.md) ·
  [every MCP tool](docs/reference/mcp.md) ·
  [every `.doc-lint.toml` key](docs/reference/config.md) ·
  [every diagnostic code](docs/error-codes.md).

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/AIhiAI/doc-linter
cd /path/to/your/repo
doc-linter init
doc-linter scip-index   # optional: code graph for every detected language
doc-linter check
```

After the last command, the SQLite graph at `<repo>/.doc-lint/graph.sqlite`
holds every doc, ontology entity, source-code Function / Type, and
the typed edges between them.

---

## What you get out of the box

| Surface | What it does | When the agent reaches for it |
|---------|--------------|-------------------------------|
| CLI `doc-linter query` | 18 typed graph queries | One-off questions during planning |
| MCP server `doc-linter mcp` | 17 agent tools over stdio JSON-RPC | Persistent integration in Claude Desktop / Claude Code / Cursor |
| LSP server `doc-linter lsp` | Inline diagnostics in any LSP editor | Authoring feedback while editing |
| Semantic search | `query similar --backend embedding` via the bundled bge-small-en-v1.5 model | "Find code that does X" when you don't know the right keywords |
| Type / call graph | METHOD_OF / USES_TYPE / IMPLEMENTS / EXTENDS / CALLS / FUNCTION_MENTIONS edges | Structural reasoning ("what implements this trait?") |
| Cluster diagnostic | `doc-linter cluster --promote <id>` | Surfacing candidate entities from code structure |

---

## Install

Pick one (full details, checksums, and what is still untested before the first tagged release: [docs/install.md](docs/install.md)):

```bash
# Prebuilt binary, falls back to a source build
curl -fsSL https://raw.githubusercontent.com/AIhiAI/doc-linter/main/scripts/install.sh | bash

# Python tooling (wheel carries the same binary; not on PyPI yet)
uv tool install doc-linter      # or: pipx install doc-linter

# From source (needs a C and a C++ compiler, Rust 1.88; no cmake)
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/AIhiAI/doc-linter
```

The repository is private, so downloads need GitHub credentials (`gh auth login`). Source-build variants (BM25-only, air-gapped) are in [docs/install.md](docs/install.md#build-from-source).

---

## Initialize a repo

```bash
cd /path/to/repo
doc-linter init
```

This is idempotent. It writes (and refuses to overwrite if already
present):

- `.doc-lint.toml` — per-repo config, every option commented inline.
- `docs/ontology/` — a starter four-axis ontology (`role`, `kind`,
  `lifecycle`, `covers`), a placeholder example entity, and a v1
  bootstrap migration. The scaffold lints clean as-is.
- Appends `.doc-lint/` to `.gitignore` — that's the cache directory
  (SQLite graph, generated Vale config, optional SCIP file).

After `init`, replace the placeholder ontology with one entity doc
per real domain concept in the codebase. See
[§ Authoring conventions](#authoring-conventions) below.

---

## Authoring conventions

### Every doc needs frontmatter

Required fields: `id`, `role`, `title`, `summary`, `status`, `updated`.

```markdown
---
id: my-doc-id                # unique within the repo, kebab-case
role: doc                    # the role axis declares this is regular prose
kind: reference              # optional — one of the kind axis values
lifecycle: stable            # optional — one of the lifecycle axis values
title: My document title
summary: One-line summary used for embeddings + sitemap.
status: stable               # one of allowed_statuses in .doc-lint.toml
updated: 2026-05-24          # ISO date
tags: [agent, mcp]           # optional
covers: [entity-pricing-rule]  # optional — entities this doc explains
---

# My document title

Body text. Use [[entity-pricing-rule]] wikilink syntax to reference
ontology entities; doc-linter resolves these and emits warnings on
broken links.
```

### Entity docs (`role: ontology-entity`)

One doc per domain concept. Lives at `docs/ontology/entities/<id>.md`.

```markdown
---
id: pricing-rule
role: ontology-entity
title: "Entity: Pricing Rule"
summary: Domain concept for a single pricing computation rule.
status: stable
updated: 2026-05-24
axis_id: covers
value_id: pricing-rule
display: Pricing Rule          # human-readable name agents see
description: A single declarative rule that maps order metadata to a price.
synonyms: [rule, pricing rules, price rule]   # alternate names agents may use
source_modules:                # optional — file globs that "implement" this entity
  - "crates/pricing-core/src/**/*.rs"
  - "backend/pricing/**/*.py"
code_terms: [pricing-rule]     # optional — bind code symbols only on these adjacent tokens (PricingRule)
code_veto: [rule-engine]       # optional — never bind a symbol containing these (RuleEngineConfig)
---

# Pricing Rule

Narrative description. Linked from doc-comments, used by `query
similar`, and the body's wikilinks become RELATES_TO edges.
```

After `doc-linter check`, this Entity gains:

- `FUNCTION_MENTIONS` edges from any Function whose doc-comment names
  it (or any synonym).
- `FUNCTION_BELONGS_TO` edges from any Function whose source file
  matches a `source_modules` glob.
- `Entity.is_god_node = true` if it's in the top 3 by mention count.
- A 384-dim embedding of `display + description` (semantic-search
  ready).

### Doc-comments in code

Code-graph queries get richer when source-level doc-comments name
ontology entities. The pattern in any language:

```rust
/// Computes a pricing rule for an order.
/// Returns `None` when no [[entity-pricing-rule]] matches.
fn compute_price(order: &Order) -> Option<Price> { ... }
```

```python
def compute_price(order: Order) -> Optional[Price]:
    """Computes a pricing rule for an order.

    Returns None when no pricing rule matches.
    """
```

Same convention for TypeScript / JS. `doc-linter check` scans these
during ingest and emits `FUNCTION_MENTIONS` with
`confidence: 'high'` (doc-comment match) or `'low'` (symbol-path
match — `fn create_pricing_rule()` even with no doc-comment).

---

## Step-by-step ingest

```bash
# Optional: produce a SCIP code-graph index. Runs the indexer of every
# detected language (rust-analyzer, scip-typescript, scip-python,
# scip-java, scip-dotnet, scip_dart) and merges them. Without it,
# doc-linter still works on docs alone.
doc-linter scip-index

# Walks docs + SCIP, populates the full graph.
doc-linter check
```

Re-run `check` whenever docs or code change meaningfully. `check`'s
exit code: `0` clean, `1` lint violations, `2` internal error. CI gates
on the exit code.

---

## MCP setup (for downstream agents)

Once `check` has populated the graph, expose it as MCP tools.

### Claude Desktop

`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS
(`%APPDATA%\Claude\claude_desktop_config.json` on Windows):

```json
{
  "mcpServers": {
    "doc-linter": {
      "command": "doc-linter",
      "args": ["mcp", "--root", "/absolute/path/to/your/repo"]
    }
  }
}
```

### Claude Code

`.mcp.json` at the repo root:

```json
{
  "mcpServers": {
    "doc-linter": {
      "command": "doc-linter",
      "args": ["mcp"]
    }
  }
}
```

Claude Code starts project servers from the repo root, so `--root`
can be omitted.

### Cursor / Continue / other clients

The binary speaks stdio JSON-RPC 2.0 per the MCP spec. Use the
same `command: "doc-linter"` + `args: ["mcp"]` shape any MCP client
documents.

### Sanity check

Restart the MCP client and prompt the agent:

> Use doc-linter's `query_schema` MCP tool and dump the result.

Expected: a JSON dump of every node + rel table with row counts.
That confirms the wire is correct.

---

## MCP tool catalog

Every tool, its description and its parameters:
[docs/reference/mcp.md](docs/reference/mcp.md) (generated from the
server's `tools/list`). The agent-facing convention: **start with
`query_schema`** to discover what's present, then call the specific
tools for known questions, and fall back to `sql` for novel patterns.

## CLI catalog

Every subcommand and flag: [docs/reference/cli.md](docs/reference/cli.md)
(generated from the clap definition). `--format=json` on most queries
returns structured JSON for programmatic consumption.

---

## Schema reference

Node tables (each is a SQL table you can `SELECT` from with `query sql` / the `sql` tool):

| Node | Key | Notable columns |
|------|-----|-----------------|
| `Doc` | `id` | `path`, `role`, `kind`, `lifecycle`, `title`, `summary`, `summary` |
| `Entity` | `id` | `display`, `description`, `synonyms[]`, `source_modules[]`, `is_god_node` |
| `Function` | `symbol` | `crate`, `file`, `line`, `doc_comment`, `signature`, `body_excerpt` |
| `Type` | `symbol` | `kind` (`struct`/`enum`/`trait`/`module`/`type_alias`), `crate`, `file` |
| `File` | `path` | `loc`, `language` |
| `Module` | `id` | one per crate / package boundary |
| `Endpoint` | `id` | `kind` (axum/clap/mcp/fastapi/flask/express), `method`, `path`, `handler_symbol` |
| `Finding` | `id` | `kind` (TODO/FIXME/XXX/HACK), `file`, `line`, `body` |
| `Migration` | `id` | ontology migration history |
| `RepoMeta` | (singleton) | repo-level metadata |

Edge tables:

| Edge | From → To | Source |
|------|-----------|--------|
| `WIKILINK` / `MD_LINK` | Doc → Doc | doc body links |
| `DEPENDS_ON` / `INFORMED_BY` / `SUPERSEDES` / `CRATE_REF` | Doc → Doc | frontmatter typed-edge fields |
| `COVERS` | Doc → Entity | frontmatter `covers:` |
| `RELATES_TO` | Entity → Entity | frontmatter `relates_to:`, with `type` / `weight` / `source` |
| `FUNCTION_DEFINED_IN` / `TYPE_DEFINED_IN` | Function/Type → Doc | crate-README mapping |
| `FUNCTION_MENTIONS` / `TYPE_MENTIONS` | Function/Type → Entity, `confidence` | doc-comment + symbol-path scan |
| `FUNCTION_BELONGS_TO` / `TYPE_BELONGS_TO` | Function/Type → Entity | `source_modules:` glob match |
| `CALLS` | Function → Function | SCIP reference occurrences |
| `ENTITY_CALLS` | Entity → Entity | derived from CALLS + FUNCTION_BELONGS_TO |
| `TEST_FOR` | Function → Function | name-based test detection |
| `METHOD_OF` | Function → Type | method's parent type |
| `USES_TYPE` | Function → Type | type references in function body |
| `IMPLEMENTS` | Type → Type | `impl Trait for Struct` |
| `EXTENDS` | Type → Type | trait inheritance |
| `DEFINED_IN_FILE` / `IN_MODULE` / `IMPORTS_MODULE` / `IMPORTS` / `DESCRIBED_BY` | File/Module fabric | structural |
| `COUPLED_WITH` | File → File, `commits`, `jaccard` | git co-change history |
| `ENDPOINT_HANDLED_BY` / `ENDPOINT_TOUCHES_ENTITY` | Endpoint → Function/Entity | endpoint extraction |
| `HAS_FINDING` | File → Finding | TODO/FIXME scan |

To see this from a live graph: `doc-linter query schema` or the
`query_schema` MCP tool.

---

## Agent recipes

Common patterns. Run from the CLI or call the equivalent MCP tool.

**"Find code that does X, even if the keywords don't match."**

```bash
doc-linter query similar --backend embedding --type function \
  "long document sharding map-reduce" --top 5
```

**"What types implement Validate?"**

```bash
doc-linter query sql \
  "SELECT src AS type FROM IMPLEMENTS WHERE dst LIKE '%Validate#' ORDER BY src"
```

**"What does this function actually touch?"**

```bash
doc-linter query function-context \
  "rust-analyzer cargo your-crate 0.1.0 src/lib.rs/compute()."
```

**"Show me dark code that has no entity coverage."**

```bash
doc-linter query dead-code --top 20
```

**"Which entities does the pricing-rule entity reach?"**

```bash
doc-linter query subgraph --entity pricing-rule --depth 2
```

**"Surface candidate entities from code clusters."**

```bash
doc-linter cluster --top-n 10 --output docs/ontology/entities/candidates/
# review each candidate.md
doc-linter cluster --promote <candidate-id>   # graduate it into the ontology
```

**"At `src/foo.rs:42`, what's in scope and connected?"**

```bash
doc-linter query at src/foo.rs:42
```

---

## Configuration

`.doc-lint.toml` lives at the repo root; `init` writes a commented one.
Every key with its type, default and description:
[docs/reference/config.md](docs/reference/config.md) (generated from the
config structs). The keys most repos change:
[docs/quickstart/config.md](docs/quickstart/config.md).

### Runtime env vars

| Variable | Purpose |
|----------|---------|
| `DOC_LINTER_EMBED_MODEL` | Override the bundled ONNX model with your own |
| `DOC_LINTER_EMBED_TOKENIZER` | Override the bundled tokenizer.json |
| `DOC_LINTER_SKIP_MODEL_DOWNLOAD` | Skip the build-time model download (set during install) |

---

## CI integration

Minimal GitHub Actions:

```yaml
name: doc-lint
on: [push, pull_request]
jobs:
  lint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install doc-linter
        # Use --no-default-features if your CI doesn't need semantic search.
        run: cargo install --locked --git https://github.com/AIhiAI/doc-linter
      - name: Lint docs
        run: doc-linter check
```

Exit code: `0` clean, `1` lint violations, `2` internal error.

---

## Editor wiring (LSP)

`doc-linter lsp` is an LSP server on stdio. Inline diagnostics in
any LSP-capable editor.

**VS Code** — via the
[vscode-glspc](https://github.com/llllvvuu/vscode-glspc) generic
LSP client:

```jsonc
{
  "glspc.serverCommand": "doc-linter",
  "glspc.serverCommandArguments": ["lsp"],
  "glspc.languageId": "markdown",
  "glspc.enabledLanguageIds": ["markdown", "rust"]
}
```

**Helix** (`languages.toml`):

```toml
[language-server.doc-linter]
command = "doc-linter"
args = ["lsp"]

[[language]]
name = "markdown"
language-servers = ["doc-linter"]
```

**Neovim** (`init.lua`):

```lua
vim.api.nvim_create_autocmd("FileType", {
  pattern = { "markdown", "rust" },
  callback = function()
    vim.lsp.start({
      name = "doc-linter",
      cmd = { "doc-linter", "lsp" },
      root_dir = vim.fs.root(0, { ".doc-lint.toml", ".git" }),
    })
  end,
})
```

Server logs land in `<workspace>/.doc-lint/lsp.log` by default.

---

## Optional companions

| Tool | What it enables | Install |
|------|----------------|---------|
| [Vale](https://vale.sh) | Vocabulary-closure on doc bodies (warns when a doc uses a domain term the ontology doesn't know) | Per Vale docs |
| `rust-analyzer` | Rust SCIP code-graph | `rustup component add rust-analyzer` |
| `scip-python` | Python SCIP code-graph | `pip install scip-python` |
| `scip-typescript` | TypeScript SCIP code-graph | `npm i -g @sourcegraph/scip-typescript` |

Each is opt-in. When the binary is absent, `doc-linter check` emits
one diagnostic and continues; nothing is fatal.

---

## When NOT to use this

- **Single-repo prose, no code structure.** doc-linter overlaps
  ~70 % with standard markdown linters here; pick a lighter tool.
- **Mostly-binary repos** with no markdown or no code worth indexing
  — the ontology layer doesn't pay for itself.
- **You need a hosted graph DB.** The graph is an embedded SQLite
  file; multi-host / multi-tenant deployments are out of scope.
- **You need vector search at scale > a few million rows.** The
  HNSW index (usearch) is rebuilt on every `check --embeddings` and
  loaded into memory by readers.

---

## Issue codes

The full (code → meaning → fix) table is at
[`docs/error-codes.md`](docs/error-codes.md), generated from source by
`doc-linter gen-docs`. Run `check` with `--format=json` to consume
diagnostics programmatically.

---

## No telemetry

doc-linter sends nothing anywhere at runtime. The only network
touchpoints are: the opt-in `llm` feature ([`src/llm/anthropic.rs`](src/llm/anthropic.rs), calls
the Anthropic API only when you enable it and supply a key), and
`build.rs` fetching the pinned embedding model from Hugging Face at
build time (skippable with `DOC_LINTER_SKIP_MODEL_DOWNLOAD=1`). Local
`git` is invoked for history/coupling; nothing is uploaded.

## Build details for the curious

- **SQLite** is bundled (`rusqlite`, compiled from C by the `cc`
  crate); no system SQLite and no cmake. Hosts need a C compiler.
- **usearch** (the default `vector-usearch` feature) is a C++ HNSW
  index built by the `cc` crate: a C++ compiler is needed, cmake is
  not. `--no-default-features` drops it and falls back to an exact
  brute-force index.
- **tract-onnx + tokenizers** (default-features) are pure-Rust ONNX
  inference and HuggingFace tokenizers. No C++ toolchain needed for
  these.
- **bge-small-en-v1.5** (bundled model) is downloaded by `build.rs`
  via system `curl` to the user cache dir on first build. SHA-256
  pinned. ~34 MB.

---

## Related

- [[entity-doc-graph]] — the property graph this linter assembles and queries
- [USAGE.md](USAGE.md) — task-oriented walk-through for adopting doc-linter
- [docs/error-codes.md](docs/error-codes.md) — every diagnostic code with its fix
- [CONTRIBUTING.md](CONTRIBUTING.md) — build, test, lint your change; contributors will be asked to agree to a CLA (draft in [CLA.md](CLA.md), CLA Assistant bot not yet set up)
