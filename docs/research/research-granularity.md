---
id: research-granularity
role: doc
kind: explanation
lifecycle: stable
covers: [doc-graph]
title: "Entity granularity — research note (issue #5)"
summary: "Decision note from doc-linter issue #5. Keeps the coarse author-curated entity model as the primary ontology surface; routes fine-grained structural communities (the 197 / 49 surfaced by graphify on spaCy / spacy-llm) through the `cluster` subcommand as a promotion queue rather than auto-populating a parallel ontology."
status: stable
updated: 2026-05-30
tags: [research, ontology, cluster]
---

# Entity granularity — research note (issue #5)

**Decision:** keep doc-linter's coarse, author-curated entity model as
the primary ontology surface in v0.4.0. The fine-grained structural
vocabulary graphify surfaces (197 communities in spaCy, 49 in
spacy-llm) is now reachable via the `cluster` subcommand without
requiring a schema change to the Entity table. The cluster output is
a **promotion queue** the author opts into per-candidate, not a
parallel ontology that auto-populates.

This closes #5 for v0.4.0.

## The original question

[#5](https://github.com/AIhiAI/doc-linter/issues/5) opened with an
observation: graphify run on spaCy found 197 structural communities
in a codebase that doc-linter represented with **3 entities**. Same
for spacy-llm — 49 communities vs 3 entities. The biggest gaps were
real architectural clusters (`ShardReducer` map-reduce in spacy-llm,
the multilingual-tokenizer family in spaCy, the Listener / Tok2Vec
ML-layer cluster) that doc-linter's coarse vocabulary couldn't even
name.

The issue proposed a two-tier model:
- **Tier 1 (current):** Coarse domain entities authored by humans.
  High semantic quality, low coverage.
- **Tier 2 (proposed):** Fine-grained structural entities
  auto-derived from graphify communities. High coverage, lower
  semantic quality, would need human promotion.

The downstream-consumer use case explicitly needs Tier 2 — a 3-entity
ontology can't catch meaningful wrong-direction signals against a
5,000-function codebase.

## What v0.3.0 + v0.4.0 actually shipped

The "two-tier" framing turned out to be a false dichotomy: doc-linter
already grew a parallel surface in v0.3.0 + v0.4.0 that delivers
Tier 2 functionality **without** committing the Entity table to a
second representation.

### v0.3.0 — the structural surface that doesn't need new entities

- **#9 CALLS** (Function → Function) — the structural edge graphify
  derives entities from is now first-class in the Kuzu graph. Agents
  asking "what is the structural shape of this codebase" hit the
  call graph directly via Cypher; no Entity intermediary required.
- **#10 ENTITY_CALLS** — derived from CALLS + FUNCTION_BELONGS_TO.
  Surfaces inter-entity structural traffic at the Entity tier
  without requiring authors to maintain a parallel fine-grained
  ontology.
- **#14 cluster diagnostic** — `doc-linter cluster` runs Louvain
  community detection over the call graph and emits **candidate
  entity stubs** (status: `candidate`) to disk. Each candidate
  carries:
  - The god-node SCIP symbol with its in-degree.
  - The community density / modularity score.
  - The top-5 member files.
  - Suggested `source_modules:` globs.
- **#14 v11 (`cluster --promote <id>`)** — the human-in-the-loop
  step. An author reviews a candidate, decides to keep it, and runs
  `cluster --promote spacy-shard-reducer`. The CLI moves the file
  from `candidates/` into `entities/`, flips status from
  `candidate` to `stable`, and from there it's a normal Tier-1
  entity that lints, surfaces in `query list`, gets god-node-flagged
  by #11, etc.

### v0.4.0 — the Type surface that adds another lens

- **#32 v4–v9** — METHOD_OF / USES_TYPE / IMPLEMENTS / EXTENDS,
  plus the Function/Type SPLIT, give agents structural shape **at
  the type level** independent of the entity layer. The
  ShardReducer / Listener / Tok2Vec clusters graphify pointed at
  are all reachable via `MATCH (t:Type)-[:USES_TYPE|IMPLEMENTS]->(t2:Type)`
  without needing fine-grained entities at all.
- **#28 Phase B (embeddings)** — semantic similarity ranking gives
  agents a *retrieval* mechanism for structural concepts that
  doesn't depend on having pre-named entities for them. Asking
  `query similar --backend embedding "long-document sharding"`
  returns the right code regardless of whether `ShardReducer` is
  a named entity.

## Why not auto-promote

Auto-emitting every cluster as a Tier-2 entity would:

1. **Swamp the matcher's signal-to-noise.** doc-linter's pattern
   matcher reads severity classifications off `Entity.is_god_node`
   (#11). 197 entities × N candidate edges per entity would push
   every existing matcher rule into the noise floor on a
   spaCy-sized repo. Authors couldn't tell which entity flagged
   them.
2. **Make every cluster permanent.** Louvain communities shift
   with each meaningful refactor — a new module split, a renamed
   class, an extracted trait. Auto-emitted Tier-2 entities would
   churn on every re-cluster and pollute the diff with phantom
   add/remove pairs.
3. **Violate the `feedback_ontology_organic` rule.** Saved in the
   project's collaboration memory: *don't bulk-create entity
   stubs to silence lint; each entity should come with the
   narrative doc that justifies it*. Auto-promotion is the
   architectural equivalent of that anti-pattern.

The opt-in promotion workflow (`cluster --promote`) sidesteps all
three. Authors see the candidate list, judge each one, and only
real architectural clusters become Tier-1 entities with real
narrative docs.

## Downstream-consumer use case

The original issue said *"A downstream consumer needs Tier 2 for the reference
library to be useful. At 3 entities for a 5,000-function codebase,
the structural diff is too coarse to catch meaningful
wrong-direction signals."*

Under the v0.4.0 surface, a downstream consumer can:

1. Run `doc-linter cluster --top-n 50 --output docs/ontology/entities/candidates/`
   to materialise 50 high-density candidate entities for review.
2. Promote the ones that match real architectural concepts via
   `cluster --promote`. Typical large repos surface 10–30 real
   clusters worth promoting after a 30-minute review; the rest
   are tokenizer-family-style noise.
3. For everything else, hit the structural shape directly via
   `query cypher` / `query similar --backend embedding` /
   `query type-context` (Type-side neighbour walks).

The structural diff is no longer 3 entities deep — it's the full
call graph + the full type-relationship graph + the embedding
vector space, with a human-curated Entity ontology riding on top.

## What we keep open

- **`status: candidate` is a real status value** (not a special
  flag), so future tooling can filter on it.
- **`source_modules:` glob lists** propagate from candidates into
  promoted entities cleanly — no schema migration when a
  candidate ships.
- **The cluster algorithm is pluggable** (`--algorithm lpa|louvain`).
  Adding a third algorithm in v0.5.0 (e.g. Infomap, leiden) is a
  per-clusterer additive change; the promotion workflow doesn't
  care which algorithm produced the candidate.

## When to revisit

v0.5.0 or later if:

- An adopter empirically demonstrates that the
  cluster-promote workflow is too high-friction at scale (e.g.
  3,000 candidates from a monorepo is unreviewable in practice).
  Possible mitigations short of auto-promotion: cluster-of-
  clusters hierarchical promotion, `cluster --auto-promote-above
  <density>` for the very-dense candidates.
- LLM-assisted candidate review proves reliable enough to act as
  the human-in-the-loop step. Then "auto-promote what the LLM
  approves" becomes a sensible halfway position between the
  current opt-in flow and full auto-emission.
- A second adopter reports the v0.4.0 surface still leaves
  meaningful structural concepts unreachable. The Type-edge
  family (`METHOD_OF` / `USES_TYPE` / `IMPLEMENTS` / `EXTENDS`)
  was designed to close most of those gaps; if it doesn't, the
  next move is probably more typed edges (e.g. `DERIVES_FROM` for
  cfg-derived data flow) rather than fine-grained entities.

## Acceptance for the issue

| #5 acceptance criterion | Status |
|-------------------------|--------|
| Identify which concepts graphify surfaces that doc-linter misses | Documented in the issue body (spaCy: Language / Errors / Scorer / registry / Listener; spacy-llm: ShardReducer / FewshotExample / Prompt Cache) |
| Decide: support fine-grained entities? | **No, keep coarse Tier-1 entities** as the authoritative ontology |
| Provide a path for adopters who need finer granularity | **`doc-linter cluster --promote`** + Type-side edge surface + embedding-based retrieval |

Issue #5 closes; the cluster-promotion workflow and the
v0.4.0 type-edge surface together deliver the underlying need
without committing the schema to a parallel fine-grained tier.
