---
id: entity-example
role: ontology-entity
title: "Entity: Example"
summary: Placeholder entity created by `doc-linter init` to demonstrate the entity-doc shape. Replace this with one entity per real domain concept in your codebase.
status: stable
updated: 2026-04-30
axis_id: covers
value_id: example
display: Example
description: Placeholder domain entity. Replace with concepts from your own domain (e.g. user, order, tenant, …).
synonyms: [sample, placeholder]
introduced_in_version: 1
---

# Entity — Example

This is a placeholder entity. It exists so the `covers:` axis has at
least one value out of the box and so this scaffolded vault is
self-coherent for the doc-linter to validate.

## What goes in an entity doc

Each real entity in your domain gets one doc here. The frontmatter
declares the canonical id, display name, description, and any
synonyms the linter should recognise as referring to this entity.

The body answers:

- **What is it?** — a one-paragraph definition of the concept
- **What does it relate to?** — links to other entities it touches
- **Where does it appear in the vault?** — discovery via the linter's
  `query backlinks` subcommand, or a `Dataview` block if you're using
  Obsidian

## How to discover usage

```bash
doc-linter query backlinks entity-example
```

returns every doc that lists `example` in its `covers:` field.

## Replace this doc

Delete this file and add your real entities. A typical starter set
might be one entity doc per concept (user, order, tenant, etc.),
each named `<entity-id>.md` and placed alongside this file.

Each one follows the shape of this doc.

## Related

- [[axis-covers]]
- [[ontology-index]]
