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

[Diátaxis](https://diataxis.fr) by Daniele Procida. Used by Django,
Cloudflare, GitLab, NumPy, FastAPI, and many others.

## Related

- [[ontology-index]]
- [[axis-role]]
- [[value-role-doc]]
