---
id: value-role-ontology-axis
role: ontology-value
title: "Role: ontology-axis (meta)"
summary: Meta-role used by the docs in ontology/axes/. Defines one classification axis (role, kind, lifecycle, covers).
status: stable
updated: 2026-04-30
axis_id: role
value_id: ontology-axis
display: Ontology axis (meta)
description: Defines one classification axis used by the doc-linter to validate other docs.
requires_axes: []
introduced_in_version: 1
---

# Role: ontology-axis

Meta-role. Used by docs in [`../../axes/`](../../axes/) that define
one classification axis. Each axis doc declares the axis id, whether
it's required (always / conditionally / never), whether it accepts
multiple values, and whether the vocabulary is open or closed.

## Required frontmatter

```yaml
axis_id: <id>           # role | kind | lifecycle | covers
required_when: always | conditional | never
multiple: true | false
open: true | false
```

## Bootstrap

This meta-role is hardcoded into the doc-linter so that the
ontology can describe itself without circular dependency.

## Related

- [[ontology-index]]
- [[axis-role]]
