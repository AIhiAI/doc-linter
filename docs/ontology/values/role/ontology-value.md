---
id: value-role-ontology-value
role: ontology-value
title: "Role: ontology-value (meta)"
summary: Meta-role used by the docs in ontology/values/<axis>/. Defines one allowed value for a closed axis (e.g., role=doc, kind=how-to, lifecycle=stable).
status: stable
updated: 2026-04-30
axis_id: role
value_id: ontology-value
display: Ontology value (meta)
description: Defines one allowed value for a closed-vocabulary classification axis.
requires_axes: []
introduced_in_version: 1
---

# Role: ontology-value

Meta-role. Used by docs in [`../../values/<axis>/`](../../values/) that
define one allowed value for a closed-vocabulary axis. Each value doc
declares the axis it belongs to, the value id, a display name, and a
description.

## Required frontmatter

```yaml
axis_id: <axis>          # role | kind | lifecycle
value_id: <value>        # the literal frontmatter value
display: <human label>
description: <one-line gloss>
```

Optional: `requires_axes`, `allowed_lifecycle`, `default_lifecycle`,
`filename_pattern`, `introduced_in_version`.

## Bootstrap

This meta-role is hardcoded into the doc-linter so the ontology can
describe itself without circular dependency.

## Related

- [[ontology-index]]
- [[axis-role]]
- [[value-role-doc]]
