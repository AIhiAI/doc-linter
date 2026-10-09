---
id: value-role-index
role: ontology-value
title: "Role: index"
summary: Navigation aid that lists or groups other docs. README files, "what's where" maps, auto-generated catalogs. Not user-facing content itself.
status: stable
updated: 2026-04-30
axis_id: role
value_id: index
display: Index
description: A navigation aid that lists or groups other docs. Often a README at a folder root.
requires_axes: []
default_lifecycle: stable
introduced_in_version: 1
---

# Role: index

A doc whose primary purpose is to point readers at other docs.
README files at a folder root, "what's where" maps, auto-generated
catalogs.

## When to use

- A folder root README that lists what's in the folder
- A topic-index doc grouping related content

## When NOT to use

If the doc has substantive content of its own beyond pointing at
others — that's `role: doc, kind: explanation` or similar.

## Convention

Index docs typically include a list or table of the docs they group.
Keep them focused — pointers in, pointers out, no narrative.

## Related

- [[axis-role]]
- [[ontology-index]] — itself a role: index
