//! End-to-end integration tests for `scribe init`.

use std::process::Command;

fn scribe_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scribe"))
}

// ── Stdout output tests ──

#[test]
fn test_init_stdout_valid_json() {
    let output = scribe_bin().arg("init").output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let config: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout should be valid JSON");
    assert!(config["hooks"].is_object());
}

#[test]
fn test_init_stdout_all_21_events() {
    let output = scribe_bin().arg("init").output().unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let hooks = config["hooks"].as_object().unwrap();
    assert_eq!(hooks.len(), 21);
    assert!(!hooks.contains_key("WorktreeCreate"));
}

#[test]
fn test_init_stdout_matcher_correctness() {
    let output = scribe_bin().arg("init").output().unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let hooks = config["hooks"].as_object().unwrap();

    // Spot check: PreToolUse should have matcher
    assert_eq!(hooks["PreToolUse"][0]["matcher"].as_str(), Some("*"));
    // Stop should NOT have matcher
    assert!(hooks["Stop"][0].get("matcher").is_none());
}

#[test]
fn test_init_stdout_command_and_timeout() {
    let output = scribe_bin().arg("init").output().unwrap();
    let config: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap();
    let hooks = config["hooks"].as_object().unwrap();

    for (event, entries) in hooks {
        let hook = &entries[0]["hooks"][0];
        assert_eq!(
            hook["command"].as_str(),
            Some("scribe log"),
            "{event}: command"
        );
        assert_eq!(hook["timeout"].as_i64(), Some(10), "{event}: timeout");
    }
}

// ── File write tests ──

#[test]
fn test_init_project_creates_file() {
    let dir = tempfile::tempdir().unwrap();

    let output = scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());

    let settings_path = dir.path().join(".claude").join("settings.json");
    assert!(settings_path.exists(), "settings.json should be created");

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(content["hooks"].as_object().unwrap().len(), 21);

    // Confirmation on stderr
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("wrote hooks to"));
}

#[test]
fn test_init_global_creates_file() {
    let dir = tempfile::tempdir().unwrap();

    let output = scribe_bin()
        .args(["init", "--global"])
        .env("HOME", dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());

    let settings_path = dir.path().join(".claude").join("settings.json");
    assert!(
        settings_path.exists(),
        "global settings.json should be created"
    );

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&settings_path).unwrap()).unwrap();
    assert_eq!(content["hooks"].as_object().unwrap().len(), 21);
}

// ── Merge tests ──

#[test]
fn test_init_project_preserves_existing_keys() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"permissions":{"allow":["Bash"]}}"#,
    )
    .unwrap();

    let output = scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(output.status.success());

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claude_dir.join("settings.json")).unwrap())
            .unwrap();
    assert!(content["permissions"]["allow"].is_array());
    assert_eq!(content["hooks"].as_object().unwrap().len(), 21);
}

#[test]
fn test_init_project_preserves_non_scribe_events() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"hooks":{"WorktreeCreate":[{"hooks":[{"type":"command","command":"my-handler"}]}]}}"#,
    )
    .unwrap();

    scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claude_dir.join("settings.json")).unwrap())
            .unwrap();
    let hooks = content["hooks"].as_object().unwrap();
    assert!(hooks.contains_key("WorktreeCreate"));
    assert_eq!(hooks.len(), 22); // 21 scribe + 1 custom
}

#[test]
fn test_init_project_preserves_user_hooks_on_same_event() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"my-linter"}]}]}}"#,
    )
    .unwrap();

    scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let content: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(claude_dir.join("settings.json")).unwrap())
            .unwrap();
    let pre_tool_use = content["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(pre_tool_use.len(), 2);
    assert_eq!(
        pre_tool_use[0]["hooks"][0]["command"].as_str(),
        Some("my-linter")
    );
    assert_eq!(
        pre_tool_use[1]["hooks"][0]["command"].as_str(),
        Some("scribe log")
    );
}

#[test]
fn test_init_project_idempotent() {
    let dir = tempfile::tempdir().unwrap();

    scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let first = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();

    scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    let second = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
    assert_eq!(first, second);
}

#[test]
fn test_init_project_invalid_json_error() {
    let dir = tempfile::tempdir().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(claude_dir.join("settings.json"), "not json {{{").unwrap();

    let output = scribe_bin()
        .args(["init", "--project"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success(), "should fail on invalid JSON");

    // File unchanged
    assert_eq!(
        std::fs::read_to_string(claude_dir.join("settings.json")).unwrap(),
        "not json {{{"
    );
}

// ── Argument validation tests ──

#[test]
fn test_init_project_global_mutually_exclusive() {
    let output = scribe_bin()
        .args(["init", "--project", "--global"])
        .output()
        .unwrap();

    assert!(
        !output.status.success(),
        "--project and --global should be mutually exclusive"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with") || stderr.contains("conflict"),
        "stderr should mention conflict: {stderr}"
    );
}

#[test]
fn test_init_no_db_created() {
    let dir = tempfile::tempdir().unwrap();

    // Run init with a --db path that doesn't exist
    let db_path = dir.path().join("should_not_exist.db");
    scribe_bin()
        .args(["--db", db_path.to_str().unwrap(), "init"])
        .output()
        .unwrap();

    assert!(!db_path.exists(), "init should NOT create a database file");
}
