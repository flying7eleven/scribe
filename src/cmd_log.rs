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

    if raw.trim().is_empty() {
        eprintln!("scribe log: empty stdin, nothing to log");
        return Ok(());
    }

    // Parse as raw Value first (resilience: captures everything)
    let value: serde_json::Value = match serde_json::from_str(&raw) {
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
            let fallback: HookInput = serde_json::from_str(&raw).unwrap_or_default();
            fallback
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
        &raw, // original stdin string as raw_payload
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

        // Parse and extract like the handler does
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        let input: HookInput = serde_json::from_value(value).unwrap();

        let tool_input_str = input
            .tool_input
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .unwrap();
        let tool_response_str = input
            .tool_response
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .unwrap();

        db::insert_event(
            &pool,
            &input.session_id,
            &input.hook_event_name,
            input.tool_name.as_deref(),
            tool_input_str.as_deref(),
            tool_response_str.as_deref(),
            &input.cwd,
            input.permission_mode.as_deref(),
            raw,
        )
        .await
        .unwrap();

        // Verify the inserted event
        let row = sqlx::query("SELECT session_id, event_type, tool_name, tool_input, tool_response, cwd, permission_mode, raw_payload FROM events ORDER BY id DESC LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();

        // hook_event_name → event_type mapping
        let event_type: String = row.get("event_type");
        assert_eq!(event_type, "PreToolUse");

        let session_id: String = row.get("session_id");
        assert_eq!(session_id, "sess-42");

        let tool_name: Option<String> = row.get("tool_name");
        assert_eq!(tool_name.as_deref(), Some("Bash"));

        // tool_input Value → JSON string
        let tool_input: Option<String> = row.get("tool_input");
        assert_eq!(tool_input.as_deref(), Some(r#"{"command":"ls -la"}"#));

        // null tool_response → None
        let tool_response: Option<String> = row.get("tool_response");
        assert!(tool_response.is_none());

        let cwd: Option<String> = row.get("cwd");
        assert_eq!(cwd.as_deref(), Some("/project"));

        let permission_mode: Option<String> = row.get("permission_mode");
        assert_eq!(permission_mode.as_deref(), Some("default"));

        // raw_payload is the original string
        let raw_payload: String = row.get("raw_payload");
        assert_eq!(raw_payload, raw);

        pool.close().await;
    }
}
