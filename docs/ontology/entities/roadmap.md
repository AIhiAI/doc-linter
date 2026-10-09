---
id: entity-roadmap
role: ontology-entity
title: "Entity: Roadmap"
summary: A numbered planning doc under `docs/roadmap/` that groups a single body of forward-looking work — the problem, the cluster of issues / PRs that ship it, and the sequencing — into one anchor that both prose and source code can reference.
status: stable
updated: 2026-05-31
axis_id: covers
value_id: roadmap
display: Roadmap
description: "The repo's convention for cluster-scale planning. Each roadmap entry is one `docs/roadmap/<number>-<slug>.md` file with `role: roadmap-entry`, a `lifecycle:` field that tracks the work from `planning` to `stable`, and a stable numeric id (`roadmap-N`) that source-level comments and GitHub issues quote when they're implementing it."
synonyms:
  - roadmap-entry
  - planning-doc
  - roadmap-doc
source_modules:
  # Roadmap entries are doc-anchored, not code-anchored. Code merely
  # *references* them via `Roadmap issue #N` / `roadmap-N` comments;
  # the entity's home is `docs/roadmap/`.
  - docs/roadmap/**/*.md
introduced_in_version: 1
---

# Entity — Roadmap

A roadmap entry is the cluster doc for a body of work that's too large to live as a single PR description and too coherent to scatter across unrelated issues. The canonical example is [[5-mcp-hardening]] — one numbered doc that frames the MCP server review, enumerates the twelve GitHub issues it spawned, recommends the sequencing across roughly eight PRs, and survives as the durable record of *why* the work happened long after the individual PRs have merged. Without the roadmap entry, the same information would be split between a closed issue thread, eight unrelated PR descriptions, and a Slack message no one can find.

The convention is mechanical. Each entry lives at `docs/roadmap/<number>-<slug>.md`, where `<number>` is a monotonically increasing integer that becomes the entry's stable id (`roadmap-5`, `roadmap-43`). Frontmatter declares `role: roadmap-entry`, registered by [[ontology-mig-0002]]. The `lifecycle:` field tracks the *work*, not the prose: `planning` while the entry is being scoped, `decided` once issues are filed, `implementing` while PRs land, `stable` once the cluster is shipped, `superseded` if a follow-up entry replaces it. The doc's own editorial state is the orthogonal `status:` field — a roadmap entry can be `status: stable` (the prose is finished) while still `lifecycle: implementing` (the code isn't).

Source code anchors implementation work back to its planning doc via two comment conventions used pervasively across `src/`. `Roadmap issue #N` quotes the GitHub issue number (see [`src/scip_ingest_cache.rs`](../../../src/scip_ingest_cache.rs) — `Roadmap issue #33 (v0.3.0): SCIP ingest cache.`), and `roadmap-N` quotes the roadmap entry id (see [`src/lsp.rs`](../../../src/lsp.rs) — `Phase 3 of roadmap-43: dark-endpoint and the global`). Both forms are durable: an agent reading the code can trace any module back through the comment to the issue, and through the issue's "tracking" link to the roadmap entry that frames the whole cluster. The numbered-id form is the lighter-weight backlink; the issue-number form is preferred when a single sub-task within a roadmap entry needs to be quoted precisely.

Authoring rule of thumb: if a piece of work spans more than one or two PRs, more than one or two issues, or needs sequencing reasoning that won't fit in an issue description, it wants a roadmap entry. The signal is the cluster — a single PR landing a bug fix doesn't need one; a multi-PR review that produces twelve issues across three priority tiers does. Roadmap entries are also the right home for proposals that haven't been decided yet ([[1-dropin-distribution]] sits at `lifecycle: planning` precisely because the design debate happens *in* the doc).

## Related

- [[axis-covers]]
- [[ontology-mig-0001]]
- [[ontology-mig-0002]]
- [[5-mcp-hardening]]
