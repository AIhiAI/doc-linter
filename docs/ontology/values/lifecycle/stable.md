---
id: value-lifecycle-stable
role: ontology-value
title: "Lifecycle: stable"
summary: Built, shipped, in production. The default for documentation that describes how things work today.
status: stable
updated: 2026-04-30
axis_id: lifecycle
value_id: stable
display: Stable
description: Built and working — describes the current shape of the system.
introduced_in_version: 1
---

# Lifecycle: stable

The work is complete, shipped, and in production use. This is the
default lifecycle for `role: doc` content describing how the system
works today.

## When to use

- A README for a shipped feature
- An explanation of a subsystem currently in production
- An ADR whose decision is current and the implementation has shipped

## Difference from `decided`

Decided = "we've chosen, but it isn't built yet"

Stable = "we've chosen, built, and shipped it"

## Related

- [[axis-lifecycle]]
- [[value-lifecycle-implementing]]
- [[value-lifecycle-superseded]]
