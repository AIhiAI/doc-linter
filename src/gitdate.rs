//! Last-commit dates for tracked files, so freshness survives a fresh
//! clone (where every file mtime is "today"). One batched `git log`
//! pass per repository, cached for the process; shallow clones and
//! non-git trees return `None` and callers fall back to the mtime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

type Dates = Arc<HashMap<String, i64>>;

/// Unix seconds of the newest commit touching `path`, or `None` when
/// `path` is outside a full-history git checkout, untracked, or the
/// `git` binary is unavailable.
pub fn last_commit_secs(path: &Path) -> Option<i64> {
    let abs = path.canonicalize().ok()?;
    let top = abs.ancestors().skip(1).find(|d| d.join(".git").exists())?;
    let dates = repo_dates(top)?;
    let rel = abs.strip_prefix(top).ok()?.to_str()?.replace('\\', "/");
    dates.get(&rel).copied()
}

fn repo_dates(top: &Path) -> Option<Dates> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Dates>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Mutex::default);
    // ponytail: lock held across the git run so concurrent callers do not
    // each spawn one; lookups after the first are instant.
    let mut guard = cache.lock().ok()?;
    guard
        .entry(top.to_path_buf())
        .or_insert_with(|| load(top).map(Arc::new))
        .clone()
}

fn load(top: &Path) -> Option<HashMap<String, i64>> {
    // A shallow clone's one grafted commit "touches" every file.
    if top.join(".git/shallow").exists() {
        return None;
    }
    let out = Command::new("git")
        .args([
            "-c",
            "core.quotepath=off",
            "log",
            "--no-renames",
            "--name-only",
            "--pretty=format:@%ct",
        ])
        .current_dir(top)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_log(&String::from_utf8_lossy(&out.stdout)))
}

/// Newest-first `@<ts>` / path lines to path → newest timestamp.
fn parse_log(log: &str) -> HashMap<String, i64> {
    let mut dates = HashMap::new();
    let mut ts = 0;
    for line in log.lines().filter(|l| !l.is_empty()) {
        if let Some(t) = line.strip_prefix('@') {
            ts = t.parse().unwrap_or(0);
        } else {
            dates.entry(line.to_string()).or_insert(ts);
        }
    }
    dates
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_commit_wins() {
        let d = parse_log("@300\na.md\nb.md\n\n@100\na.md\nc.md\n");
        assert_eq!((d["a.md"], d["b.md"], d["c.md"]), (300, 300, 100));
    }

    #[test]
    fn real_repo_dates_beat_checkout_mtime() {
        let dir = std::env::temp_dir().join(format!("dl-gitdate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&dir)
                .env("GIT_COMMITTER_DATE", "2020-01-02T03:04:05Z")
                .env("GIT_AUTHOR_DATE", "2020-01-02T03:04:05Z")
                .status()
                .map(|s| s.success());
            assert_eq!(ok.ok(), Some(true), "git {args:?}");
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("a.md"), "x").unwrap();
        std::fs::write(dir.join("untracked.md"), "x").unwrap();
        git(&["add", "a.md"]);
        git(&["commit", "-q", "-m", "init"]);
        assert_eq!(last_commit_secs(&dir.join("a.md")), Some(1_577_934_245));
        assert_eq!(last_commit_secs(&dir.join("untracked.md")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
