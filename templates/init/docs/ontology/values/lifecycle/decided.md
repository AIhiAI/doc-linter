---
id: value-lifecycle-decided
role: ontology-value
title: "Lifecycle: decided"
summary: Approach chosen, recorded in an ADR. Implementation hasn't started or isn't yet stable.
status: stable
updated: 2026-04-30
axis_id: lifecycle
value_id: decided
display: Decided
description: Approach chosen — implementation has not yet started or is in progress.
introduced_in_version: 1
---

# Lifecycle: decided

A choice has been made. The approach is settled; what remains is
implementation. ADRs in this state describe a decision whose path
forward is clear.

## When to use

- An ADR whose decision is current and unmodified
- A roadmap entry whose design is approved but not yet built

## Difference from `implementing`

Decided = "we've chosen, code may or may not be in flight"

Implementing = "we've chosen and we're actively building"

The distinction is loose; many teams collapse the two. Use whichever
your team finds expressive.

## Related

- [[axis-lifecycle]]
- [[value-lifecycle-planning]]
- [[value-lifecycle-implementing]]
- [[value-role-adr]]
