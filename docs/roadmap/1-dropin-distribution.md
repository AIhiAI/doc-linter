---
id: 1-dropin-distribution
role: roadmap-entry
lifecycle: planning
title: 1 — Drop-in distribution and ontology bundles
summary: Proposal for how doc-linter installs into a fresh repo with convention-over-config defaults, ships ontology + sitemap + Vale styles as a versioned bundle that joins the local symbol table flat, and resolves wikilinks across local + bundled docs via Wikipedia-style disambiguation when ids collide. Replaces the current `cross_repo_roots` mechanism for the cross-repo case while keeping the standalone case unchanged.
status: draft
updated: 2026-05-30
covers: [doc-graph, roadmap]
tags: [roadmap, distribution, ontology]
---

# 1 — Drop-in distribution and ontology bundles

## Why this exists

doc-linter today is convention-bearing but config-heavy: a fresh `init` writes the ontology skeleton and a starter `.doc-lint.toml`, but cross-repo doc-sharing still goes through hand-edited `cross_repo_roots` paths and there's no published-bundle mechanic. A team running doc-linter across N repos with a shared ontology has to vendor the ontology into each repo or hardcode sibling paths — both fragile.

This entry proposes the drop-in shape: what an opinionated `doc-linter init` writes, the convention-over-config layout, a versioned-bundle distribution mechanism that supersedes `cross_repo_roots` for the multi-repo case, and the resolution model that lets a wikilink like `[[some-entity]]` work transparently whether the target lives locally or in a downloaded bundle.

## The one-sentence model

> **doc-linter ships a convention. `init` writes the convention. Every repo can extend from a parent bundle that's pulled like a versioned dependency. Wikilinks resolve against one flat symbol table that spans local + bundled docs.**

No `cross_repo_roots` for the bundled case. No relative paths between sibling repos. No version markers in wikilink syntax.

## Default skeleton

`doc-linter init` would write this into an empty repo:

```
.
├── ONTOLOGY.md              # roles / kinds / lifecycle / contexts inline
├── .doc-lint.yaml           # opt-out config; absent = pure defaults
├── .doc-lint/               # generated, gitignored: vale styles, vocab cache, bundle cache
├── docs/
│   ├── README.md            # entry doc, frontmatter scaffolded
│   ├── explanations/        # role: doc, kind: explanation
│   ├── how-to/              # role: doc, kind: how-to
│   ├── reference/           # role: doc, kind: reference
│   ├── roadmap/             # role: roadmap-entry
│   └── ontology/            # only created when ONTOLOGY.md outgrows itself
└── .claude/
    └── settings.json        # PostToolUse hook, written if absent
```

The role↔folder mapping IS the convention. A `.md` file outside its expected folder produces a hard `wrong-folder` diagnostic with the suggested path. A `.md` with no `role:` frontmatter is suggested to run `doc-linter scaffold <slug>`.

## Config file shape

`.doc-lint.yaml` at repo root, every key optional:

```yaml
# Inherit from a published bundle. Resolved via doc-linter's own
# resolver; cached under .doc-lint/cache/<bundle>/<version>/.
extends: example-ontology@^1.4

# Override the role↔folder mapping only where it differs from defaults.
layout:
  roles:
    explanation: docs/explanations
    how-to:      docs/how-to

# Where ontology lives. Inline ONTOLOGY.md is the default for small
# repos; split form (docs/ontology/{axes,values}/...) is suggested
# automatically once entity count crosses a threshold.
ontology:
  source: ONTOLOGY.md

# Frontmatter requirements. Defaults to the canonical six.
required_fields: [id, role, title, summary, status, updated]

# Exempt patterns and visibility filters carry forward from the
# current TOML config; semantics unchanged.
exempt: ["archive/**"]
```

YAML over TOML because the schema is now stable enough that human readability beats parser strictness. A repo with a legacy `.doc-lint.toml` is auto-migrated on first run; both formats are accepted indefinitely. **Open question** — TOML works fine today, and the migration churn is real; this is the call we'd want to debate carefully before committing.

## Nested config cascade

A `.doc-lint.yaml` may live in any directory, not only the repo root. When linting a doc, the resolver walks from the doc's directory up to the repo root, layering each config it finds. Closer-to-the-doc overlays apply on top of parents; ontology and rules merge per the table below.

### Skeleton

```
.
├── .doc-lint.yaml             # root: extends bundle, base ontology
├── docs/payments/
│   └── .doc-lint.yaml         # adds payment-specific entities
└── packages/api/
    └── .doc-lint.yaml         # narrows allowed_kinds, adds api entities
```

### Merge rules — child tightens, never loosens

| Field | Child may | Child may NOT |
|---|---|---|
| `ontology.entities` | add new entities + synonyms | override or remove parent entities |
| `allowed_roles`, `allowed_kinds`, `allowed_lifecycles` | narrow (subset of parent) | widen (add values absent from parent) |
| `required_fields` | require more fields | drop a parent-required field |
| `bounded_context` | set explicitly for this subtree | — |
| `exempt` | add patterns | remove parent patterns |

The principle: child configs tighten, never loosen. Root invariants always hold; nesting only adds quality, never erodes it. The same add-only convention governs bundle inheritance — one consistent merge semantics across both axes (bundle → consumer, parent dir → child dir).

### Discovery

`doc-linter ontology --at <path>` prints the effective vocabulary visible to a doc at that path — root config plus every nested overlay walked up from that directory. Without `--at`, the bare command prints the root view.

### Depth limit

Configurable via `max_nesting_depth` in the root config (default unlimited — most repos have at most root + 1–2 levels). A repo that wants to forbid deep nesting sets `max_nesting_depth: 2` and the linter rejects any `.doc-lint.yaml` deeper than that, pointing the author at lifting fields to a parent.

### Bundle interaction

Bundles ship their own nested `.doc-lint.yaml` files (e.g. inside `ontology/payments/`). When a client `extends:` the bundle, those nested configs participate in the same walk, transparently. A client doc under `docs/payments/` sees: root config + bundle root + bundle's payments overlay + client's payments overlay (if any), merged in declaration order with the same tighten-only rules at every layer.

## Bundle distribution

A bundle is a versioned tarball produced from any host repo by `doc-linter publish --tag v1.4.2`. Contents:

```
example-ontology@1.4.2/
├── ontology/                # axes + values; the full vocabulary
├── .doc-lint.yaml           # base config the consumer extends
├── vale/                    # accept-lists, dictionaries, generated styles
├── sitemap.json             # all public docs: id → path → role → kind → context
├── graph.sqlite.zst           # optional: prebuilt graph snapshot
└── docs/                    # optional: source .md for browsable previews
```

### Tier shapes

- **lite** — `ontology/`, `.doc-lint.yaml`, `vale/`, `sitemap.json`. Tens of KB. Enough to validate wikilinks and inherit vocabulary. Most consumers only need this.
- **full** — adds `docs/` and `graph.sqlite.zst`. MB-scale. Enables `query path`, `query backlinks`, and link previews against bundled docs.

### Visibility filter at publish time

Bundles are built by stripping every doc whose `visibility:` isn't in the publish allowlist (default: `public`). The `allowed_visibility` schema field is the gate. The bundle is the **public projection** of the host graph; consumers never see internals.

### Distribution mechanism

Tarball-over-git-tag (or registry). doc-linter's own resolver fetches the bundle for a given version, verifies a hash, caches under `.doc-lint/cache/`. No Cargo coupling at the *consumption* level — keeps the bundle consumable by repos that don't compile Rust.

### Combined release with the host

The bundle ships as part of the same semver release as the host's published artifacts (crates, npm packages, container images — whatever the host actually ships). One git tag, one version, one publish pipeline. A consumer pinning host version `1.4.2` gets the matching docs bundle automatically; doc/artefact drift is impossible by construction.

The full publish pipeline, lockfile shape, update flow, and semver discipline rules belong in a sibling entry to grow as we iterate: [[2-release-pipeline]].

## Resolution model

### One flat symbol table

The bundle's docs join the local namespace as if they'd always been there. The resolver indexes every Doc node — local + bundled — by bare id. `[[some-entity]]` resolves the same way regardless of which side defines it.

Provenance lives **on the node, not on the edge**:

```
(Doc {id: "some-entity", source: "example-ontology@1.4.2", path: "..."})
(Doc {id: "local-policy",  source: "local", path: "docs/..."})

(:Doc {id: "local-policy"})-[:WIKILINK]->(:Doc {id: "some-entity"})
```

One `Doc` node type, one `WIKILINK` edge type. `source` is a node attribute. Cypher consumers can filter by `WHERE doc.source = 'local'` when they want to scope a query to the host repo, but no new edge types or namespacing grammar is introduced.

### Wikilink syntax stays bare

Authors write `[[some-entity]]`. Always. The version pinned in `.doc-lint.yaml` resolves the bundle; the resolver picks the unambiguous winner. Renaming, version bumps, and bundle upgrades surface at lint time as normal broken-wikilink diagnostics — no migration burden hidden in the syntax.

### Collision handling — Wikipedia-style disambiguation

When two docs claim the same bare id (typically: consumer locally defines an id the bundle already publishes), the linter refuses to resolve and offers two paths. The author picks one:

1. **Rename.** The consumer's doc adopts a qualified form like `team-some-entity`. The bundle is immutable to the consumer, so the consumer always renames — the bundle never has to know about the consumer.

2. **Disambiguation doc** (optional, in addition to the rename). The consumer creates a new doc at the bare id `some-entity` with `kind: disambiguation`. Body lists the alternatives. Bare `[[some-entity]]` then resolves to the disambiguation doc, mirroring how Wikipedia's `Mercury` page lists `Mercury (planet)`, `Mercury (element)`, etc.

The lint requirement is **uniqueness only**. The disambiguation doc is a courtesy to humans browsing the bare id, not a mandatory step.

### Pointing into the bundle from a disambiguation doc

This is the single case where authors need to identify a specific source — because the bare id has been claimed by the disambiguation doc itself. The bundle's entity is reachable via path-form wikilink:

```markdown
`[[example-ontology/some-entity]]`
```

This reuses existing wikilink path semantics (Obsidian and standard markdown vaults already treat slashes inside `[[...]]` as paths). No new syntax is invented. The qualified form is **only used inside disambiguation docs**; if it appears elsewhere, the linter raises a soft warning ("you probably meant the bare id").

### What's already in the graph today

Worth being explicit about scope: the resolution model above isn't only about `.md` files. The graph already spans both prose and source via the SCIP ingest — `Function` and `Type` nodes carry `FUNCTION_MENTIONS` edges to ontology entities. The drop-in design extends this naturally: a published bundle can include public-API entries in its sitemap, and a consumer wikilink could resolve into the bundle the same way a doc wikilink does. Out of scope for this entry; flagged here so the "one flat symbol table" framing is understood to span both prose and source going in.

## Migration from the current `cross_repo_roots` model

For repos that use `cross_repo_roots` today:

1. **Build the first bundle**: `doc-linter publish --tag v1.0.0` from the repo that hosts the shared ontology. Verify the produced sitemap covers every public doc.
2. **Each consumer repo gains an `extends:` line**: `extends: <bundle-name>@^1.0`. Their existing `.doc-lint.yaml` (or migrated `.toml`) keeps consumer-specific entities, exemptions, vocabulary additions.
3. **The host repo drops `cross_repo_roots`** once every consumer has migrated. Host stops needing to know consumer paths.

After migration, the host repo lints itself only. Each consumer lints itself only. The graph remains unified at query time because every consumer's resolver pulls the same bundled sitemap.

Standalone repos (no cross-repo doc-sharing) are unaffected. `cross_repo_roots` and the sibling-path model continue to work; `extends:` is opt-in.

## Open questions

1. **YAML migration vs. TOML evolution.** The current `.doc-lint.toml` works. Switching to YAML is real churn for every adopter. Could keep TOML, add `extends:` and the nested cascade on top of it, and skip the format change entirely. Decision deferred.
2. **Bundle granularity.** One bundle per host vs. per bounded context. Per-context grain deferred until a consumer asks; the lockfile model scales to N bundles without consumer-side change if needed later.
3. **Discovery breadth**: should `doc-linter ontology --at <path>` accept a directory glob (e.g. `--at docs/**`) for repo-wide effective-vocab audits, or stay strictly single-path? Single-path is simpler; glob is useful for "show me where the ontology actually differs across the tree."
4. **Stale-bundle behaviour.** Bundle ships with the host's semver release; the consumer's `.doc-lint-lock.yaml` would pin the resolved version atomically. Details belong in [[2-release-pipeline]].

## What this supersedes (if shipped)

- `cross_repo_roots` in `.doc-lint.toml` for the multi-repo case. Becomes obsolete once consumers adopt `extends:`. Standalone repos retain the existing behaviour.

## Out of scope

- The publish-pipeline implementation itself (which CI runner, signing keys, registry choice). That's a follow-up entry — see [[2-release-pipeline]].
- Editor integration for bundle-aware previews (rendering `[[example-ontology/some-entity]]` correctly in Obsidian). Doable later via an Obsidian plugin that reads `.doc-lint/cache/`.
- Breaking-change policy for bundle versions (when a major bump is allowed, deprecation windows). Will need its own entry once we have one bundle and one consumer.

## Related

- [[2-release-pipeline]] — sibling entry covering the combined release model, lockfile shape, publish pipeline, update flow, and semver discipline rules (planned for migration from the private monorepo, #188)
- [[using-doc-linter]] — current operator's guide; will gain an `extends:` and bundle-cache section once this lands
