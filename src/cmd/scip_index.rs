//! `doc-linter scip-index` subcommand. Detects every language in the
//! repo that has a SCIP indexer, runs each one (idle CPU + I/O, plus a
//! memory cap on rust-analyzer, behind a pre-flight free-memory check),
//! and merges the per-language indexes into `.doc-lint/code.scip`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use doc_linter::config::LintConfig;
use doc_linter::scip_ingest;

/// Reads `MemAvailable:` from `/proc/meminfo` and returns the value in
/// MiB. Returns `None` on non-Linux hosts or any read / parse failure
/// — callers should warn but continue rather than block on a missing
/// pre-flight signal. The kernel's `MemAvailable` field is "free + buff/
/// cache + reclaimable" — the right number for "can I allocate N MiB
/// without swapping". For roadmap-49's pre-flight gate.
/// On macOS (no `/proc`) the same figure comes from `vm_stat`.
fn read_mem_available_mb() -> Option<u64> {
    if let Ok(raw) = std::fs::read_to_string("/proc/meminfo") {
        for line in raw.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                let kb_str = rest.split_whitespace().next()?;
                let kb: u64 = kb_str.parse().ok()?;
                return Some(kb / 1024);
            }
        }
        return None;
    }
    let out = Command::new("vm_stat").output().ok()?;
    parse_vm_stat_available_mb(&String::from_utf8_lossy(&out.stdout))
}

/// Memory macOS can hand out without swapping, from `vm_stat` output:
/// free + inactive + speculative pages times the page size in its header
/// (`… (page size of 16384 bytes)`). The analogue of Linux `MemAvailable`.
fn parse_vm_stat_available_mb(text: &str) -> Option<u64> {
    let page_size: u64 = text
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |label: &str| -> Option<u64> {
        let line = text.lines().find(|l| l.starts_with(label))?;
        line.rsplit(':')
            .next()?
            .trim()
            .trim_end_matches('.')
            .parse()
            .ok()
    };
    let free = pages("Pages free:")?;
    let inactive = pages("Pages inactive:").unwrap_or(0);
    let speculative = pages("Pages speculative:").unwrap_or(0);
    Some((free + inactive + speculative) * page_size / (1024 * 1024))
}

/// `(root, parts_dir, markers) -> project path` for an [`Indexer`].
type ProjectFn = fn(&Path, &LintConfig, &Path, &[PathBuf]) -> Result<PathBuf>;
type TrailingFn = fn(&[PathBuf]) -> &'static [&'static str];

/// One SCIP indexer and how to tell a repo needs it.
struct Indexer {
    lang: &'static str,
    bin: &'static str,
    /// File names, or `.ext` suffixes, that mark the language as present
    /// (searched to [`MARKER_DEPTH`]). The first marker kind with a hit
    /// supplies the `{projects}` argument.
    markers: &'static [&'static str],
    /// Source extensions; the language counts as present only when a
    /// marker AND one such file exist (a Flutter app's tooling
    /// `package.json` is not a TypeScript project).
    sources: &'static [&'static str],
    /// `{out}` → output file; `{project}` → what [`Indexer::project`]
    /// resolves.
    args: &'static [&'static str],
    /// Resolves `{project}` from the matched marker files.
    project: Option<ProjectFn>,
    /// Run once per marker directory, with that directory as `{project}`
    /// (indexers that take a package root, like scip_dart), instead of
    /// once over the repo.
    per_package: bool,
    /// `prlimit --as` the process. Only rust-analyzer: node, CoreCLR and
    /// the Dart VM reserve large virtual address ranges at start-up and
    /// fail under an address-space cap.
    address_space_cap: bool,
    /// Environment for the run. A `*_OPTS` value is appended
    /// (space-separated) to the caller's; any other key is set only when
    /// the caller hasn't set it.
    env: &'static [(&'static str, &'static str)],
    /// Output lines containing this are counted and summarised instead of
    /// printed one by one (per-project load failures that don't stop the
    /// index).
    load_failure: Option<&'static str>,
    /// Arguments appended after [`Indexer::args`], chosen from the
    /// matched marker files.
    trailing: Option<TrailingFn>,
}

/// How deep marker search descends: repos keep solutions / `package.json`
/// / `pubspec.yaml` at the root or one or two folders down.
const MARKER_DEPTH: usize = 3;

const INDEXERS: &[Indexer] = &[
    Indexer {
        lang: "rust",
        bin: "rust-analyzer",
        markers: &["Cargo.toml"],
        sources: &["rs"],
        args: &["scip", ".", "--output", "{out}"],
        project: None,
        per_package: false,
        address_space_cap: true,
        env: &[],
        trailing: None,
        load_failure: None,
    },
    Indexer {
        lang: "typescript",
        bin: "scip-typescript",
        markers: &["tsconfig.json", "package.json"],
        sources: &["ts", "tsx", "js", "jsx", "mjs", "cjs"],
        args: &["index", "--output", "{out}", "{project}"],
        project: Some(ts_project),
        per_package: false,
        address_space_cap: false,
        env: &[],
        trailing: None,
        load_failure: None,
    },
    Indexer {
        // One generated solution over every project on disk: a repo's own
        // solution may list projects that aren't checked out (submodules),
        // and passing projects one by one loads shared references twice.
        lang: "csharp",
        bin: "scip-dotnet",
        markers: &[".csproj"],
        sources: &["cs"],
        args: &[
            "index",
            "{project}",
            "--working-directory",
            ".",
            "--output",
            "{out}",
        ],
        project: Some(dotnet_solution),
        per_package: false,
        address_space_cap: false,
        // MSBuild reads environment variables as properties. NuGet's
        // vulnerability audit makes MSBuildWorkspace report the project as
        // failed to load, though it still indexes; an index isn't a build.
        env: &[("NuGetAudit", "false")],
        trailing: None,
        load_failure: Some("Msbuild failed when processing the file"),
    },
    Indexer {
        // Needs `.dart_tool/package_config.json` — run `dart pub get` /
        // `flutter pub get` first. Takes the package root as its
        // positional argument, so it runs once per pubspec.yaml.
        lang: "dart",
        bin: "scip_dart",
        markers: &["pubspec.yaml"],
        sources: &["dart"],
        args: &["--output", "{out}", "{project}"],
        project: None,
        per_package: true,
        address_space_cap: false,
        env: &[],
        trailing: None,
        load_failure: None,
    },
    Indexer {
        // Runs the Gradle / Maven / sbt build with the SemanticDB compiler
        // plugin injected, so the repo must compile (JDK on PATH).
        lang: "java",
        bin: "scip-java",
        markers: &[
            "settings.gradle",
            "settings.gradle.kts",
            "build.gradle",
            "build.gradle.kts",
            "pom.xml",
        ],
        sources: &["java"],
        args: &["index", "--output", "{out}"],
        project: None,
        per_package: false,
        address_space_cap: false,
        // scip-java's Gradle plugin reads `Task.project` at execution
        // time, which fails when the repo enables the configuration cache
        // (`org.gradle.configuration-cache=true`). Maven ignores this.
        env: &[("GRADLE_OPTS", "-Dorg.gradle.configuration-cache=false")],
        trailing: Some(java_trailing),
        load_failure: None,
    },
    Indexer {
        lang: "python",
        bin: "scip-python",
        markers: &["pyproject.toml", "setup.py"],
        sources: &["py"],
        args: &["index", ".", "--output", "{out}"],
        project: None,
        per_package: false,
        address_space_cap: false,
        env: &[],
        trailing: None,
        load_failure: None,
    },
];

/// Produces `.doc-lint/code.scip` for every indexable language in the
/// repo; `cmd_check` then ingests it. Each indexer writes
/// `.doc-lint/scip/<lang>.scip`, and the merged file is their
/// concatenation — protobuf merges concatenated messages by appending
/// repeated fields, so the result is one `Index` holding every
/// language's documents. A missing or failing indexer is reported and
/// skipped; the command fails if any detected language produced no index.
///
/// **Memory-heavy by nature** (rust-analyzer holds the whole-workspace
/// symbol table, 4–6 GiB; Roslyn is similar on big solutions). Defense
/// in depth:
///
///   1. Pre-flight: refuse to launch if `MemAvailable < 4 GiB`
///      (`DOC_LINTER_SCIP_MIN_FREE_MB`). Stop docker and close
///      memory-hungry apps first.
///   2. `prlimit --as=4G` on rust-analyzer (`DOC_LINTER_SCIP_MEMORY_MB`) —
///      hitting the cap OOM-kills only the indexer.
///   3. `nice -n 19` + `ionice -c 3` on every indexer.
///
/// Future AI agents: do NOT bypass the pre-flight check. It exists
/// because the memory cap alone wasn't enough on a swap-saturated box.
/// @endpoint CLI scip-index
pub(crate) fn run(root: &Path, config: &LintConfig) -> Result<ExitCode> {
    let scip_path = scip_ingest::default_scip_path(root);
    let parts_dir = root.join(".doc-lint").join("scip");
    std::fs::create_dir_all(&parts_dir)
        .with_context(|| format!("create {}", parts_dir.display()))?;
    if !memory_preflight_ok() {
        return Ok(ExitCode::FAILURE);
    }
    let memory_mb: u64 = std::env::var("DOC_LINTER_SCIP_MEMORY_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4096);

    let present = source_extensions(root, config);
    let mut produced: Vec<PathBuf> = Vec::new();
    let mut failed: Vec<&str> = Vec::new();
    'indexers: for ix in INDEXERS {
        let has_sources = ix.sources.iter().any(|e| present.contains(*e));
        let Some(projects) = find_markers(root, config, ix.markers) else {
            // Sources without a project file used to be skipped silently,
            // so a repo's TypeScript just never showed up in the graph.
            if has_sources {
                eprintln!("doc-linter: {}", no_marker_note(ix));
            }
            continue;
        };
        if !has_sources {
            continue;
        }
        let Some(bin) = indexer_bin(ix) else {
            eprintln!(
                "doc-linter: {} files found but `{}` is not on PATH — {} not indexed. \
                 Install it, put it on PATH, or set {} to its path.",
                ix.lang,
                ix.bin,
                ix.lang,
                bin_env_var(ix)
            );
            failed.push(ix.lang);
            continue;
        };
        // One run per project directory for indexers that take a package
        // root (scip_dart); one run over the whole repo otherwise.
        let runs = project_runs(ix, &projects);
        for (i, package) in runs.iter().enumerate() {
            let out = if runs.len() > 1 {
                parts_dir.join(format!("{}-{i}.scip", ix.lang))
            } else {
                parts_dir.join(format!("{}.scip", ix.lang))
            };
            let _ = std::fs::remove_file(&out);
            let mut cmd = idle_command(&bin, ix.address_space_cap.then_some(memory_mb));
            for arg in ix.args {
                match *arg {
                    "{out}" => {
                        cmd.arg(&out);
                    }
                    "{project}" => {
                        let project = match package {
                            Some(dir) => Some(Ok(dir.clone())),
                            None => ix.project.map(|f| f(root, config, &parts_dir, &projects)),
                        };
                        match project {
                            Some(Ok(p)) => {
                                cmd.arg(p);
                            }
                            None => {}
                            Some(Err(e)) => {
                                eprintln!("doc-linter: {} indexer skipped — {e:#}", ix.lang);
                                failed.push(ix.lang);
                                continue 'indexers;
                            }
                        }
                    }
                    a => {
                        cmd.arg(a);
                    }
                }
            }
            if let Some(trailing) = ix.trailing {
                cmd.args(trailing(&projects));
            }
            for (key, value) in ix.env {
                match std::env::var(key) {
                    Ok(prev) if !prev.is_empty() && key.ends_with("_OPTS") => {
                        cmd.env(key, format!("{prev} {value}"));
                    }
                    Ok(prev) if !prev.is_empty() => {}
                    _ => {
                        cmd.env(key, value);
                    }
                }
            }
            match package {
                Some(dir) => eprintln!(
                    "doc-linter: indexing {} ({}) with {} …",
                    ix.lang,
                    dir.display(),
                    ix.bin
                ),
                None => eprintln!("doc-linter: indexing {} with {} …", ix.lang, ix.bin),
            }
            cmd.current_dir(root);
            let (status, hints) = run_summarising(&mut cmd, ix.lang, ix.load_failure)
                .with_context(|| format!("spawn {}", bin.display()))?;
            if status.success() && out.is_file() {
                if let Err(e) = rebase_document_paths(&out, root) {
                    eprintln!("doc-linter: {} index paths left as-is — {e:#}", ix.lang);
                }
                produced.push(out);
            } else {
                let oom = ix.address_space_cap && matches!(status.code(), None | Some(137));
                eprintln!(
                    "doc-linter: {} indexer exited with {status}{}",
                    ix.lang,
                    if oom {
                        format!(
                            " — likely the {memory_mb} MiB cap; raise DOC_LINTER_SCIP_MEMORY_MB"
                        )
                    } else {
                        String::new()
                    }
                );
                for hint in hints {
                    eprintln!("doc-linter: hint: {hint}");
                }
                failed.push(ix.lang);
            }
        }
    }

    if produced.is_empty() {
        eprintln!(
            "doc-linter: no SCIP index produced; nothing written to {}",
            scip_path.display()
        );
        return Ok(ExitCode::FAILURE);
    }
    let mut merged = Vec::new();
    for part in &produced {
        merged.extend(std::fs::read(part).with_context(|| format!("read {}", part.display()))?);
    }
    std::fs::write(&scip_path, &merged)
        .with_context(|| format!("write {}", scip_path.display()))?;
    eprintln!(
        "doc-linter: wrote {} ({} bytes from {} indexer(s))",
        scip_path.display(),
        merged.len(),
        produced.len()
    );
    Ok(if failed.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The marker files of the first marker kind present within
/// [`MARKER_DEPTH`] (skip-dirs honoured), repo-relative; `None` when the
/// language is absent.
fn find_markers(root: &Path, config: &LintConfig, markers: &[&str]) -> Option<Vec<PathBuf>> {
    let files: Vec<PathBuf> = WalkDir::new(root)
        .max_depth(MARKER_DEPTH)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
        .flatten()
        .filter(|e| e.file_type().is_file())
        .map(|e| {
            e.path()
                .strip_prefix(root)
                .unwrap_or(e.path())
                .to_path_buf()
        })
        .collect();
    markers.iter().find_map(|m| {
        let hits: Vec<PathBuf> = files
            .iter()
            .filter(|f| {
                let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if m.starts_with('.') {
                    name.ends_with(m)
                } else {
                    name == *m
                }
            })
            .cloned()
            .collect();
        (!hits.is_empty()).then_some(hits)
    })
}

/// `DOC_LINTER_<BIN>` (e.g. `DOC_LINTER_SCIP_TYPESCRIPT`): an explicit
/// indexer path for setups where a global install isn't on PATH (proto
/// shims, `~/.pub-cache/bin`).
fn bin_env_var(ix: &Indexer) -> String {
    format!("DOC_LINTER_{}", ix.bin.to_uppercase().replace('-', "_"))
}

/// The indexer binary: [`bin_env_var`] when it names a file, else PATH.
fn indexer_bin(ix: &Indexer) -> Option<PathBuf> {
    std::env::var_os(bin_env_var(ix))
        .map(PathBuf::from)
        .filter(|p| p.is_file())
        .or_else(|| which::which(ix.bin).ok())
}

/// Known "prerequisite missing" indexer messages: `(lang, output substring,
/// one-line hint)`. A hint is printed only when the indexer fails.
const PREREQ_HINTS: &[(&str, &str, &str)] = &[
    (
        "dart",
        "Unable to locate packageConfig",
        "run `dart pub get` (or `flutter pub get`) first",
    ),
    (
        "csharp",
        "NuGet package restore",
        "run `dotnet restore` first",
    ),
    (
        "typescript",
        "Cannot find module",
        "run `npm install` (or yarn/pnpm install) first",
    ),
];

fn prereq_hints(lang: &str, lines: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (l, pat, hint) in PREREQ_HINTS {
        if *l == lang && lines.iter().any(|x| x.contains(pat)) && !out.contains(hint) {
            out.push(hint);
        }
    }
    out
}

/// Run `cmd`, passing its output through except lines containing
/// `pattern` (if any), which are counted and summarised once at the end.
/// Also returns the [`PREREQ_HINTS`] matching its output.
fn run_summarising(
    cmd: &mut Command,
    lang: &str,
    pattern: Option<&str>,
) -> std::io::Result<(std::process::ExitStatus, Vec<&'static str>)> {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let filter = move |stream: Box<dyn std::io::Read + Send>, to_stderr: bool| {
        let pattern = pattern.map(str::to_string);
        std::thread::spawn(move || {
            let mut hits: Vec<String> = Vec::new();
            let mut all: Vec<String> = Vec::new();
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                all.push(line.clone());
                if pattern.as_deref().is_some_and(|p| line.contains(p)) {
                    hits.push(line);
                } else if to_stderr {
                    eprintln!("{line}");
                } else {
                    println!("{line}");
                }
            }
            (hits, all)
        })
    };
    let out = child.stdout.take().map(|s| filter(Box::new(s), false));
    let err = child.stderr.take().map(|s| filter(Box::new(s), true));
    let status = child.wait()?;
    let (hits, all): (Vec<Vec<String>>, Vec<Vec<String>>) = [out, err]
        .into_iter()
        .flatten()
        .map(|h| h.join().unwrap_or_default())
        .unzip();
    let hits: Vec<String> = hits.concat();
    if let Some(summary) = load_failure_summary(lang, &hits) {
        eprintln!("doc-linter: {summary}");
    }
    Ok((status, prereq_hints(lang, &all.concat())))
}

/// "N project(s) failed to load" with the first few project file names,
/// from the matching output lines. `None` when there were none.
fn load_failure_summary(lang: &str, hits: &[String]) -> Option<String> {
    if hits.is_empty() {
        return None;
    }
    let mut names: Vec<&str> = hits
        .iter()
        .filter_map(|l| l.split('\'').nth(1))
        .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p))
        .collect();
    names.sort_unstable();
    names.dedup();
    let shown = names.iter().take(3).copied().collect::<Vec<_>>().join(", ");
    Some(format!(
        "{lang}: MSBuild reported {} project(s) as failing to load ({shown}{}), usually over \
         restore warnings; their symbols are normally still indexed",
        if names.is_empty() {
            hits.len()
        } else {
            names.len()
        },
        if names.len() > 3 { ", …" } else { "" }
    ))
}

/// The `{project}` of each run of `ix`: every marker's directory for a
/// [`Indexer::per_package`] indexer, else one run resolved by
/// [`Indexer::project`] (`None`).
fn project_runs(ix: &Indexer, markers: &[PathBuf]) -> Vec<Option<PathBuf>> {
    if !ix.per_package {
        return vec![None];
    }
    markers
        .iter()
        .map(|m| {
            let dir = m.parent().unwrap_or(Path::new(""));
            Some(if dir.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                dir.to_path_buf()
            })
        })
        .collect()
}

/// Make every document path in the SCIP part at `part` relative to
/// `root`. Indexers record paths relative to their own project root
/// (`metadata.project_root`): scip_dart's are package-relative (`lib/…`),
/// so they never matched the `pkg/lib/…` File rows. Must happen per part:
/// the merged file concatenates `Index` messages, and their `metadata`
/// fields merge into one, losing every project root but the last.
fn rebase_document_paths(part: &Path, root: &Path) -> Result<()> {
    use protobuf::Message;
    use scip::types::{Index, Metadata};

    let bytes = std::fs::read(part).with_context(|| format!("read {}", part.display()))?;
    // Cheap check first: `metadata` is field 1, serialized ahead of the
    // documents, so its project root is readable without decoding a
    // multi-hundred-MB index that needs no rewrite.
    let metadata = leading_field_1(&bytes).and_then(|m| Metadata::parse_from_bytes(m).ok());
    let project_root = match metadata {
        Some(m) => m.project_root,
        None => Index::parse_from_bytes(&bytes)?
            .metadata
            .project_root
            .clone(),
    };
    let Some(project) = file_uri_to_path(&project_root) else {
        return Ok(());
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let project = project.canonicalize().unwrap_or(project);
    let Ok(prefix) = project.strip_prefix(&root) else {
        return Ok(()); // indexed outside the root: leave as recorded
    };
    if prefix.as_os_str().is_empty() {
        return Ok(()); // already root-relative
    }
    let mut index = Index::parse_from_bytes(&bytes)?;
    for doc in &mut index.documents {
        let rel = lexical_normalize(&prefix.join(&doc.relative_path));
        doc.relative_path = rel.to_string_lossy().replace('\\', "/");
    }
    index.metadata.mut_or_insert_default().project_root = format!("file://{}", root.display());
    std::fs::write(part, index.write_to_bytes()?)
        .with_context(|| format!("write {}", part.display()))?;
    Ok(())
}

/// The payload of a length-delimited field 1 at the start of `bytes`.
fn leading_field_1(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.first() != Some(&0x0a) {
        return None;
    }
    let mut len: usize = 0;
    for (i, b) in bytes.iter().skip(1).take(10).enumerate() {
        len |= usize::from(b & 0x7f) << (7 * i);
        if b & 0x80 == 0 {
            let start = i + 2;
            return bytes.get(start..start.checked_add(len)?);
        }
    }
    None
}

/// `file:///a/b%20c/` → `/a/b c`. `None` for anything else (empty, other
/// schemes).
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    tower_lsp::lsp_types::Url::parse(uri)
        .ok()?
        .to_file_path()
        .ok()
}

/// Resolve `.` and `..` without touching the filesystem.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Every file extension present outside the skip dirs, in one walk.
fn source_extensions(root: &Path, config: &LintConfig) -> std::collections::HashSet<String> {
    WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !config.should_skip_dir(e.path(), root))
        .flatten()
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(str::to_string)
        })
        .collect()
}

/// Why a language with sources on disk wasn't indexed: none of its
/// project files is within [`MARKER_DEPTH`] levels of the root.
fn no_marker_note(ix: &Indexer) -> String {
    format!(
        "{} sources found but no {} within {MARKER_DEPTH} levels of the root — not indexed. \
         Add one (for TypeScript, a tsconfig.json) to index them.",
        ix.lang,
        ix.markers.join(" / ")
    )
}

/// The TypeScript project to index: the repo root when its own
/// `tsconfig.json` is usable. When there is none (a JS project), or it
/// extends a file that isn't there (Nuxt's `.nuxt/tsconfig.json` only
/// exists after `nuxi prepare`), a generated `<dir>/tsconfig.json` over
/// every TS / JS source stands in — never `--infer-tsconfig`, which
/// writes a tsconfig into the repo. Paths and aliases go unresolved;
/// definitions and docs still index.
fn ts_project(
    root: &Path,
    config: &LintConfig,
    dir: &Path,
    _markers: &[PathBuf],
) -> Result<PathBuf> {
    let own = std::fs::read_to_string(root.join("tsconfig.json")).ok();
    let broken_extends = regex::Regex::new(r#""extends"\s*:\s*"(\.[^"]+)""#)
        .context("extends regex")?
        .captures(own.as_deref().unwrap_or_default())
        .and_then(|c| c.get(1))
        .is_some_and(|m| !root.join(m.as_str()).exists());
    if own.is_some() && !broken_extends {
        return Ok(PathBuf::from("."));
    }
    let up = "../".repeat(dir.strip_prefix(root).map_or(2, |d| d.components().count()));
    // `skip_dirs` (directory names, any depth) is excluded like the
    // built-in output dirs, so the indexer doesn't walk trees the graph
    // drops anyway.
    let mut exclude: Vec<String> = ["node_modules", ".nuxt", ".output", "dist"]
        .iter()
        .map(|d| (*d).to_string())
        .chain(config.skip_dirs.iter().cloned())
        .collect();
    exclude.sort();
    exclude.dedup();
    let mut exclude: Vec<String> = exclude.iter().map(|d| format!("\"{up}**/{d}\"")).collect();
    exclude.push(format!("\"{up}.doc-lint\""));
    let tsconfig = format!(
        r#"{{"compilerOptions": {{"allowJs": true, "noEmit": true, "skipLibCheck": true}},
 "include": ["{up}**/*.ts", "{up}**/*.tsx", "{up}**/*.js", "{up}**/*.mjs"],
 "exclude": [{}]}}
"#,
        exclude.join(", ")
    );
    std::fs::write(dir.join("tsconfig.json"), tsconfig)
        .with_context(|| format!("write {}", dir.display()))?;
    Ok(dir.strip_prefix(root).unwrap_or(dir).to_path_buf())
}

/// Build-tool arguments for scip-java. On Gradle, test sources are not
/// compiled: the graph excludes tests, and a test source set whose javac
/// plugins are replaced (errorprone does this) fails with `plug-in not
/// found: semanticdb`. Maven takes no extras. Trailing arguments replace
/// scip-java's default Gradle task list, so it is restated here.
fn java_trailing(markers: &[PathBuf]) -> &'static [&'static str] {
    let gradle = markers.iter().any(|m| {
        m.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.contains("gradle"))
    });
    if gradle {
        &[
            "--",
            "clean",
            "scipPrintDependencies",
            "scipCompileAll",
            "-x",
            "compileTestJava",
        ]
    } else {
        &[]
    }
}

/// Writes `<dir>/all.sln` listing `projects` (repo-relative), via the
/// dotnet CLI that scip-dotnet itself needs.
fn dotnet_solution(
    root: &Path,
    _config: &LintConfig,
    dir: &Path,
    projects: &[PathBuf],
) -> Result<PathBuf> {
    let sln = dir.join("all.sln");
    let _ = std::fs::remove_file(&sln);
    let steps: [Vec<std::ffi::OsString>; 2] = [
        ["new", "sln", "--format", "sln", "-n", "all", "-o"]
            .map(Into::into)
            .into_iter()
            .chain([dir.as_os_str().to_owned()])
            .collect(),
        ["sln".into(), sln.clone().into_os_string(), "add".into()]
            .into_iter()
            .chain(projects.iter().map(|p| p.clone().into_os_string()))
            .collect(),
    ];
    for args in steps {
        let out = Command::new("dotnet")
            .args(&args)
            .current_dir(root)
            .output()
            .context("run dotnet (needed to build the solution scip-dotnet indexes)")?;
        anyhow::ensure!(
            out.status.success(),
            "dotnet {} failed: {}",
            args.first()
                .map(|a| a.to_string_lossy())
                .unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(sln)
}

/// `ionice -c 3 nice -n 19 [prlimit --as=N] <bin>`, each wrapper only
/// when it is on PATH.
fn idle_command(bin: &Path, cap_mb: Option<u64>) -> Command {
    let mut chain: Vec<std::ffi::OsString> = Vec::new();
    if let Ok(ionice) = which::which("ionice") {
        chain.extend([ionice.into_os_string(), "-c".into(), "3".into()]);
    }
    if let Ok(nice) = which::which("nice") {
        chain.extend([nice.into_os_string(), "-n".into(), "19".into()]);
    }
    if let Some(mb) = cap_mb.filter(|_| cfg!(target_os = "macos")) {
        // macOS doesn't enforce RLIMIT_AS (setrlimit accepts it, nothing
        // checks it), so there is no cap to apply; say so instead of
        // implying one.
        eprintln!(
            "doc-linter: note — macOS has no address-space cap; {} runs without the \
             {mb} MiB limit. The free-memory pre-flight still applies.",
            bin.display()
        );
    } else if let Some(mb) = cap_mb {
        match which::which("prlimit") {
            Ok(prlimit) => chain.extend([
                prlimit.into_os_string(),
                format!("--as={}", mb * 1024 * 1024).into(),
            ]),
            Err(_) => eprintln!(
                "doc-linter: WARNING — `prlimit` not on PATH; running {} without a \
                 memory cap. Install util-linux to get prlimit.",
                bin.display()
            ),
        }
    }
    chain.push(bin.as_os_str().to_owned());
    let mut cmd = Command::new(&chain[0]);
    cmd.args(&chain[1..]);
    cmd
}

/// Refuses to start when `MemAvailable` is under
/// `DOC_LINTER_SCIP_MIN_FREE_MB` (default 4 GiB): Linux `MemAvailable`,
/// or the `vm_stat` equivalent on macOS. Hosts with neither proceed with
/// a warning.
fn memory_preflight_ok() -> bool {
    let min_free_mb: u64 = std::env::var("DOC_LINTER_SCIP_MIN_FREE_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4096);
    match read_mem_available_mb() {
        Some(avail_mb) if avail_mb < min_free_mb => {
            eprintln!(
                "doc-linter: REFUSING to run scip-index — only {avail_mb} MiB \
                 MemAvailable, need ≥ {min_free_mb} MiB. Free RAM first:\n\n  \
                 docker stop $(docker ps -q)\n  \
                 pkill -f 'rust-analyzer'   # close VS Code / IDE first\n\n\
                 The gate is load-bearing: the indexer has crashed a 16 GiB box \
                 with swap already saturated. Override via DOC_LINTER_SCIP_MIN_FREE_MB."
            );
            false
        }
        Some(avail_mb) => {
            eprintln!("doc-linter: pre-flight OK — {avail_mb} MiB MemAvailable (≥ {min_free_mb} required)");
            true
        }
        None => {
            eprintln!(
                "doc-linter: WARNING — could not read available memory (/proc/meminfo or \
                 vm_stat); proceeding without the memory pre-flight. Stop docker + close \
                 heavy apps first."
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_skips_test_compile_on_gradle_only() {
        let gradle = [
            PathBuf::from("settings.gradle"),
            PathBuf::from("build.gradle.kts"),
        ];
        assert_eq!(
            java_trailing(&gradle),
            [
                "--",
                "clean",
                "scipPrintDependencies",
                "scipCompileAll",
                "-x",
                "compileTestJava"
            ]
        );
        assert!(java_trailing(&[PathBuf::from("pom.xml")]).is_empty());
    }

    /// scip_dart runs once per pubspec.yaml directory; other indexers once.
    #[test]
    fn dart_runs_once_per_package() {
        let dart = INDEXERS.iter().find(|i| i.lang == "dart").unwrap();
        let runs = project_runs(
            dart,
            &[
                PathBuf::from("pubspec.yaml"),
                PathBuf::from("apps/fa/pubspec.yaml"),
            ],
        );
        assert_eq!(
            runs,
            [Some(PathBuf::from(".")), Some(PathBuf::from("apps/fa"))]
        );
        let rust = INDEXERS.iter().find(|i| i.lang == "rust").unwrap();
        assert_eq!(project_runs(rust, &[PathBuf::from("Cargo.toml")]), [None]);
    }

    /// Gap 8: scip_dart records `lib/…` relative to the package; after the
    /// rebase the paths are root-relative (`fa_app/lib/…`).
    #[test]
    fn document_paths_are_rebased_onto_the_root() {
        use protobuf::Message;
        use scip::types::{Document, Index};
        let root = std::env::temp_dir().join(format!("doc-linter-rebase-{}", std::process::id()));
        let pkg = root.join("fa_app");
        std::fs::create_dir_all(&pkg).unwrap();
        let mut index = Index::new();
        index.metadata.mut_or_insert_default().project_root =
            format!("file://{}/", pkg.canonicalize().unwrap().display());
        let mut doc = Document::new();
        doc.relative_path = "lib/src/pricing.dart".to_string();
        index.documents.push(doc);
        let part = root.join("dart.scip");
        std::fs::write(&part, index.write_to_bytes().unwrap()).unwrap();

        rebase_document_paths(&part, &root).unwrap();
        let rebased = Index::parse_from_bytes(&std::fs::read(&part).unwrap()).unwrap();
        assert_eq!(
            rebased.documents[0].relative_path,
            "fa_app/lib/src/pricing.dart"
        );

        // A second pass is a no-op: the project root is now the repo root.
        rebase_document_paths(&part, &root).unwrap();
        let again = Index::parse_from_bytes(&std::fs::read(&part).unwrap()).unwrap();
        assert_eq!(
            again.documents[0].relative_path,
            "fa_app/lib/src/pricing.dart"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// Gap 3: the generated tsconfig excludes `skip_dirs`.
    #[test]
    fn generated_tsconfig_excludes_skip_dirs() {
        let root = std::env::temp_dir().join(format!("doc-linter-tsx-{}", std::process::id()));
        let dir = root.join(".doc-lint/scip");
        std::fs::create_dir_all(&dir).unwrap();
        let config: LintConfig = toml::from_str("skip_dirs = [\"legacy\"]").unwrap();
        ts_project(&root, &config, &dir, &[]).unwrap();
        let tsconfig = std::fs::read_to_string(dir.join("tsconfig.json")).unwrap();
        assert!(tsconfig.contains("\"../../**/legacy\""), "{tsconfig}");
        assert!(tsconfig.contains("\"../../**/node_modules\""), "{tsconfig}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Gap 4: the missing-indexer message named rust-analyzer for any
    /// language; the override variable is derived from the binary name.
    #[test]
    fn indexer_override_env_var_is_named_after_the_binary() {
        let ts = INDEXERS.iter().find(|i| i.lang == "typescript").unwrap();
        assert_eq!(bin_env_var(ts), "DOC_LINTER_SCIP_TYPESCRIPT");
        let dart = INDEXERS.iter().find(|i| i.lang == "dart").unwrap();
        assert_eq!(bin_env_var(dart), "DOC_LINTER_SCIP_DART");
    }

    #[test]
    fn missing_prerequisite_gets_a_hint() {
        let out = vec!["ERROR: Unable to locate packageConfig".to_string()];
        assert_eq!(
            prereq_hints("dart", &out),
            ["run `dart pub get` (or `flutter pub get`) first"]
        );
        assert!(prereq_hints("rust", &out).is_empty());
        assert!(prereq_hints("dart", &[]).is_empty());
    }

    /// Gap 6: one summary line instead of one line per failed project.
    #[test]
    fn load_failures_are_summarised() {
        let hits: Vec<String> = ["A", "B", "B", "C", "D"]
            .iter()
            .map(|p| {
                format!(
                    "07:16:08 fail: Microsoft.CodeAnalysis.MSBuild.MSBuildWorkspace[0] Failure: \
                     Msbuild failed when processing the file '/r/src/{p}/{p}.csproj' with message: …"
                )
            })
            .collect();
        let summary = load_failure_summary("csharp", &hits).unwrap();
        assert!(
            summary.starts_with(
                "csharp: MSBuild reported 4 project(s) as failing to load (A.csproj, B.csproj, C.csproj, …)"
            ),
            "{summary}"
        );
        assert!(load_failure_summary("csharp", &[]).is_none());
    }

    /// Gap 1: macOS has no /proc/meminfo; the pre-flight reads vm_stat.
    /// Sample from a Darwin arm64 host (16 KiB pages).
    #[test]
    fn vm_stat_available_memory() {
        let text = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                               12000.\n\
Pages active:                            400000.\n\
Pages inactive:                          150000.\n\
Pages speculative:                         2000.\n\
Pages throttled:                              0.\n\
Pages wired down:                        120000.\n";
        // (12000 + 150000 + 2000) * 16384 / 2^20 = 2562 MiB
        assert_eq!(parse_vm_stat_available_mb(text), Some(2562));
        assert_eq!(parse_vm_stat_available_mb("garbage"), None);
    }
}
