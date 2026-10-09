---
id: value-role-doc
role: ontology-value
title: "Role: doc"
summary: User-facing technical content. Most common role. Requires a Diátaxis kind. Covers playbooks, references, explanations, tutorials, READMEs, etc.
status: stable
updated: 2026-04-30
axis_id: role
value_id: doc
display: Documentation
description: User-facing technical content meant to be read by a human to learn, do, look up, or understand something.
requires_axes: [kind]
default_lifecycle: stable
introduced_in_version: 1
---

# Role: doc

The most common role. Use it for content meant to be read by a human
to learn, do, look up, or understand something. Every `role: doc`
must also declare a `kind:` (see [[axis-kind]]).

## When to use

- A how-to guide explaining a workflow → `role: doc, kind: how-to`
- A README → `role: doc, kind: reference`
- A walkthrough explaining how a system works → `role: doc, kind: explanation`
- A learning-oriented tutorial → `role: doc, kind: tutorial`

## When NOT to use

- The doc records a project decision → [[value-role-adr]]
- The doc only points at other docs → [[value-role-index]]

## Lifecycle

Optional. Defaults to `stable`. Use `lifecycle:` only if you want to
flag the *thing being described* as in-flight (e.g., a feature that
isn't built yet but is already documented).

## Related

- [[axis-role]]
- [[axis-kind]]
- [[value-kind-tutorial]]
- [[value-kind-how-to]]
- [[value-kind-reference]]
- [[value-kind-explanation]]
