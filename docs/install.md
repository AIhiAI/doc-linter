---
id: install
role: doc
kind: how-to
lifecycle: stable
covers: [doc-graph]
title: Install doc-linter
summary: Every way to install doc-linter, from prebuilt binaries and Python wheels to building from source, and which paths are still untested.
status: draft
updated: 2026-10-08
tags: [install, release]
---

# Install doc-linter

Status key: **untested** means the path is wired up but no tagged release exists yet, so it has never run end to end. Source builds are the only path verified today.

| Path | Needs | Status |
|------|-------|--------|
| [Prebuilt binary](#prebuilt-binary) | `tar`, `curl` or `gh` | untested until first tagged release |
| [`install.sh`](#install-script) | same as above, falls back to source | untested (binary path); source fallback works |
| [`uv tool` / `pipx`](#python-wheel) | Python 3.8+ | untested, and not on PyPI yet |
| [Build from source](#build-from-source) | Rust 1.88, a C and a C++ compiler | works |

Release binaries target Linux x86_64 and aarch64 (glibc), macOS x86_64 and arm64, and Windows x86_64. They include the default `embeddings` feature. They are unsigned.

## Prebuilt binary

Download `doc-linter-<tag>-<target>.tar.gz` (`.zip` on Windows) and its `.sha256` from the GitHub release, then:

```bash
sha256sum -c doc-linter-v0.3.1-x86_64-unknown-linux-gnu.tar.gz.sha256
tar xzf doc-linter-v0.3.1-x86_64-unknown-linux-gnu.tar.gz
install -m 755 doc-linter-v0.3.1-*/doc-linter ~/.local/bin/
```

The repository is private, so use `gh release download --repo AIhiAI/doc-linter` or a browser logged into GitHub. macOS may quarantine the unsigned binary: `xattr -d com.apple.quarantine doc-linter`.

## Install script

```bash
curl -fsSL https://raw.githubusercontent.com/AIhiAI/doc-linter/main/scripts/install.sh | bash
```

Outside a checkout and with no arguments, [`scripts/install.sh`](../scripts/install.sh) downloads the matching release binary into `~/.local/bin` (override with `DOC_LINTER_INSTALL_DIR`), verifies the SHA-256, and otherwise falls back to building from source. Inside a checkout, or with arguments, it builds from source as before. Anonymous `curl` cannot read a private repo, so on private access install `gh` and log in.

## Python wheel

```bash
uv tool install doc-linter    # or: pipx install doc-linter
```

The wheel only carries the same Rust binary (maturin `bindings = "bin"`, see `pyproject.toml`). It is built by the release workflow but not published. Until it is, install the wheel attached to the release: `uv tool install ./doc_linter-*.whl`.

## Build from source

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --locked --git https://github.com/AIhiAI/doc-linter
```

Requirements: Rust 1.88, a C compiler (bundled SQLite, tree-sitter grammars), a C++ compiler (the usearch vector index, built by the `cc` crate) and `curl` for the bundled embedding model download on first build. cmake is not needed. Variants:

- BM25 only, no semantic search: add `--no-default-features`. That also drops the vector index, so no C++ compiler is needed either.
- Air-gapped: set `DOC_LINTER_SKIP_MODEL_DOWNLOAD=1` and supply `DOC_LINTER_EMBED_MODEL` and `DOC_LINTER_EMBED_TOKENIZER` at runtime.

The graph is a single SQLite file; there is no database server to install. A `graph.kuzu` left by an older version is ignored and can be deleted.

## Maintainer decisions still open

- PyPI name `doc-linter` is unchecked; the publish job in `.github/workflows/release.yml` is commented out.
- No signing (cosign, notarization, Authenticode) and no crates.io publish.
- Releases are created as drafts; review and publish by hand.
