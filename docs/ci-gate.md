---
id: ci-gate
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph]
title: Gate pull requests with doc-linter
summary: "Copy-paste GitHub Actions workflow that runs doc-linter check on every pull request, in warn mode first, with the one-line change that makes it blocking."
status: stable
updated: 2026-10-08
tags: [ci, how-to]
---

# Gate pull requests with doc-linter

The workflow lives in [`examples/github-actions/doc-linter-gate.yml`](../examples/github-actions/doc-linter-gate.yml). Copy it to `.github/workflows/` in your repo.

It installs doc-linter with `cargo install --locked --git`, caches the binary, then runs `doc-linter check --no-vale`. `--no-vale` is a real flag; drop it if Vale is installed on the runner.

## Exit codes

`doc-linter check` exits `0` when clean and `1` when any diagnostic counts as an error. Some diagnostics are warnings by config (for example `orphan_entity_severity` defaults to `"warning"`) and do not change the exit code. Each code is explained in [[error-codes]]; the severity keys are listed in [[reference-config]].

## Step 1: warn mode

The shipped workflow sets `continue-on-error: true` on the lint step. The job shows the diagnostics in the log, the PR check shows a pass, and nothing is blocked. Run it for a week or two while you clear the backlog.

## Step 2: blocking mode

Delete the `continue-on-error: true` line. A non-zero exit from `check` now fails the job. Then mark the job as a required status check in the repository's branch protection rules.

## Optional extras

- Machine-readable output: `doc-linter check --format json`.
- Minimum code-to-doc reach: `doc-linter check --coverage-min-global=70` emits a `coverage-below-min` diagnostic below the floor. It needs a code graph, so run `doc-linter scip-index` first.
- Code graph in CI: install the SCIP indexer for each language (see the README language table), then run `doc-linter scip-index` before `check`.

All flags are listed in [[reference-cli]].
