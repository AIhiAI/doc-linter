---
id: kuzu-removal-results
role: doc
kind: explanation
lifecycle: planning
title: Kuzu removal measurements
summary: Plan item B9 — what removing Kuzu changed in build time, target size, index size, query latency and vector recall, measured on 2026-10-08, with what could not be measured stated plainly.
status: draft
updated: 2026-10-08
covers: [doc-graph]
tags: [roadmap, storage]
---

# Kuzu removal measurements

Follows [[store-trait]]. All runs on one shared, busy 20-core laptop (load average 11 to 39 during the runs), `CARGO_BUILD_JOBS=2`, `nice 19`. Treat timings as rough.

## Results

| Measure | Before (Kuzu, cf5bf51) | After (SQLite) | Note |
|---|---|---|---|
| Clean `cargo build --release` | not measured; README said about 7 min for the Kuzu C++ part alone | 4 min 28 s | after: fresh empty target dir, 2 jobs |
| `target/` after that build | not measured; the shared dev target had grown to 58 GB with Kuzu artifacts | 1.06 GB | after: release only |
| Release binary | not measured | 46 MB | |
| Debug binary | 923 MB | 419 MB | same sources otherwise |
| Graph on disk (this repo at cf5bf51, docs plus SCIP-less ingest, `--embeddings`) | `graph.kuzu` 42.1 MB | `graph.sqlite` 2.7 MB plus `graph.vec.*` 0.86 MB | 12x smaller |
| `check --embeddings`, debug binary | 16 min 33 s | 15 min 31 s | embedding inference in a debug build dominates both |
| Vector recall@10 vs exact | not applicable (linear scan, exact) | 0.977 | `usearch_recall_at_10_vs_exact`, usearch default SIMD features on; target was 0.95 |

Query latency, saved queries on the same corpus, debug binaries, best of 3, process start included:

| Saved query | Kuzu | SQLite |
|---|---|---|
| orphan-entities | 509 ms | 61 ms |
| hub-functions | 490 ms | 63 ms |
| corpus-cold-start-summary | 607 ms | 59 ms |
| stale-narrative-docs | 480 ms | 62 ms |
| coverage-by-entity | 498 ms | 63 ms |

Row counts match on four of five. `corpus-cold-start-summary` returns 11 rows on the old binary and 19 on the new: the old catalog had the zero-row defect fixed in the first commit of this series.

## Not measured

- Fineract (the 490 MB Kuzu figure) was not available, so index size is this repo's own graph, which is small. The ratio on a large corpus is unknown; SQLite pages and the HNSW file scale differently from Kuzu's column store.
- No clean Kuzu release build or release `target/` size: building Kuzu from scratch was too costly on the shared machine. The README's "about 7 min" was left uncorroborated.
- Query timings use debug builds; absolute numbers will be lower in release. The comparison is like for like, not a production figure.
- The old and new `check` runs had no git history (the corpus was a `git archive`), so coupling ingest was skipped in both.

## Side finding

The goldens exposed a coupling bug present in both engines: jaccard counted commit timestamps in a set, so commits in the same second collapsed. Fixed by keying on commit position (unit test `same_second_commits_count_separately`).
