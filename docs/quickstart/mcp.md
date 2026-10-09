---
id: quickstart-mcp
role: doc
kind: how-to
lifecycle: stable
covers: [mcp, doc-graph]
title: "Quick start: MCP server"
summary: Register doc-linter's MCP server with Claude Code, Claude Desktop or Cursor and check that it answers.
status: stable
updated: 2026-10-01
tags: [quickstart, mcp]
---

# Quick start: MCP server

`doc-linter mcp` serves the graph over stdio JSON-RPC. Every tool and
parameter is in [[reference-mcp]]. If the repo has no graph yet, the
first start builds it (run `doc-linter check` first to see lint
results).

## Claude Code

Add `.mcp.json` at the repo root:

```json
{
  "mcpServers": {
    "doc-linter": { "command": "doc-linter", "args": ["mcp"] }
  }
}
```

Claude Code starts the server from the repo root, so no `--root` is
needed. Run `/mcp` in a session to confirm it's connected.

## Claude Desktop, Cursor and other clients

Use the same command with an absolute root:
`"args": ["mcp", "--root", "/abs/path/to/repo"]`. Claude Desktop reads
`claude_desktop_config.json`; Cursor reads `.cursor/mcp.json`. Both use
the `mcpServers` shape above.

## Check it answers

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | doc-linter mcp | head -c 300
```

The reply lists the tools. In an agent session, start with
`query_schema`, then `query_similar` for free-text questions,
`query_at` for a file and line, `function_context` and `query_impact`
for a function, and `read_source` to read code by path.

After editing docs or code, call the `reingest` tool (or rerun
`doc-linter check`) to rebuild the graph.
