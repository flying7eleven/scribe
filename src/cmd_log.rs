use std::io::{self, IsTerminal, Read};

use chrono::Utc;
use sqlx::SqlitePool;

use crate::db;
use crate::models::HookInput;

/// Retention config passed from main.rs.
pub struct RetentionConfig {
    pub retention: String,
    pub check_interval: String,
}

/// Read hook JSON from stdin and insert into the database.
///
/// Returns Ok(()) on success or if an error was handled gracefully.
/// The caller should always exit 0 regardless of the result.
pub async fn run(
    pool: &SqlitePool,
    retention: Option<&RetentionConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    // TTY detection: if stdin is a terminal, print hint and return
    if io::stdin().is_terminal() {
        eprintln!("scribe log: reads hook JSON from stdin (not a TTY)");
        eprintln!("  Usage: echo '{{\"session_id\":\"...\", ...}}' | scribe log");
        return Ok(());
    }

    // Read entire stdin
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;

    process_payload(pool, &raw, retention).await
}

/// Process a raw JSON payload string: parse, extract fields, insert into DB.
///
/// Separated from `run()` so integration tests can call it directly without
/// needing to simulate stdin.
pub async fn process_payload(
    pool: &SqlitePool,
    raw: &str,
    retention: Option<&RetentionConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    if raw.trim().is_empty() {
        eprintln!("scribe log: empty stdin, nothing to log");
        return Ok(());
    }

    // Parse as raw Value first (resilience: captures everything)
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("scribe log: malformed JSON: {e}");
            return Ok(());
        }
    };

    // Extract known fields into HookInput
    let input: HookInput = match serde_json::from_value(value) {
        Ok(h) => h,
        Err(e) => {
            // Fallback: try to extract minimal fields from raw string
            eprintln!("scribe log: failed to extract fields: {e}, inserting with minimal data");
            serde_json::from_str(raw).unwrap_or_default()
        }
    };

    // Insert into DB (insert_event now accepts &HookInput directly)
    db::insert_event(pool, &input, raw).await?;

    // Auto-retention: if configured, maybe clean up expired events
    if let Some(ret) = retention {
        if let Err(e) = maybe_run_retention(pool, &ret.retention, &ret.check_interval).await {
            eprintln!("scribe: auto-retention error: {e}");
        }
    }

    Ok(())
}

/// Check if auto-retention should run, and if so, execute it.
/// Returns immediately (< 1ms) if the check interval has not elapsed.
pub async fn maybe_run_retention(
    pool: &SqlitePool,
    retention_str: &str,
    check_interval_str: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let check_interval = humantime::parse_duration(check_interval_str)
        .map_err(|e| format!("invalid retention_check_interval '{check_interval_str}': {e}"))?;
    let check_interval_chrono = chrono::Duration::from_std(check_interval)?;

    // Check if we need to run retention
    let now = Utc::now();
    if let Some(last_check_str) = db::get_metadata(pool, "last_retention_check").await? {
        if let Ok(last_check) = chrono::DateTime::parse_from_rfc3339(&last_check_str) {
            if now - last_check.with_timezone(&Utc) < check_interval_chrono {
                return Ok(()); // Not time yet
            }
        }
        // If parsing fails, treat as "never checked" and proceed
    }

    // Parse retention duration and compute cutoff
    let retention_duration = humantime::parse_duration(retention_str)
        .map_err(|e| format!("invalid retention '{retention_str}': {e}"))?;
    let cutoff = now - chrono::Duration::from_std(retention_duration)?;
    let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // Delete expired events and orphaned sessions
    db::delete_events_before(pool, &cutoff_str).await?;
    db::delete_orphaned_sessions(pool).await?;

    // Reclaim disk space
    sqlx::query("PRAGMA incremental_vacuum")
        .execute(pool)
        .await?;
    sqlx::query("PRAGMA journal_size_limit = 0")
        .execute(pool)
        .await?;

    // Update last check timestamp
    let now_str = now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    db::set_metadata(pool, "last_retention_check", &now_str).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use sqlx::Row;

    async fn setup_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn test_field_mapping_and_serialization() {
        let (pool, _dir) = setup_db().await;

        let raw = r#"{"session_id":"sess-42","hook_event_name":"PreToolUse","cwd":"/project","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"ls -la"},"tool_response":null}"#;

        process_payload(&pool, raw, None).await.unwrap();

        let row = sqlx::query("SELECT session_id, event_type, tool_name, tool_input, tool_response, cwd, permission_mode, raw_payload FROM events ORDER BY id DESC LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(row.get::<String, _>("event_type"), "PreToolUse");
        assert_eq!(row.get::<String, _>("session_id"), "sess-42");
        assert_eq!(
            row.get::<Option<String>, _>("tool_name").as_deref(),
            Some("Bash")
        );
        assert_eq!(
            row.get::<Option<String>, _>("tool_input").as_deref(),
            Some(r#"{"command":"ls -la"}"#)
        );
        assert!(row.get::<Option<String>, _>("tool_response").is_none());
        assert_eq!(
            row.get::<Option<String>, _>("cwd").as_deref(),
            Some("/project")
        );
        assert_eq!(
            row.get::<Option<String>, _>("permission_mode").as_deref(),
            Some("default")
        );
        assert_eq!(row.get::<String, _>("raw_payload"), raw);

        pool.close().await;
    }

    /// Helper: insert a payload and return the last inserted event row
    async fn insert_and_get(pool: &SqlitePool, json: &str) -> sqlx::sqlite::SqliteRow {
        process_payload(pool, json, None).await.unwrap();
        sqlx::query("SELECT * FROM events ORDER BY id DESC LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    // ── All 21 event types ──

    #[tokio::test]
    async fn test_event_pre_tool_use() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{"command":"ls"}}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "PreToolUse");
        assert_eq!(
            row.get::<Option<String>, _>("tool_name").as_deref(),
            Some("Bash")
        );
        assert!(row.get::<Option<String>, _>("tool_input").is_some());
    }

    #[tokio::test]
    async fn test_event_post_tool_use() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":"/tmp","tool_name":"Write","tool_response":{"content":"ok"}}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "PostToolUse");
        assert_eq!(
            row.get::<Option<String>, _>("tool_response").as_deref(),
            Some(r#"{"content":"ok"}"#)
        );
    }

    #[tokio::test]
    async fn test_event_post_tool_use_failure() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"PostToolUseFailure","cwd":"/tmp","tool_name":"Bash","error":"timeout"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "PostToolUseFailure");
    }

    #[tokio::test]
    async fn test_event_user_prompt_submit() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/tmp","prompt":"fix the bug"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "UserPromptSubmit");
    }

    #[tokio::test]
    async fn test_event_permission_request() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"PermissionRequest","cwd":"/tmp","tool_name":"Bash","permission_suggestions":{"allow":true}}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "PermissionRequest");
        assert_eq!(
            row.get::<Option<String>, _>("tool_name").as_deref(),
            Some("Bash")
        );
    }

    #[tokio::test]
    async fn test_event_session_start() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/tmp","source":"startup","model":"claude-sonnet-4-5-20250514"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "SessionStart");
    }

    #[tokio::test]
    async fn test_event_session_end() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"SessionEnd","cwd":"/tmp","reason":"clear"}"#,
        )
        .await;
        assert_eq!(row.get::<String, _>("event_type"), "SessionEnd");
    }

    #[tokio::test]
    async fn test_event_subagent_start() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"SubagentStart","cwd":"/tmp","agent_id":"a1","agent_type":"Explore"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "SubagentStart");
    }

    #[tokio::test]
    async fn test_event_subagent_stop() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"SubagentStop","cwd":"/tmp","agent_id":"a1","stop_hook_active":true}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "SubagentStop");
    }

    #[tokio::test]
    async fn test_event_stop() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"Stop","cwd":"/tmp","stop_hook_active":false,"last_assistant_message":"done"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "Stop");
    }

    #[tokio::test]
    async fn test_event_stop_failure() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"StopFailure","cwd":"/tmp","error":"rate_limit"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "StopFailure");
    }

    #[tokio::test]
    async fn test_event_notification() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"Notification","cwd":"/tmp","message":"hi","notification_type":"permission_prompt"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "Notification");
    }

    #[tokio::test]
    async fn test_event_pre_compact() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"PreCompact","cwd":"/tmp","trigger":"auto"}"#,
        )
        .await;
        assert_eq!(row.get::<String, _>("event_type"), "PreCompact");
    }

    #[tokio::test]
    async fn test_event_post_compact() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"PostCompact","cwd":"/tmp","trigger":"manual","compact_summary":"done"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "PostCompact");
    }

    #[tokio::test]
    async fn test_event_instructions_loaded() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"InstructionsLoaded","cwd":"/tmp","file_path":"/CLAUDE.md","load_reason":"session_start"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "InstructionsLoaded");
    }

    #[tokio::test]
    async fn test_event_config_change() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"ConfigChange","cwd":"/tmp","source":"user_settings"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "ConfigChange");
    }

    #[tokio::test]
    async fn test_event_worktree_remove() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"WorktreeRemove","cwd":"/tmp","worktree_path":"/wt"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "WorktreeRemove");
    }

    #[tokio::test]
    async fn test_event_elicitation() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"Elicitation","cwd":"/tmp","elicitation_id":"e1","mode":"form"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "Elicitation");
    }

    #[tokio::test]
    async fn test_event_elicitation_result() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"ElicitationResult","cwd":"/tmp","elicitation_id":"e1","action":"accept"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "ElicitationResult");
    }

    #[tokio::test]
    async fn test_event_teammate_idle() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"TeammateIdle","cwd":"/tmp","teammate_name":"bob"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "TeammateIdle");
    }

    #[tokio::test]
    async fn test_event_task_completed() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"TaskCompleted","cwd":"/tmp","task_id":"t1","task_subject":"fix"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "TaskCompleted");
    }

    #[tokio::test]
    async fn test_event_worktree_create() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"WorktreeCreate","cwd":"/tmp","worktree_path":"/worktree/feature-x"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "WorktreeCreate");
    }

    #[tokio::test]
    async fn test_event_file_changed() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"FileChanged","cwd":"/project"}"#,
        )
        .await;
        assert_eq!(row.get::<String, _>("event_type"), "FileChanged");
    }

    #[tokio::test]
    async fn test_event_cwd_changed() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"CwdChanged","cwd":"/new/working/dir"}"#,
        )
        .await;
        assert_eq!(row.get::<String, _>("event_type"), "CwdChanged");
        assert_eq!(row.get::<String, _>("cwd"), "/new/working/dir");
    }

    #[tokio::test]
    async fn test_event_task_created() {
        let (pool, _dir) = setup_db().await;
        let row = insert_and_get(&pool, r#"{"session_id":"s1","hook_event_name":"TaskCreated","cwd":"/tmp","task_id":"t1","task_subject":"implement feature","task_description":"add auth","teammate_name":"bob","team_name":"alpha"}"#).await;
        assert_eq!(row.get::<String, _>("event_type"), "TaskCreated");
    }

    // ── Sessions table verification ──

    #[tokio::test]
    async fn test_sessions_upsert_via_handler() {
        let (pool, _dir) = setup_db().await;

        // Two events, same session, different cwd
        process_payload(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/project-a"}"#,
            None,
        )
        .await
        .unwrap();
        process_payload(&pool, r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/project-b","tool_name":"Bash"}"#, None).await.unwrap();

        let row = sqlx::query(
            "SELECT event_count, cwd, first_seen, last_seen FROM sessions WHERE session_id = 's1'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        assert_eq!(row.get::<i32, _>("event_count"), 2);
        assert_eq!(row.get::<String, _>("cwd"), "/project-b"); // latest
        assert!(row.get::<String, _>("last_seen") >= row.get::<String, _>("first_seen"));
    }

    #[tokio::test]
    async fn test_sessions_separate_rows() {
        let (pool, _dir) = setup_db().await;

        process_payload(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/a"}"#,
            None,
        )
        .await
        .unwrap();
        process_payload(
            &pool,
            r#"{"session_id":"s2","hook_event_name":"SessionStart","cwd":"/b"}"#,
            None,
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);
    }

    // ── Error cases ──

    #[tokio::test]
    async fn test_malformed_json_returns_ok() {
        let (pool, _dir) = setup_db().await;
        let result = process_payload(&pool, "not json {{{", None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_empty_stdin_returns_ok() {
        let (pool, _dir) = setup_db().await;
        let result = process_payload(&pool, "", None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_whitespace_only_returns_ok() {
        let (pool, _dir) = setup_db().await;
        let result = process_payload(&pool, "   \n  ", None).await;
        assert!(result.is_ok());
    }

    // ── Raw payload losslessness ──

    #[tokio::test]
    async fn test_raw_payload_preserved_exactly() {
        let (pool, _dir) = setup_db().await;

        // Payload with specific formatting, field order, and extra fields
        let raw = "{ \"session_id\" : \"s1\" , \"hook_event_name\" : \"Stop\" , \"cwd\" : \"/tmp\" , \"unknown_field\" : 42 , \"nested\" : { \"a\" : 1 } }";
        process_payload(&pool, raw, None).await.unwrap();

        let stored: String =
            sqlx::query_scalar("SELECT raw_payload FROM events ORDER BY id DESC LIMIT 1")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, raw, "raw_payload must match original input exactly");
    }

    // ── Latency measurement ──

    #[tokio::test]
    async fn test_latency_warm_db() {
        let (pool, _dir) = setup_db().await;

        // Warm the DB with one insert
        process_payload(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/tmp"}"#,
            None,
        )
        .await
        .unwrap();

        // Measure warm insert
        let payload = r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{"command":"echo hi"}}"#;
        let start = std::time::Instant::now();
        process_payload(&pool, payload, None).await.unwrap();
        let elapsed = start.elapsed();

        // Soft assertion: print timing, only fail if way over budget
        eprintln!("Warm insert latency: {:?}", elapsed);
        assert!(
            elapsed.as_millis() < 100,
            "Warm insert took {:?} — expected < 10ms (100ms hard limit for CI)",
            elapsed
        );
    }

    // ── Auto-retention tests ──

    async fn insert_event_at(pool: &SqlitePool, session: &str, ts: &str) {
        sqlx::query(
            "INSERT INTO events (timestamp, session_id, event_type, cwd, raw_payload) VALUES (?, ?, 'PreToolUse', '/tmp', '{}')",
        )
        .bind(ts)
        .bind(session)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count) VALUES (?, ?, ?, '/tmp', 1) ON CONFLICT(account_id, session_id) DO UPDATE SET last_seen = excluded.last_seen, event_count = event_count + 1",
        )
        .bind(session)
        .bind(ts)
        .bind(ts)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_retention_check_not_elapsed() {
        let (pool, _dir) = setup_db().await;

        // Set last check to 1 hour ago
        let one_hour_ago = (chrono::Utc::now() - chrono::Duration::hours(1))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        db::set_metadata(&pool, "last_retention_check", &one_hour_ago)
            .await
            .unwrap();

        // Insert an old event
        insert_event_at(&pool, "s1", "2020-01-01T00:00:00.000Z").await;

        // With 24h check interval, should NOT run (only 1h since last check)
        maybe_run_retention(&pool, "1d", "24h").await.unwrap();

        // Old event should still be there
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_retention_check_elapsed() {
        let (pool, _dir) = setup_db().await;

        // Set last check to 25 hours ago
        let old_check = (chrono::Utc::now() - chrono::Duration::hours(25))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        db::set_metadata(&pool, "last_retention_check", &old_check)
            .await
            .unwrap();

        // Insert old and new events
        insert_event_at(&pool, "s1", "2020-01-01T00:00:00.000Z").await;
        let now = chrono::Utc::now()
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        insert_event_at(&pool, "s2", &now).await;

        // With 24h check interval, should run (25h since last check)
        maybe_run_retention(&pool, "1d", "24h").await.unwrap();

        // Only new event should remain
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_retention_missing_key_triggers_cleanup() {
        let (pool, _dir) = setup_db().await;

        // No last_retention_check set — should run cleanup
        insert_event_at(&pool, "s1", "2020-01-01T00:00:00.000Z").await;

        maybe_run_retention(&pool, "1d", "24h").await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);

        // Metadata should now be set
        let check = db::get_metadata(&pool, "last_retention_check")
            .await
            .unwrap();
        assert!(check.is_some());
    }

    #[tokio::test]
    async fn test_retention_invalid_retention_string() {
        let (pool, _dir) = setup_db().await;
        let result = maybe_run_retention(&pool, "not-valid", "24h").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_retention_invalid_check_interval() {
        let (pool, _dir) = setup_db().await;
        let result = maybe_run_retention(&pool, "90d", "not-valid").await;
        assert!(result.is_err());
    }
}
