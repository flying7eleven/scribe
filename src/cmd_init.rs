use serde_json::{json, Value};

/// Hook events and whether they support the `matcher` field.
/// Ordered canonically per the plan (PreToolUse through TaskCompleted).
/// WorktreeCreate is intentionally excluded.
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
];

/// Generate the complete hooks configuration as a serde_json::Value.
///
/// Pure function — no I/O. Output modes (stdout, file write) are handled by the
/// init handler (E03-S02).
#[allow(dead_code)] // Wired in by E03-S02 (init handler)
pub fn generate_hooks_config() -> Value {
    let mut hooks = serde_json::Map::new();

    for &(event, has_matcher) in HOOK_EVENTS {
        let hook_obj = json!({
            "type": "command",
            "command": "scribe log",
            "timeout": 10
        });

        let entry = if has_matcher {
            json!({
                "matcher": "*",
                "hooks": [hook_obj]
            })
        } else {
            json!({
                "hooks": [hook_obj]
            })
        };

        hooks.insert(event.to_string(), json!([entry]));
    }

    json!({ "hooks": hooks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generates_valid_json() {
        let config = generate_hooks_config();
        // Should be serializable without error
        let json_str = serde_json::to_string_pretty(&config).unwrap();
        // And parseable back
        let _: Value = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_all_21_events_present() {
        let config = generate_hooks_config();
        let hooks = config["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 21);
    }

    #[test]
    fn test_no_worktree_create() {
        let config = generate_hooks_config();
        let hooks = config["hooks"].as_object().unwrap();
        assert!(!hooks.contains_key("WorktreeCreate"));
    }

    #[test]
    fn test_matcher_events_have_matcher() {
        let config = generate_hooks_config();
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
        let config = generate_hooks_config();
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
        let config = generate_hooks_config();
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
        let config = generate_hooks_config();
        let hooks = config["hooks"].as_object().unwrap();
        let keys: Vec<&String> = hooks.keys().collect();

        let expected: Vec<&str> = HOOK_EVENTS.iter().map(|(name, _)| *name).collect();

        assert_eq!(keys.len(), expected.len());
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(key.as_str(), expected[i], "Event at position {i} mismatch");
        }
    }
}
