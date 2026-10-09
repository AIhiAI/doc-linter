#!/usr/bin/env bash
# install.sh — install doc-linter: a prebuilt release binary when one
# exists, otherwise `cargo install`.
#
# The source build needs a Rust toolchain and a C/C++ compiler (tree-sitter
# grammars and the usearch vector index are compiled by the `cc` crate).
# It does NOT need cmake.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/AIhiAI/doc-linter/main/scripts/install.sh | bash
#   # or, from a checkout:
#   ./scripts/install.sh                  # defaults to `cargo install --path .`
#   ./scripts/install.sh --git URL        # forwards extra args to `cargo install`
#   ./scripts/install.sh doc-linter       # `cargo install doc-linter` from crates.io
#
# Override the source by passing args; with no args we install from the
# current directory if it looks like the doc-linter checkout, otherwise
# from the public mirror.

set -euo pipefail

# Fast path: with no args outside a checkout, try the prebuilt release binary
# (no compiler needed). Falls through to the source build on any failure.
# UNTESTED until the first tagged release exists.
if [ "$#" -eq 0 ] && ! { [ -f Cargo.toml ] && grep -q '^name = "doc-linter"' Cargo.toml; }; then
    case "$(uname -s)-$(uname -m)" in
        Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
        Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-gnu ;;
        Darwin-x86_64) target=x86_64-apple-darwin ;;
        Darwin-arm64) target=aarch64-apple-darwin ;;
        *) target= ;;
    esac
    repo=AIhiAI/doc-linter
    if [ -n "$target" ] && command -v tar >/dev/null 2>&1; then
        tmp=$(mktemp -d)
        got=0
        if command -v gh >/dev/null 2>&1 && gh release download --repo "$repo" --pattern "doc-linter-*-$target.tar.gz*" --dir "$tmp" 2>/dev/null; then
            got=1
        elif command -v curl >/dev/null 2>&1; then
            # Public repos only; a private repo makes this 404 and we fall back.
            tag=$(curl -fsSL "https://api.github.com/repos/$repo/releases/latest" 2>/dev/null | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)
            [ -n "$tag" ] && base="https://github.com/$repo/releases/download/$tag/doc-linter-$tag-$target.tar.gz" &&
                curl -fsSL -o "$tmp/a.tar.gz" "$base" && curl -fsSL -o "$tmp/a.tar.gz.sha256" "$base.sha256" && got=1
        fi
        if [ "$got" = 1 ]; then
            f=$(ls "$tmp"/*.tar.gz | head -n1)
            want=$(cut -d' ' -f1 "$f.sha256")
            have=$( (sha256sum "$f" 2>/dev/null || shasum -a 256 "$f") | cut -d' ' -f1)
            if [ "$want" = "$have" ] && tar xzf "$f" -C "$tmp"; then
                dest="${DOC_LINTER_INSTALL_DIR:-$HOME/.local/bin}"
                mkdir -p "$dest" && install -m 755 "$tmp"/doc-linter-*/doc-linter "$dest/doc-linter" &&
                    { echo "install: installed prebuilt binary to $dest/doc-linter"; exit 0; }
            fi
        fi
        echo "install: prebuilt binary unavailable, building from source" >&2
    fi
fi

# Sanity-check the source-build toolchain: cargo and a C/C++ compiler.
missing=()
command -v cargo >/dev/null 2>&1 || missing+=("cargo")
if ! command -v cc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1; then
    missing+=("a C compiler (gcc or clang)")
fi
if ! command -v c++ >/dev/null 2>&1 && ! command -v clang++ >/dev/null 2>&1 && ! command -v g++ >/dev/null 2>&1; then
    missing+=("a C++ compiler (g++ or clang++)")
fi
if [ "${#missing[@]}" -gt 0 ]; then
    echo "install: missing required tools: ${missing[*]}" >&2
    echo "install: on Debian/Ubuntu: sudo apt install build-essential" >&2
    echo "install: on Fedora/RHEL:   sudo dnf install gcc-c++ make" >&2
    echo "install: on macOS:         xcode-select --install" >&2
    exit 1
fi

if [ "$#" -gt 0 ]; then
    exec cargo install "$@"
fi

if [ -f Cargo.toml ] && grep -q '^name = "doc-linter"' Cargo.toml; then
    echo "install: detected doc-linter checkout — running 'cargo install --path .'"
    exec cargo install --path .
fi

echo "install: no args and not in a doc-linter checkout — installing from the public mirror"
exec cargo install --git https://github.com/AIhiAI/doc-linter
