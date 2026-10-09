# doc-linter — agent prompt

This file is the **system-prompt input** that teaches an AI coding
agent (Claude Code, opencode, Cursor, Aider, …) how to author and
edit documentation in a repo that uses doc-linter. It is intentionally
self-contained: paste the content below into your agent runtime's
config, or drop this file at your repo root so runtimes that look
for `AGENTS.md` pick it up automatically.

For Claude Code, append the relevant sections to your `CLAUDE.md`.
For opencode / Cursor / Aider / Codex / your own runtime, follow
their respective conventions for loading prompt context.

---

## Prompt: working with doc-linter

You are working in a repository that uses **doc-linter** for
documentation lint and graph validation. doc-linter treats `.md`
files and source-code doc comments as a single connected knowledge
graph governed by a per-repo ontology. **Read this section before
authoring or editing any `.md` file or `///` / `//!` doc comment.**

### First-touch bootstrap

Before authoring, learn the repo's specific vocabulary and
conventions. Run these once per session:

```bash
doc-linter ontology         # list valid roles, kinds, lifecycles, entities
doc-linter check            # learn what's currently green; baseline
cat .doc-lint.toml          # read this repo's lint config
```

The vocabulary is repo-specific. Do not assume role/kind/lifecycle
values from another project apply here — `doc-linter ontology` is
the source of truth.

### Authoring rules

1. **Every `.md` file MUST have YAML frontmatter** with at minimum:
   `id`, `role`, `title`, `summary`, `status`, `updated`. Some
   roles also require `kind` and/or `lifecycle` per the ontology —
   if `doc-linter check` complains about a missing axis field, add
   it.

2. **The doc's role determines its directory.** Read the
   `[[layout_rules]]` section of `.doc-lint.toml`. A doc matched by
   a layout entry must live under one of the entry's
   `allowed_paths`. Put the file in the correct folder *before*
   adding content; renaming later is needless churn.

3. **Use wikilinks `[[id]]`** to reference other docs. The id is
   the frontmatter `id:` field, NOT the filename stem (though they
   often match). Wikilinks are how the graph forms — they're how
   queries find related docs.

4. **Use markdown links `[label](relative/path.md)`** for files
   outside the doc graph (source files, scripts, configs).
   doc-linter validates these point at real files, so don't fabricate.

5. **Don't use bare ambiguous nouns.** Words listed in
   `vale_ambiguous_words` (`rule`, `module`, `service`, …) must be
   qualified with a wikilink to the entity (e.g.
   `[[pricing-rule]]`) or backticked as a code symbol. Bare usage
   triggers a lint error.

6. **Frontmatter `updated:` is today's date.** Don't backdate.
   Don't leave it stale on substantive edits.

7. **Wikilinks resolve by id, not path.** Moving a doc between
   folders doesn't break `[[id]]`. Markdown-style links DO break on
   move — update them yourself.

### Common roles (your repo's exact set: `doc-linter ontology`)

Typical conventions across repos using doc-linter; your repo may
differ:

- `roadmap-entry` — numbered milestones in `docs/roadmap/`. Filename
  pattern usually `^\d+-.*\.md$`.
- `doc` with `kind: explanation` — narrative explanations of how
  things work. Lives under `docs/explanations/`.
- `doc` with `kind: how-to` — step-by-step recipes / runbooks.
  Lives under `docs/recipes/`.
- `doc` with `kind: reference` — schemas, READMEs of code packages.
  Lives under `docs/explanations/` or co-located with the code.
- `doc` with `kind: explanation`, `lifecycle: planning` — design /
  planning notes. Lives under `docs/planning/`.
- `ontology-axis`, `ontology-value`, `ontology-entity` — the
  vocabulary itself. Lives under `docs/ontology/`.
- `index` — navigation hubs. Often unconstrained.
- `status-report` — generated dashboards. Often unconstrained.

### Verification — always lint before claiming done

After every doc edit:

```bash
doc-linter check --file <path-you-edited>
```

For larger changes (multiple files, ontology updates, code-comment
edits):

```bash
doc-linter check                          # full corpus
doc-linter check --no-vale                # skip Vale prose closure
doc-linter check --format=json | jq .     # programmatic
```

A non-zero exit means lint violations exist. **Do not declare a
task complete while the lint is dirty.** Fix or, with the user's
explicit permission, add a justified exemption.

### Common errors — how to fix

The canonical (code → meaning → fix) table for every diagnostic the
linter emits lives in [`docs/error-codes.md`](docs/error-codes.md),
generated from `src/validator/code_table.rs`. Read that first when
an unfamiliar code fires — it stays in sync with the actual variants
the validator can produce, so no parallel hand-maintained list here
to drift.

A handful of fixes that are policy-level rather than mechanical:

- For `unknown-role` / `unknown-axis-value` / `unknown-entity`:
  prefer correcting the value to one already in the ontology. Only
  add a new value doc under `docs/ontology/values/<axis>/<name>.md`
  (and bump the migration) with the user's explicit agreement.
- For `wrong-folder`: `git mv` the file to the suggested path. If
  the layout entry's `allowed_paths` is genuinely wrong for this
  repo, ask the user before extending it.
- For `vale-alert` / `comment-vocab-violation`: add the term to the
  ontology if it's a real domain concept; add to `vale_extra_accept`
  / `code_comment_extra_accept` if it's a tech term; rephrase
  otherwise. Don't fake-link with `[[entity-foo]]` just to silence a
  diagnostic — that's dishonest and defeats the graph's purpose.
- For `homepage-stale` / `homepage-missing`: always run
  `doc-linter homepage --write` after adding, renaming, or describing
  an entity / narrative doc, *before* committing.

### Don'ts

- **Do not silence errors with `--no-vale`** unless you're
  iterating fast on prose and you know you're hiding signal. Never
  ship a commit that needed `--no-vale` to pass.
- **Do not add to `exempt = [...]` to bypass a problem you should
  fix.** Exempt is for genuine edge cases (third-party READMEs,
  generated files), not for "I don't want to deal with this lint."
- **Do not fake-link entities.** A doc-comment that mentions
  `[[entity-pricing-rule]]` should actually be about pricing rules.
  Adding the link just to silence `missing-anchor` is dishonest and
  defeats the graph's purpose.
- **Do not author docs without first running
  `doc-linter ontology`.** Guessing role/kind values produces
  unknown-role errors and wastes a round-trip.
- **Do not edit `.doc-lint.toml`'s rules without explaining why.**
  Configuration drift to silence inconvenient lints is a smell.
- **Do not assume FA-specific conventions.** This prompt is the
  generic doc-linter contract. Repo-specific facts come from
  `doc-linter ontology` + `.doc-lint.toml` at runtime.

### When you're stuck

- Run `doc-linter ontology --format=json` to see the full
  vocabulary structurally.
- Run `doc-linter query backlinks <id>` to see who points at a doc.
- Run `doc-linter query path <from> <to>` to see if two docs are
  graph-connected.
- Run `doc-linter explain <function-or-symbol>` (Rust + SCIP only)
  to see the entity context of a function.
- Read [USAGE.md](USAGE.md) for the human-targeted version of this
  same content.
- Read the comprehensive reference: [README.md](README.md).

### One-line summary

> Read the ontology, author into the right folder with full
> frontmatter and real wikilinks, lint every edit, fix every error
> before claiming done.

---

## How to use this prompt

**Claude Code:** Append the "Prompt: working with doc-linter"
section above to your `CLAUDE.md` (project-level). Claude Code loads
`CLAUDE.md` automatically on every session in the repo.

**opencode:** opencode looks for `AGENTS.md` at the repo root.
Either drop this file there or symlink it.

**Cursor:** Append the prompt section to `.cursor/rules` (or
`.cursorrules` in older versions).

**Aider:** Pass the prompt section via `--message-file` or include
it in your `CONVENTIONS.md`.

**Generic / your own runtime:** Load the "Prompt: working with
doc-linter" section as system-prompt context.

For all runtimes, the prompt assumes the agent has shell access (to
run `doc-linter check` and read `.doc-lint.toml`). If your runtime
sandboxes shell, also expose `doc-linter ontology --format=json`
output in advance.
