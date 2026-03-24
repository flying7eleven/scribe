//! End-to-end integration tests for `scribe log`.
//!
//! These tests invoke the compiled binary via `std::process::Command` to test
//! the full stdin-to-DB pipeline including process exit codes.

use std::process::Command;

fn scribe_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scribe"))
}

#[test]
fn test_binary_log_valid_json() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "log"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let stdin = child.stdin.as_mut().unwrap();
            stdin
                .write_all(
                    br#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash"}"#,
                )
                .unwrap();
            child.wait_with_output()
        })
        .unwrap();

    assert!(output.status.success(), "should exit 0");
    // DB file should have been created
    assert!(db_path.exists());
}

#[test]
fn test_binary_log_malformed_json_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "log"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(b"not json at all").unwrap();
            child.wait_with_output()
        })
        .unwrap();

    assert!(output.status.success(), "must exit 0 even on bad JSON");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("malformed JSON"),
        "stderr should mention malformed JSON: {stderr}"
    );
}

#[test]
fn test_binary_log_empty_stdin_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "log"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            drop(child.stdin.take()); // close stdin immediately (empty)
            child.wait_with_output()
        })
        .unwrap();

    assert!(output.status.success(), "must exit 0 on empty stdin");
}
