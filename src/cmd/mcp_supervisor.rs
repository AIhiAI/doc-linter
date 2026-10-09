//! gap-mcp-supervisor Slice B.2 — supervisor process.
//!
//! Reads JSON-RPC from stdin, writes responses to stdout. Holds the
//! stable Claude Code stdio connection. The work itself happens in
//! a child `doc-linter mcp-worker --socket <PATH>` process; the
//! supervisor proxies bytes in both directions over a unix-domain
//! socket. The architecture means a future `swap_worker` MCP tool
//! (Slice B.3) can SIGTERM the child and spawn a fresh one without
//! Claude Code's stdio FDs ever closing — fixing the
//! `reload_self`-drops-connection bug noted in gap-009 and
//! gap-mcp-supervisor.
//!
//! This slice ships a supervisor without swap support: spawn once,
//! proxy until either end closes, clean up. Swap lands separately.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// `doc-linter mcp-supervisor --root <root>` entry point.
///
/// gap-mcp-supervisor Slice B.3c: the supervisor is now a real
/// proxy with respawn-and-replay. Architecture:
///
/// - Main thread reads lines from stdin (Claude Code → us). For
///   each line, it tries to forward to the current worker socket.
///   Before forwarding, it snapshots the FIRST `initialize`
///   request into `initialize_cache`.
/// - A spawned helper thread copies socket → stdout. Each respawn
///   gets a fresh helper thread.
/// - When a forward write fails (worker closed the socket), the
///   main thread reaps the worker, examines its exit code:
///     - `42` → voluntary swap (`swap_worker` MCP tool). Respawn,
///       replay the cached `initialize`, REPLAY the failing line
///       (CRITICAL: that line was what came in after the swap_worker
///       response — losing it would drop a real user request).
///     - non-42 → crash. Up to MAX_CRASHES respawns; then bail.
/// - When stdin closes, the supervisor half-shuts the socket so
///   the worker drains and exits cleanly, joins the helper, kills
///   the worker as belt-and-suspenders, exits 0.
pub(crate) fn run(root: &Path) -> Result<ExitCode> {
    let socket_path = derive_socket_path(root);
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let _ = std::fs::remove_file(&socket_path);

    let exe = super::util::own_exe().context("locate own executable path")?;
    let initialize_cache: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    // gap-mcp-supervisor Slice B.3d: when the supervisor replays
    // the cached `initialize` into a freshly-spawned worker, the
    // worker emits a fresh `initialize` response that Claude Code
    // never asked for. The id-to-suppress is set right before
    // replay and cleared on first match by the socket→stdout
    // helper, so the duplicate response is dropped before it
    // reaches Claude.
    let suppress: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));

    let mut worker = spawn_worker(&exe, root, &socket_path)?;
    let mut stream = match connect_with_retry(&socket_path, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => {
            let _ = worker.kill();
            let _ = worker.wait();
            let _ = std::fs::remove_file(&socket_path);
            return Err(e);
        }
    };
    let mut to_stdout = spawn_socket_to_stdout(&stream, Arc::clone(&suppress))?;

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut buf = String::new();
    let mut crashes: u32 = 0;
    const MAX_CRASHES: u32 = 3;
    // Set after forwarding a `swap_worker` call. That worker replies and
    // then exits 42, and a line written to its socket in between is
    // accepted by the kernel but never read, so the write doesn't fail
    // and the request is lost. Wait for the exit before forwarding the
    // next line instead.
    let mut swap_pending = false;

    loop {
        buf.clear();
        let n = reader.read_line(&mut buf).context("read line from stdin")?;
        if n == 0 {
            break;
        }
        if is_initialize_request(&buf) {
            if let Ok(mut slot) = initialize_cache.lock() {
                *slot = Some(buf.clone());
            }
        }
        // Try to forward. If the current worker has died, swap and
        // resend (with cached initialize) before continuing.
        let swapped =
            std::mem::take(&mut swap_pending) && exited_within(&mut worker, SWAP_EXIT_GRACE);
        swap_pending = is_swap_request(&buf);
        let mut write_ok = !swapped && stream.write_all(buf.as_bytes()).is_ok();
        if write_ok {
            write_ok = stream.flush().is_ok();
        }
        if !write_ok {
            // Old worker socket is broken. Reap; check exit code.
            let status = worker.wait().context("reap dead worker")?;
            let voluntary = status.code() == Some(42);
            if voluntary {
                crashes = 0;
                eprintln!("doc-linter mcp-supervisor: worker requested swap; respawning");
            } else {
                crashes += 1;
                eprintln!(
                    "doc-linter mcp-supervisor: worker exited unexpectedly (status {status:?}), respawn {crashes}/{MAX_CRASHES}"
                );
                if crashes >= MAX_CRASHES {
                    anyhow::bail!("worker crashed {crashes} times in a row; aborting supervisor");
                }
            }
            // Tear down old socket helper.
            let _ = stream.shutdown(std::net::Shutdown::Both);
            let _ = to_stdout.join();
            let _ = std::fs::remove_file(&socket_path);
            // Brief cooldown to avoid tight crash loops.
            thread::sleep(Duration::from_millis(100));
            // Respawn.
            worker = spawn_worker(&exe, root, &socket_path)?;
            stream = connect_with_retry(&socket_path, Duration::from_secs(5))?;
            to_stdout = spawn_socket_to_stdout(&stream, Arc::clone(&suppress))?;
            // Replay cached initialize so the new worker has its
            // session state. Arm Slice B.3d response suppression
            // for the cached init's id BEFORE replay so the
            // helper drops the duplicate response on the way back.
            if let Some(init) = initialize_cache.lock().ok().and_then(|g| g.clone()) {
                if let Some(id) = extract_jsonrpc_id(&init) {
                    if let Ok(mut slot) = suppress.lock() {
                        *slot = Some(id);
                    }
                }
                stream
                    .write_all(init.as_bytes())
                    .context("replay initialize after swap")?;
                stream.flush().context("flush initialize after swap")?;
            }
            // Replay the buffered line that failed against the old
            // socket. Without this, the first request after a swap
            // would be silently lost.
            stream
                .write_all(buf.as_bytes())
                .context("replay failed line after swap")?;
            stream.flush().context("flush replayed line after swap")?;
        }
    }

    // Clean shutdown: half-close socket so worker drains + exits cleanly.
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let _ = to_stdout.join();
    let _ = worker.kill();
    let _ = worker.wait();
    let _ = std::fs::remove_file(&socket_path);
    Ok(ExitCode::SUCCESS)
}

/// Spawn the socket→stdout pump. Each worker iteration gets a
/// fresh helper; teardown is by shutting the read half of the
/// socket so the read loop returns 0 and the thread exits.
///
/// Slice B.3d: reads JSON-RPC LINES (the wire protocol is
/// newline-delimited) so the helper can filter out the
/// duplicate `initialize` response the replayed init triggers
/// on the post-swap worker. `suppress` carries `Some(id)` when
/// the supervisor has just replayed an initialize; the first
/// response matching that id is dropped and the suppression is
/// cleared.
///
/// Slice C: `io::stdout()` in Rust is BLOCK-buffered when
/// connected to a pipe. We flush after each line so JSON-RPC
/// responses reach Claude Code immediately.
fn spawn_socket_to_stdout(
    stream: &UnixStream,
    suppress: Arc<Mutex<Option<i64>>>,
) -> Result<thread::JoinHandle<()>> {
    let read_half = stream
        .try_clone()
        .context("clone unix stream for socket→stdout helper")?;
    Ok(thread::spawn(move || {
        let mut reader = BufReader::new(read_half);
        let stdout = io::stdout();
        let mut buf = String::new();
        loop {
            buf.clear();
            match reader.read_line(&mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    if should_suppress_line(&buf, &suppress) {
                        continue;
                    }
                    let mut sink = stdout.lock();
                    if sink.write_all(buf.as_bytes()).is_err() {
                        break;
                    }
                    if sink.flush().is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }))
}

/// Slice B.3d: returns true if `line` is a JSON-RPC response
/// whose `id` matches the currently-armed suppression target.
/// On match, the suppression is cleared so subsequent responses
/// (including any legitimate response with the same id) flow
/// through unchanged.
fn should_suppress_line(line: &str, suppress: &Arc<Mutex<Option<i64>>>) -> bool {
    let target = {
        let guard = match suppress.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        match *guard {
            Some(t) => t,
            None => return false,
        }
    };
    let Some(id) = extract_jsonrpc_id(line) else {
        return false;
    };
    if id != target {
        return false;
    }
    if let Ok(mut slot) = suppress.lock() {
        *slot = None;
    }
    true
}

/// Slice B.3d: extract the JSON-RPC `id` (numeric) from a line.
/// Falls back to `None` for malformed JSON, missing id (notifications),
/// or non-numeric id (string ids are valid JSON-RPC but the supervisor
/// only ever caches Claude Code's initialize, which uses numeric ids).
fn extract_jsonrpc_id(line: &str) -> Option<i64> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    v.get("id").and_then(serde_json::Value::as_i64)
}

/// Derive a stable, per-root socket path. Hashing the root means
/// two supervisor invocations against the same workspace share
/// nothing accidentally with a different workspace's supervisor.
fn derive_socket_path(root: &Path) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    root.hash(&mut h);
    let hash = h.finish();
    let dir = std::env::var_os("HOME")
        .map_or_else(std::env::temp_dir, PathBuf::from)
        .join(".doc-lint");
    dir.join(format!("mcp-{hash:x}.sock"))
}

/// Spawn the worker subprocess. Its stdin/stdout are explicitly
/// detached — the supervisor owns Claude Code's stdio FDs and
/// proxies them over the socket. The worker's stderr inherits the
/// supervisor's stderr so worker panics / errors are still visible
/// to the user.
fn spawn_worker(exe: &Path, root: &Path, socket: &Path) -> Result<Child> {
    Command::new(exe)
        .arg("--root")
        .arg(root)
        .arg("mcp-worker")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("spawn mcp-worker child process: {}", exe.display()))
}

/// Poll the socket path until `UnixStream::connect` succeeds or
/// `max_wait` elapses. The worker races to bind before the
/// supervisor connects; 50 ms poll spacing is fine in practice
/// (worker's bind is one syscall).
fn connect_with_retry(path: &Path, max_wait: Duration) -> Result<UnixStream> {
    let start = Instant::now();
    loop {
        match UnixStream::connect(path) {
            Ok(s) => return Ok(s),
            Err(_) if start.elapsed() < max_wait => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!(
                        "worker socket {} did not become connectable within {:?}",
                        path.display(),
                        max_wait
                    )
                });
            }
        }
    }
}

/// gap-mcp-supervisor Slice B.3a: line-buffered stdin→socket pump
/// that snapshots the FIRST `initialize` request into the shared
/// cache as it flows through. Subsequent `initialize` requests
/// (Claude Code shouldn't send them, but be defensive) overwrite
/// the cache — the swap replay only ever uses the most recent.
/// Other JSON-RPC methods stream through unchanged.
///
/// Slice B.3c inlines the same logic into `run` to track the
/// in-flight `buf` for swap-and-replay; this helper is retained
/// for tests that pin the request-detection + line-forwarding
/// contract independent of the supervisor lifecycle.
#[cfg(test)]
fn line_pump_with_cache<R: BufRead, W: Write>(
    reader: &mut R,
    writer: &mut W,
    initialize_cache: Arc<Mutex<Option<String>>>,
) -> io::Result<()> {
    let mut buf = String::new();
    loop {
        buf.clear();
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if is_initialize_request(&buf) {
            if let Ok(mut slot) = initialize_cache.lock() {
                *slot = Some(buf.clone());
            }
        }
        writer.write_all(buf.as_bytes())?;
        writer.flush()?;
    }
}

/// How long a worker that accepted `swap_worker` gets to exit.
const SWAP_EXIT_GRACE: Duration = Duration::from_secs(5);

/// True once `worker` has exited, polling for up to `grace`.
fn exited_within(worker: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match worker.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            _ => return false,
        }
    }
}

/// A `tools/call` of the `swap_worker` tool; same prefilter-then-parse
/// shape as [`is_initialize_request`].
fn is_swap_request(line: &str) -> bool {
    if !line.contains("\"swap_worker\"") {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
        return false;
    };
    v.get("method").and_then(serde_json::Value::as_str) == Some("tools/call")
        && v.pointer("/params/name")
            .and_then(serde_json::Value::as_str)
            == Some("swap_worker")
}

/// Cheap textual prefilter + JSON parse only if the prefilter
/// hits. Identifies a JSON-RPC `initialize` request by its
/// `"method": "initialize"` field. Notifications (no `id`) and
/// requests both qualify — both shapes need to be replayed for
/// the swap to be transparent.
fn is_initialize_request(line: &str) -> bool {
    if !line.contains("\"initialize\"") {
        return false;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return false;
    };
    v.get("method").and_then(|m| m.as_str()) == Some("initialize")
}

/// Generic byte pump used by the proxy threads. Extracted so the
/// proxy can be unit-tested against in-memory pairs without
/// spawning child processes.
#[allow(
    dead_code,
    reason = "kept for symmetry with proxy_until_close + future test reuse"
)]
fn pump<R: Read, W: Write>(mut src: R, mut dst: W) -> io::Result<u64> {
    io::copy(&mut src, &mut dst)
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
    use std::io::Cursor;

    /// gap-mcp-supervisor Slice B.2: the proxy's per-direction byte
    /// pump is just `io::copy`. Wrapping it in a function is the
    /// thing that lets us swap in mock readers/writers; the test
    /// pins the contract (every byte from src appears at dst in
    /// order) so a future refactor that adds rate-limiting or
    /// framing peeking can't silently lose bytes.
    #[test]
    fn pump_copies_every_byte_in_order() {
        let src = b"first line\nsecond line\ntrailing without newline";
        let cursor = Cursor::new(&src[..]);
        let mut dst: Vec<u8> = Vec::new();
        let n = pump(cursor, &mut dst).expect("pump should succeed");
        assert_eq!(n as usize, src.len());
        assert_eq!(&dst[..], &src[..]);
    }

    /// gap-mcp-supervisor Slice B.3a: `is_initialize_request`
    /// recognises real JSON-RPC initialize lines and rejects
    /// look-alikes. The prefilter is just an early-exit
    /// optimization — the real test is the full JSON parse.
    #[test]
    fn is_initialize_request_recognises_real_init() {
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#;
        assert!(is_initialize_request(init));
        // Trailing newline is normal off the wire.
        assert!(is_initialize_request(&format!("{init}\n")));
    }

    #[test]
    fn is_swap_request_matches_only_swap_worker_calls() {
        assert!(is_swap_request(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"swap_worker","arguments":{}}}"#
        ));
        assert!(!is_swap_request(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"query","arguments":{"q":"swap_worker"}}}"#
        ));
        assert!(!is_swap_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#
        ));
    }

    #[test]
    fn is_initialize_request_rejects_other_methods() {
        assert!(!is_initialize_request(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#
        ));
        // Notification with no method — should not crash and not match.
        assert!(!is_initialize_request(r#"{"jsonrpc":"2.0","id":3}"#));
        // Non-JSON.
        assert!(!is_initialize_request("hello world"));
        // Looks like it ("initialize" in a string literal) but
        // method is something else.
        assert!(!is_initialize_request(
            r#"{"jsonrpc":"2.0","id":4,"method":"foo","params":{"hint":"initialize"}}"#
        ));
        assert!(!is_initialize_request(""));
    }

    /// gap-mcp-supervisor Slice B.3a: the line pump forwards
    /// every line to the writer in order AND snapshots the
    /// initialize request into the cache. A subsequent swap
    /// (B.3b) replays this cached value into the freshly-spawned
    /// worker before any user-driven traffic resumes.
    #[test]
    fn line_pump_caches_initialize_and_forwards_all() {
        use std::io::Cursor;

        let init = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2026-05-19\",\"capabilities\":{},\"clientInfo\":{\"name\":\"t\",\"version\":\"0\"}}}\n";
        let tools_list = "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\"}\n";
        let input = format!("{init}{tools_list}");
        let mut reader = Cursor::new(input.as_bytes());
        let mut sink: Vec<u8> = Vec::new();
        let cache: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        line_pump_with_cache(&mut reader, &mut sink, Arc::clone(&cache))
            .expect("pump succeeds at EOF");

        // Every line forwarded verbatim.
        assert_eq!(String::from_utf8(sink).unwrap(), input);
        // Cache filled with the initialize line.
        let cached = cache.lock().unwrap().clone();
        assert!(cached.is_some(), "initialize should be cached");
        let cached = cached.unwrap();
        assert!(
            cached.contains("\"method\":\"initialize\""),
            "cached line is the initialize request: {cached:?}"
        );
        assert!(cached.ends_with('\n'), "cached line preserves newline");
    }

    /// gap-mcp-supervisor Slice B.3d: `extract_jsonrpc_id` pulls
    /// the numeric id from a line, returns None for shapes the
    /// suppression logic shouldn't match against (notifications,
    /// non-JSON, string ids).
    #[test]
    fn extract_jsonrpc_id_works_on_request_and_response() {
        assert_eq!(
            extract_jsonrpc_id(r#"{"jsonrpc":"2.0","id":7,"method":"initialize"}"#),
            Some(7)
        );
        assert_eq!(
            extract_jsonrpc_id(r#"{"id":99,"jsonrpc":"2.0","result":{"ok":true}}"#),
            Some(99)
        );
        assert_eq!(
            extract_jsonrpc_id(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#),
            None,
            "notifications have no id"
        );
        assert_eq!(extract_jsonrpc_id("not json"), None);
        assert_eq!(extract_jsonrpc_id(""), None);
        // String ids are valid JSON-RPC but we don't suppress them.
        assert_eq!(
            extract_jsonrpc_id(r#"{"jsonrpc":"2.0","id":"x","result":{}}"#),
            None
        );
    }

    /// gap-mcp-supervisor Slice B.3d: `should_suppress_line` drops
    /// only the FIRST response whose id matches the armed
    /// suppression target, and clears the target on match so
    /// subsequent traffic (including a legitimate response with
    /// the same id) flows through.
    #[test]
    fn should_suppress_line_consumes_one_match_then_clears() {
        let s: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(Some(1)));
        let dup_init = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2026-05-19"}}"#;
        let other = r#"{"jsonrpc":"2.0","id":2,"result":{"tools":[]}}"#;

        assert!(should_suppress_line(dup_init, &s), "first match drops");
        assert_eq!(
            s.lock().unwrap().clone(),
            None,
            "suppression cleared after first match"
        );
        // A second copy of the same id should NOT be suppressed —
        // we cleared the target.
        assert!(!should_suppress_line(dup_init, &s));
        // Other ids pass through regardless.
        assert!(!should_suppress_line(other, &s));
    }

    #[test]
    fn should_suppress_line_passes_when_unarmed() {
        let s: Arc<Mutex<Option<i64>>> = Arc::new(Mutex::new(None));
        let line = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
        assert!(!should_suppress_line(line, &s));
    }

    /// gap-mcp-supervisor Slice B.2: socket path derivation is
    /// stable per root. Two calls for the same root collide; two
    /// calls for different roots diverge. That stability matters
    /// when the supervisor restarts — the worker reuses the same
    /// path so any client tooling that knew the path doesn't break.
    #[test]
    fn socket_path_is_stable_per_root() {
        let a = derive_socket_path(Path::new("/work/workspace"));
        let b = derive_socket_path(Path::new("/work/workspace"));
        let c = derive_socket_path(Path::new("/work/other-workspace"));
        assert_eq!(a, b, "same root → same socket path");
        assert_ne!(a, c, "different root → different socket path");
        assert!(a.to_string_lossy().contains("mcp-"));
        assert!(a.extension().unwrap_or_default() == "sock");
    }
}
