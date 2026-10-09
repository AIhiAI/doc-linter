---
id: value-kind-reference
role: ontology-value
title: "Kind: reference"
summary: Diátaxis quadrant — information-oriented. Describes the technical machinery exhaustively. The reader looks something up; they don't read it cover-to-cover.
status: stable
updated: 2026-04-30
axis_id: kind
value_id: reference
display: Reference
description: Information-oriented. Exhaustive catalog of technical detail. Read by lookup, not narrative.
introduced_in_version: 1
---

# Kind: reference

Diátaxis: **information-oriented**. Reference docs describe the
technical machinery — every flag, every field, every endpoint.
Readers don't read them linearly; they look something up.

## When to write a reference

- Exhaustive flag/option catalog for a CLI
- Schema definition (every field, type, default)
- API surface listing
- Configuration file documentation

## Reference vs explanation

Reference = "What exists?"  Catalog. Lookup.

Explanation = "Why does it exist?"  Narrative. Read.

If you find yourself writing prose paragraphs about *why*, that
content belongs in a [[value-kind-explanation]] doc, with the
reference linking to it.

## Convention

- Tables, lists, headers — optimized for skim + ctrl-F
- Each item documented in a uniform shape
- Don't editorialise; describe what is
- Keep narrative to a 1-line summary at the top

## Reference

[Diátaxis: reference](https://diataxis.fr/reference/).

## Related

- [[axis-kind]]
- [[value-kind-how-to]]
- [[value-kind-explanation]]
