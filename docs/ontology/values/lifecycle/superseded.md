---
id: value-lifecycle-superseded
role: ontology-value
title: "Lifecycle: superseded"
summary: Replaced by a newer doc or feature. Kept for audit trail; no longer the source of truth.
status: stable
updated: 2026-04-30
axis_id: lifecycle
value_id: superseded
display: Superseded
description: Replaced by a newer doc / feature. Kept for audit trail.
introduced_in_version: 1
---

# Lifecycle: superseded

Terminal state. The thing this doc describes has been replaced by
something newer. The doc itself is preserved for the audit trail —
ADRs especially are append-only; an old decision is marked
superseded rather than deleted.

## When to use

- An ADR whose decision was reversed by a newer ADR
- A doc replaced by a newer version that links back via `supersedes:`
- Any state where the previous shape is preserved but no longer in
  use

## Convention

The successor doc declares `supersedes: <old-id>` in frontmatter.
The superseded doc may declare `superseded_by: <new-id>` symmetrically.

## Related

- [[axis-lifecycle]]
- [[value-role-adr]]
