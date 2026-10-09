---
id: ontology-mig-0002
role: ontology-migration
title: "Migration 0002 — Roadmap-entry role"
summary: Registers the `roadmap-entry` role so forward-looking work items can land in `docs/roadmap/` with a typed lifecycle. Required to migrate the planning + roadmap docs inherited from the project's earlier private origin.
status: stable
updated: 2026-05-30
from_version: 1
to_version: 2
applied: true
applied_at: 2026-05-30
affects_query: "every doc with role: roadmap-entry"
---

# Migration 0002 — Roadmap-entry role

Adds a new role value so forward-looking work items can declare themselves typed instead of borrowing `role: doc, kind: explanation`. The role was already referenced in source (`filename-pattern-mismatch` mentions roadmap-entry, `cmd::query::list` documents `roadmap-entry` as a filter option) but no value doc existed; this migration closes that gap.

## What changes

- New value [[value-role-roadmap-entry]] under [`../values/role/`](../values/role/).
- New folder `docs/roadmap/` for the entries themselves. Filename must match `^\d+-.*\.md$`.
- Allowed lifecycles: `planning`, `decided`, `implementing`, `stable`, `superseded`.

## Why now

The legacy FA monorepo carried 12 docs under `docs/roadmap/` and `docs/planning/` that need a home in this repo. None of them resolved against the v1 vocabulary because v1 only registered `doc`, `index`, `adr`, plus the meta-roles. Bumping to v2 lets the forward-looking subset of those migrations land as first-class typed nodes rather than as ad-hoc explanation docs.

## Effect on existing docs

None. No v1 doc carries `role: roadmap-entry` today; the migration is additive.

## Related

- [[ontology-index]]
- [[value-role-roadmap-entry]]
- [[ontology-mig-0001]]
