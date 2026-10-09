---
id: 5-mcp-hardening
role: roadmap-entry
lifecycle: planning
title: 5 — MCP server hardening
summary: Cluster of fixes, gaps, and polish items surfaced by an unbiased functional review of the `doc-linter mcp` stdio server. Two P1s (broken ego-graph tool, no read-only guard on raw cypher), five P2s (silent failures, inconsistent 404 semantics, a missing source-read tool), and five P3s (additional list tools, pagination, JSON-RPC batches, richer schema introspection, protocol polish). The MCP surface is the contract every downstream agent harness talks to — fixing these before adoption widens is cheaper than fixing them after.
status: draft
updated: 2026-10-08
covers: [doc-graph, roadmap, mcp, sql]
tags: [roadmap, mcp, agent, protocol]
---

# 5 — MCP server hardening

> Update 2026-10-08: the Kuzu database and its `cypher` tool are gone. The raw-query tool is now `sql` (read-only: one `SELECT` / `WITH`, enforced by SQLite itself), so the #206 keyword guard described below no longer exists; the findings are kept as written.

## Why this exists

The `doc-linter mcp` subcommand shipped in v0.3.0 as roadmap-35 and now exposes 17 read-only tools over stdio JSON-RPC. Every downstream agent harness — Claude Code, Claude Desktop, opencode, Cursor — wires the binary in via a `.mcp.json` entry and never looks at the underlying CLI. The MCP shape is now the public contract.

This roadmap entry collects the findings of a deliberately-unbiased review of that contract: a fresh agent session that knew nothing about how the server was built, drove every tool via raw JSON-RPC, and reported what was broken, missing, or surprising. The review's full session log is captured in the linked issues below; this doc frames the cluster and sequences the work.

The single-line motivation: the MCP surface is the agent ergonomics surface. Bad ergonomics here propagate into every downstream harness and become visible as worse agent behavior — extra cypher fallbacks, silent wrong answers, brittle workflows. Fixing them is upstream of every adopter.

## The findings

### P1 — must-fix bugs

Both are silent: the server returns a syntactically-valid response that misrepresents the real state of the graph.

- **[#205 — query_entity returns empty ego-graph despite inbound edges](https://github.com/AIhiAI/doc-linter/issues/205)**. The advertised "entity in one call" tool returns zero nodes and zero edges for `doc-graph` — even though the entity has 979 inbound `FUNCTION_BELONGS_TO`, 10 inbound `COVERS`, and 28 inbound `ENDPOINT_TOUCHES_ENTITY`. Verified on the self-host corpus. BFS walk is either filtering edges or walking the wrong direction.

- **[#206 — cypher tool executes DDL/DML; needs read-only guard](https://github.com/AIhiAI/doc-linter/issues/206)**. Tool description says "Read-only patterns are recommended" but the server executes any Cypher verbatim. An LLM-driven agent can `CREATE`, `DELETE`, `DROP TABLE`. The reproducer in the issue confirms a CREATE landed and survived. Default to read-only, opt-in escape hatch.

### P2 — silent-failure and capability cluster

These don't crash, but they make the MCP unreliable for agent workflows that don't have a human reviewing every response.

- **[#207 — query_endpoints.kind silently accepts bogus values + add entity_id filter](https://github.com/AIhiAI/doc-linter/issues/207)**. Schema describes an enum but the validator doesn't enforce it; bogus value returns `{count: 0}`. Indistinguishable from "no axum endpoints". Plus an agent-ergonomic gap: no way to ask "which endpoints touch this entity?" without raw cypher.

- **[#208 — function_context / query_impact should error on unknown symbol](https://github.com/AIhiAI/doc-linter/issues/208)**. `query_doc` and `query_entity` raise `-32602` on unknown id; `function_context` and `query_impact` return `null`-y shapes. Inconsistent — pick one.

- **[#209 — query_at must validate line >= 1 and signal past-EOF](https://github.com/AIhiAI/doc-linter/issues/209)**. `line: 0` returns partial response; `line: 99999` returns the nearest function silently.

- **[#210 — query_similar backend=embedding silently empty when feature disabled](https://github.com/AIhiAI/doc-linter/issues/210)**. The schema even claims "Errors when the embedder isn't available" but the server returns `{corpus_size: 0, hits: []}`. Agent thinks the search succeeded; really the binary lacks the feature.

- **[#211 — add read_source tool — return code snippet for `<file>:<line>`](https://github.com/AIhiAI/doc-linter/issues/211)**. `query_at` returns graph context but never the source itself. An MCP-only agent has to shell out via Bash to look at the code, defeating the isolation.

### P3 — scaling and protocol polish

Not blocking adoption today; will block scale. Worth landing before the surface stabilizes enough that breaking changes get expensive.

- **[#212 — expose list_modules, list_files, list_migrations](https://github.com/AIhiAI/doc-linter/issues/212)**. Three node tables in the schema with no list tool; agents fall back to cypher to enumerate them.

- **[#213 — pagination via offset/cursor — top alone won't scale](https://github.com/AIhiAI/doc-linter/issues/213)**. Fine on a 36-Doc self-host corpus; blocks adoption on larger repos. Add `offset:` to every list-style tool.

- **[#214 — support JSON-RPC batches (required by MCP 2025-03-26)](https://github.com/AIhiAI/doc-linter/issues/214)**. Today a batched array fails as `-32700`. Required by the next protocol revision.

- **[#215 — enrich query_schema with per-column dtype + table grouping](https://github.com/AIhiAI/doc-linter/issues/215)**. Cold-start endpoint ships table names and row counts but not column metadata — agent has to issue follow-up cypher to learn each table's properties.

- **[#216 — protocol-compliance polish — error codes, negative ints, describe-saved](https://github.com/AIhiAI/doc-linter/issues/216)**. A cluster of small nits: wrong error code on `query_saved` missing-param, silent coercion of negative `top` / `max_hops` / `depth`, no way to describe a saved query without running it, missing `ping` method.

## Sequencing

Recommended order:

1. **P1 cluster** (#205, #206) — independent, both small. Land first; #205 unblocks every agent workflow that relies on `query_entity`, #206 unblocks any deployment where the MCP server is exposed to a less-trusted agent.

2. **P2 silent-failure cluster** (#207, #208, #209, #210). These share a common theme: replace silent empty responses with explicit signals. Cheaper as one PR if the fixes are colocated, but each issue is self-contained.

3. **P2 capability gap** (#211 — `read_source`). New tool, larger surface change, path-traversal safety to think about. Worth its own PR.

4. **P3 scaling** (#212, #213). These are coupled — pagination would ideally land alongside or shortly after the new list tools so the latter ship paginated from day one.

5. **P3 protocol polish** (#214, #215, #216). Independent of each other; pick up in any order, or bundle as a single PR.

## Out of scope (for now)

- **HTTP / SSE transport.** Stdio is what every shipped MCP client speaks today. Adding an HTTP transport is additive but only worth it once there's a cross-host use case.
- **Resources and Prompts capabilities.** The review surfaced these as gaps; they're a separate roadmap entry's worth of design work (which prompts? what does a resource look like for a doc vault? URI scheme?). Track separately if/when there's a concrete adopter ask.
- **Authentication.** Stdio is per-process — no auth needed at the transport layer. If/when an HTTP transport lands, that's where auth design starts.

## Success criteria

The cluster is done when:

- Every issue above is closed.
- A re-run of the unbiased MCP review session produces no new findings in the "bugs" or "silent failures" sections.
- The MCP test suite covers a happy + sad path for every tool (today: coverage is uneven — note that `cargo test --bin doc-linter cmd::mcp::tests` needed `-- --test-threads=1` under Kuzu, an orthogonal pre-existing flake that went away with it).
- `tools/list` description claims match observed behavior (schema honesty).

## Related

- [Roadmap-35 (MCP server v1)](https://github.com/AIhiAI/doc-linter/issues/35) — what shipped.
- [`src/cmd/mcp.rs`](../../src/cmd/mcp.rs) — the whole MCP module; per-issue line refs in each issue body.
- [doc-linter README](../../README.md) — MCP integration section.
