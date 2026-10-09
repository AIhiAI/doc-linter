---
id: axis-covers
role: ontology-axis
title: Axis — covers (domain entities)
summary: Open vocabulary of domain entities a doc touches. New entities can be added without an ontology version bump — drop a new doc in entities/.
status: stable
updated: 2026-04-30
axis_id: covers
required_when: never
multiple: true
open: true
---

# Axis — `covers`

The list of domain entities a doc is about. DDD-style ubiquitous
language: each entity has one canonical glossary doc, and any other
doc that talks about that entity declares it via `covers:`.

```yaml
covers: [doc-graph]
```

## Why open vocabulary

The domain grows. New entities appear (a new concept, a new type of
object). Forcing each addition through a migration would be friction.
Adding an entity here is a regular doc PR — just drop a new file in
[`../entities/`](../entities/).

## Discovery

```bash
doc-linter query backlinks entity-doc-graph
```

## Naming convention

Entity ids are kebab-case singular nouns. The doc id is `entity-<id>`.
Examples: `doc-graph`, `user`, `order`.

## Allowed values

See [`../entities/`](../entities/) for the starter set.

## Related

- [[ontology-index]]
- [[axis-role]]
- [[entity-doc-graph]]
