//! Roadmap issue #38 (v0.3.0): COUPLED_WITH edges from git
//! co-change history.
//!
//! Two files that repeatedly change in the same commits are
//! conceptually coupled even if there's no explicit import or call
//! edge between them — and that signal predicts what an agent should
//! look at when modifying a file. Pre-#38 the graph carried no
//! co-change information; this ingest adds it as a derived File→File
//! edge.
//!
//! ## Pipeline
//!
//! 1. Read the File table to bound the search to files the graph
//!    already knows about. Files that no longer exist on disk but
//!    appear in `git log` get filtered out at this step — no point
//!    surfacing coupling on a deleted file.
//! 2. Spawn `git log --since='6 months ago' --name-only
//!    --pretty=format:%h|%at|`. Parse the output into per-commit
//!    file sets. Each commit emits one timestamp (Unix epoch
//!    seconds; we convert to ISO-8601 at write time).
//! 3. For each commit, generate pairs of files touched in that
//!    commit. Accumulate a per-pair commit count + the max
//!    timestamp seen. This is O(sum_per_commit (k_i choose 2)),
//!    which scales with how broad each commit is — much better
//!    than O(N^2) over all files.
//! 4. For each pair (A, B) with `commits >= MIN_COMMITS` and
//!    `jaccard >= MIN_JACCARD`, emit one COUPLED_WITH edge where
//!    `FROM` < `TO` alphabetically (so the edge set is canonical;
//!    SQL queries match it undirected via `()-[:COUPLED_WITH]-()`).
//!
//! ## Tunables
//!
//! The thresholds and window are hard-coded for v1. The issue calls
//! out reasonable defaults (≥3 commits, ≥0.2 jaccard, 6-month
//! window); a follow-up can promote them to `LintConfig` if repos
//! want to tune.
//!
//! ## Bounding
//!
//! Best-effort throughout. `git log` failures (non-repo, shallow
//! clone with no history, missing `git` binary) are logged and the
//! ingest no-ops — the rest of the graph build continues. Same
//! posture as `scip_is_stale`: never let an opportunistic data
//! source break the linter.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;
use std::process::Command;

/// Stats returned to `cmd_check` for the stderr one-liner.
#[derive(Debug, Default, Clone, Copy)]
pub struct CouplingIngestStats {
    /// Number of commits walked from `git log` output.
    pub commits_scanned: usize,
    /// Total candidate pairs found (commits >= 1).
    pub pairs_examined: usize,
    /// Pairs that passed both thresholds and got an edge.
    pub coupled_with_edges: usize,
}

/// Minimum number of co-touching commits before a pair is treated as
/// coupled. Mirrors the issue's recommendation.
pub(crate) const MIN_COMMITS: u32 = 3;
/// Minimum jaccard similarity (intersection / union) before a pair
/// is treated as coupled. Mirrors the issue's recommendation.
pub(crate) const MIN_JACCARD: f64 = 0.2;
/// History window passed to `git log --since=...`.
const WINDOW_ARG: &str = "6 months ago";
/// Commits touching more tracked files than this are skipped: mass edits
/// (license bumps, reformats, a shallow clone's single root commit) say
/// nothing about coupling and cost `k²` pairs — one 7,000-file commit is
/// 24.5M pairs. Same idea as code-maat's `max-changeset-size`.
const MAX_FILES_PER_COMMIT: usize = 50;

/// Invoke `git log` and capture stdout. Returns `None` (with a
/// stderr note) on any failure mode — git not on PATH, not a repo,
/// shallow clone, etc. The rest of the ingest no-ops on `None`.
pub(crate) fn run_git_log(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args([
            "log",
            "--since",
            WINDOW_ARG,
            "--name-only",
            "--pretty=format:%h|%at|",
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        eprintln!(
            "doc-linter: coupling ingest — `git log` exited non-zero ({}); skipping COUPLED_WITH derivation",
            output.status
        );
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// One commit's worth of co-changed files + the commit timestamp.
pub(crate) struct CommitFiles {
    /// Unix epoch seconds; converted to ISO-8601 only when we write
    /// `last_co_change_at`.
    timestamp: u64,
    files: Vec<String>,
}

/// Parse the `git log --name-only --pretty=format:%h|%at|` output.
///
/// Each commit starts with a header line `<hash>|<unix_ts>|`
/// followed by blank-separated filenames, then a blank line before
/// the next commit. Files not in `file_paths` are filtered out at
/// parse time — that keeps the per-commit vectors small for repos
/// with lots of non-source files (docs, generated artifacts, etc).
pub(crate) fn parse_git_log(output: &str, file_paths: &HashSet<String>) -> Vec<CommitFiles> {
    let mut commits: Vec<CommitFiles> = Vec::new();
    let mut current_ts: Option<u64> = None;
    let mut current_files: Vec<String> = Vec::new();

    let flush = |out: &mut Vec<CommitFiles>, ts: Option<u64>, files: &mut Vec<String>| {
        if let Some(ts) = ts {
            if files.is_empty() {
                files.clear();
            } else {
                let mut taken = Vec::new();
                std::mem::swap(&mut taken, files);
                out.push(CommitFiles {
                    timestamp: ts,
                    files: taken,
                });
            }
        }
    };

    for line in output.lines() {
        if let Some((hash_and_ts, rest)) = line.split_once('|') {
            // Header form `<hash>|<ts>|`.
            if let Some((ts_str, _)) = rest.split_once('|') {
                // Close the previous commit before starting a new one.
                flush(&mut commits, current_ts, &mut current_files);
                current_files.clear();
                current_ts = ts_str.trim().parse::<u64>().ok();
                let _ = hash_and_ts;
                continue;
            }
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if file_paths.contains(trimmed) {
            current_files.push(trimmed.to_string());
        }
    }
    flush(&mut commits, current_ts, &mut current_files);
    commits
}

/// Map from a canonical (a < b) file-pair to (co-change count, max
/// timestamp seen). The pair is the canonical key — only one entry
/// per unordered pair.
pub(crate) type PairCounts = BTreeMap<(String, String), (u32, u64)>;

/// Map from a file path to the set of commits (by position in the log)
/// that touched it. The set's `len()` is the jaccard denominator's
/// per-file side (combined via `union()` for the pair denominator). Keyed by
/// commit, not by timestamp: commits made within the same second (scripted
/// history, rebases) must still count separately, or the denominator
/// shrinks and jaccard exceeds 1.
pub(crate) type PerFileCommits = BTreeMap<String, BTreeSet<u64>>;

/// For each pair of files touched by a commit, accumulate (count,
/// max_ts). Also build the per-file set of all commits that touched
/// it (needed for the jaccard denominator).
pub(crate) fn aggregate_pairs(commits: &[CommitFiles]) -> (PairCounts, PerFileCommits) {
    let mut pair_counts: BTreeMap<(String, String), (u32, u64)> = BTreeMap::new();
    let mut per_file_commits: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();

    for (idx, commit) in commits
        .iter()
        .enumerate()
        .filter(|(_, c)| c.files.len() <= MAX_FILES_PER_COMMIT)
    {
        for f in &commit.files {
            per_file_commits
                .entry(f.clone())
                .or_default()
                .insert(idx as u64);
        }
        // Pairs within this commit: generate (a, b) with a < b
        // alphabetically so each unordered pair gets one canonical
        // key.
        let mut sorted: Vec<&String> = commit.files.iter().collect();
        sorted.sort();
        sorted.dedup();
        for i in 0..sorted.len() {
            for j in (i + 1)..sorted.len() {
                let key = (sorted[i].clone(), sorted[j].clone());
                let entry = pair_counts.entry(key).or_insert((0, 0));
                entry.0 += 1;
                if commit.timestamp > entry.1 {
                    entry.1 = commit.timestamp;
                }
            }
        }
    }
    (pair_counts, per_file_commits)
}

/// Format Unix epoch seconds as ISO-8601 UTC (`YYYY-MM-DDTHH:MM:SSZ`).
/// Mirrors `file_module_ingest::format_mtime` — kept inline rather
/// than pulled into a shared helper because the civil-from-days
/// arithmetic is small and the two ingest passes are independent.
pub(crate) fn format_iso8601(epoch_secs: u64) -> String {
    let days = epoch_secs / 86400;
    let secs_of_day = epoch_secs % 86400;
    let hour = (secs_of_day / 3600) as u8;
    let minute = ((secs_of_day % 3600) / 60) as u8;
    let second = (secs_of_day % 60) as u8;

    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y } as i32;
    format!(
        "{year:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        m as u8, d as u8, hour, minute, second
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "tests panic on broken invariants — that is the point of a test"
)]
mod tests {
    use super::*;

    #[test]
    fn format_iso8601_round_trips_known_epochs() {
        assert_eq!(format_iso8601(1_704_067_200), "2024-01-01T00:00:00Z");
        assert_eq!(format_iso8601(1_779_148_800), "2026-05-19T00:00:00Z");
    }

    #[test]
    fn parse_git_log_keeps_only_tracked_files() {
        let log = "\
abc|1700000000|
src/lib.rs
docs/foo.md
target/debug/build.rs

def|1700001000|
src/lib.rs
src/main.rs
README.md
";
        let mut tracked: HashSet<String> = HashSet::new();
        tracked.insert("src/lib.rs".to_string());
        tracked.insert("src/main.rs".to_string());

        let commits = parse_git_log(log, &tracked);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].timestamp, 1_700_000_000);
        assert_eq!(
            commits[0].files,
            vec!["src/lib.rs".to_string()],
            "untracked files filtered at parse time"
        );
        assert_eq!(commits[1].timestamp, 1_700_001_000);
        assert_eq!(
            commits[1].files,
            vec!["src/lib.rs".to_string(), "src/main.rs".to_string()]
        );
    }

    #[test]
    fn aggregate_pairs_canonical_order_and_count() {
        let commits = vec![
            CommitFiles {
                timestamp: 100,
                files: vec!["b.rs".to_string(), "a.rs".to_string()],
            },
            CommitFiles {
                timestamp: 200,
                files: vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()],
            },
        ];
        let (pairs, per_file) = aggregate_pairs(&commits);

        // (a.rs, b.rs) seen in both commits → count 2, max ts 200.
        let ab = pairs
            .get(&("a.rs".to_string(), "b.rs".to_string()))
            .copied();
        assert_eq!(ab, Some((2, 200)));
        // (a.rs, c.rs) only in commit 2 → count 1, ts 200.
        let ac = pairs
            .get(&("a.rs".to_string(), "c.rs".to_string()))
            .copied();
        assert_eq!(ac, Some((1, 200)));
        // No (b.rs, a.rs) key — alphabetical canonicalisation.
        assert!(!pairs.contains_key(&("b.rs".to_string(), "a.rs".to_string())));
        // Per-file commit sets exercise the jaccard denominator.
        assert_eq!(per_file.get("a.rs").map(BTreeSet::len), Some(2));
        assert_eq!(per_file.get("c.rs").map(BTreeSet::len), Some(1));
    }

    /// Commits within one second are still separate commits: the jaccard
    /// denominator counts them all (it used to collapse them by timestamp,
    /// pushing jaccard above 1 on scripted histories).
    #[test]
    fn same_second_commits_count_separately() {
        let at = |files: &[&str]| CommitFiles {
            timestamp: 100,
            files: files.iter().map(|f| (*f).to_string()).collect(),
        };
        let commits = vec![at(&["a.rs", "b.rs"]), at(&["a.rs", "b.rs"]), at(&["a.rs"])];
        let (pairs, per_file) = aggregate_pairs(&commits);
        assert_eq!(
            pairs
                .get(&("a.rs".to_string(), "b.rs".to_string()))
                .copied(),
            Some((2, 100))
        );
        assert_eq!(per_file.get("a.rs").map(BTreeSet::len), Some(3));
        assert_eq!(per_file.get("b.rs").map(BTreeSet::len), Some(2));
    }

    #[test]
    fn aggregate_pairs_skips_mass_edit_commits() {
        let commits = vec![CommitFiles {
            timestamp: 100,
            files: (0..=MAX_FILES_PER_COMMIT)
                .map(|i| format!("f{i}.rs"))
                .collect(),
        }];
        let (pairs, per_file) = aggregate_pairs(&commits);
        assert!(pairs.is_empty() && per_file.is_empty());
    }

    #[test]
    fn aggregate_pairs_dedupes_within_a_commit() {
        // Same file appearing twice in one commit's name-only output
        // (the same path can be listed once per mode change e.g.
        // rename source AND destination) shouldn't inflate counts.
        let commits = vec![CommitFiles {
            timestamp: 100,
            files: vec!["a.rs".to_string(), "a.rs".to_string(), "b.rs".to_string()],
        }];
        let (pairs, _) = aggregate_pairs(&commits);
        let ab = pairs
            .get(&("a.rs".to_string(), "b.rs".to_string()))
            .copied();
        assert_eq!(ab, Some((1, 100)));
        assert_eq!(pairs.len(), 1, "no (a.rs, a.rs) self-edge");
    }
}
