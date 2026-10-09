---
id: using-doc-linter
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph, lsp, scip, onnx, roadmap, mcp, sql, endpoint]
title: Using doc-linter day-to-day
summary: Practitioner's guide to driving doc-linter from the CLI, your editor, the Claude Code PostToolUse hook, the MCP server, and CI. Covers every subcommand exposed by the binary, the BM25 vs embedding `query similar` backends, the `--embeddings` opt-in for vector search, the SCIP code-graph, Vale vocabulary closure, and the `.doc-lint.toml` knobs. Self-hosted — this repo lints itself, so every example here is a command you can run from the repo root.
status: draft
updated: 2026-10-08
tags: [docs, how-to, tooling, doc-graph]
---

# Using doc-linter day-to-day

doc-linter validates the documentation graph of any repo it's pointed at: typed frontmatter, link resolution, vocabulary closure against an ontology, and a bridge to the Rust code graph via SCIP. This doc is the practitioner's how-to — what each CLI surface does, when to reach for it, and which knobs in `.doc-lint.toml` change the behaviour.

For the diagnostics catalog (codes the linter emits, what they mean, how to fix them) see [error-codes](error-codes.md). For the architectural reference (how the linter is built, not how to use it) see the crate `README.md`.

## TL;DR

```bash
# Build once
PATH=/usr/bin:$PATH CC=/usr/bin/gcc CXX=/usr/bin/g++ cargo build --release --features embeddings

# Everyday
./target/release/doc-linter check                        # full vault sweep (~30s on this repo)
./target/release/doc-linter check --file docs/foo.md     # one file (LSP-fast)
./target/release/doc-linter query similar "your query"   # BM25 lexical search
./target/release/doc-linter query backlinks <id>         # who links to this doc?
./target/release/doc-linter ontology                     # dump the vocabulary as JSON
./target/release/doc-linter mcp                          # stdin/stdout MCP server for AI agents
```

`check` is the default subcommand — running the binary with no args is the same as running it with `check`. Exit is non-zero on any issue.

## Mental model

The linter operates on three overlapping graphs that all persist into a single embedded SQLite database at `.doc-lint/graph.sqlite` (gitignored), with a usearch vector index file beside it when embeddings are on.

1. **Doc graph** — every `.md` file outside `exempt` is a typed `Doc` node carrying `role / kind / lifecycle / covers / informed-by / supersedes`. Edges: `WIKILINK`, `MD_LINK`, `COVERS`, `INFORMED_BY`, `IMPLEMENTS`, `DEPENDS_ON`, `BLOCKS`, `SUPERSEDES`.
2. **Ontology graph** — the vocabulary docs author against. Lives under `docs/ontology/`: axes (e.g. `role`, `kind`), values (allowed values per axis), entities (domain nouns like `doc-graph`). Every doc body is checked against this vocabulary by Vale.
3. **Code graph** — Rust/TS/Python functions, types, modules pulled from a SCIP index file (`.doc-lint/code.scip`, gitignored, regenerated via `scip-index`). Edges connect functions to entities they mention in their doc-comments and to the docs that cover those entities.

`check` rebuilds the appropriate slice of the DB on every run. The vault subset always rebuilds; the code subset reuses the prior ingest's cache when the SCIP file hasn't changed (overridden by `--rebuild`).

## The CLI surface

Every subcommand below is exposed by the release binary. `--help` after any name prints the full per-subcommand flag list.

### `check` — lint pass (default)

```bash
./target/release/doc-linter check
./target/release/doc-linter check --file docs/using-doc-linter.md
./target/release/doc-linter check --format json | jq '.issues[]'
./target/release/doc-linter check --no-vale            # skip vocabulary closure
./target/release/doc-linter check --lint-code-comments # also vocab-check Rust ///
./target/release/doc-linter check --rebuild            # force full SCIP re-ingest
./target/release/doc-linter check --embeddings         # opt into embedding population
./target/release/doc-linter check --coverage-min-global 70
```

Walks the corpus, rebuilds the SQLite graph (in a staging file that is swapped in, so running `mcp` servers never block), refreshes the Vale config from the live ontology, ingests SCIP if newer than the source, runs every lint rule, prints a status line:

```text
doc-linter: 28 file(s) scanned, all clean
```

`--embeddings` is opt-in and defaults off. The post-ingest embedding pass runs an ONNX forward over every Doc / Entity / Function / Type row — on a mid-sized corpus that's tens of minutes on CPU, dominating wall-clock. `query similar` defaults to `--backend bm25` which doesn't touch the vector index, so most users never need to flip the flag on. Hash-skip means re-running with `--embeddings` after a small edit only re-embeds the rows whose text actually changed.

### `query` — graph traversals

```bash
./target/release/doc-linter query list --kind reference
./target/release/doc-linter query list --covers doc-graph
./target/release/doc-linter query backlinks using-doc-linter
./target/release/doc-linter query neighbors using-doc-linter
./target/release/doc-linter query path using-doc-linter error-codes
./target/release/doc-linter query subgraph using-doc-linter --depth 2
./target/release/doc-linter query context using-doc-linter
./target/release/doc-linter query sql 'SELECT count(*) AS n FROM Doc'
```

Output is JSON by default. `subgraph` and `context` return doc IDs plus edge types, so an agent can decide where to walk next without loading prose.

`query sql` is the raw escape hatch: one read-only `SELECT` or `WITH` statement (anything that could write is refused). Run `query schema` to see the tables; node kinds are tables, every edge kind is a table with `(src, dst, ...)` columns, and `REGEXP` matches the whole string. A `graph.kuzu` left by an older version is ignored (and can be deleted): `check` builds the SQLite graph next to it and says so once.

Code-graph queries:

```bash
./target/release/doc-linter query functions-mentioning doc-graph
./target/release/doc-linter query function-context '<scip-symbol>'
./target/release/doc-linter query types --kind struct --substring Embed
./target/release/doc-linter query impact <scip-symbol> --depth 3
./target/release/doc-linter query dead-code
./target/release/doc-linter query coverage-report --write
./target/release/doc-linter query endpoints --dark
./target/release/doc-linter query at src/embeddings.rs:328
./target/release/doc-linter query map --scope entity --name doc-graph
```

`query at <file>:<line>` is the inverse: given a source location, return the surrounding graph context (function, file, module, entities, covering docs, nearby endpoints). One call, full context.

#### `query similar` — natural-language search

```bash
# Default: BM25 lexical scoring. No setup, no embedding cost.
./target/release/doc-linter query similar "rayon parallel batched embedding" --type function --top 5

# Vector cosine search. Requires `check --embeddings` to have run first.
./target/release/doc-linter query similar "rayon parallel batched embedding" \
    --type function --backend embedding --top 5
```

`--type` is one of `doc | function | entity | all`. The default backend is `bm25` — it reads the existing text columns (title, summary, body, doc-comment), tokenises lowercase + non-alphanumeric split, and ranks by Okapi BM25 score. Zero embedding-model dependency at query time.

The `embedding` backend asks the vector index (HNSW) for the nearest rows by cosine similarity; the index is built by `check --embeddings`. Vectors are produced by a bundled bge-small-en-v1.5 model that `build.rs` downloads on first build (~34 MB, cached). When the vectors are empty (no opt-in `--embeddings` run), the embedding backend errors with a clear pointer at the flag.

Pick `bm25` when the user knows the vocabulary (it'll outrank embeddings on exact term matches). Pick `embedding` when the user is searching by intent and the corpus uses synonymous phrasing.

`query saved` exposes a curated catalog of high-value SQL views so agents can reach for `query saved orphan-entities` instead of composing the SQL by hand. `query saved --list` enumerates them.

`query schema` is the self-describing introspection surface: every node table, every rel table, columns with types + PK flags, live row counts, plus example SQL queries.

`query graph-summary` is a one-shot per-table-row-count summary — apples-to-apples comparison with external graph builders.

### `ontology` — dump the vocabulary

```bash
./target/release/doc-linter ontology | jq '.entities | keys'
./target/release/doc-linter ontology | jq '.roles[].id'
```

Cold-start endpoint for AI agents. Returns axes + values + entities + migration history.

### `scip-index` — refresh the code graph

```bash
./target/release/doc-linter scip-index
DOC_LINTER_SCIP_MEMORY_MB=8192 ./target/release/doc-linter scip-index   # raise heap
```

Wraps `rust-analyzer scip . --output .doc-lint/code.scip`. The next `check` re-ingests automatically. A SCIP file older than the newest `.rs` source emits `scip-stale` on stderr — informational unless `scip_required = true` in the config. The 4 GB rust-analyzer default heap is enough for most repos; raise via `DOC_LINTER_SCIP_MEMORY_MB` for larger trees.

### `lsp` — editor integration

```bash
./target/release/doc-linter lsp                       # stdio LSP server
./target/release/doc-linter lsp --log-file /tmp/lsp.log
```

Re-lints individual files on open / change / save. Does NOT run Vale (per-keystroke shell-out is unworkable). For VS Code use the [generic-LSP](https://github.com/llllvvuu/vscode-glspc) extension; for Helix and Neovim see editor-specific snippets at the bottom of this doc.

The LSP does NOT cover Vale vocabulary closure, cross-file invariants (id uniqueness, broken wikilinks to unsaved files), or SCIP refresh. Workflow: **editor for fast iteration; `check` as the canonical pre-commit gate**.

### `mcp` — Model Context Protocol server

```bash
./target/release/doc-linter mcp
```

Line-delimited JSON-RPC over stdin/stdout. Implements `initialize`, `tools/list`, `tools/call` over a read-only tool set including `sql`, `list_docs`, `list_entities`, `list_types`, `list_findings`, `function_context`, `query_similar`, `query_impact`, `query_path`, `query_at`. AI agents wired through MCP get the same graph queries the human CLI exposes, without spawning a subprocess per query.

### `init` — bootstrap a fresh repo

```bash
cd /path/to/new-repo
doc-linter init
```

Writes `.doc-lint.toml`, scaffolds `docs/ontology/` (axes + values + placeholder entity + the v1 bootstrap migration), creates `.doc-lint/`, appends to `.gitignore`. Refuses to overwrite an already-populated `docs/ontology/` — your authored vocabulary is authoritative.

### `export` — publish-pipeline subset

```bash
./target/release/doc-linter export --out /tmp/public-docs --strip internal,private
```

Filters by `visibility:` frontmatter and writes the surviving subset plus a `.publish-manifest.json`. Cross-repo client docs are not exported — only the primary `--root` tree.

### `homepage` — generate the AI-agent landing page

```bash
./target/release/doc-linter homepage --write
```

Emits `MAP.md` (default) with every entity, every narrative doc grouped by kind, every bounded context, and a pointer to the public-API graph — all linked via Obsidian `[[wikilinks]]`. Deterministic so the `homepage-stale` lint rule can byte-compare on-disk against the in-memory output.

### `explain` — narrate a single function

```bash
./target/release/doc-linter explain '<scip-symbol-or-substring>'
./target/release/doc-linter explain --list           # list resolvable symbols
./target/release/doc-linter explain populate_embeddings --json
```

Walks the graph from one function: metadata, every entity it mentions (display + summary), every narrative doc covering those entities, and the top 5 sibling functions in the same crate ranked by entity-mention overlap. The "what is this thing for" query agents reach for after `query at`.

### `scaffold-coverage` — propose dark-function doc-comments

```bash
./target/release/doc-linter scaffold-coverage <CRATE>          # dry-run preview
./target/release/doc-linter scaffold-coverage <CRATE> --write  # apply edits
./target/release/doc-linter scaffold-coverage <CRATE> --json
```

Walks every dark Function in the named crate and emits `///` doc-comment templates. The entity guess uses the SCIP-symbol tokenizer; functions whose symbol doesn't match any entity get a TODO stub plus the three closest fallback candidates by mention frequency. `--write` refuses to run on a dirty git tree as a safety measure.

### `migrate-anchor-required` — grandfather the existing dark surface

```bash
./target/release/doc-linter migrate-anchor-required
```

Walks every dark public function in the freshly-ingested graph and writes their names to the `coverage_anchor_exempt_file` (default — a gitignored `anchor-exempt.txt` under the runtime `.doc-lint` directory). Run once when flipping `require_anchor_per_pubapi = true`; new functions added after the snapshot must reference an entity or be regex-exempted.

### `migrate-endpoint-markers` — backfill `@endpoint` doc-comment lines

```bash
./target/release/doc-linter migrate-endpoint-markers --dry-run
./target/release/doc-linter migrate-endpoint-markers --include-typescript
```

Walks the regex extractor's endpoint output, resolves each handler via SCIP, stamps `/// @endpoint <METHOD> <path>` on the existing doc-comment. Refuses to run on a dirty git tree. Endpoints whose handler can't be resolved go to `/tmp/endpoint-marker-manual.txt` for hand follow-up. Flip `endpoint_marker_exclusive = true` once the manual report is empty.

### `cluster` — propose entity candidates from code

```bash
./target/release/doc-linter cluster --top-n 10
```

Runs label-propagation community detection over `FUNCTION_BELONGS_TO` + `CALLS`, picks each community's god node, prints a JSON report. Use when you suspect the ontology is missing concepts the code already structures around. `cluster --write` scaffolds candidate stubs under `docs/ontology/entities/candidates/`; `cluster --promote <id>` flips a reviewed candidate to `status: stable` and moves it to `docs/ontology/entities/`.

### Self-healing ontology — the `unauthored-cluster` lint

`check` runs the same LPA pass post-ingest and emits an `unauthored-cluster` warning per community whose `suggested_id` doesn't match any registered entity. The diagnostic carries the copy-paste fix path (`cluster --write` → edit stub → `cluster --promote <id>`), so the contributor's iteration loop is "see the warning, run two commands, the cluster drops out on the next check". The ontology grows in lockstep with the code's actual structure instead of drifting.

Knobs live under `[cluster_lint]` in `.doc-lint.toml`. Defaults: on, severity warning, `min_members = 5`, `min_density = 0.05`, `top_n = 10`, plus an `ignore` list pre-seeded with the generic names LPA tends to produce (`common`, `util`, `tests`, `mod`, `main`, `lib`). Flip `enabled = false` to mute entirely; widen `ignore` to skip per-repo tooling concerns that aren't domain concepts.

### `codes` — regenerate the diagnostics table

```bash
./target/release/doc-linter codes --markdown > docs/error-codes.md
```

Source of truth lives in [`code_table.rs`](../src/validator/code_table.rs); this subcommand only renders. The staleness lint on [`error-codes.md`](error-codes.md) byte-compares the on-disk file against `codes --markdown`, so any change to the code table needs a re-render commit.

## Graph-query cookbook (SQL)

SQLite's dialect. The graph is rebuilt on every full `check`; queries are read-only. Run any of these via `doc-linter query sql '<QUERY>'`.

```sql
-- Every doc and its declared kind
SELECT id, kind FROM Doc;

-- Top entities by doc coverage
SELECT dst AS entity, count(*) AS docs FROM COVERS
GROUP BY dst ORDER BY docs DESC LIMIT 10;

-- Orphan docs (no wikilink, markdown link or covers edge in either direction)
SELECT id FROM Doc d
WHERE NOT EXISTS (SELECT 1 FROM WIKILINK w WHERE w.src = d.id OR w.dst = d.id)
  AND NOT EXISTS (SELECT 1 FROM MD_LINK m WHERE m.src = d.id OR m.dst = d.id)
  AND NOT EXISTS (SELECT 1 FROM COVERS c WHERE c.src = d.id);

-- Functions that mention a domain entity (where is the concept implemented?)
SELECT f.symbol, f.crate FROM Function f
JOIN FUNCTION_MENTIONS m ON m.src = f.symbol
WHERE m.dst = 'doc-graph';

-- Roadmap entries that depend on another roadmap entry
SELECT a.id, b.id FROM DEPENDS_ON e
JOIN Doc a ON a.id = e.src JOIN Doc b ON b.id = e.dst
WHERE a.role = 'roadmap-entry' AND b.role = 'roadmap-entry';
```

Paths between two docs are a recursive walk; use `doc-linter query path using-doc-linter error-codes` (a recursive CTE under the hood) rather than writing one by hand.

`query saved --list` enumerates the curated catalog of named recipes — reach for those before composing SQL by hand; the saved set is the supported surface that survives schema bumps.

## PostToolUse hook (Claude Code)

Wire the linter into Claude Code's edit pipeline so every `.md` save re-lints automatically. In `.claude/settings.json`:

```json
{
  "hooks": {
    "PostToolUse": [{
      "matcher": "Edit|Write",
      "hooks": [{
        "type": "command",
        "command": "/abs/path/to/doc-linter check --file $CLAUDE_FILE_PATHS"
      }]
    }]
  }
}
```

If the lint fails the hook blocks the edit. Use `check --file` (per-file mode) not the full `check` — the hook fires per-keystroke and a full sweep is too expensive.

## Vale vocabulary closure

`check` regenerates a self-contained Vale config at `.doc-lint/vale/` on every run from the live ontology, then shells out to `vale` to enforce that every domain noun in doc bodies resolves to a registered entity. If `vale` isn't on PATH the lint emits `vale-missing` and continues. Disable per-repo with `vale_enabled = false` in `.doc-lint.toml` or per-invocation with `--no-vale`.

Install Vale:

```bash
curl -sSfL "https://github.com/errata-ai/vale/releases/download/v3.9.1/vale_3.9.1_Linux_64-bit.tar.gz" \
  -o /tmp/vale.tar.gz \
  && tar xzf /tmp/vale.tar.gz -C /tmp \
  && install -m755 /tmp/vale ~/.local/bin/vale
```

Three accept-list sources merge into one effective vocabulary:

1. **Ontology entities** — each `docs/ontology/entities/<slug>.md` contributes its `id`, `display`, every `synonym`, plus the bare acronym extracted from a parenthetical display (`MCP (Model Context Protocol)` also accepts `MCP`).
2. **`vale_extra_accept`** in `.doc-lint.toml` — universal technical terms that aren't repo-domain (`Postgres`, `JSON`, `HTTP`, AWS product names).
3. **`[vale_dictionaries]` packs** in `.doc-lint.toml` — cspell-style `.txt` files in `dicts/`. Each becomes its own Vale Vocab pack.

Every term runs through case-variant + plural expansion at generation time. For an ontology synonym `sku` written once, Vale accepts `sku`, `Sku`, `SKU`, `skus`, `Skus`, `SKUs`. All-caps variants emit only when the source is ≤6 chars (likely an acronym).

### The `vale_ambiguous_words` trap

`vale_ambiguous_words` in `.doc-lint.toml` (and its underlying `Vocabulary.AmbiguousBare` Vale check) is meant for words like `rule` that ought to be qualified (`pricing rule`, `tenant rule`) so the bare noun resolves to a single registered entity. The Rust post-processor decides per-alert what to do:

```text
if entity index has no match for the term:
    → keep alert (warn the author)
else if doc has no bounded_context:
    → suppress (fallback while bounded_context backfill is pending)
else if entity context matches doc context:
    → suppress
else:
    → downgrade to cross-context-reference
```

The trap: if you add a term to `vale_ambiguous_words` whose lower-cased form **doesn't** appear as any entity's `id` / `display` / synonym / naive plural, the lookup returns empty and every occurrence becomes a kept alert. You've effectively built a "discouraged words" reject-list rather than a smart bounded-context filter.

Guideline: **only add a term to `vale_ambiguous_words` if it's already covered by an existing ontology entity's id / display / synonyms.** Anything else belongs in `vale_extra_accept`.

## Rust source-comment lint

Opt-in with `lint_code_comments = true` in `.doc-lint.toml` (or `--lint-code-comments` on the CLI). Walks every `.rs` file matching `code_comment_includes`, parses with tree-sitter-rust, extracts each `///` / `//!` / `/** */` doc comment, runs the prose through the same `TermIndex` Vale uses for markdown. Output diagnostics are `comment-vocab-violation` with `file:line:col`.

Don't flip this on for a demo week. Cold-on a real repo, expect hundreds to thousands of violations — mostly common technical terms or English connectives, not domain nouns. The fix path: grow the ontology for terms that ARE domain concepts, add synonyms for terms already covered under another label, add common-tech terms to `vale_extra_accept`. The TypeScript / TSX / Python paths follow the same model.

JSX text content is deliberately NOT linted — prose inside `<button>Click me</button>` is end-user UX copy that lives in the i18n / copy-review pipeline. doc-linter's role is internal-coherence on the declarations developers reason about (functions, types, contracts). Plain `// …` and plain `/* … */` block comments are also skipped — only `/** … */` blocks participate, matching the JSDoc / TSDoc convention.

## Force-growth lint rules

Three rules turn previously-cheap escapes ("rephrase the unknown term out of existence", "leave a new entity dangling") into bounded, actionable choices. Each one fires through the same `check` pipeline as the rest of the linter; each comes with a small fixed set of resolutions.

### Rule A — `dark-public-function`

Fires when a public function in a crate listed under `anchor_required_in` lands without a `///` doc-comment that references an ontology entity or roadmap entry. Three resolutions:

1. **Author the doc-comment.** `doc-linter scaffold-coverage <CRATE>` proposes templates that reference the most-likely entity by symbol-token match; pick one and customise. Default for genuine domain functions.
2. **Add the bare function name to `coverage_anchor_exempt_file`** (default `<root>/.doc-lint/anchor-exempt.txt`, managed by `doc-linter migrate-anchor-required`). This list is a **frozen migration artefact** — populated once when flipping `require_anchor_per_pubapi = true`, then committed. New functions added after the snapshot must take path 1 (real entity link) or path 3 (regex exempt); path 2 is closed to new entries.
3. **Add a regex pattern to `coverage_function_exempt`** in `.doc-lint.toml` when a whole family of functions (e.g. `^cell_as_`) is irreducibly generic. Use narrow anchored patterns to avoid silently exempting real domain code on the next refactor.

### Rule B — `comment-vocab-violation` / `unknown-entity`

Fires when a capitalised token in a doc-comment or a `covers:` value doesn't resolve to any registered ontology entity. The diagnostic appends a copy-paste `entity-<term>.md` stub with frontmatter prefilled (`id`, `role: ontology-entity`, `axis_id: covers`, `value_id`, `display`, `description`, `synonyms`, `introduced_in_version`) plus a `## Related entities` block seeded with the three closest-by-token-similarity entities already in the ontology. Two resolutions:

- **Rephrase.** When the closest-candidates list contains the concept the author meant, rewrite the prose to use that entity id (or its display / synonym) and the lint clears.
- **Promote.** Save the stub to `docs/ontology/entities/<slug>.md`, fill in the placeholders, and the lint clears the moment the file lands. The next `check` re-validates the new entity against the ontology schema.

JSON consumers (LSP code-action plumbing, CI bots) read the stub from the `promote_stubs` array on the JSON entry — same string the human renderer prints inline, just structured for programmatic consumption.

### Rule C — `orphan-entity`

Fires when an `entity-<id>.md` doc has no inbound `covers:` references AND no `[[entity-<id>]]` wikilinks anywhere else in the corpus. No grace period — you can't introduce an entity in one PR and "land the cover next sprint." Three resolutions:

1. **Write a narrative doc that references the entity.** A how-to / explanation / reference doc with `covers: [<id>]` in its frontmatter clears the warning.
2. **Add a `[[entity-<id>]]` wikilink to an existing narrative doc's body.** Same result; the substring scan picks it up.
3. **Remove the entity.** `rm docs/ontology/entities/<id>.md` — appropriate when the entity was added to clear a vocab-closure violation but the surrounding code didn't actually need a new ontology concept.

Severity is `"warning"` by default — visible in CI / LSP / human output but doesn't break the build. Repos that have completed Rule A's grandfathering migration can flip `orphan_entity_severity = "error"` in `.doc-lint.toml` to enforce zero-orphan from that point forward.

## SCIP code graph

The Round 3B ingest folds Rust definitions into the `Function` / `Type` node tables, keyed by SCIP symbol, and emits two edge tables:

- `FUNCTION_DEFINED_IN` — Function → the crate's README Doc when present.
- `FUNCTION_MENTIONS` — Function → ontology Entity, one edge per entity term (id, display, synonym, naive plural) found in the doc-comment.

Refresh:

```bash
rust-analyzer scip . --output .doc-lint/code.scip   # manual
./target/release/doc-linter scip-index              # wrapped
```

The next `check` re-ingests automatically. Set `scip_required = true` to make absent SCIP a hard failure.

### Generated code and freshness

Generated code stays out of the graph, the file table, the endpoint scan, the code-comment lint and coverage. A file counts as generated when it matches `coverage_codegen_exclude`, carries a header such as `DO NOT EDIT` or `Code generated`, sits under a `generated` or `__generated__` directory, is named `*.gen.*`, sits in a Gradle `build/` directory next to its build script, or is ignored by git (`.gitignore`). Set `include_generated = true` to index it anyway; extend `coverage_codegen_exclude` for anything else. A doc without frontmatter, and every File row, takes its date from the last git commit that touched it, so a fresh clone shows real ages; outside git, or in a shallow clone, it falls back to the file's mtime.

## Endpoint coverage

Phase 2 of roadmap-43 adds an `Endpoint` node populated on every `check` from three sources: axum `.route()` / `.nest()` calls, clap `Subcommand` derive enums (in crates listed under `clap_crates`), and `McpTool { name: ... }` literals in MCP-namespaced files. Java JAX-RS resources (`@Path` on the class, `@GET`/`@POST`/... plus an optional method `@Path`, `javax.ws.rs` or `jakarta.ws.rs`) are read too. The syntactic extractors run unless `endpoint_marker_exclusive = true`. Each endpoint links to its handler via `ENDPOINT_HANDLED_BY` (SCIP-symbol resolution) and inherits the handler's entity mentions as `ENDPOINT_TOUCHES_ENTITY` edges.

`query endpoints --dark` lists every API surface with no entity coverage — the precise input to the `dark-endpoint` lint.

`@endpoint` markers in handler doc-comments replace the regex extractor's discovery role for repos that flip `endpoint_marker_exclusive = true`:

```rust
/// Returns the price for a SKU at a given outlet.
///
/// @endpoint GET /v1/outlets/:id/prices
pub async fn get_outlet_prices(...) {}
```

Method is `GET`/`POST`/...; or `CLI` for clap subcommands; or `MCP` for MCP tools. Path is verbatim — `:id` params, query strings, special characters survive untouched.

## Authoring docs — frontmatter shapes

Required fields per the default `.doc-lint.toml`: `id`, `role`, `title`, `summary`, `status`, `updated`.

Allowed `role` values come from `docs/ontology/values/role/`; allowed `kind` and `lifecycle` from their respective subdirectories. Run `doc-linter ontology | jq '.roles[].id'` to enumerate.

### Explanation (mental model / why)

```yaml
---
id: <slug>
role: doc
kind: explanation
lifecycle: stable
covers: [doc-graph]
title: <Display>
summary: <one-paragraph>
status: stable
updated: 2026-10-08
tags: [docs, explanation]
informed-by:
  - <other-id>
---
```

### How-to (task-oriented, like this doc)

```yaml
---
id: <slug>
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph]
title: <Display>
summary: <one-paragraph>
status: stable
updated: 2026-10-08
tags: [docs, how-to]
---
```

### Reference (catalog of facts)

```yaml
---
id: <slug>
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph]
title: <Display>
summary: <one-paragraph>
status: stable
updated: 2026-10-08
tags: [docs, reference]
---
```

The `id` must match the filename stem (this doc's filename is `using-doc-linter.md`, its `id` is `using-doc-linter`). Crate READMEs get a free pass: `id: crate-<name>` is the convention since `README.md` doesn't carry a unique stem.

## Authoring ontology entities

Each entity is one `.md` under `docs/ontology/entities/<slug>.md`:

```yaml
---
id: entity-<slug>
role: ontology-entity
title: "Entity: <Display>"
summary: <one-liner>
status: stable
updated: 2026-10-08
axis_id: covers
value_id: <slug>
display: <Display>
description: <one-sentence definition>
synonyms: [list, of, alternates]
introduced_in_version: 2
---

# Entity — <Display>

<one-paragraph definition that names what the concept is and what makes it
distinct from neighbouring concepts. Cite a real file or doc id where the
concept is exercised.>
```

Bump `introduced_in_version` when adding entities. The migrations manifest under `docs/ontology/migrations/` records each version bump.

Bias toward consolidation: add synonyms first, separate entities only when a concept has a stable, distinct meaning load-bearing in 5+ docs.

After adding an entity, the next `check` re-validates every doc against the new vocabulary. If the linter starts flagging previously-OK terms as `cross-context-reference`, you have three knobs:

1. **Add `synonyms:`** to the new entity so existing prose resolves under the new label.
2. **Add `covers: [<id>]`** to the affected docs so they're declared in-context for the new entity.
3. **Tighten the entity's bounded context** via the axis definition (advanced — read `docs/ontology/axes/`).

## `.doc-lint.toml` — config knobs

Every field is optional. Omitted fields fall back to compiled-in defaults.

```toml
include = ["**/*.md"]
skip_dirs = ["target", ".git", "node_modules"]
exempt = ["**/CLAUDE.md", "**/AGENTS.md", "docs/archive/**"]

cross_repo_roots = ["../sibling-repo-1", "../sibling-repo-2"]

required_fields = ["id", "role", "title", "summary", "status", "updated"]
allowed_statuses = ["draft", "stable", "archived", "deprecated"]
allowed_visibility = ["public", "internal", "private"]

vale_enabled = true
vale_extra_accept = ["Postgres", "JSON", "HTTP"]

[vale_dictionaries]
AWS = "dicts/aws.txt"
Rust = "dicts/rust.txt"

lint_code_comments = false
code_comment_includes = ["src/**/*.rs", "crates/**/src/**/*.rs"]
code_comment_excludes = ["**/tests/**"]
require_anchor_per_pubapi = false
anchor_required_in = []

scip_required = false
clap_crates = ["doc-linter"]
endpoint_marker_exclusive = false

[coverage]
coverage_min_global = 0.0
coverage_min_per_entity_doc_ratio = 0.0
coverage_entity_exempt = []
coverage_function_exempt = []
coverage_endpoint_exempt = []
```

See [error-codes](error-codes.md) for the diagnostic codes each rule emits and the canonical fix for each.

## Performance notes

A cold `--rebuild` on this repo:

- Without `--embeddings`: ~30 seconds. SCIP ingest dominates at ~24s; everything else is sub-second per stage.
- With `--embeddings` (first run, populating all rows): tens of minutes on CPU. The embedding pass runs an ONNX forward pass per row batch; tract's matmul scales to ~3-4 cores on a 384-dim model. Pay this once.
- With `--embeddings` (re-run, nothing changed): near-zero. The hash-skip path recognises unchanged text and reuses the stored vector.

The SQLite store builds with the crate (bundled SQLite, a small C++ vector index via the `cc` crate), so a cold `cargo build` no longer carries the multi-minute graph-database compile it used to.

If your iteration loop hits the embedding cost repeatedly, audit whether you actually need `query similar --backend embedding`. The default BM25 backend reads existing text columns and needs zero embedding setup.

## Common workflows

### Adding a new explanation doc

1. Create `docs/explanations/<slug>.md` with the kind=explanation frontmatter.
2. Save. The PostToolUse hook (or LSP) re-lints.
3. If the linter complains about a missing entity in `covers:`, either drop the entity or author it under `docs/ontology/entities/`.
4. Run `./target/release/doc-linter check` for a final sanity pass.

### Renaming a doc

1. `git mv docs/old/path.md docs/new/path.md`
2. Update the `id:` in the moved file to the new filename stem.
3. `./target/release/doc-linter check` to find inbound `[[old-id]]` wikilinks. Update each one.
4. Re-run `check` until clean.

### Investigating "where is X used / who reads X"

```bash
./target/release/doc-linter query backlinks <doc-id>
./target/release/doc-linter query subgraph <doc-id> --depth 2
./target/release/doc-linter query functions-mentioning <entity-id>
./target/release/doc-linter query similar "<phrase>" --type all --top 10
```

### Closing a coverage gap on a specific crate

1. **Find a low-reach crate.** `doc-linter query coverage-report --crate <name>` shows the per-crate function-reach percentage and the list of dark functions.
2. **Preview scaffolded doc-comments.** `doc-linter scaffold-coverage <name>` runs the SCIP-symbol tokenizer to guess the most-likely entity per dark function; functions with no token match get a TODO stub plus the three closest fallback candidates by mention frequency in the same crate.
3. **Commit any pending work, then apply.** `doc-linter scaffold-coverage <name> --write` writes the `///` doc-comments in place. The writer refuses to run on a dirty git tree as a safety measure.
4. **Hand-fix wrong guesses.** For any function whose autonomous entity guess is wrong, `doc-linter explain <symbol>` walks the graph from that one function and emits the canonical context (entities mentioned, narrative docs covering them, sibling functions in the same crate ranked by entity overlap). Use the surfaced context to pick the right anchor and edit the comment by hand.

### Deciding what to do with a dark function

1. **Author a `///` doc-comment** — the default. The function does real domain work; the comment names the entity it serves. Use `scaffold-coverage <CRATE>` to draft templates.
2. **Add to `coverage_function_exempt`** — when the function is irreducibly generic and has no plausible domain entity link (date parsers, accessors, serde defaults). Use narrow anchored regex patterns (`^parse_iso_` not `^parse_`).
3. **Propose a new entity** — when the function reveals a domain concept the ontology doesn't yet capture. Author `docs/ontology/entities/<slug>.md`, add the function name as a synonym; the symbol-token bridge will link the function on the next `check` without code edits.

Prefer (1) over (2). A doc-comment is permanent; an exempt regex compounds technical debt every time a matching function lands.

## Editor integration

### VS Code

```bash
code --install-extension llllvvuu.vscode-glspc
```

Add to workspace `settings.json`:

```jsonc
{
  "glspc.serverCommand": "doc-linter",
  "glspc.serverCommandArguments": ["lsp"],
  "glspc.languageId": "markdown",
  "glspc.enabledLanguageIds": ["markdown", "rust"]
}
```

### Helix

```toml
[language-server.doc-linter]
command = "doc-linter"
args = ["lsp"]

[[language]]
name = "markdown"
language-servers = ["doc-linter"]
```

### Neovim (nvim-lspconfig)

```lua
require('lspconfig').configs.doc_linter = {
  default_config = {
    cmd = { 'doc-linter', 'lsp' },
    filetypes = { 'markdown' },
    root_dir = require('lspconfig.util').root_pattern('.doc-lint.toml'),
  },
}
require('lspconfig').doc_linter.setup{}
```

## Troubleshooting

**Graph errors after a doc-linter version bump.** Delete the graph; the next `check` rebuilds from scratch.

```bash
rm -rf .doc-lint/graph.sqlite* .doc-lint/graph.vec.* .doc-lint/ingest-cache.sqlite.json
./target/release/doc-linter check --rebuild
```

**LSP server crashes or never starts.** Tail the log:

```bash
tail -n 100 .doc-lint/lsp.log
```

Override the path with `--log-file <PATH>`.

**`check` is slow.** First run rebuilds the graph + ingests SCIP + regenerates the Vale config. Steady-state runs land in 5-10s once the SCIP cache is warm. If you opted into `--embeddings`, see the performance notes above — that's the expensive pass.

**Cross-repo client edits don't lint.** The PostToolUse hook needs `cross_repo_roots` in `.doc-lint.toml` to include the client path. Add the relative path and reload.

**`scip-index` runs out of memory.** Raise the rust-analyzer heap:

```bash
DOC_LINTER_SCIP_MEMORY_MB=8192 ./target/release/doc-linter scip-index
```

## Related

- [error-codes](error-codes.md) — diagnostic catalog generated from [`code_table.rs`](../src/validator/code_table.rs)
- [`.doc-lint.toml`](../.doc-lint.toml) — this repo's live config
- [`docs/ontology/`](ontology/) — the vocabulary the linter enforces
- Crate `README.md` — architectural reference for how the linter is built
