---
id: 3-agentic-autodoc
role: roadmap-entry
lifecycle: planning
title: 3 — Agentic autodoc for dark functions
summary: Use an agent to write the `///` doc-comments that close the remaining function-reach gap after the structural-bridge + scaffold-coverage path has done its work. Three waves — deterministic high-confidence backfill, source-aware single-function generation, ontology-expansion proposals for the long tail. Carved out of the original roadmap-43 (Phases 0–4 of which shipped) so this remaining piece can be discussed in isolation.
status: draft
updated: 2026-05-30
covers: [doc-graph, roadmap]
tags: [roadmap, agent, coverage, ontology]
---

# 3 — Agentic autodoc for dark functions

## Why this exists

Roadmap-43's Phases 0–4 (measure → structural-bridge → endpoint-extraction → enforcement lints → `scaffold-coverage` + `explain`) shipped. The end state today is: every dark function is enumerated, the linter can flag it, and `scaffold-coverage` proposes templated doc-comments per crate. The remaining gap is whoever has to actually write the prose into each TODO stub.

The original roadmap-43 assumed human authors would fill those in. For most adopters that doesn't scale — closing 500–2,000 dark functions by hand is the kind of work that gets started, dropped, and never finished. This entry proposes that an agent does it, with the linter as the verification check (the `FUNCTION_MENTIONS` edge fires → the function is no longer dark) rather than human review.

## The three waves

Each wave is gated on the previous landing. The lower-confidence waves only run on functions the higher-confidence ones couldn't touch.

### Wave 3a — Deterministic backfill (no LLM)

Walks every dark `Function` whose `symbol_tokens` produce a high-confidence entity match (the same matcher the existing high/low-confidence ingest dispatch uses). For each, writes a fixed-shape doc-comment that wikilinks the inferred entity:

```rust
/// Operates on [[entity-<id>]]. See `entity-<id>.md` for the canonical
/// definition. (auto-generated, roadmap-3-wave-a)
```

Zero hallucination risk — only writes when the entity match is already there in the function name, and the comment is structurally fixed. Closes ~30–50% of the dark functions on its own without invoking an LLM at all. The "auto-generated" marker in the comment lets a later pass (or a human) detect and rewrite these into hand-authored prose if it ever matters.

Open question: should the marker be machine-parseable (e.g. `// @doc-linter:auto-generated`) so a later "re-author the auto-generated comments" pass can target them precisely? Probably yes.

### Wave 3b — Source-aware single-function autodoc

For dark functions Wave 3a didn't cover, an agent reads the function's body, the ontology, and the crate README, picks the best entity (or returns `NONE`), writes a 1–2 sentence doc-comment with the right wikilink. Per-crate batching to amortize the ontology context: one agent invocation per crate, sequential to respect single-writer caps on the linter's DB.

The agent never *invents* an entity in Wave 3b. It only picks from the ontology that already exists. Returning `NONE` means "I read the function and none of the existing entities fit." Those functions stay dark and surface in a per-crate "needs ontology expansion" report — the input to Wave 3c.

Verification is the linter run after the write: if the new doc-comment doesn't produce a `FUNCTION_MENTIONS` edge to the wikilinked entity, the write is rejected and the function returns to the queue.

### Wave 3c — Ontology expansion + exempt list

Wave 3b's `NONE` returns split two ways:

- **Genuine new domain concepts.** The agent proposes new `entity-X.md` docs under `docs/ontology/entities/` for each. Each proposal includes the function(s) that surfaced the concept, a one-sentence definition drawn from the source, suggested synonyms, and the closest-existing-entities list (so a reviewer can decide "is this really new, or a synonym of `[[entity-existing]]`"). Reviewer-gated — the agent doesn't auto-merge ontology changes. Once an entity lands, Wave 3b re-runs for the affected crates.

- **Irreducibly generic utilities** (`parse_iso_8601`, `truncate_to_n_chars`, calamine cell accessors, serde defaults). Added to `coverage_function_exempt` with narrow anchored regex patterns. Already shipped as a config knob; the agent's contribution is proposing the regex + the explanation comment. Reviewer-gated to avoid silently exempting real domain code.

## Surface shape

The waves run as `doc-linter` subcommands:

```bash
doc-linter autodoc --crate <name> --wave a            # deterministic, no LLM
doc-linter autodoc --crate <name> --wave b            # source-aware, requires LLM
doc-linter autodoc --crate <name> --wave c --propose  # ontology + exempt proposals
```

Each writes to the git tree and refuses to run on a dirty working tree (same safety as `scaffold-coverage --write`). The default is dry-run; `--write` applies. The LLM dependency is opt-in — Wave 3a runs without any model.

Wave 3b's LLM interaction stays narrow: input is `(function source, ontology snapshot, crate README)`, output is `(entity-id | NONE, one-or-two-sentence doc-comment)`. No tool use, no multi-turn — the prompt is sized for fast deterministic completion.

## Success metrics

| Wave | Dark functions closed (target) | Author intervention |
|---|---:|---|
| 3a | ~30–50% of remaining | none |
| 3b | ~80–90% of post-3a remaining | review only |
| 3c (entity proposals) | adds entities for the residual | reviewer approves entity PRs |
| 3c (exempt proposals) | adds regex exemptions for the residual | reviewer approves config PRs |

The "is this working" check is just: `doc-linter query coverage-report --crate <name>` before vs after. If function-reach percentage moves and the lint pass stays clean, the wave did its job.

## Risks

1. **Hallucinated entity references.** The biggest risk in Wave 3b — the agent writes ``[[entity-foo]]`` but `foo` doesn't exist, or writes prose that mentions an entity it didn't wikilink. Mitigation: the post-write linter run is the verification check; rejected writes go back in the queue. The agent never gets credit for a write that doesn't move the `FUNCTION_MENTIONS` count.
2. **Wave 3a over-marks "auto-generated" prose as if it were thoughtful documentation.** The function-reach metric goes up but the prose is uninformative ("Operates on `[[entity-foo]]`"). Mitigation: the marker comment is the disclosure. A later pass can target only `roadmap-3-wave-a` comments for human rewrite, and the reach metric remains accurate (it measures graph connectivity, not prose quality).
3. **Wave 3c entity proposals create ontology bloat.** An overeager agent splits one concept into three near-synonyms. Mitigation: reviewer gate is hard. The agent's role is to *propose*; a human has to accept. The "closest existing entities" list in each proposal makes the duplicate-vs-distinct judgment cheap.
4. **LLM cost.** Wave 3b runs one agent per crate; on a large workspace that's dozens of invocations. Mitigation: per-crate batching keeps each context bounded; the deterministic Wave 3a should close ~30–50% before any LLM cost is paid.

## Open questions

- **Which model?** The deterministic Wave 3a needs no model. Wave 3b is a narrow well-defined task — Haiku or Sonnet probably suffice; Opus would be wasteful. Worth a measurement pass on a sample of 100 dark functions before committing.
- **Where does the LLM call live — inside doc-linter, or in a wrapping skill?** Probably the wrapping skill. doc-linter ships the subcommand and the prompt; the caller wires up whichever model client they have. Keeps doc-linter free of HTTP / API-key plumbing.
- **Does Wave 3c's entity-proposal output integrate with the existing `cluster --write` / `--promote` flow?** Very plausibly yes — the `unauthored-cluster` lint already surfaces "the code organises around X but no entity exists", and `cluster --write` already scaffolds a candidate. Wave 3c's entity proposals could route through the same scaffold pipeline rather than introducing a parallel one.

## Out of scope

- LLM client plumbing (auth, retries, rate limits). The caller's concern.
- Multi-language support — the deterministic Wave 3a generalises to any language whose code-ingest produces `FUNCTION_MENTIONS` edges (already true for Rust, TS, Python). Wave 3b's prompt would need per-language framing; deferred until the Rust path lands.
- Re-authoring Wave 3a's auto-generated comments into hand-prose. A later, optional pass; not in scope for this entry.

## Related

- [[1-dropin-distribution]] — unrelated; listed because new adopters who flip the structural-bridge on will hit the same coverage-gap workflow this entry addresses
- [[using-doc-linter]] § Closing a coverage gap — the current human-driven workflow this entry would automate
