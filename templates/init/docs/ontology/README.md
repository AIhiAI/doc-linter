---
id: ontology-index
role: index
title: Ontology — Documentation Taxonomy
summary: Index of this repo's documentation ontology. Defines the axes (role, kind, lifecycle, covers), their values, the domain entities, and the migration history. The doc-linter loads this directory to validate every other doc in the vault.
status: stable
updated: 2026-04-30
ontology_version: 1
tags: [docs, navigation, ontology]
---

# Ontology — Documentation Taxonomy

Every doc in the vault declares what kind of artifact it is, how it's
written, and what domain entities it touches. Those declarations live
in YAML frontmatter. This directory defines the *vocabulary* — what
values are allowed, what they mean, and how the doc-linter enforces
them.

## The four axes

| Axis | Required | Vocabulary | Purpose |
|---|---|---|---|
| [[axis-role]]      | always        | closed | What kind of artifact is this? (doc, index, adr, …) |
| [[axis-kind]]      | when role=doc | closed | Diátaxis quadrant — what does the reader need? |
| [[axis-lifecycle]] | conditional   | closed | Where in the project lifecycle is the work this doc describes? |
| [[axis-covers]]    | optional      | open   | Which domain entities does this doc touch? |

**Closed** vocabularies (role, kind, lifecycle) require a migration
doc + version bump to add a value. **Open** vocabularies (covers)
take new entries via a regular doc PR — no version bump needed.

## How to add a doc

Minimum frontmatter for a regular doc:

```yaml
---
id: my-doc
role: doc                  # see axis-role
kind: how-to               # required because role: doc
lifecycle: stable          # optional; defaults to stable for role: doc
covers: [example]          # optional; entity ids from entities/
title: ...
summary: ...
status: stable             # editorial state of THIS doc (separate from lifecycle)
updated: 2026-04-30
---
```

`status:` (`draft` / `stable` / `archived` / `deprecated`) is the
editorial state of the doc itself — is the *prose* finished.
`lifecycle:` is the state of the *thing the doc is about* — is the
*work* done. They're orthogonal.

## How to add a value

- **New entity** (open vocabulary, no version bump): drop a new file
  in [`entities/`](entities/). Follow the shape of [[entity-example]].
- **New role / kind / lifecycle value** (closed vocabulary): bump
  `ontology_version`, add a value doc under `values/<axis>/`, write
  a migration doc under `migrations/`.

## Discovery via the linter

```bash
doc-linter ontology                          # axes + their value sets as JSON
doc-linter check                             # validate every doc against the ontology
doc-linter query backlinks entity-example    # all docs that cover Example
```

## Related

- [[ontology-mig-0001]] — the bootstrap migration introducing this v1
- [[entity-example]] — example entity stub
