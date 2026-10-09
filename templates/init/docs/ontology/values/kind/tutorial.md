---
id: value-kind-tutorial
role: ontology-value
title: "Kind: tutorial"
summary: Diátaxis quadrant — learning-oriented. Walks a beginner through a worked example so they end up knowing how to do something they couldn't do before.
status: stable
updated: 2026-04-30
axis_id: kind
value_id: tutorial
display: Tutorial
description: Learning-oriented. A walkthrough that teaches the reader something new.
introduced_in_version: 1
---

# Kind: tutorial

Diátaxis: **learning-oriented**. The reader doesn't yet know the
domain; the tutorial teaches by guiding them through a concrete
worked example.

## When to write a tutorial

- A new contributor's first walkthrough of the codebase
- "Hello world" for a new tool / library
- An onboarding doc that introduces concepts as they come up

## Tutorial vs how-to

Tutorial = "Let me teach you the shape of this." Reader is new.

How-to = "Here's the procedure for THIS task." Reader has context.

If the reader already knows what they want and just needs the steps,
write a [[value-kind-how-to]]. If they don't yet know what they want
because they're still learning the domain, write a tutorial.

## Convention

- Concrete worked example, not an abstract description
- Promise an outcome the reader will achieve
- Encourage tinkering — make sure the reader sees results
- Avoid digressions — link to [[value-kind-explanation]] for theory

## Reference

[Diátaxis: tutorials](https://diataxis.fr/tutorials/).

## Related

- [[axis-kind]]
- [[value-kind-how-to]]
