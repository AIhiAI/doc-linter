---
id: axis-lifecycle
role: ontology-axis
title: Axis — lifecycle
summary: Where in the project / feature lifecycle the work this doc describes currently sits. Required for ADRs and similar process artifacts; optional for plain docs.
status: stable
updated: 2026-04-30
axis_id: lifecycle
required_when: conditional
multiple: false
open: false
---

# Axis — `lifecycle`

The state of the *work* this doc describes. Distinct from the doc's
own editorial state (`status:`).

An ADR that's `lifecycle: superseded` describes a decision that was
later replaced. A `role: doc` that's `lifecycle: stable` describes a
shipped, working feature.

## When it's required

Required when `role:` is one of:
- `adr` (decisions have a state — decided, superseded)

Optional (typically omitted, defaults to `stable`) for `role: doc` —
documentation usually describes things in their current shape.

## Allowed values

Ordered earlier → later, with `superseded` as a terminal exit:

- [[value-lifecycle-planning]] — exploration; no chosen approach yet
- [[value-lifecycle-decided]] — approach chosen, not yet built
- [[value-lifecycle-implementing]] — being built
- [[value-lifecycle-stable]] — built and working
- [[value-lifecycle-superseded]] — replaced by a newer doc/feature

## Related

- [[ontology-index]]
- [[axis-role]]
- [[value-role-adr]]
