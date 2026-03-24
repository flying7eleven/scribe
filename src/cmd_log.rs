use std::io::{self, IsTerminal, Read};

use sqlx::SqlitePool;

use crate::db;
use crate::models::HookInput;

/// Read hook JSON from stdin and insert into the database.
///
/// Returns Ok(()) on success or if an error was handled gracefully.
/// The caller should always exit 0 regardless of the result.
pub async fn run(pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
    // TTY detection: if stdin is a terminal, print hint and return
    if io::stdin().is_terminal() {
        eprintln!("scribe log: reads hook JSON from stdin (not a TTY)");
        eprintln!("  Usage: echo '{{\"session_id\":\"...\", ...}}' | scribe log");
        return Ok(());
    }

    // Read entire stdin
    let mut raw = String::new();
    io::stdin().read_to_string(&mut raw)?;

    process_payload(pool, &raw).await
}

/// Process a raw JSON payload string: parse, extract fields, insert into DB.
///
/// Separated from `run()` so integration tests can call it directly without
/// needing to simulate stdin.
pub async fn process_payload(
    pool: &SqlitePool,
    raw: &str,
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

    // Serialize Value fields to JSON strings for DB storage
    let tool_input_str = input
        .tool_input
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let tool_response_str = input
        .tool_response
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    // Insert into DB
    db::insert_event(
        pool,
        &input.session_id,
        &input.hook_event_name, // hook_event_name → event_type
        input.tool_name.as_deref(),
        tool_input_str.as_deref(),
        tool_response_str.as_deref(),
        &input.cwd,
        input.permission_mode.as_deref(),
        raw, // original stdin string as raw_payload
    )
    .await?;

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

        process_payload(&pool, raw).await.unwrap();

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
        process_payload(pool, json).await.unwrap();
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

    // ── Sessions table verification ──

    #[tokio::test]
    async fn test_sessions_upsert_via_handler() {
        let (pool, _dir) = setup_db().await;

        // Two events, same session, different cwd
        process_payload(
            &pool,
            r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/project-a"}"#,
        )
        .await
        .unwrap();
        process_payload(&pool, r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/project-b","tool_name":"Bash"}"#).await.unwrap();

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
        )
        .await
        .unwrap();
        process_payload(
            &pool,
            r#"{"session_id":"s2","hook_event_name":"SessionStart","cwd":"/b"}"#,
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
        let result = process_payload(&pool, "not json {{{").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_empty_stdin_returns_ok() {
        let (pool, _dir) = setup_db().await;
        let result = process_payload(&pool, "").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_whitespace_only_returns_ok() {
        let (pool, _dir) = setup_db().await;
        let result = process_payload(&pool, "   \n  ").await;
        assert!(result.is_ok());
    }

    // ── Raw payload losslessness ──

    #[tokio::test]
    async fn test_raw_payload_preserved_exactly() {
        let (pool, _dir) = setup_db().await;

        // Payload with specific formatting, field order, and extra fields
        let raw = "{ \"session_id\" : \"s1\" , \"hook_event_name\" : \"Stop\" , \"cwd\" : \"/tmp\" , \"unknown_field\" : 42 , \"nested\" : { \"a\" : 1 } }";
        process_payload(&pool, raw).await.unwrap();

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
        )
        .await
        .unwrap();

        // Measure warm insert
        let payload = r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{"command":"echo hi"}}"#;
        let start = std::time::Instant::now();
        process_payload(&pool, payload).await.unwrap();
        let elapsed = start.elapsed();

        // Soft assertion: print timing, only fail if way over budget
        eprintln!("Warm insert latency: {:?}", elapsed);
        assert!(
            elapsed.as_millis() < 100,
            "Warm insert took {:?} — expected < 10ms (100ms hard limit for CI)",
            elapsed
        );
    }
}
