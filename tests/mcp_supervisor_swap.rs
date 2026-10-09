//! gap-mcp-supervisor Slice C — end-to-end swap integration test.
//!
//! Spawns `doc-linter mcp-supervisor` against a temp workspace,
//! drives the JSON-RPC protocol over its stdio:
//!
//!   1. `initialize` — supervisor proxies to worker, caches the
//!      line per Slice B.3a.
//!   2. `tools/list` — confirms the supervisor is forwarding and
//!      the worker is healthy.
//!   3. `tools/call swap_worker` — worker replies OK, then exits
//!      with code 42. Supervisor's Slice B.3c outer loop reaps it,
//!      respawns a fresh worker, replays the cached `initialize`.
//!   4. `tools/list` again — proves the new worker is alive and
//!      answering through the SAME supervisor stdio connection.
//!
//! Closing supervisor's stdin then EOFs the supervisor cleanly.
//!
//! The whole round-trip should finish in well under 5 seconds; we
//! cap reads at 8 s so a hang fails loudly instead of leaving a
//! zombie process.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]

mod common;

use std::process::{Command, Stdio};
use std::time::Duration;

use common::{
    read_line_with_timeout, run_doc_linter, spawn_line_reader, unique_tmpdir, write_line,
};

/// First isolate the supervisor round-trip BEFORE the swap path
/// is exercised. If this test hangs, the issue is in
/// initialize-forwarding, not swap-and-replay.
#[cfg(unix)]
#[test]
fn supervisor_proxies_initialize_round_trip() {
    let root = unique_tmpdir("supervisor-init");
    init_with_graph(&root);

    eprintln!("[test] spawning supervisor");
    let mut child = Command::new(common::doc_linter_bin())
        .arg("--root")
        .arg(&root)
        .arg("mcp-supervisor")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn supervisor");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let lines = spawn_line_reader(stdout);

    eprintln!("[test] writing initialize");
    write_line(
        &mut stdin,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#,
    );

    eprintln!("[test] reading response");
    let resp = read_line_with_timeout(&lines, Duration::from_secs(8));
    eprintln!("[test] got {} bytes", resp.len());
    let v: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
    assert_eq!(v["id"].as_i64(), Some(1));

    eprintln!("[test] closing stdin");
    drop(stdin);
    let status = child.wait().unwrap();
    eprintln!("[test] supervisor exited: {status:?}");
    assert!(status.success());
}

#[cfg(unix)]
#[test]
fn supervisor_swap_keeps_stdio_alive_across_worker_swap() {
    let root = unique_tmpdir("supervisor-swap");
    init_with_graph(&root);

    let mut child = Command::new(common::doc_linter_bin())
        .arg("--root")
        .arg(&root)
        .arg("mcp-supervisor")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mcp-supervisor");

    let mut stdin = child.stdin.take().expect("supervisor stdin piped");
    let stdout = child.stdout.take().expect("supervisor stdout piped");
    let lines = spawn_line_reader(stdout);

    // 1. initialize
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"swap-it","version":"0"}}}"#;
    write_line(&mut stdin, init);
    let resp1 = read_line_with_timeout(&lines, Duration::from_secs(8));
    let v1: serde_json::Value = serde_json::from_str(resp1.trim()).expect("initialize JSON");
    assert_eq!(v1["id"].as_i64(), Some(1));
    assert_eq!(v1["result"]["protocolVersion"].as_str(), Some("2026-05-19"));

    // 2. tools/list against the FIRST worker
    let list1 = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    write_line(&mut stdin, list1);
    let resp2 = read_line_with_timeout(&lines, Duration::from_secs(8));
    let v2: serde_json::Value = serde_json::from_str(resp2.trim()).expect("first tools/list JSON");
    assert_eq!(v2["id"].as_i64(), Some(2));
    let names: Vec<&str> = v2["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(
        names.contains(&"swap_worker"),
        "tools/list must advertise swap_worker: {names:?}",
    );

    // 3. tools/call swap_worker — first worker replies, then exits 42.
    let swap = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"swap_worker","arguments":{"reason":"slice-C-test"}}}"#;
    write_line(&mut stdin, swap);
    let resp3 = read_line_with_timeout(&lines, Duration::from_secs(8));
    let v3: serde_json::Value = serde_json::from_str(resp3.trim()).expect("swap_worker JSON");
    assert_eq!(v3["id"].as_i64(), Some(3));
    assert!(v3.get("result").is_some(), "swap_worker returned OK: {v3}");

    // 4. tools/list against the SECOND (post-swap) worker, through
    // the SAME supervisor stdio. This is the key assertion: the
    // Claude Code-facing connection survived the worker swap AND
    // Slice B.3d's response-suppression dropped the duplicate
    // initialize response so the very next line on stdout is the
    // response to list2 (id=4).
    let list2 = r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#;
    write_line(&mut stdin, list2);
    let resp4 = read_line_with_timeout(&lines, Duration::from_secs(8));
    let v4: serde_json::Value = serde_json::from_str(resp4.trim()).expect("post-swap JSON");
    assert_eq!(
        v4["id"].as_i64(),
        Some(4),
        "Slice B.3d suppression should drop the replayed-init response so the next stdout line is the list2 reply: {v4}",
    );
    assert!(
        v4["result"]["tools"].as_array().is_some(),
        "post-swap tools/list returned a catalog: {v4}",
    );

    // Close stdin → supervisor half-shuts socket → worker drains → exits → supervisor exits 0.
    drop(stdin);
    let status = child.wait().expect("supervisor exit");
    assert!(
        status.success(),
        "supervisor exited non-zero after clean shutdown: {status:?}",
    );
}

/// `init` plus one `check`: the worker opens the graph read-only, so it
/// must exist before the supervisor spawns one. `check` exits non-zero on
/// the scaffold's placeholder entity; only the graph it writes matters.
fn init_with_graph(root: &std::path::Path) {
    assert!(
        run_doc_linter(root, &["init"]).status.success(),
        "init failed"
    );
    run_doc_linter(root, &["--no-vale", "check"]);
    assert!(
        root.join(".doc-lint/graph.sqlite").exists(),
        "check did not build the graph"
    );
}
