---
id: value-role-ontology-migration
role: ontology-value
title: "Role: ontology-migration (meta)"
summary: Meta-role used by the docs in ontology/migrations/. Records an ontology version bump and the migration that produced it.
status: stable
updated: 2026-04-30
axis_id: role
value_id: ontology-migration
display: Ontology migration (meta)
description: Records a closed-vocabulary version bump — what changed and how existing docs need to migrate.
requires_axes: []
introduced_in_version: 1
---

# Role: ontology-migration

Meta-role. Used by docs in [`../../migrations/`](../../migrations/)
that record an ontology version bump. Each migration doc describes
what changed in the closed vocabulary and how existing docs need to
update.

## Required frontmatter

```yaml
from_version: <N>
to_version: <N+1>
applied: true | false
applied_at: <YYYY-MM-DD>     # optional, when applied: true
affects_query: <description> # optional, e.g. "all docs with role=adr"
```

## Bootstrap

This meta-role is hardcoded into the doc-linter so the ontology can
describe itself without circular dependency.

## Related

- [[ontology-index]]
- [[ontology-mig-0001]]
