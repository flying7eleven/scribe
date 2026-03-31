//! End-to-end integration tests for `scribe query`.
//!
//! Tests use a temporary DB populated with known test data.

use std::process::Command;

fn scribe_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scribe"))
}

/// Create a temp DB and populate it with test events via `scribe log`.
fn setup_test_db() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    let events = vec![
        r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/project-a","source":"startup"}"#,
        r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/project-a","tool_name":"Bash","tool_input":{"command":"echo hello"}}"#,
        r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":"/project-a","tool_name":"Bash","tool_response":{"output":"hello"}}"#,
        r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/project-a","tool_name":"Write","tool_input":{"file_path":"/tmp/out.txt","content":"data"}}"#,
        r#"{"session_id":"s2","hook_event_name":"SessionStart","cwd":"/project-b","source":"resume"}"#,
        r#"{"session_id":"s2","hook_event_name":"PreToolUse","cwd":"/project-b","tool_name":"Bash","tool_input":{"command":"ls -la"}}"#,
    ];

    for event in events {
        let mut child = scribe_bin()
            .args(["--db", db, "log"])
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
            .write_all(event.as_bytes())
            .unwrap();
        child.wait().unwrap();
    }

    (dir, db_path)
}

// ── Event query tests ──

#[test]
fn test_query_basic() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "query"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("TIMESTAMP"));
    assert!(stdout.contains("EVENT"));
    assert!(stdout.contains("TOOL"));
}

#[test]
fn test_query_since_date() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--since",
            "2020-01-01",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should have results (all test data is recent)
    assert!(stdout.contains("PreToolUse") || stdout.contains("SessionStart"));
}

#[test]
fn test_query_session_filter() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--session",
            "s1",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Bash") || stdout.contains("Write"));
}

#[test]
fn test_query_event_type_filter() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--event",
            "SessionStart",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("SessionStart"));
    assert!(!stdout.contains("PreToolUse"));
}

#[test]
fn test_query_tool_filter() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--tool",
            "Write",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Write"));
}

#[test]
fn test_query_search_filter() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--search",
            "echo hello",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Bash"));
}

#[test]
fn test_query_limit() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--limit",
            "2",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 2);
}

#[test]
fn test_query_json_output() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "query", "--json"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.trim().lines() {
        let _: serde_json::Value =
            serde_json::from_str(line).expect("each line should be valid JSON");
    }
}

#[test]
fn test_query_csv_output() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "query", "--csv"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert!(lines[0].contains("id,timestamp,session_id"));
    assert!(lines.len() > 1); // header + data rows
}

#[test]
fn test_query_json_csv_mutually_exclusive() {
    let output = scribe_bin()
        .args(["query", "--json", "--csv"])
        .output()
        .unwrap();

    assert!(!output.status.success());
}

#[test]
fn test_query_empty_result() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--tool",
            "NonExistentTool",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.is_empty() || stdout.trim().is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("No events found"));
}

// ── Session query tests ──

#[test]
fn test_query_sessions_basic() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "query", "sessions"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("SESSION"));
    assert!(stdout.contains("DURATION"));
}

#[test]
fn test_query_sessions_json() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "sessions",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.trim().lines() {
        let val: serde_json::Value = serde_json::from_str(line).unwrap();
        // Full session_id (not truncated)
        assert!(val["session_id"].as_str().unwrap().len() >= 2);
        // Duration field present
        assert!(val["duration"].is_string());
    }
}

#[test]
fn test_query_sessions_csv() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "sessions",
            "--csv",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert!(lines[0].contains("session_id,account_id"));
    assert!(lines[0].contains("duration"));
}

#[test]
fn test_query_sessions_limit() {
    let (_dir, db_path) = setup_test_db();
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "sessions",
            "--limit",
            "1",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    assert_eq!(lines.len(), 1);
}

// ── Error handling ──

#[test]
fn test_query_invalid_since() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let output = scribe_bin()
        .args([
            "--db",
            db_path.to_str().unwrap(),
            "query",
            "--since",
            "not-a-time",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid --since"));
}
