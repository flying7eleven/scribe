//! End-to-end integration tests for config auto-creation (US-0025-E007).

use std::process::Command;

fn scribe_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scribe"))
}

/// Running `scribe stats` (interactive) should succeed even when config doesn't exist.
/// This verifies the auto-creation path doesn't break normal operation.
#[test]
fn stats_works_with_no_config() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "stats"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stats should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Running `scribe log` with piped input should not trigger config creation.
/// We verify this indirectly: log should succeed without touching the config path.
#[test]
fn log_does_not_fail_without_config() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let mut child = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "log"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();

    use std::io::Write;
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(
            br#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash"}"#,
        )
        .unwrap();

    let status = child.wait().unwrap();
    assert!(status.success(), "log should always exit 0");
}
