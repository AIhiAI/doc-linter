---
id: research-confidence-taxonomy
role: doc
kind: explanation
lifecycle: stable
covers: [doc-graph]
title: "Confidence taxonomy — research note (issue #40)"
summary: "Decision note from doc-linter issue #40. Keeps the v0.3.0 high/low two-tier confidence on FUNCTION_MENTIONS edges; revisits the three-band EXTRACTED / INFERRED / AMBIGUOUS taxonomy in v0.4.0 once two-tier produces wrong calls on a real adopting repo."
status: stable
updated: 2026-05-30
tags: [research, confidence, function-mentions]
---

# Confidence taxonomy — research note (issue #40)

**Decision:** keep the v0.3.0 `high` / `low` two-tier confidence on
`FUNCTION_MENTIONS`. Don't migrate to graphify's three-tier
`EXTRACTED` / `INFERRED` / `AMBIGUOUS` taxonomy in v0.3.0; revisit
in v0.4.0 once we have evidence of two-tier producing wrong calls
on a real adopting repo.

## Background

`FUNCTION_MENTIONS` carries `confidence: "high" | "low"` today.
Issue #40 proposes adopting graphify's three-band taxonomy as a
universal edge-confidence axis:

| graphify band | what it means                                  | score    |
|---------------|------------------------------------------------|----------|
| `EXTRACTED`   | edge is explicit in source                     | 1.0      |
| `INFERRED`    | reasonable inference                            | 0.6–0.9  |
| `AMBIGUOUS`   | flagged for review, kept but called out         | 0.1–0.3  |

Plus a numeric `confidence_score` 0.0–1.0 for finer ranking.

Migration would map existing `high` → `EXTRACTED`, `low` →
`INFERRED`, leave `AMBIGUOUS` empty until an inference layer
populates it.

## Why defer

Three reasons to keep the two-tier system in v0.3.0:

1. **The two tiers already separate the cases that matter.** Pre-#23,
   the failure mode was *project-name false positives* — every
   doc-comment mentioning the repo's own name fired a `high`-
   confidence edge. The fix (#23 self-match demote) didn't need a
   third tier; it demoted the noisy class to `low` and dropped
   it when a more-specific entity also matched. The dichotomy
   "did the doc-comment explicitly name this concept" vs "did
   the symbol path coincidentally tokenise to it" maps cleanly
   to high/low.

2. **The new edge tables added in v0.3.0 don't reuse the same axis.**
   - `CALLS` (#9): no confidence column — every edge is
     extracted from a SCIP reference occurrence, all are
     `EXTRACTED`-equivalent.
   - `COUPLED_WITH` (#38): carries `commits` + `jaccard`. A
     continuous score, not a tier. Mapping it back to
     three tiers would lose information.
   - `IMPORTS` (#16): every edge is extracted from a SCIP
     Import role — all `EXTRACTED`.
   - `ENTITY_CALLS` (#10): `frequency` + `weight` (continuous).
   - `RELATES_TO` (#15): `weight` + `source` columns. `source`
     already distinguishes `"frontmatter"` (extracted) vs the
     reserved-for-derivation `"derived"` / `"inferred"` tokens.
   - `FUNCTION_BELONGS_TO`: no confidence; every edge is a
     glob-match on author-declared `source_modules:`.
   - `TEST_FOR` (#24): two-tier `high` / `low` mirroring
     FUNCTION_MENTIONS. Same design rationale as that edge.

   So the only edges using the tier today are FUNCTION_MENTIONS
   and TEST_FOR. The proposed universal axis would apply to
   exactly two tables — which doesn't deliver enough cross-table
   uniformity to justify the migration cost.

3. **The matcher's existing severity classifier consumes the
   two-tier signal cleanly via `Entity.is_god_node` (#11).**
   God-node detection picks up the architectural-importance
   signal; the per-edge tier picks up the textual-evidence
   signal. Adding a middle tier (`INFERRED`) muddies which axis
   the matcher should branch on.

## When to revisit

In v0.4.0 if any of the following lands:

- **#14 cluster diagnostic** — Louvain communities emit
  candidate entities with a confidence score (community density).
  That's a natural fit for a numeric `confidence_score` column,
  and it'd be the first edge type that genuinely wants three
  bands (high-density cluster vs low-density vs flagged).
- **A second inference pipeline** producing edges (e.g. semantic-
  similarity-based ENTITY_CALLS refinement, or LLM-inferred
  RELATES_TO suggestions). Two inference sources is the point
  where "is this edge extracted, inferred, or ambiguous?" stops
  being a one-bit answer.
- **Real reports** from adopting repos that high/low is producing
  wrong matcher calls. The example-app self-match case #23 was
  fixable without the three-tier promotion; another case might
  not be.

## What we keep open

Two columns make the future migration cheap:

- `RELATES_TO.source` already carries the `"frontmatter"` /
  `"derived"` / `"inferred"` tokens — the third tier is reserved
  here even though it's unused in v0.3.0.
- `ConfidenceTier` enum (`crate::kuzu_graph::projections`) is
  `#[non_exhaustive]`, so adding a `Ambiguous` variant is
  backward-compatible — match arms in downstream consumers will
  get a compile warning rather than silently dropping the new
  variant.

## Acceptance for the issue

| Issue acceptance criterion | Status |
|---|---|
| "Decide: adopt or stay with high/low" | **Stay with high/low**, see v0.4.0 revisit below for the closed-out rationale |
| "If adopted, plan the rollout across edge tables" | N/A — not adopted |

## v0.4.0 revisit (closes #40)

The v0.3.0 note left the door open for a re-decision in v0.4.0 if the
cluster diagnostic, a second inference pipeline, or real
adopter-reported false matches surfaced. v0.4.0 lands all three of
the named triggers, so we have evidence to re-decide rather than
defer again.

**Decision (v0.4.0): keep two-tier `high` / `low`.** The
graphify-style three-tier `EXTRACTED` / `INFERRED` / `AMBIGUOUS`
taxonomy still doesn't fit doc-linter's shape post-v0.4.0. Reasons,
updated for the new edge / score surface:

### What v0.4.0 actually added

| Surface | Confidence shape | Why it doesn't want 3-tier |
|---------|------------------|----------------------------|
| `METHOD_OF` (#32 v4) | None — every edge is derived from SCIP symbol shape | All `EXTRACTED`-equivalent. A `confidence` column would be 100% `high` and add zero signal. |
| `USES_TYPE` (#32 v5) | None — same SCIP-derivation argument | All `EXTRACTED`-equivalent. |
| `IMPLEMENTS` (#32 v6) | None — `SymbolInformation.relationships.is_implementation: true` is authoritative | All `EXTRACTED`-equivalent. |
| `EXTENDS` (#32 v8) | None — classifier rule is exact, not heuristic | All `EXTRACTED`-equivalent. |
| `TYPE_DEFINED_IN` / `TYPE_BELONGS_TO` (#32 v9) | None — mirror of FUNCTION_* | All `EXTRACTED`-equivalent. |
| `TYPE_MENTIONS` (#32 v9) | Inherits FUNCTION_MENTIONS' two-tier `high` / `low` | Same axis as FUNCTION_MENTIONS — already a two-tier signal that maps cleanly. |
| Embedding cosine score (#28 v4) | Continuous `[-1, 1]` value on each `query similar` hit | Not an edge column. Stored on the FLOAT[384] vector itself; the ranker computes cosine at query time. Three tiers would discard precision. |
| `query similar --min-score` (#28 v7) | Continuous threshold the caller sets | Same — continuous. |

### What the cluster diagnostic actually emits

#14 v11's `cluster --promote` flow writes candidate entity stubs
with a numeric `confidence` field driven by community density (or
modularity score for Louvain). That number lives in the
**Entity** frontmatter, not on an edge — so it doesn't reuse the
edge-confidence axis we're deciding about. Density-based scoring
of candidates is its own first-class signal.

### What about the embeddings layer

#28's semantic-search backend was the most plausible candidate for
introducing an `INFERRED` tier — "this edge was suggested by cosine
similarity, not declared in code". But the v0.4.0 implementation
landed the embedding signal as a **query-time** ranker
(`query similar --backend embedding`), not as an edge-emitting
ingest pass. There is no `FUNCTION_MENTIONS_EMBEDDED` (or
similar) table that needs a tier. So the trigger named in the
v0.3.0 doc — "a second inference pipeline producing edges" —
didn't actually materialise.

### What we keep open

The hooks the v0.3.0 doc reserved still hold; nothing in v0.4.0
narrowed them:

- `RELATES_TO.source` carries `"frontmatter"` / `"derived"` /
  `"inferred"` tokens. The third token is still unused but the
  column accepts it.
- `ConfidenceTier` is still `#[non_exhaustive]`. Adding an
  `Ambiguous` variant remains backward-compatible — match arms
  in downstream consumers get a compile warning rather than a
  silent drop.
- The dim choice for the embedding column (384, fixed for
  MiniLM) leaves room for a per-row `embedding_confidence`
  numeric if a future "this row's text was too short to embed
  meaningfully" signal becomes useful. v0.5.0 territory.

### When to revisit next

v0.5.0 if:

- An LLM-inferred edge type lands (the kind of inference where the
  source is "model said so" not "rule fired"). That edge would
  legitimately benefit from a third tier the v0.3.0 / v0.4.0 edge
  set didn't have a use for.
- A second adopter reports the two-tier system producing wrong
  matcher calls. The v0.3.0 self-match case was fixable without
  promoting to three tiers; v0.4.0's adopter cohort might surface
  a different shape.
- `RELATES_TO.source = "inferred"` actually gets populated by
  something. Today nothing writes that value.

### Acceptance for the v0.4.0 close-out

| Trigger from v0.3.0 doc | What landed in v0.4.0 |
|-------------------------|------------------------|
| #14 cluster diagnostic with density score | Landed (#14 v11) — density lives on Entity frontmatter, not edges. Doesn't drive an edge-tier rethink. |
| Second inference pipeline producing edges | Did NOT land — embeddings ship as a query-time ranker, not an edge emitter. |
| Real reports of two-tier producing wrong calls | None received during the v0.4.0 cycle. |

Issue #40 closes with "kept two-tier on edges, will re-open in
v0.5.0 if any of the three triggers above materialise."
