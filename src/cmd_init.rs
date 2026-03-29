use std::path::PathBuf;

use serde_json::{json, Value};

/// Hook events and whether they support the `matcher` field.
/// Ordered canonically (PreToolUse through WorktreeCreate).
const HOOK_EVENTS: &[(&str, bool)] = &[
    ("PreToolUse", true),
    ("PostToolUse", true),
    ("PostToolUseFailure", true),
    ("UserPromptSubmit", false),
    ("PermissionRequest", true),
    ("SessionStart", true),
    ("SessionEnd", true),
    ("SubagentStart", true),
    ("SubagentStop", true),
    ("Stop", false),
    ("StopFailure", true),
    ("Notification", true),
    ("PreCompact", true),
    ("PostCompact", true),
    ("InstructionsLoaded", true),
    ("ConfigChange", true),
    ("WorktreeRemove", false),
    ("Elicitation", true),
    ("ElicitationResult", true),
    ("TeammateIdle", false),
    ("TaskCompleted", false),
    ("TaskCreated", false),
    ("CwdChanged", false),
    ("FileChanged", true),
    ("WorktreeCreate", false),
];

pub enum OutputTarget {
    Stdout,
    Project,
    Global,
}

/// Generate the complete hooks configuration as a serde_json::Value.
/// When `with_guard` is true, PreToolUse gets `scribe guard` before `scribe log`.
pub fn generate_hooks_config(with_guard: bool) -> Value {
    let mut hooks = serde_json::Map::new();

    let log_hook = json!({
        "type": "command",
        "command": "scribe log",
        "timeout": 10
    });

    let guard_hook = json!({
        "type": "command",
        "command": "scribe guard",
        "timeout": 10
    });

    for &(event, has_matcher) in HOOK_EVENTS {
        // PreToolUse gets guard + log when --with-guard is set
        let hook_array = if with_guard && event == "PreToolUse" {
            json!([guard_hook.clone(), log_hook.clone()])
        } else {
            json!([log_hook.clone()])
        };

        let entry = if has_matcher {
            json!({
                "matcher": "*",
                "hooks": hook_array
            })
        } else {
            json!({
                "hooks": hook_array
            })
        };

        hooks.insert(event.to_string(), json!([entry]));
    }

    json!({ "hooks": hooks })
}

/// Run the init handler: generate hooks config and output to the selected target.
///
/// `home_override` replaces `dirs::home_dir()` for testing (avoids writing to real home).
pub fn run(
    target: OutputTarget,
    home_override: Option<PathBuf>,
    with_guard: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let config = generate_hooks_config(with_guard);

    match target {
        OutputTarget::Stdout => {
            println!("{}", serde_json::to_string_pretty(&config)?);
        }
        OutputTarget::Project => {
            let path = PathBuf::from(".claude/settings.json");
            merge_and_write(&path, &config)?;
            eprintln!("scribe: wrote hooks to .claude/settings.json");
        }
        OutputTarget::Global => {
            let home = match home_override {
                Some(h) => h,
                None => dirs::home_dir().ok_or("could not determine home directory")?,
            };
            let path = home.join(".claude").join("settings.json");
            merge_and_write(&path, &config)?;
            eprintln!("scribe: wrote hooks to {}", path.display());
        }
    }

    Ok(())
}

/// Merge scribe's hooks into an existing settings file, or create a new one.
fn merge_and_write(path: &PathBuf, config: &Value) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut existing: Value = if path.exists() {
        let content = std::fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|e| {
            format!(
                "existing file {} contains invalid JSON: {e}",
                path.display()
            )
        })?
    } else {
        json!({})
    };

    // Merge hooks
    let generated_hooks = config["hooks"].as_object().unwrap();

    let existing_obj = existing
        .as_object_mut()
        .ok_or("existing settings file is not a JSON object")?;

    // Ensure "hooks" key exists as an object
    if !existing_obj.contains_key("hooks") {
        existing_obj.insert("hooks".to_string(), json!({}));
    }

    let existing_hooks = existing_obj["hooks"]
        .as_object_mut()
        .ok_or("existing 'hooks' key is not a JSON object")?;

    for (event_name, generated_entries) in generated_hooks {
        let generated_entry = &generated_entries[0]; // scribe's single entry for this event

        if let Some(existing_array) = existing_hooks.get_mut(event_name) {
            if let Some(arr) = existing_array.as_array_mut() {
                // Find and replace existing scribe entry, or append
                let scribe_idx = arr.iter().position(is_scribe_entry);
                if let Some(idx) = scribe_idx {
                    arr[idx] = generated_entry.clone();
                } else {
                    arr.push(generated_entry.clone());
                }
            }
        } else {
            // Event not present in existing file — add it
            existing_hooks.insert(event_name.clone(), json!([generated_entry]));
        }
    }

    // Write back with pretty-printing
    let output = serde_json::to_string_pretty(&existing)?;
    std::fs::write(path, output + "\n")?;

    Ok(())
}

/// Check if a hook entry is a scribe entry (command starts with "scribe log").
fn is_scribe_entry(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .map(|hooks| {
            hooks.iter().any(|h| {
                h["command"]
                    .as_str()
                    .is_some_and(|cmd| cmd.starts_with("scribe log"))
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── S01 tests (JSON generation) ──

    #[test]
    fn test_generates_valid_json() {
        let config = generate_hooks_config(false);
        let json_str = serde_json::to_string_pretty(&config).unwrap();
        let _: Value = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_all_25_events_present() {
        let config = generate_hooks_config(false);
        let hooks = config["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 25);
    }

    #[test]
    fn test_matcher_events_have_matcher() {
        let config = generate_hooks_config(false);
        let hooks = config["hooks"].as_object().unwrap();

        let with_matcher = [
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionRequest",
            "SessionStart",
            "SessionEnd",
            "SubagentStart",
            "SubagentStop",
            "StopFailure",
            "Notification",
            "PreCompact",
            "PostCompact",
            "InstructionsLoaded",
            "ConfigChange",
            "Elicitation",
            "ElicitationResult",
        ];

        for event in with_matcher {
            let entries = hooks[event].as_array().unwrap();
            assert_eq!(entries.len(), 1, "{event} should have one entry");
            let entry = &entries[0];
            assert_eq!(
                entry["matcher"].as_str(),
                Some("*"),
                "{event} should have matcher: *"
            );
        }
    }

    #[test]
    fn test_non_matcher_events_omit_matcher() {
        let config = generate_hooks_config(false);
        let hooks = config["hooks"].as_object().unwrap();

        let without_matcher = [
            "UserPromptSubmit",
            "Stop",
            "WorktreeRemove",
            "TeammateIdle",
            "TaskCompleted",
        ];

        for event in without_matcher {
            let entries = hooks[event].as_array().unwrap();
            let entry = &entries[0];
            assert!(
                entry.get("matcher").is_none(),
                "{event} should NOT have matcher field"
            );
        }
    }

    #[test]
    fn test_hook_command_and_timeout() {
        let config = generate_hooks_config(false);
        let hooks = config["hooks"].as_object().unwrap();

        for (event, _) in HOOK_EVENTS {
            let entries = hooks[*event].as_array().unwrap();
            let hook = &entries[0]["hooks"][0];
            assert_eq!(hook["type"].as_str(), Some("command"), "{event}: type");
            assert_eq!(
                hook["command"].as_str(),
                Some("scribe log"),
                "{event}: command"
            );
            assert_eq!(hook["timeout"].as_i64(), Some(10), "{event}: timeout");
        }
    }

    #[test]
    fn test_canonical_event_order() {
        let config = generate_hooks_config(false);
        let hooks = config["hooks"].as_object().unwrap();
        let keys: Vec<&String> = hooks.keys().collect();

        let expected: Vec<&str> = HOOK_EVENTS.iter().map(|(name, _)| *name).collect();

        assert_eq!(keys.len(), expected.len());
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(key.as_str(), expected[i], "Event at position {i} mismatch");
        }
    }

    // ── S02 tests (output modes & merge) ──

    #[test]
    fn test_new_file_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.json");

        let config = generate_hooks_config(false);
        merge_and_write(&path, &config).unwrap();

        assert!(path.exists());
        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["hooks"].as_object().unwrap().len(), 25);
    }

    #[test]
    fn test_merge_preserves_non_hooks_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // Write existing file with non-hooks keys
        std::fs::write(&path, r#"{"permissions":{"allow":["Bash"]},"hooks":{}}"#).unwrap();

        let config = generate_hooks_config(false);
        merge_and_write(&path, &config).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        // Non-hooks keys preserved
        assert!(content["permissions"]["allow"].is_array());
        // Hooks added
        assert_eq!(content["hooks"].as_object().unwrap().len(), 25);
    }

    #[test]
    fn test_merge_preserves_non_scribe_event_hooks() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // Existing file with a custom event hook on an event scribe also registers
        std::fs::write(
            &path,
            r#"{"hooks":{"WorktreeCreate":[{"hooks":[{"type":"command","command":"my-worktree-handler"}]}]}}"#,
        )
        .unwrap();

        let config = generate_hooks_config(false);
        merge_and_write(&path, &config).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let hooks = content["hooks"].as_object().unwrap();
        // WorktreeCreate has both user hook and scribe hook
        let wc_entries = hooks["WorktreeCreate"].as_array().unwrap();
        assert_eq!(wc_entries.len(), 2); // user + scribe
        assert_eq!(wc_entries[0]["hooks"][0]["command"].as_str(), Some("my-worktree-handler"));
        assert_eq!(wc_entries[1]["hooks"][0]["command"].as_str(), Some("scribe log"));
        assert_eq!(hooks.len(), 25); // all 25 scribe events
    }

    #[test]
    fn test_merge_preserves_non_scribe_hooks_on_same_event() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // Existing file with a user hook on PreToolUse alongside scribe's
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"my-linter"}]},{"matcher":"*","hooks":[{"type":"command","command":"scribe log","timeout":10}]}]}}"#,
        )
        .unwrap();

        let config = generate_hooks_config(false);
        merge_and_write(&path, &config).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let pre_tool_use = content["hooks"]["PreToolUse"].as_array().unwrap();

        // Both entries present
        assert_eq!(pre_tool_use.len(), 2);
        // User's linter hook preserved
        assert_eq!(
            pre_tool_use[0]["hooks"][0]["command"].as_str(),
            Some("my-linter")
        );
        // Scribe's hook updated in-place
        assert_eq!(
            pre_tool_use[1]["hooks"][0]["command"].as_str(),
            Some("scribe log")
        );
    }

    #[test]
    fn test_merge_appends_when_no_scribe_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // Existing file with only a user hook on PreToolUse (no scribe)
        std::fs::write(
            &path,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"my-linter"}]}]}}"#,
        )
        .unwrap();

        let config = generate_hooks_config(false);
        merge_and_write(&path, &config).unwrap();

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let pre_tool_use = content["hooks"]["PreToolUse"].as_array().unwrap();

        // User hook + scribe hook appended
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
    fn test_merge_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        let config = generate_hooks_config(false);
        merge_and_write(&path, &config).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();

        merge_and_write(&path, &config).unwrap();
        let second = std::fs::read_to_string(&path).unwrap();

        assert_eq!(
            first, second,
            "Running init twice should produce identical output"
        );
    }

    #[test]
    fn test_invalid_json_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");

        std::fs::write(&path, "not json {{{").unwrap();

        let config = generate_hooks_config(false);
        let result = merge_and_write(&path, &config);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid JSON"));

        // File should not be overwritten
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json {{{");
    }

    #[test]
    fn test_run_global_with_home_override() {
        let dir = tempfile::tempdir().unwrap();
        run(OutputTarget::Global, Some(dir.path().to_path_buf()), false).unwrap();

        let path = dir.path().join(".claude").join("settings.json");
        assert!(path.exists());

        let content: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content["hooks"].as_object().unwrap().len(), 25);
    }

    // ── Guard registration tests (US-0036) ──

    #[test]
    fn test_with_guard_adds_guard_to_pretooluse() {
        let config = generate_hooks_config(true);
        let pre = &config["hooks"]["PreToolUse"][0]["hooks"];
        let hooks = pre.as_array().unwrap();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0]["command"], "scribe guard");
        assert_eq!(hooks[1]["command"], "scribe log");
    }

    #[test]
    fn test_with_guard_only_on_pretooluse() {
        let config = generate_hooks_config(true);
        // PostToolUse should still only have log
        let post = &config["hooks"]["PostToolUse"][0]["hooks"];
        let hooks = post.as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "scribe log");

        // SessionStart should still only have log
        let session = &config["hooks"]["SessionStart"][0]["hooks"];
        let s_hooks = session.as_array().unwrap();
        assert_eq!(s_hooks.len(), 1);
        assert_eq!(s_hooks[0]["command"], "scribe log");
    }

    #[test]
    fn test_without_guard_unchanged() {
        let with = generate_hooks_config(false);
        // PreToolUse should have only log
        let pre = &with["hooks"]["PreToolUse"][0]["hooks"];
        let hooks = pre.as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["command"], "scribe log");
    }

    #[test]
    fn test_guard_has_correct_timeout() {
        let config = generate_hooks_config(true);
        let guard = &config["hooks"]["PreToolUse"][0]["hooks"][0];
        assert_eq!(guard["timeout"], 10);
    }
}
