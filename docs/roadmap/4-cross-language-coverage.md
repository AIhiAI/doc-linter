---
id: 4-cross-language-coverage
role: roadmap-entry
lifecycle: planning
title: 4 — Cross-language coverage (TypeScript SCIP + frontend↔backend drift detection)
summary: Extends the Function / Endpoint coverage pipeline beyond Rust by ingesting a SCIP index for TypeScript, adding a new `FRONTEND_CALL` rel that links frontend `fetch('/api/...')` sites to backend `Endpoint` nodes, and shipping a `dark-frontend-call` lint so a renamed backend route surfaces as a CI failure on the frontend's next build. JSDoc/TSDoc vocabulary closure (the Phase 1 piece of the original private-monorepo roadmap-45) already shipped; this entry covers the remaining four phases.
status: draft
updated: 2026-05-30
covers: [doc-graph, roadmap]
tags: [roadmap, typescript, coverage, drift]
---

# 4 — Cross-language coverage

## Why this exists

doc-linter's Function / Endpoint coverage today is Rust-only. The SCIP-Rust ingest populates `Function` nodes and `ENDPOINT_HANDLED_BY` edges, the `dark-public-function` / `dark-endpoint` / `entity-coverage-gap` lints enforce reach, and the `query coverage-report` surface measures progress. Every non-Rust language in any consumer repo — TypeScript, Dart, Python beyond docstrings — is dark from the graph's point of view.

The TypeScript JSDoc/TSDoc comment-lint piece shipped — [`src/code_comments_ts.rs`](../../src/code_comments_ts.rs) walks `.ts` / `.tsx` files with `tree-sitter-typescript`, extracts each `/** … */` block, runs the prose through the same `TermIndex` markdown uses. That solves the prose-coherence problem on the TS side.

What's *not* solved yet — and what this entry proposes — is the structural cross-language piece: ingesting TypeScript symbols into the same Function table, linking frontend API calls to backend Endpoint nodes, and turning frontend↔backend drift into a hard lint failure.

The motivating shape: a frontend `fetch('/api/v1/items/:id')` call becomes a `FRONTEND_CALL` edge to the matching backend `Endpoint(method=GET, path=/api/v1/items/:id)`. When a backend route gets renamed and the frontend isn't updated in the same PR, the linter surfaces a `dark-frontend-call` diagnostic *on the frontend file* the next time `check` runs, before either side ships.

This is the kind of guarantee a monorepo with a strict graph wants. It's *more* than what a regular type-checker can offer because the contract crosses the language boundary.

## What this is

Four phases, each shippable. Phase numbering preserves the original private-monorepo roadmap-45 mapping (Phase 1 was the JSDoc/TSDoc comment lint and shipped; this entry covers 2–5).

### Phase 2 — scip-typescript ingest

[Sourcegraph's scip-typescript](https://github.com/sourcegraph/scip-typescript) produces the same `.scip` protobuf format as `rust-analyzer scip`. The existing SCIP reader should accept it unchanged. The work:

- Add a `language: STRING` column to the existing `Function` node table (`"rust"` / `"typescript"`; defaults to `"rust"` for back-compat).
- New subcommand `doc-linter scip-index --language typescript` detects `scip-typescript` on PATH and invokes it for each configured TypeScript app root, writing to `<root>/.doc-lint/code-typescript.scip`.
- Cross-language symbol tokenization reuses the existing `symbol_tokens` helper — JS/TS identifiers also benefit from snake/camel/kebab tokenization, so `fetchItemDetails` reaches the same entities `fetch_item_details()` would in Rust.

Config additions:

```toml
typescript_apps = ["apps/web"]                      # paths to TS app roots
typescript_skip_dirs = ["node_modules", "dist", "coverage", ".vite"]
```

### Phase 3 — TypeScript endpoint extraction

Three TS-native surfaces become `Endpoint` nodes:

- **3a — Frontend route declarations.** Detection v1: `<Route path="/X" element={<Y/>} />` (react-router) and default exports under `src/pages/` or `src/screens/`. Each becomes `Endpoint(kind="frontend-route", method="GET", path="/X")`. Skipped when no router convention is detected.
- **3b — State-store actions.** Generic over store libraries (Zustand `create<State>()(set => ({...}))`, Redux Toolkit `createSlice({reducers: {...}})`, Pinia/Vue `defineStore('x', () => ({...}))`). Each top-level action becomes `Endpoint(kind="store-action", path="<store>.<action>")`. Brings frontend state surfaces under the same coverage rules as HTTP routes.
- **3c — MCP client wrappers, frontend side.** Any `callTool('<name>', ...)` site becomes an outbound `FRONTEND_MCP_CALL` edge to the corresponding `Endpoint(kind="mcp", path="<name>")`. Mirrors Phase 4's `FRONTEND_CALL` but for the MCP surface — same drift-detection contract.

### Phase 4 — `FRONTEND_CALL` edges + URL pattern matching

The headline cross-language feature. Walk every TS function body for `fetch(<expr>)`, `axios.<method>(<expr>)`, plus the common library wrappers (`ky.get`, `got.post`, etc.) via tree-sitter-typescript. Extract the URL literal (best-effort), resolve against the existing `Endpoint` node set.

The resolver:

- **Static string literals** → high-confidence edge. `fetch('/api/v1/items')` resolves to `Endpoint(path=/api/v1/items)` unambiguously.
- **Template literals with simple interpolation** → resolved by pattern shape. `` fetch(`${baseUrl}/api/v1/items/${id}`) `` resolves to `Endpoint(path=/api/v1/items/:id)` (the `${id}` token matches the `:id` path param).
- **Computed URLs that involve string concatenation across function boundaries** → emit a `low-confidence` edge with the best-effort match, surfaced as a separate `query call-sites --confidence low` view. Only `high` count toward `dark-frontend-call` enforcement (Phase 5).

A new `query frontend-calls --dark` lists every `fetch(...)` site whose URL doesn't match any known Endpoint — the precise input to the Phase 5 lint.

### Phase 5 — `dark-frontend-call` lint

Three new `Issue` variants. Per-app rampable via a new `typescript_anchor_required_in: Vec<String>` knob mirroring `anchor_required_in` for Rust crates.

- **`dark-frontend-call`** (HARD ERROR): a `fetch('/api/...')` call whose URL pattern doesn't match any known `Endpoint`. Surfaces TS↔backend drift. Only fires on high-confidence FRONTEND_CALL edges.
- **`dark-typescript-function`** (config-gated, warn by default): TS export with no JSDoc and no entity-symbol match — TS analogue of `dark-public-function`.
- **`unmounted-frontend-component`** (warn): a default-export React component in a recognized component path that has zero references in the routing layer. Surfaces dead pages.

The existing `--coverage-min-global` flag operates over `(Rust + TS functions reaching entity) / total` once Phase 2 lands. New flags `--coverage-min-rust=N` and `--coverage-min-typescript=N` let CI gate per-language thresholds independently while the cross-language story matures.

## Success metrics

| Metric | Today | Phase 2 target | Phase 5 target |
|---|---:|---|---|
| TS function nodes in graph | 0 | first app's TS functions ingested | same + any additional apps |
| TS function reach % via FUNCTION_MENTIONS | 0% | 40–50% (same heuristic as Rust Phase 1) | 80%+ |
| FRONTEND_CALL high-confidence edges | 0 | 100% of static-literal frontend calls | 100% with template-literal coverage |
| `dark-frontend-call` diagnostics in CI | n/a | n/a | exactly 0 (hard fail) |

## Risks

1. **scip-typescript installation friction.** Not part of standard Node toolchains — requires `npm install -g @sourcegraph/scip-typescript`. Mitigation: ingest is opportunistic (same pattern as scip-rust) — missing binary emits a single `scip-indexer-missing` diagnostic and the linter degrades to comment-only TS coverage without failing CI. The hard `dark-frontend-call` gate has a clean opt-in story via `typescript_anchor_required_in`.
2. **URL-literal matching is fuzzy.** A call like `` fetch(`${baseUrl}${path}`) `` resolves to no usable pattern. Mitigation: emit `low-confidence` edges for interpolated URLs, `high-confidence` only for static or single-template-interpolation URLs; only `high` count toward `dark-frontend-call`. Surface low-confidence via `query call-sites --confidence low` for spot-checking.
3. **JSX text vs developer-vocabulary ambiguity.** JSX text is excluded from vocab closure by default (UX copy belongs to i18n, not doc-linter). But internal-tool JSX may carry developer-vocabulary in labels. Mitigation: extension knob `typescript_lint_jsx_text_in: Vec<String>` to opt specific dirs back in. Default empty.
4. **Vite dev server / hot-reload conflicts with the SCIP indexer.** scip-typescript runs `tsc`, which can race with Vite's compilation pipeline. Mitigation: run scip-index only via the `doc-linter scip-index --language typescript` subcommand, never as a side-effect of `check`. CI invokes it explicitly between dependency-install and the lint step.
5. **State-store extraction is convention-driven.** Phase 3b assumes consistent store-hook naming. Risk is theoretical until a repo has multiple stores with inconsistent naming. Mitigation: skip the check when fewer than 2 stores exist.

## Open questions

- **Source-only vs build-output ingest.** Vite's tree-shaking can elide whole modules from the prod bundle; SCIP indexes source. For drift detection, source is correct (the calls that *could* run, not just the ones that *do*). Any analysis that needs the post-build artifact is out of scope.
- **Other languages.** Dart (Flutter) and Python (beyond docstrings) follow the same per-language plug-in pattern this entry establishes. Out of scope here; each gets its own roadmap entry once an adopter asks.
- **HTTP framework support beyond axum.** The existing endpoint extractor handles axum. actix-web / warp / rocket would need analogous extractors. Defer until a real consumer needs one.

## What this depends on

- [[1-dropin-distribution]] is unrelated. This entry stands alone and can ship without bundle distribution.

## Out of scope

- Storybook / MDX support. Defer until a real adopter has these surfaces and asks.
- Editor previews of cross-language edges (showing the backend Endpoint when hovering a frontend `fetch` call). Doable via the LSP later.
- The post-build bundle artefact — only the source tree is in scope.

## Related

- [[using-doc-linter]] — operator guide; will gain a TypeScript-app section once Phase 2 lands
- [[3-agentic-autodoc]] — independent; the autodoc waves don't depend on cross-language coverage but would benefit from the wider Function set once it lands
