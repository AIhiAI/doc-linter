---
id: axis-kind
role: ontology-axis
title: Axis — kind (Diátaxis)
summary: Diátaxis quadrant. Required when role=doc. Says what the reader needs from this doc — to learn (tutorial), to do (how-to), to look up (reference), or to understand (explanation).
status: stable
updated: 2026-04-30
axis_id: kind
required_when: conditional
multiple: false
open: false
---

# Axis — `kind`

The Diátaxis quadrant. Required when `role: doc`. Optional
(typically omitted) for other roles, since process artifacts (ADRs,
indexes, etc.) aren't user-facing documentation in the Diátaxis
sense.

## Why this matters

Each Diátaxis quadrant serves a different reader intent. Mixing
intents in one doc produces unfocused content. Declaring the kind
forces a single purpose per doc and lets readers find the right
shape of doc for their need.

## Allowed values

See [`../values/kind/`](../values/kind/):

- [[value-kind-tutorial]] — learning by doing
- [[value-kind-how-to]] — solve a specific problem
- [[value-kind-reference]] — look up exact facts
- [[value-kind-explanation]] — understand the why

## Reference

The four kinds follow the [Diátaxis](https://diataxis.fr) documentation
framework.

## Related

- [[ontology-index]]
- [[axis-role]]
- [[value-role-doc]]
