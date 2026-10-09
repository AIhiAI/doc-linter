---
id: value-role-ontology-entity
role: ontology-value
title: "Role: ontology-entity (meta)"
summary: Meta-role used by the docs in ontology/entities/. Defines one domain entity for the open-vocabulary covers axis.
status: stable
updated: 2026-04-30
axis_id: role
value_id: ontology-entity
display: Ontology entity (meta)
description: Defines one domain entity. Open vocabulary — adding a new entity does not require an ontology version bump.
requires_axes: []
introduced_in_version: 1
---

# Role: ontology-entity

Meta-role. Used by docs in [`../../entities/`](../../entities/) that
define one domain entity. Each entity doc declares an id, a display
name, a description, and any synonyms.

## Required frontmatter

```yaml
axis_id: covers
value_id: <entity-id>      # kebab-case singular noun
display: <human label>
description: <one-line gloss>
synonyms: [...]            # optional list of alternate names
```

## Bootstrap

This meta-role is hardcoded into the doc-linter so the ontology can
describe itself without circular dependency.

## Related

- [[ontology-index]]
- [[axis-covers]]
- [[entity-doc-graph]]
