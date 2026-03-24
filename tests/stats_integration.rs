//! End-to-end integration tests for `scribe stats`.

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
fn test_stats_populated_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s2","hook_event_name":"SessionStart","cwd":"/project"}"#,
    );

    let output = scribe_bin().args(["--db", db, "stats"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Database:"));
    assert!(stdout.contains("Events:    2"));
    assert!(stdout.contains("Sessions:  2"));
    assert!(stdout.contains("Oldest:"));
    assert!(stdout.contains("Newest:"));
    assert!(stdout.contains("Size:"));
}

#[test]
fn test_stats_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    // Create DB without inserting events — need to at least connect to create the file
    // Use scribe stats itself to trigger DB creation via connect()
    let output = scribe_bin().args(["--db", db, "stats"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Events:    0"));
    assert!(stdout.contains("Sessions:  0"));
    assert!(stdout.contains("\u{2014}")); // em dash for missing dates
}
