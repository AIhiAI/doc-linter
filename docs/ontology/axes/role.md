---
id: axis-role
role: ontology-axis
title: Axis — role
summary: Top-level classifier on every doc. Says what kind of artifact this is — documentation, index, decision record, etc. Closed vocabulary.
status: stable
updated: 2026-04-30
axis_id: role
required_when: always
multiple: false
open: false
---

# Axis — `role`

Every doc declares a `role:` saying what kind of artifact it is.
Role is the top-level classifier; other axes (kind, lifecycle) only
apply for certain roles.

## How values are added

Closed vocabulary. New roles require:
1. A new value doc under [`../values/role/`](../values/role/)
2. An ontology version bump (edit `ontology_version` in [[ontology-index]])
3. A migration doc under [`../migrations/`](../migrations/) describing
   how existing docs may need to change

## Allowed values

See [`../values/role/`](../values/role/). Starter set:
[[value-role-doc]], [[value-role-index]], [[value-role-adr]],
plus the meta-roles for the ontology itself:
[[value-role-ontology-axis]], [[value-role-ontology-value]],
[[value-role-ontology-entity]], [[value-role-ontology-migration]].

## Cross-axis rules

Some role values mandate other axis fields. For example, `role: doc`
requires a `kind:` (Diátaxis quadrant). The requirement is declared
on each value doc via `requires_axes:`.

## Related

- [[ontology-index]]
- [[axis-kind]]
- [[axis-lifecycle]]
- [[axis-covers]]
