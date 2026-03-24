//! End-to-end integration tests for `scribe retain`.

use std::process::Command;

fn scribe_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scribe"))
}

fn insert_event(db: &str, json: &str) {
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
        .write_all(json.as_bytes())
        .unwrap();
    child.wait().unwrap();
}

#[test]
fn test_retain_deletes_old_events() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    // Insert events (all recent since they use DB default timestamp)
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":"/tmp","tool_name":"Bash"}"#,
    );

    // Retain with a very long duration — nothing should be deleted
    let output = scribe_bin()
        .args(["--db", db, "retain", "365d"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("No events older than 365d found."));
}

#[test]
fn test_retain_reports_deleted_count() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    // Insert an event
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/tmp"}"#,
    );

    // Retain with 0s duration — deletes everything
    let output = scribe_bin()
        .args(["--db", db, "retain", "0s"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Deleted"));
    assert!(stdout.contains("events older than 0s"));
}

#[test]
fn test_retain_invalid_duration() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");

    let output = scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "retain", "xyz"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("invalid duration"));
}

#[test]
fn test_retain_orphan_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    // Insert events for two sessions
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/a"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s2","hook_event_name":"SessionStart","cwd":"/b"}"#,
    );

    // Retain 0s deletes everything — both sessions become orphaned
    let output = scribe_bin()
        .args(["--db", db, "retain", "0s"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("orphaned sessions"));

    // Verify sessions are actually gone via query
    let query_output = scribe_bin()
        .args(["--db", db, "query", "sessions", "--json"])
        .output()
        .unwrap();
    let query_stdout = String::from_utf8_lossy(&query_output.stdout);
    assert!(
        query_stdout.trim().is_empty(),
        "sessions should be empty after orphan cleanup"
    );
}
