---
id: contributing
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph]
title: Contributing to doc-linter
summary: How to build, test and lint doc-linter, how to check your own changes, and the contributor licence agreement process.
status: stable
updated: 2026-10-08
tags: [contributing, process]
---

# Contributing to doc-linter

Thanks for helping. This page covers the build, the tests, how to lint your
own change, and the licence terms for contributions.

## Build

Rust 1.88 (pinned by `rust-toolchain.toml`) and a C and C++ compiler are
needed, because SQLite and the vector index are compiled from source. No cmake.

```bash
cargo build
```

The first build downloads a pinned embedding model through `build.rs`. Set
`DOC_LINTER_SKIP_MODEL_DOWNLOAD=1` to skip it. The `target/` directory gets
large, so check free disk space before a first full build.

## Test

```bash
cargo test -- --test-threads=1
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo deny check
```

Run the tests with `--test-threads=1`. Several tests share on-disk stores and
the MCP tests are not parallel-safe. If your machine is small, add
`CARGO_BUILD_JOBS=2`.

## Check your changes with doc-linter

This repo lints its own docs and doc comments. After editing a `.md` file or a
`///` comment, run the binary you just built:

```bash
cargo run -- check
```

For the files you touched only, use `--file <path>`. If you changed a CLI flag
or a config field, regenerate the reference docs and commit the result:

```bash
cargo run -- gen-docs
```

`gen-docs --check` fails CI when the generated docs are stale. See
[the error code table](docs/error-codes.md) for what each diagnostic means.
Please fix findings rather than adding exemptions.

## Pull requests

- Keep a change focused; one concern per pull request.
- Add or update tests for behaviour changes.
- Do not include secrets, customer data or names of private projects in code,
  docs, tests or fixtures.

## Licence and CLA

doc-linter is licensed under the Apache License 2.0 (see `LICENSE`). A
Developer Certificate of Origin sign-off (`Signed-off-by`) is not required.

Instead, contributors are asked to agree to a Contributor Licence Agreement
(see [CLA.md](CLA.md)). The plan is to collect agreement through the CLA
Assistant GitHub bot, which comments on a pull request and asks you to sign
once. The bot is not configured yet, and the CLA text is still a draft pending
legal review, so until both are in place maintainers will tell you on the pull
request what is needed.
