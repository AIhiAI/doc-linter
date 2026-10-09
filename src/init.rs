//! Round 4B: `doc-linter init` — bootstrap a target codebase into a
//! fresh [[entity-doc-graph]] vault.
//!
//! Writes a starter `.doc-lint.toml` at the target root, scaffolds
//! `docs/ontology/` (axes + values + a placeholder entity + the v1
//! bootstrap migration), creates `.doc-lint/` and appends it to
//! `.gitignore`. The scaffolded ontology lints clean against the
//! linter's own checker — verified by `tests/dropin_init.rs`.
//!
//! All scaffold templates are embedded at build time via `include_str!`
//! from `crates/doc-linter/templates/init/`. They're largely static —
//! only one substitution (`{{TODAY}}`, used in nothing currently but
//! reserved for future dynamic fields) and zero per-target variables.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Each entry: (relative path under target root, file contents).
/// Order doesn't matter; we create parent dirs on demand.
struct Template {
    rel_path: &'static str,
    contents: &'static str,
}

const STARTER_CONFIG: &str = include_str!("../templates/init/doc-lint.toml");

const ONTOLOGY_TEMPLATES: &[Template] = &[
    Template {
        rel_path: "docs/ontology/README.md",
        contents: include_str!("../templates/init/docs/ontology/README.md"),
    },
    Template {
        rel_path: "docs/ontology/axes/role.md",
        contents: include_str!("../templates/init/docs/ontology/axes/role.md"),
    },
    Template {
        rel_path: "docs/ontology/axes/kind.md",
        contents: include_str!("../templates/init/docs/ontology/axes/kind.md"),
    },
    Template {
        rel_path: "docs/ontology/axes/lifecycle.md",
        contents: include_str!("../templates/init/docs/ontology/axes/lifecycle.md"),
    },
    Template {
        rel_path: "docs/ontology/axes/covers.md",
        contents: include_str!("../templates/init/docs/ontology/axes/covers.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/doc.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/doc.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/index.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/index.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/adr.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/adr.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/ontology-axis.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/ontology-axis.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/ontology-value.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/ontology-value.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/ontology-entity.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/ontology-entity.md"),
    },
    Template {
        rel_path: "docs/ontology/values/role/ontology-migration.md",
        contents: include_str!("../templates/init/docs/ontology/values/role/ontology-migration.md"),
    },
    Template {
        rel_path: "docs/ontology/values/kind/tutorial.md",
        contents: include_str!("../templates/init/docs/ontology/values/kind/tutorial.md"),
    },
    Template {
        rel_path: "docs/ontology/values/kind/how-to.md",
        contents: include_str!("../templates/init/docs/ontology/values/kind/how-to.md"),
    },
    Template {
        rel_path: "docs/ontology/values/kind/reference.md",
        contents: include_str!("../templates/init/docs/ontology/values/kind/reference.md"),
    },
    Template {
        rel_path: "docs/ontology/values/kind/explanation.md",
        contents: include_str!("../templates/init/docs/ontology/values/kind/explanation.md"),
    },
    Template {
        rel_path: "docs/ontology/values/lifecycle/planning.md",
        contents: include_str!("../templates/init/docs/ontology/values/lifecycle/planning.md"),
    },
    Template {
        rel_path: "docs/ontology/values/lifecycle/decided.md",
        contents: include_str!("../templates/init/docs/ontology/values/lifecycle/decided.md"),
    },
    Template {
        rel_path: "docs/ontology/values/lifecycle/implementing.md",
        contents: include_str!("../templates/init/docs/ontology/values/lifecycle/implementing.md"),
    },
    Template {
        rel_path: "docs/ontology/values/lifecycle/stable.md",
        contents: include_str!("../templates/init/docs/ontology/values/lifecycle/stable.md"),
    },
    Template {
        rel_path: "docs/ontology/values/lifecycle/superseded.md",
        contents: include_str!("../templates/init/docs/ontology/values/lifecycle/superseded.md"),
    },
    Template {
        rel_path: "docs/ontology/migrations/0001-initial.md",
        contents: include_str!("../templates/init/docs/ontology/migrations/0001-initial.md"),
    },
    Template {
        rel_path: "docs/ontology/entities/example.md",
        contents: include_str!("../templates/init/docs/ontology/entities/example.md"),
    },
];

/// Total number of scaffolded files (config + ontology) for the
/// [[entity-doc-graph]] init scaffold. Useful as a constant for the
/// integration test and end-of-run summary. The `.gitignore` line append
/// is not counted — it's a mutation, not a fresh write.
#[cfg(test)]
pub fn scaffold_doc_count() -> usize {
    1 + ONTOLOGY_TEMPLATES.len()
}

/// Lower-level entry point that bootstraps a fresh [[entity-doc-graph]]
/// repo: writes the starter `.doc-lint.toml`, scaffolds `docs/ontology/`
/// (axes, values, placeholder entity, v1 bootstrap migration), creates
/// `.doc-lint/`, and appends the gitignore line. Returns a structured
/// summary instead of an `ExitCode` — used by `tests/dropin_init.rs` to
/// assert exact scaffold contents.
pub fn init_at(root: &Path) -> Result<InitSummary> {
    std::fs::create_dir_all(root).with_context(|| format!("create {}", root.display()))?;

    let config_path = root.join(".doc-lint.toml");
    let ontology_dir = root.join("docs").join("ontology");
    let ontology_pre_existed = existing_ontology_present(&ontology_dir);

    // Step 1: .doc-lint/ + .gitignore append.
    let lint_dir = root.join(".doc-lint");
    std::fs::create_dir_all(&lint_dir).with_context(|| format!("create {}", lint_dir.display()))?;
    let gitignore_modified = ensure_gitignore(root)?;

    // Step 2: starter .doc-lint.toml. Don't overwrite an existing
    // config — if the user has tuned it, respect that.
    let config_existed = config_path.exists();
    if !config_existed {
        std::fs::write(&config_path, STARTER_CONFIG)
            .with_context(|| format!("write {}", config_path.display()))?;
    }

    // Step 3: ontology scaffold. Skip wholesale if it already exists —
    // we never overwrite a vocabulary that's been authored.
    let mut written: Vec<PathBuf> = Vec::new();
    if !ontology_pre_existed {
        for tmpl in ONTOLOGY_TEMPLATES {
            let target = root.join(tmpl.rel_path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            std::fs::write(&target, tmpl.contents)
                .with_context(|| format!("write {}", target.display()))?;
            written.push(target);
        }
    }

    Ok(InitSummary {
        root: root.to_path_buf(),
        config_written: !config_existed,
        config_path,
        ontology_pre_existed,
        ontology_files_written: written,
        gitignore_modified,
    })
}

/// Result of `init_at` — describes what got scaffolded into the new
/// [[entity-doc-graph]] vault. Used by the dropin test + by main.rs to
/// print the end-of-run summary.
pub struct InitSummary {
    pub root: PathBuf,
    pub config_written: bool,
    pub config_path: PathBuf,
    pub ontology_pre_existed: bool,
    pub ontology_files_written: Vec<PathBuf>,
    pub gitignore_modified: bool,
}

impl InitSummary {
    /// Print the human-readable end-of-run report describing what got
    /// scaffolded into the new [[entity-doc-graph]] vault.
    pub fn render(&self) {
        let display_root = self.root.display();
        println!("Initialized doc-linter at {display_root}.");
        if self.config_written {
            println!("  - wrote {}", self.config_path.display());
        } else {
            println!(
                "  - kept existing {} (not overwritten)",
                self.config_path.display()
            );
        }
        if self.ontology_pre_existed {
            println!(
                "  - kept existing docs/ontology/ (not overwritten — \
                 your vocabulary is authoritative)"
            );
        } else {
            println!(
                "  - scaffolded {} ontology doc(s) under docs/ontology/",
                self.ontology_files_written.len()
            );
        }
        if self.gitignore_modified {
            println!("  - appended `.doc-lint/` to .gitignore");
        }
        println!();
        println!("Next steps:");
        println!(
            "  1. Edit docs/ontology/entities/ to define your domain \
             (replace entity-example with real concepts)."
        );
        println!("  2. Run `doc-linter check` to validate.");
        println!(
            "  3. Install Vale (https://vale.sh) — vocabulary closure is on \
             by default and `check` warns `vale-missing` without it. \
             Optionally install rust-analyzer for the SCIP code graph."
        );
        println!(
            "  4. Wire the LSP into your editor — see the Drop-in setup \
             section in the doc-linter README."
        );
    }
}

/// Append `.doc-lint/` to `<root>/.gitignore` so the [[entity-doc-graph]]
/// SQLite graph and Vale config don't get committed. If the line already
/// exists (or .gitignore doesn't exist yet, in which case we create it),
/// this is a no-op for that line. Returns true if .gitignore was changed
/// by this call.
fn ensure_gitignore(root: &Path) -> Result<bool> {
    let path = root.join(".gitignore");
    let entry = ".doc-lint/";
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing
        .lines()
        .any(|l| l.trim() == entry || l.trim() == ".doc-lint")
    {
        return Ok(false);
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(entry);
    next.push('\n');
    std::fs::write(&path, next).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

/// Returns true if the [[entity-doc-graph]] ontology directory exists AND
/// already has at least one ontology file in it. An empty
/// `docs/ontology/` directory is treated as not-yet-initialised (so a
/// user who created the dir expecting init to fill it sees the scaffold
/// land).
fn existing_ontology_present(dir: &Path) -> bool {
    if !dir.exists() {
        return false;
    }
    let Ok(read) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in read.flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            // any populated subdirectory counts
            if let Ok(sub) = std::fs::read_dir(entry.path()) {
                if sub.flatten().next().is_some() {
                    return true;
                }
            }
        } else if entry
            .file_name()
            .to_str()
            .is_some_and(|s| s.ends_with(".md"))
        {
            return true;
        }
    }
    false
}

/// Tests for the [[entity-doc-graph]] drop-in `init` scaffold that
/// bootstraps a target repo with `.doc-lint.toml` + ontology files.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    /// Tripwire: asserts the [[entity-doc-graph]] init scaffold writes the
    /// expected number of template files. Update when templates change.
    #[test]
    fn scaffold_count_is_what_we_expect() {
        // 1 .doc-lint.toml + ontology files.
        // If you change templates, update this.
        assert_eq!(scaffold_doc_count(), 24);
    }

    /// Asserts that `ensure_gitignore` is idempotent — running the
    /// [[entity-doc-graph]] init twice doesn't duplicate the `.doc-lint/`
    /// entry.
    #[test]
    fn ensure_gitignore_is_idempotent() {
        let dir = std::env::temp_dir().join(format!(
            "doc-linter-init-gi-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(ensure_gitignore(&dir).unwrap()); // first call writes
        assert!(!ensure_gitignore(&dir).unwrap()); // second is no-op
        let body = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert_eq!(body.matches(".doc-lint/").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
