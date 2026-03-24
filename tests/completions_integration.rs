//! End-to-end integration tests for `scribe completions`.

use std::process::Command;

fn scribe_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_scribe"))
}

#[test]
fn test_completions_bash() {
    let output = scribe_bin().args(["completions", "bash"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty());
    assert!(stdout.contains("scribe"));
}

#[test]
fn test_completions_zsh() {
    let output = scribe_bin().args(["completions", "zsh"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty());
    assert!(stdout.contains("scribe"));
}

#[test]
fn test_completions_fish() {
    let output = scribe_bin().args(["completions", "fish"]).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty());
    assert!(stdout.contains("scribe"));
}

#[test]
fn test_completions_invalid_shell() {
    let output = scribe_bin()
        .args(["completions", "invalid"])
        .output()
        .unwrap();

    assert!(!output.status.success());
}
