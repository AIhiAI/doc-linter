//! `check` while MCP servers hold the graph open, as when several
//! agents each run a server: the new graph is built beside the live one
//! and swapped in, and every server serves it from its next request.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "integration tests panic on broken invariants — that is the point"
)]

mod common;

use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::Receiver;
use std::time::Duration;

use common::{
    read_line_with_timeout, run_doc_linter, spawn_line_reader, unique_tmpdir, write, write_line,
};

struct McpServer {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

impl McpServer {
    fn start(root: &std::path::Path) -> Self {
        let mut child = Command::new(common::doc_linter_bin())
            .arg("--root")
            .arg(root)
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn doc-linter mcp");
        let lines = spawn_line_reader(child.stdout.take().expect("stdout piped"));
        let mut server = McpServer {
            stdin: child.stdin.take(),
            child,
            lines,
        };
        server.call(
            1,
            r#""method":"initialize","params":{"protocolVersion":"2026-05-19","capabilities":{},"clientInfo":{"name":"t","version":"0"}}"#,
        );
        server
    }

    fn call(&mut self, id: u32, body: &str) -> String {
        let stdin = self.stdin.as_mut().expect("server stdin open");
        write_line(stdin, &format!(r#"{{"jsonrpc":"2.0","id":{id},{body}}}"#));
        read_line_with_timeout(&self.lines, Duration::from_secs(15))
    }

    fn list_docs(&mut self, id: u32) -> String {
        self.call(
            id,
            r#""method":"tools/call","params":{"name":"list_docs","arguments":{}}"#,
        )
    }

    fn stop(mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
#[test]
fn check_publishes_while_mcp_servers_hold_the_graph() {
    let root = unique_tmpdir("graph-swap");
    assert!(
        run_doc_linter(&root, &["init"]).status.success(),
        "init failed"
    );
    run_doc_linter(&root, &["--no-vale", "check"]);

    let mut servers = [McpServer::start(&root), McpServer::start(&root)];
    for (id, server) in (10..).zip(servers.iter_mut()) {
        assert!(!server.list_docs(id).contains("swap-note"));
    }

    write(
        &root.join("docs/swap-note.md"),
        "---\nid: swap-note\nrole: doc\nkind: reference\ntitle: \"Swap note\"\n\
         summary: \"Written while two servers hold the graph.\"\nstatus: stable\n\
         updated: 2026-01-01\n---\n\nBody.\n",
    );
    let out = run_doc_linter(&root, &["--no-vale", "check"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("Could not set lock"),
        "check collided with the servers' open graph:\n{stderr}"
    );
    assert_ne!(out.status.code(), Some(2), "check failed:\n{stderr}");

    for (id, server) in (20..).zip(servers.iter_mut()) {
        assert!(
            server.list_docs(id).contains("swap-note"),
            "a server kept serving the graph from before the check"
        );
    }
    for server in servers {
        server.stop();
    }
}
