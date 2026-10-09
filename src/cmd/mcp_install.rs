//! `doc-linter mcp-install --client claude-code|cursor`: merge a
//! `doc-linter` entry into the client's project-level MCP config.

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// Project-relative config path per client. claude-code: `.mcp.json`
/// (documented). cursor: `.cursor/mcp.json` (documented).
fn config_rel(client: &str) -> Result<&'static str> {
    match client {
        "claude-code" => Ok(".mcp.json"),
        "cursor" => Ok(".cursor/mcp.json"),
        other => bail!("unknown client `{other}` (expected claude-code or cursor)"),
    }
}

/// Merge the doc-linter server into `existing` (a JSON object or empty)
/// and return the new file body. Other servers are left untouched.
fn merge(existing: &str, client: &str, root: &Path) -> Result<String> {
    let mut doc: Value = if existing.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(existing).context("existing config is not valid JSON")?
    };
    // Claude Code launches project servers from the repo root; Cursor's
    // cwd is not guaranteed, so pin the root there.
    let args = if client == "cursor" {
        json!(["mcp", "--root", root.display().to_string()])
    } else {
        json!(["mcp"])
    };
    let servers = doc
        .as_object_mut()
        .context("config root is not a JSON object")?
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    servers
        .as_object_mut()
        .context("`mcpServers` is not a JSON object")?
        .insert(
            "doc-linter".into(),
            json!({"command": "doc-linter", "args": args}),
        );
    Ok(serde_json::to_string_pretty(&doc)? + "\n")
}

pub(crate) fn run(root: &Path, client: &str) -> Result<std::process::ExitCode> {
    let path: PathBuf = root.join(config_rel(client)?);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let body = merge(&existing, client, &std::fs::canonicalize(root)?)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, body)?;
    println!("wrote doc-linter MCP entry to {}", path.display());
    Ok(std::process::ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_preserves_other_servers() {
        let out = merge(
            r#"{"mcpServers":{"x":{"command":"y"}}}"#,
            "claude-code",
            Path::new("/r"),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mcpServers"]["x"]["command"], "y");
        assert_eq!(v["mcpServers"]["doc-linter"]["args"], json!(["mcp"]));
        let c = merge("", "cursor", Path::new("/r")).unwrap();
        assert!(c.contains("--root"));
        assert!(config_rel("vim").is_err());
    }
}
