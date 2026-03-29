//! `scribe guard` subcommand: PreToolUse hook handler that evaluates rules and blocks/allows tool calls.

use std::io::Read;
use std::time::Instant;

use regex::Regex;
use sqlx::SqlitePool;

use crate::db;

/// Run the guard subcommand. Returns the process exit code (0 = allow, 2 = deny).
pub async fn run(pool: &SqlitePool) -> i32 {
    match run_inner(pool).await {
        Ok(code) => code,
        Err(e) => {
            // Fail-open: any error means allow
            eprintln!("scribe guard: internal error (allowing): {e}");
            0
        }
    }
}

async fn run_inner(pool: &SqlitePool) -> Result<i32, Box<dyn std::error::Error>> {
    let start = Instant::now();

    // TTY detection — don't block if run manually
    if atty_stdin() {
        eprintln!("scribe guard: no piped input (run as a Claude Code PreToolUse hook)");
        return Ok(0);
    }

    // Read stdin
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;

    if input.trim().is_empty() {
        return Ok(0);
    }

    // Parse JSON
    let payload: serde_json::Value = serde_json::from_str(&input)?;

    let session_id = payload
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_name = payload
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // No tool_name means this isn't a tool event — allow
    if tool_name.is_empty() {
        return Ok(0);
    }

    let tool_input_str = payload
        .get("tool_input")
        .map(|v| v.to_string())
        .unwrap_or_default();

    // Load enabled rules
    let rules = db::load_enabled_rules(pool).await?;

    // Evaluate rules (first match wins)
    for rule in &rules {
        // Match tool_pattern against tool_name
        let tool_regex = match Regex::new(&rule.tool_pattern) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "scribe guard: invalid regex in rule {}: {} (skipping)",
                    rule.id, e
                );
                continue;
            }
        };

        if !tool_regex.is_match(tool_name) {
            continue;
        }

        // Match input_pattern against tool_input (if pattern is set)
        if let Some(ref input_pattern) = rule.input_pattern {
            let input_regex = match Regex::new(input_pattern) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!(
                        "scribe guard: invalid input regex in rule {}: {} (skipping)",
                        rule.id, e
                    );
                    continue;
                }
            };
            if !input_regex.is_match(&tool_input_str) {
                continue;
            }
        }

        // Rule matched — apply action
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        match rule.action.as_str() {
            "allow" => {
                let _ = db::insert_enforcement(
                    pool,
                    session_id,
                    tool_name,
                    Some(&tool_input_str),
                    Some(rule.id),
                    "allowed",
                    Some(&rule.reason),
                    elapsed_ms,
                )
                .await;
                return Ok(0);
            }
            "deny" => {
                let _ = db::insert_enforcement(
                    pool,
                    session_id,
                    tool_name,
                    Some(&tool_input_str),
                    Some(rule.id),
                    "denied",
                    Some(&rule.reason),
                    elapsed_ms,
                )
                .await;
                eprintln!("scribe guard: DENIED — {}", rule.reason);
                return Ok(2);
            }
            other => {
                eprintln!(
                    "scribe guard: unknown action '{}' in rule {} (skipping)",
                    other, rule.id
                );
                continue;
            }
        }
    }

    // No matching rule — fail-open (allow, no enforcement record)
    Ok(0)
}

/// Check if stdin is a TTY (no piped input).
fn atty_stdin() -> bool {
    use std::os::unix::io::AsRawFd;
    unsafe { libc_isatty(std::io::stdin().as_raw_fd()) }
}

/// Minimal isatty check without pulling in the `atty` crate.
unsafe fn libc_isatty(fd: std::os::unix::io::RawFd) -> bool {
    // SAFETY: isatty is safe to call with any fd
    extern "C" {
        fn isatty(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    }
    unsafe { isatty(fd) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_guard_no_rules_allows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        // No rules → evaluate should allow
        let rules = db::load_enabled_rules(&pool).await.unwrap();
        assert!(rules.is_empty());
    }

    #[tokio::test]
    async fn test_load_rules_ordered() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        // Insert rules with different priorities
        sqlx::query(
            "INSERT INTO rules (tool_pattern, action, reason, priority, enabled) VALUES (?, ?, ?, ?, 1)",
        )
        .bind("Bash")
        .bind("deny")
        .bind("low priority")
        .bind(10)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO rules (tool_pattern, action, reason, priority, enabled) VALUES (?, ?, ?, ?, 1)",
        )
        .bind("Bash")
        .bind("allow")
        .bind("high priority")
        .bind(100)
        .execute(&pool)
        .await
        .unwrap();

        let rules = db::load_enabled_rules(&pool).await.unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].priority, 100); // higher priority first
        assert_eq!(rules[0].action, "allow");
    }

    #[tokio::test]
    async fn test_disabled_rules_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        sqlx::query(
            "INSERT INTO rules (tool_pattern, action, reason, priority, enabled) VALUES (?, ?, ?, ?, 0)",
        )
        .bind("Bash")
        .bind("deny")
        .bind("disabled rule")
        .bind(100)
        .execute(&pool)
        .await
        .unwrap();

        let rules = db::load_enabled_rules(&pool).await.unwrap();
        assert!(rules.is_empty());
    }

    #[tokio::test]
    async fn test_insert_enforcement() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        let id = db::insert_enforcement(
            &pool,
            "sess-1",
            "Bash",
            Some(r#"{"command":"ls"}"#),
            None,
            "allowed",
            None,
            1.5,
        )
        .await
        .unwrap();

        assert!(id > 0);

        // Verify it was inserted
        let row = sqlx::query("SELECT action, tool_name FROM enforcements WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let action: String = sqlx::Row::get(&row, "action");
        assert_eq!(action, "allowed");
    }

    #[test]
    fn test_regex_matching() {
        let re = Regex::new("Bash").unwrap();
        assert!(re.is_match("Bash"));
        assert!(!re.is_match("Read"));

        // Wildcard pattern
        let re_all = Regex::new(".*").unwrap();
        assert!(re_all.is_match("Bash"));
        assert!(re_all.is_match("Read"));

        // Pattern matching tool input
        let re_rm = Regex::new(r"rm\s+-rf").unwrap();
        assert!(re_rm.is_match(r#"{"command":"rm -rf /tmp"}"#));
        assert!(!re_rm.is_match(r#"{"command":"ls -la"}"#));
    }

    #[test]
    fn test_invalid_regex_handled() {
        #[allow(clippy::invalid_regex)]
        let result = Regex::new("[invalid");
        assert!(result.is_err());
    }
}
