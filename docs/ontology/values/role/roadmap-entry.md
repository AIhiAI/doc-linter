---
id: value-role-roadmap-entry
role: ontology-value
title: "Role: roadmap-entry"
summary: A forward-looking work item. Captures intent, proposed shape, and open questions for something planned, in flight, or considered. Filename must match `^\d+-.*\.md$` so the corpus orders chronologically.
status: stable
updated: 2026-05-30
axis_id: role
value_id: roadmap-entry
display: Roadmap entry
description: A forward-looking work item — proposed feature, planned refactor, considered direction. Lives under `docs/roadmap/` and is sequentially numbered.
requires_axes: [lifecycle]
allowed_lifecycle: [planning, decided, implementing, stable, superseded]
filename_pattern: '^\d+-.*\.md$'
folder: docs/roadmap
introduced_in_version: 2
---

# Role: roadmap-entry

A doc that captures intent for work that's planned, in flight, or considered. Roadmap entries are the place to record "we're thinking about X" or "we're going to do Y" before the implementation exists — the prose can sit at `lifecycle: planning` while the design is debated, move to `implementing` when the work starts, and land at `stable` once shipped.

## When to use

- A proposed feature with non-obvious tradeoffs that needs design discussion before code lands
- A planned refactor or migration where the shape isn't fully decided
- A direction we're considering and want to capture the substance of without committing yet

## When NOT to use

If the work is shipped and the prose is now describing existing behaviour, that's `role: doc, kind: explanation` (or `kind: how-to` for task-oriented procedure docs). Move the file out of `docs/roadmap/` and rewrite as retrospective once the implementation is stable.

If the doc is a recorded design decision with options-considered and chosen-approach, that's `role: adr` — ADRs are append-only audit trail; roadmap entries are mutable while the work is open.

## Convention

- **Filename**: `<n>-<slug>.md`. The number orders the entries chronologically by the date of authoring (not by priority); gaps are fine if a planned entry is dropped.
- **Lifecycle progression**: typically `planning` → `decided` → `implementing` → `stable`. An entry can also land at `superseded` if a different approach took its place.
- **Open questions** belong in the body. Resolved questions are kept inline so the iteration trail stays visible.

## Required fields

- `lifecycle:` must be one of `planning`, `decided`, `implementing`, `stable`, `superseded`.

## Related

- [[axis-role]]
- [[axis-lifecycle]]
- [[value-lifecycle-planning]]
- [[value-lifecycle-implementing]]
