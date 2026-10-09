//! The release workflow's smoke test runs `doc-linter --version`.

#[test]
fn version_flag_prints_cargo_version() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_doc-linter"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        format!("doc-linter {}", env!("CARGO_PKG_VERSION"))
    );
}
