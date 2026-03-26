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

fn populate_diverse_db(db: &str) {
    // Tool events
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/project-a","tool_name":"Bash"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":"/project-a","tool_name":"Bash"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/project-a","tool_name":"Read"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":"/project-a","tool_name":"Read"}"#,
    );
    // Error events
    insert_event(
        db,
        r#"{"session_id":"s1","hook_event_name":"PostToolUseFailure","cwd":"/project-a","tool_name":"Bash","error":"command failed"}"#,
    );
    // Different session/directory
    insert_event(
        db,
        r#"{"session_id":"s2","hook_event_name":"SessionStart","cwd":"/project-b"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s2","hook_event_name":"PreToolUse","cwd":"/project-b","tool_name":"Write"}"#,
    );
    insert_event(
        db,
        r#"{"session_id":"s2","hook_event_name":"SessionEnd","cwd":"/project-b"}"#,
    );
}

// ── Text output tests ──

#[test]
fn test_stats_populated_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    populate_diverse_db(db);

    let output = scribe_bin().args(["--db", db, "stats"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Header
    assert!(stdout.contains("Database:"));
    assert!(stdout.contains("Size:"));
    assert!(stdout.contains("Events:"));
    assert!(stdout.contains("Sessions:"));
    assert!(stdout.contains("Oldest:"));
    assert!(stdout.contains("Newest:"));

    // Top tools section
    assert!(stdout.contains("Top tools:"));
    assert!(stdout.contains("Bash"));
    assert!(stdout.contains("Read"));

    // Event types section
    assert!(stdout.contains("Event types:"));
    assert!(stdout.contains("PreToolUse"));
    assert!(stdout.contains("PostToolUse"));

    // Errors section
    assert!(stdout.contains("Errors:"));
    assert!(stdout.contains("PostToolUseFailure"));

    // Top directories section
    assert!(stdout.contains("Top directories:"));

    // Activity histogram
    assert!(stdout.contains("Activity"));
}

#[test]
fn test_stats_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    let output = scribe_bin().args(["--db", db, "stats"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Events:    0"));
    assert!(stdout.contains("Sessions:  0"));
    assert!(stdout.contains("\u{2014}")); // em dash for missing dates

    // Extended sections should NOT appear
    assert!(!stdout.contains("Top tools:"));
    assert!(!stdout.contains("Event types:"));
    assert!(!stdout.contains("Top directories:"));
    assert!(!stdout.contains("Activity"));
}

#[test]
fn test_stats_with_since() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    populate_diverse_db(db);

    let output = scribe_bin()
        .args(["--db", db, "stats", "--since", "1d"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(stdout.contains("Period:"));
    assert!(stdout.contains("since"));
}

// ── JSON output tests ──

#[test]
fn test_stats_json_populated() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    populate_diverse_db(db);

    let output = scribe_bin()
        .args(["--db", db, "stats", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    // Verify all top-level fields exist
    assert!(json.get("db_path").is_some());
    assert!(json.get("db_size_bytes").is_some());
    assert!(json.get("event_count").is_some());
    assert!(json.get("session_count").is_some());
    assert!(json.get("oldest_event").is_some());
    assert!(json.get("newest_event").is_some());
    assert!(json.get("avg_session_duration_seconds").is_some());
    assert!(json.get("top_tools").is_some());
    assert!(json.get("event_types").is_some());
    assert!(json.get("errors").is_some());
    assert!(json.get("top_directories").is_some());
    assert!(json.get("daily_activity").is_some());

    // Verify counts
    assert_eq!(json["event_count"].as_i64().unwrap(), 8);
    assert_eq!(json["session_count"].as_i64().unwrap(), 2);

    // Verify arrays are non-empty
    assert!(!json["top_tools"].as_array().unwrap().is_empty());
    assert!(!json["event_types"].as_array().unwrap().is_empty());
    assert!(!json["top_directories"].as_array().unwrap().is_empty());

    // Verify tool structure
    let first_tool = &json["top_tools"][0];
    assert!(first_tool.get("tool_name").is_some());
    assert!(first_tool.get("count").is_some());

    // Verify errors structure
    assert!(json["errors"].get("post_tool_use_failure").is_some());
    assert!(json["errors"].get("stop_failure").is_some());
    assert!(json["errors"].get("stop_failure_types").is_some());
    assert_eq!(json["errors"]["post_tool_use_failure"].as_i64().unwrap(), 1);

    // Verify paths are full (not truncated)
    let first_dir = &json["top_directories"][0];
    let cwd = first_dir["cwd"].as_str().unwrap();
    assert!(!cwd.starts_with("..."));
}

#[test]
fn test_stats_json_empty_db() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    let output = scribe_bin()
        .args(["--db", db, "stats", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    assert_eq!(json["event_count"].as_i64().unwrap(), 0);
    assert_eq!(json["session_count"].as_i64().unwrap(), 0);
    assert!(json["oldest_event"].is_null());
    assert!(json["newest_event"].is_null());
    assert!(json["avg_session_duration_seconds"].is_null());
    assert!(json["top_tools"].as_array().unwrap().is_empty());
    assert!(json["event_types"].as_array().unwrap().is_empty());
    assert!(json["top_directories"].as_array().unwrap().is_empty());
    assert!(json["daily_activity"].as_array().unwrap().is_empty());
}

#[test]
fn test_stats_json_with_since() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    populate_diverse_db(db);

    let output = scribe_bin()
        .args(["--db", db, "stats", "--json", "--since", "7d"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);

    let json: serde_json::Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    // All events are within 7d so counts should match populated totals
    assert_eq!(json["event_count"].as_i64().unwrap(), 8);
}

#[test]
fn test_stats_json_valid_single_object() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test.db");
    let db = db_path.to_str().unwrap();

    populate_diverse_db(db);

    let output = scribe_bin()
        .args(["--db", db, "stats", "--json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Must be a single JSON object (not JSON Lines)
    let trimmed = stdout.trim();
    assert!(trimmed.starts_with('{'));
    assert!(trimmed.ends_with('}'));

    // Must parse as exactly one JSON value
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(trimmed);
    assert!(parsed.is_ok());
    assert!(parsed.unwrap().is_object());
}
