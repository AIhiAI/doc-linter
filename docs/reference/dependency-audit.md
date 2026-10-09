---
id: dependency-audit
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph]
title: Dependency and asset audit
summary: "Licence and publishability audit of every non-registry dependency, bundled data file, template and downloaded model, checked against Apache-2.0."
status: stable
updated: 2026-10-08
tags: [reference, licence, audit]
---

# Dependency and asset audit

Scope: everything in the repo or fetched at build time that is not an ordinary crates.io dependency. Checked 2026-10-08 against `Cargo.toml`, `Cargo.lock`, `build.rs`, `deny.toml` and a tree-wide grep.

| Item | Source | Licence | Verdict |
|---|---|---|---|
| Rust crates (about 330) | crates.io only; `cargo deny check licenses sources` passes with `unknown-git = "deny"` | MIT, Apache-2.0, Unicode-3.0, Zlib, BSD, MPL-2.0 and similar; all on the `deny.toml` allow list | OK |
| Git deps, path deps, submodules, vendored crates | None. `Cargo.lock` has no git sources, there is no `.gitmodules`, the only path package is `doc-linter` itself | n/a | OK |
| `rusqlite` 0.32 with bundled SQLite (C, built by `cc`) | crates.io | MIT (rusqlite), public domain (SQLite) | OK. Replaced `kuzu` (archived upstream) |
| `usearch` 2 (C++ HNSW index and simsimd, built by `cc`; `vector-usearch` feature, default on) | crates.io | Apache-2.0 | OK |
| `tokenizers`, `tract-onnx` (`embeddings` feature, default on) | crates.io | Apache-2.0 / MIT | OK |
| `MIT OR Apache-2.0 OR LGPL-2.1-or-later` (2 crates) | crates.io | dual-licensed, MIT or Apache-2.0 can be chosen | OK, choose MIT/Apache |
| bge-small-en-v1.5 quantized ONNX (about 34 MB) | Downloaded by `build.rs` from `huggingface.co/Xenova/bge-small-en-v1.5` (SHA-256 pinned). Not committed, not in the crate | Upstream BAAI/bge-small-en-v1.5 is MIT; the Xenova ONNX export is listed MIT on its model card | OK, but confirm the Xenova card at release time (human check below). Never redistributed in the repo |
| `tokenizer.json` (same repo, SHA-256 pinned) | Downloaded by `build.rs` | MIT (same card) | OK, same check |
| [`data/english-common.txt`](../../data/english-common.txt) (74k words, `include_str!`) | Filtered from the Debian `american-english` word list (`/usr/share/dict`) | SCOWL-derived, permissive (public-domain-like) | Legal decision needed, see below. Contains no private content (grepped) |
| `src/config_data/*.txt` | Written in-repo | Apache-2.0 (project) | OK |
| `templates/init/**` (starter ontology) | Written in-repo | Apache-2.0 (project) | OK |
| [`examples/embed.rs`](../../examples/embed.rs), [`scripts/install.sh`](../../scripts/install.sh) | Written in-repo | Apache-2.0 (project) | OK |
| `repository` / `homepage` URLs (`github.com/AIhiAI/doc-linter`) | `Cargo.toml`, [`scripts/install.sh`](../../scripts/install.sh), README | n/a | OK if that org is the intended public home |

## Fixed in this pass

- **Packaging bug.** `Cargo.toml` `include` omitted `data/` and `src/**/*.txt`, but [`src/vale.rs`](../../src/vale.rs) and [`src/config/coverage.rs`](../../src/config/coverage.rs) `include_str!` them. A `cargo package` / `cargo install doc-linter` from crates.io would fail to compile. Both globs are now listed.
- **Internal names.** A doc comment and its generated twin in [`docs/reference/config.md`](config.md) cited a private directory name; replaced with a neutral example. A unit test used a personal home path (`/home/...`); replaced with neutral paths. A stale `crates/doc-linter/data/` path in [`src/vale.rs`](../../src/vale.rs) now says `data/`.
- **Product names scrubbed.** Two private product names, and the lowercase corpus nicknames derived from them, no longer appear in the tree: source comments, saved-query descriptions, research docs, the migration note and the generated reference are neutral, and both names are removed from `vale_extra_accept` in `.doc-lint.toml`. A case-insensitive tree grep for them finds nothing. Repository history may still contain them.

## Needs a human or legal decision

1. **Word list provenance.** A `NOTICE` file now carries an unverified SCOWL notice; counsel must verify it. Confirm the SCOWL / Debian `wamerican` licence is acceptable for a derived 74k-word file. Alternative: drop the file and generate it at build time.
2. **Model licence.** Open the Xenova model card on the release day and confirm it still says MIT. If not, the default `embeddings` feature needs review.
3. **Git history.** This audit covers the working tree only. A regex scan of the full history found no credentials (see [[secret-scan]] for method and limits), but gitleaks and trufflehog were not available: run one of them before making the repo public. History still contains the scrubbed product names and personal home paths.
4. **LGPL option.** Two crates offer LGPL-2.1-or-later as one choice of three. No action unless you want a written statement that the MIT/Apache option is the one relied on.
