---
id: secret-scan
role: doc
kind: reference
lifecycle: stable
covers: [doc-graph]
title: Secret scan of the git history
summary: Result and limits of a regex-based secret scan over every commit of the repository, run because gitleaks and trufflehog were not installed.
status: stable
updated: 2026-10-08
tags: [launch, security, audit]
---

# Secret scan of the git history

Date: 2026-10-08. Scope: `git log --all -p` of the repository, 728 commits on all local branches, added lines only (`-U0`).

## Tool and method

Neither `gitleaks` nor `trufflehog` is installed on the machine that ran this, so this is a **home-made regex scan, not a substitute for one of them**. A throwaway Python script (not committed) matched added diff lines against:

| Pattern | Looks for |
|---|---|
| `aws-access-key-id` | `AKIA` / `ASIA` followed by 16 characters |
| `github-token` | `ghp_`, `gho_`, `ghu_`, `ghs_`, `ghr_`, `github_pat_` tokens |
| `slack-token` | `xox[abprs]-` tokens |
| `private-key-header` | `-----BEGIN ... PRIVATE KEY` |
| `google-api-key`, `hf-token`, `anthropic-openai-key`, `jwt` | `AIza...`, `hf_...`, `sk-...` / `sk-ant-...`, three-part `eyJ...` tokens |
| `password-assign` | `password`, `secret`, `api_key`, `token` followed by `=` or `:` and a value of 6 or more characters |
| `url-with-credentials` | `scheme://user:pass@host` |

It also listed every file ever added whose name looks sensitive (`.env*`, `*.pem`, `*.key`, `*.p12`, `*.pfx`, `id_rsa`, keystores, `credentials*`) and, separately, database files (`*.sqlite`, `*.db`, `*.kuzu`). Matched values were never printed or stored: the output was counts, commit short hashes, file names, and for the last pattern the key name plus the length and first two characters of the value.

## Results

- No match for the AWS, GitHub, Slack, Google, Hugging Face, Anthropic/OpenAI, JWT, private-key-header or URL-credential patterns.
- No sensitive file name and no database file was ever added.
- `password-assign` matched 9 added lines in 8 commits, in src/kuzu_graph/unstubbed_concepts_ingest.rs, src/llm/anthropic.rs, src/cmd/mcp.rs, src/vale.rs, entity-discovery/llm/base.py and tests/fixtures/concepts/symbols.json. By shape (values 6 to 21 characters starting with `&`, `ra`, `li`, `ap`, `st`, `&u`) they look like code identifiers and references such as `token = raw_...` or `api_key = self...`, not credentials. This was a judgement from length and prefix, **not a line-by-line proof**; a person should open the 9 lines before release (`git log --all -S'api_key' -- src/llm/anthropic.rs` finds the commits).

Conclusion: the scan found no secret in the history, within the limits below.

## Limits

- Regexes miss high-entropy secrets with no recognisable prefix or assignment shape, secrets split across lines, base64 or otherwise encoded values, and secrets inside binary or very large files.
- Only added lines were examined, and only on local branches (`--all`). Stashes, unreachable objects, other clones, and pull request refs on the remote were not scanned.
- It does not check whether a pattern hit is live. It does not scan for customer data, only credential shapes.
- Not covered: the Co-Authored-By and author email fields of commits.

## Other findings that matter for release

- **Private product names remain in history.** About 620 added lines across history contain the two private product names that were removed from the working tree (see [[dependency-audit]]). Rewriting history (for example `git filter-repo`) or publishing from a fresh single-commit repository is needed if that matters.
- **Personal home paths remain in history.** About 116 added lines contain a `/home/<user>/` path.

## Recommended before going public

1. Install `gitleaks` and run `gitleaks detect --log-opts="--all"`, then `trufflehog git file://. --only-verified`; keep their reports out of the repo.
2. Open the 9 `password-assign` lines above.
3. Decide between history rewrite and a clean-history first public commit.
