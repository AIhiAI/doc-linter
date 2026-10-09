---
id: value-role-adr
role: ontology-value
title: "Role: adr (Architecture Decision Record)"
summary: A recorded design decision with context, options considered, and chosen approach. Survives the people who made it. Allowed lifecycles are decided and superseded.
status: stable
updated: 2026-04-30
axis_id: role
value_id: adr
display: Architecture Decision Record
description: A recorded design decision with context, options considered, and chosen approach. Survives the people who made it.
requires_axes: [lifecycle]
allowed_lifecycle: [decided, superseded]
introduced_in_version: 1
---

# Role: adr

Architecture Decision Record. A doc that captures *why* we chose one
approach over alternatives. ADRs are append-only — when a decision
is reversed, the new ADR `supersedes:` the old one (which gets
`lifecycle: superseded`); the old ADR is kept for audit trail.

## When to use

- A design choice with non-obvious tradeoffs (caching strategy, store
  selection, schema design)
- A "we considered X, Y, Z and chose Y because…" doc
- A recorded technical compromise

## Required fields

- `lifecycle:` must be `decided` or `superseded`.

## Convention

Body should answer:
1. **Context** — what problem are we solving?
2. **Options considered** — at least two
3. **Decision** — what we chose
4. **Consequences** — what this means going forward

## Related

- [[axis-role]]
- [[axis-lifecycle]]
- [[value-lifecycle-decided]]
- [[value-lifecycle-superseded]]
