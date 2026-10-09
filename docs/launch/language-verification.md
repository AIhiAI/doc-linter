---
id: language-verification
role: doc
kind: explanation
lifecycle: planning
title: C# and Dart end-to-end verification
summary: Result of running the released doc-linter 0.3.1 binary over one small real C# project and one small real Dart project, from a zero-config report through scip-index, check and the concept work list, with exact install commands, row counts, timings and the bugs found. Java was not verified.
status: draft
updated: 2026-10-09
covers: [scip]
tags: [launch, languages]
---

# C# and Dart end-to-end verification

This closes the gap named in [language-support](language-support.md): until now no C# or Dart repository had been run through `scip-index` and `check`. Java is still unverified because the test machine has no JDK.

## Setup

- Binary: `doc-linter 0.3.1`, installed from the `v0.0.0-rc2` wheel (no local cargo build).
- Toolchains: dotnet SDK 10.0.400, Dart 3.11.5. Linux, 31 GiB RAM, about 11 GiB available.
- Two fresh projects, each a git repo of about five source files plus a test file and two markdown docs in `docs/`, with no `.doc-lint.toml` and no `doc-linter init`. Both model the same shop domain: `Item`, `Pricing` (discount, tax), `Cart` (add, subtotal, total) and `Checkout` (receipt), with `///` XML-doc (C#) or dartdoc (Dart) comments on every public member and a test file.

## Install steps

Both indexers installed into a user-local directory, with no sudo.

| Language | Command | Result |
|---|---|---|
| C# | `dotnet tool install --tool-path <dir> scip-dotnet`, then put `<dir>` on PATH | worked, scip-dotnet 0.2.14, 9 s |
| Dart | `PUB_CACHE=<dir> dart pub global activate scip_dart`, then put `<dir>/bin` on PATH | worked, scip_dart 1.6.2, 7 s |

For a throwaway sandbox also set `DOTNET_CLI_HOME` (otherwise dotnet writes under the home directory) and keep `PUB_CACHE` pointing at the same directory when running `dart pub get` and `scip-index`.

## Zero-config flow

- `doc-linter report` with no config and no index prints `No code index yet (0 functions). Run doc-linter scip-index` and lists the two docs as stale. The walker already created `File` rows for every `.cs` or `.dart` file and a `Module` row for the `.csproj` or `pubspec.yaml`.
- C#: `doc-linter scip-index` needs nothing else. It generates `.doc-lint/scip/all.sln` over `src/Shop/Shop.csproj`, runs `dotnet restore` (needs NuGet access) and then the indexer. 5.3 s wall clock, about 200 MiB peak resident set size of the largest process.
- Dart: running `scip-index` before `dart pub get` fails with `ERROR: Unable to locate packageConfig` and no index. After `dart pub get` it succeeds in 1.6 s, about 240 MiB peak.
- `doc-linter check --no-vale` then ingests the index in about 0.15 s. Its only errors are the two docs lacking frontmatter, which is expected without `init`.

## What reached the graph

| Measure | C# | Dart |
|---|---|---|
| Functions | 6 | 8 |
| Types (`Type` table) | 4 | 9 (4 classes and 5 per-file `module` rows) |
| Call edges | 4 | 5 |
| `language` label on functions | `csharp` | `dart` |
| Functions with a doc comment (report) | 6 of 6 | 7 of 8 (the undocumented one is the explicit `Item` constructor) |
| Test-file functions indexed | no | yes |

- The call edges are the real ones: `Checkout.Receipt` to `Cart.Total`, `Cart.Total` to `Cart.Subtotal`, `Pricing.Discount` and `Pricing.Tax`; Dart additionally records `emptyCartTotalsZero` to `Cart.total`.
- C# methods only: the record `Item` and the implicit constructors produce no function rows. The C# test file lives in a folder with no `.csproj`, so scip-dotnet (which indexes projects) never sees it. A test project with its own `.csproj` would be indexed.
- Dart doc comments arrive as clean text, for example `Adds an item to the cart.`
- C# doc comments arrive, but as the raw scip-dotnet payload: a fenced signature block followed by the XML member element (see bug 1).
- `--lint-code-comments` reads the `///` comments of both languages: a deliberately unknown term (`Zorblax`) in a doc comment produced `comment vocab — term 'Zorblax' is not in the ontology vocabulary` with a file, line and column for both `.cs` and `.dart` (checked on copies after `doc-linter init`).
- `doc-linter report` coverage, `query saved concept-work-list`, `query dead-code`, `query types` and `ontology propose` all ran on both. Before any entity exists the saved query returns zero rows and the report's work list carries the candidates. After `ontology accept shop` on the C# project the report showed 6 of 6 functions reaching the entity.

## Bugs and rough edges

1. **C# doc comment is raw scip-dotnet output.** The stored `doc_comment` is a fenced `cs` signature followed by an XML `member` element wrapping the `summary`. Consequences: `dead-code` and the report show `doc_summary` as just the opening code fence; the concept work list shows the same fence as its sample; and the word `member` was proposed as a concept candidate with 9 symbols. Repro: the C# project above, `doc-linter scip-index && doc-linter check --no-vale && doc-linter report`. Fix direction: strip the leading fenced block and unwrap the XML in the ingest step for C#.
2. **`ontology accept` without `init` writes docs that fail the lint.** After `doc-linter ontology accept shop` in a repo with no ontology, `check` goes from 2 errors to 6: the new doc uses role `doc`, kind `reference` and lifecycle `implementing` (none registered) and its id `concept-shop` does not match the filename `shop`. The README zero-config story ends at `ontology propose`; accept needs `init` first, or should say so.
3. **Dart: missing `pub get` message does not say what to do.** The indexer's own `Unable to locate packageConfig` is printed unchanged, followed by `no SCIP index produced`. The README already says to run `dart pub get`; the error could say it too.
4. **Dart `Type` table holds per-file `module` rows** next to the class rows, so "9 types" overstates the class count. Filter with `--kind`.

## Not verified

- Java: no JDK on the test machine.
- Larger C# solutions (multi-project, warnings about project load failures) and Flutter packages; the projects here are single-package and small.
- Windows and macOS.
