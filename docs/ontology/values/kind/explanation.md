---
id: value-kind-explanation
role: ontology-value
title: "Kind: explanation"
summary: Diátaxis quadrant — understanding-oriented. Background, theory, "why does this work the way it does." The reader wants context, not steps.
status: stable
updated: 2026-04-30
axis_id: kind
value_id: explanation
display: Explanation
description: Understanding-oriented. Background, theory, design rationale.
introduced_in_version: 1
---

# Kind: explanation

Diátaxis: **understanding-oriented**. Explanation docs give the
reader context — *why* the system works the way it does, what the
trade-offs are, what other shapes were considered.

## When to write an explanation

- Architecture overview of a subsystem
- "How X works" walkthrough
- Design rationale for a non-obvious choice
- Background reading that frames a topic

## Explanation vs ADR

Explanation = "Here's how this works today, and why."

ADR = "We considered A, B, C and chose B." Specific decision record.

An ADR records one choice. An explanation describes the resulting
shape and may reference several ADRs for the rationale.

## Convention

- Narrative form, written to be read linearly
- Link liberally to ADRs and reference docs for the specifics
- Use diagrams; the reader is here for the gestalt
- Don't include step-by-step procedure — link to a how-to

## Reference

[Diátaxis: explanation](https://diataxis.fr/explanation/).

## Related

- [[axis-kind]]
- [[value-kind-reference]]
- [[value-role-adr]]
