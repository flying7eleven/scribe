use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, Row, SqlitePool};

/// A row from the `events` table.
#[derive(Debug, FromRow)]
pub struct EventRow {
    pub id: i64,
    pub timestamp: String,
    pub session_id: String,
    pub event_type: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_response: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
    pub raw_payload: String,
}

/// Filter parameters for querying events.
#[derive(Default)]
pub struct EventFilter {
    pub since: Option<String>,
    pub until: Option<String>,
    pub session_id: Option<String>,
    pub event_type: Option<String>,
    pub tool_name: Option<String>,
    pub search: Option<String>,
    pub limit: i64,
}

impl EventFilter {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            limit: 50,
            ..Default::default()
        }
    }
}

/// A row from the `sessions` table.
#[derive(Debug, FromRow)]
pub struct SessionRow {
    pub session_id: String,
    pub first_seen: String,
    pub last_seen: String,
    pub cwd: Option<String>,
    pub event_count: i64,
}

/// Filter parameters for querying sessions.
#[derive(Default)]
pub struct SessionFilter {
    pub since: Option<String>,
    pub limit: i64,
}

impl SessionFilter {
    #[cfg(test)]
    pub fn new() -> Self {
        Self {
            limit: 50,
            ..Default::default()
        }
    }
}

/// Resolve the database path with 4-layer precedence:
/// 1. `--db <path>` CLI argument (highest)
/// 2. `SCRIBE_DB` environment variable
/// 3. Config file `db_path`
/// 4. Default: `~/.claude/scribe.db`
pub fn resolve_db_path(
    cli_db: Option<&str>,
    config_db: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(path) = cli_db {
        return Ok(path.to_string());
    }

    if let Ok(path) = std::env::var("SCRIBE_DB") {
        if !path.is_empty() {
            return Ok(path);
        }
    }

    if let Some(path) = config_db {
        return Ok(path.to_string());
    }

    let home = dirs::home_dir().ok_or("could not determine home directory")?;
    Ok(home
        .join(".claude")
        .join("scribe.db")
        .to_string_lossy()
        .to_string())
}

/// Open (or create) a SQLite database at the given path with:
/// - WAL journal mode
/// - INCREMENTAL auto-vacuum (must be set before any tables exist)
/// - 5-second busy timeout
/// - Pool of 1 connection
/// - Embedded migrations applied automatically
pub async fn connect(db_path: &str) -> Result<SqlitePool, Box<dyn std::error::Error>> {
    // Ensure parent directory exists
    if let Some(parent) = PathBuf::from(db_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .pragma("journal_mode", "WAL")
        .pragma("auto_vacuum", "INCREMENTAL")
        .pragma("busy_timeout", "5000")
        .pragma("foreign_keys", "ON");

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;

    sqlx::migrate!().run(&pool).await?;

    Ok(pool)
}

/// Insert an event into `events`, upsert `sessions`, and populate the
/// appropriate Tier 1 detail table — all in a single transaction.
///
/// Returns the new event's row ID.
pub async fn insert_event(
    pool: &SqlitePool,
    hook_input: &crate::models::HookInput,
    raw_payload: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    let tool_input_str = hook_input
        .tool_input
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let tool_response_str = hook_input
        .tool_response
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;

    let session_id = &hook_input.session_id;
    let event_type = &hook_input.hook_event_name;
    let cwd = &hook_input.cwd;

    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO events (session_id, event_type, tool_name, tool_input, tool_response, cwd, permission_mode, raw_payload)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(event_type)
    .bind(hook_input.tool_name.as_deref())
    .bind(tool_input_str.as_deref())
    .bind(tool_response_str.as_deref())
    .bind(cwd)
    .bind(hook_input.permission_mode.as_deref())
    .bind(raw_payload)
    .execute(&mut *tx)
    .await?;

    let event_id = result.last_insert_rowid();

    sqlx::query(
        "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count)
         VALUES (?, ?, ?, ?, 1)
         ON CONFLICT(session_id) DO UPDATE SET
           last_seen = excluded.last_seen,
           cwd = excluded.cwd,
           event_count = event_count + 1",
    )
    .bind(session_id)
    .bind(&now)
    .bind(&now)
    .bind(cwd)
    .execute(&mut *tx)
    .await?;

    // Tier 1 detail inserts
    match event_type.as_str() {
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" => {
            let ps = hook_input
                .permission_suggestions
                .as_ref()
                .map(|v| v.to_string());
            sqlx::query(
                "INSERT INTO tool_event_details (event_id, tool_use_id, error, error_details, is_interrupt, permission_suggestions) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.tool_use_id.as_deref())
            .bind(hook_input.error.as_deref())
            .bind(hook_input.error_details.as_deref())
            .bind(hook_input.is_interrupt)
            .bind(ps.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "Stop" | "StopFailure" => {
            sqlx::query(
                "INSERT INTO stop_event_details (event_id, stop_hook_active, last_assistant_message, error, error_details) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.stop_hook_active)
            .bind(hook_input.last_assistant_message.as_deref())
            .bind(hook_input.error.as_deref())
            .bind(hook_input.error_details.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "SessionStart" | "SessionEnd" | "ConfigChange" => {
            sqlx::query(
                "INSERT INTO session_event_details (event_id, source, model, reason, file_path) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.source.as_deref())
            .bind(hook_input.model.as_deref())
            .bind(hook_input.reason.as_deref())
            .bind(hook_input.file_path.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        // Tier 2
        "SubagentStop" => {
            // stop_event_details (same as Stop/StopFailure)
            sqlx::query(
                "INSERT INTO stop_event_details (event_id, stop_hook_active, last_assistant_message, error, error_details) VALUES (?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.stop_hook_active)
            .bind(hook_input.last_assistant_message.as_deref())
            .bind(hook_input.error.as_deref())
            .bind(hook_input.error_details.as_deref())
            .execute(&mut *tx)
            .await?;
            // agent_event_details (dual insert)
            sqlx::query(
                "INSERT INTO agent_event_details (event_id, agent_id, agent_type, agent_transcript_path) VALUES (?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.agent_id.as_deref())
            .bind(hook_input.agent_type.as_deref())
            .bind(hook_input.agent_transcript_path.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "SubagentStart" => {
            sqlx::query(
                "INSERT INTO agent_event_details (event_id, agent_id, agent_type, agent_transcript_path) VALUES (?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.agent_id.as_deref())
            .bind(hook_input.agent_type.as_deref())
            .bind(hook_input.agent_transcript_path.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "Notification" | "Elicitation" | "ElicitationResult" => {
            let rs = hook_input.requested_schema.as_ref().map(|v| v.to_string());
            let ct = hook_input.content.as_ref().map(|v| v.to_string());
            sqlx::query(
                "INSERT INTO notification_event_details (event_id, notification_type, title, message, elicitation_id, mcp_server_name, mode, url, requested_schema, action, content) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.notification_type.as_deref())
            .bind(hook_input.title.as_deref())
            .bind(hook_input.message.as_deref())
            .bind(hook_input.elicitation_id.as_deref())
            .bind(hook_input.mcp_server_name.as_deref())
            .bind(hook_input.mode.as_deref())
            .bind(hook_input.url.as_deref())
            .bind(rs.as_deref())
            .bind(hook_input.action.as_deref())
            .bind(ct.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "PreCompact" | "PostCompact" => {
            sqlx::query(
                "INSERT INTO compact_event_details (event_id, `trigger`, custom_instructions, compact_summary) VALUES (?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.trigger.as_deref())
            .bind(hook_input.custom_instructions.as_deref())
            .bind(hook_input.compact_summary.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        // Tier 3
        "InstructionsLoaded" => {
            let globs_str = hook_input
                .globs
                .as_ref()
                .map(|g| serde_json::to_string(g).unwrap_or_default());
            sqlx::query(
                "INSERT INTO instruction_event_details (event_id, file_path, memory_type, load_reason, globs, trigger_file_path, parent_file_path) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.file_path.as_deref())
            .bind(hook_input.memory_type.as_deref())
            .bind(hook_input.load_reason.as_deref())
            .bind(globs_str.as_deref())
            .bind(hook_input.trigger_file_path.as_deref())
            .bind(hook_input.parent_file_path.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "TeammateIdle" | "TaskCompleted" => {
            sqlx::query(
                "INSERT INTO team_event_details (event_id, teammate_name, team_name, task_id, task_subject, task_description) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.teammate_name.as_deref())
            .bind(hook_input.team_name.as_deref())
            .bind(hook_input.task_id.as_deref())
            .bind(hook_input.task_subject.as_deref())
            .bind(hook_input.task_description.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        "UserPromptSubmit" => {
            sqlx::query("INSERT INTO prompt_event_details (event_id, prompt) VALUES (?, ?)")
                .bind(event_id)
                .bind(hook_input.prompt.as_deref())
                .execute(&mut *tx)
                .await?;
        }
        "WorktreeRemove" => {
            sqlx::query(
                "INSERT INTO worktree_event_details (event_id, worktree_path) VALUES (?, ?)",
            )
            .bind(event_id)
            .bind(hook_input.worktree_path.as_deref())
            .execute(&mut *tx)
            .await?;
        }
        _ => {} // truly unknown events
    }

    tx.commit().await?;
    Ok(event_id)
}

/// Test helper: construct a minimal `HookInput` from individual fields and insert.
#[cfg(test)]
pub async fn insert_test_event(
    pool: &SqlitePool,
    session_id: &str,
    event_type: &str,
    tool_name: Option<&str>,
    tool_input: Option<&str>,
    tool_response: Option<&str>,
    cwd: &str,
    permission_mode: Option<&str>,
    raw_payload: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let hook = crate::models::HookInput {
        session_id: session_id.to_string(),
        hook_event_name: event_type.to_string(),
        cwd: cwd.to_string(),
        tool_name: tool_name.map(String::from),
        tool_input: tool_input.and_then(|s| serde_json::from_str(s).ok()),
        tool_response: tool_response.and_then(|s| serde_json::from_str(s).ok()),
        permission_mode: permission_mode.map(String::from),
        ..Default::default()
    };
    insert_event(pool, &hook, raw_payload).await
}

/// Query events with dynamic filters, ordered by timestamp descending.
pub async fn query_events(
    pool: &SqlitePool,
    filter: &EventFilter,
) -> Result<Vec<EventRow>, Box<dyn std::error::Error>> {
    let mut sql = String::from(
        "SELECT id, timestamp, session_id, event_type, tool_name, tool_input, tool_response, cwd, permission_mode, raw_payload FROM events WHERE 1=1",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(ref since) = filter.since {
        sql.push_str(" AND timestamp >= ?");
        binds.push(since.clone());
    }
    if let Some(ref until) = filter.until {
        sql.push_str(" AND timestamp <= ?");
        binds.push(until.clone());
    }
    if let Some(ref session_id) = filter.session_id {
        sql.push_str(" AND session_id = ?");
        binds.push(session_id.clone());
    }
    if let Some(ref event_type) = filter.event_type {
        sql.push_str(" AND event_type = ?");
        binds.push(event_type.clone());
    }
    if let Some(ref tool_name) = filter.tool_name {
        sql.push_str(" AND tool_name = ?");
        binds.push(tool_name.clone());
    }
    if let Some(ref search) = filter.search {
        sql.push_str(" AND tool_input LIKE '%' || ? || '%'");
        binds.push(search.clone());
    }

    sql.push_str(" ORDER BY timestamp DESC LIMIT ?");
    binds.push(filter.limit.to_string());

    let mut query = sqlx::query_as::<_, EventRow>(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    Ok(rows)
}

/// Query sessions with optional filters, ordered by last_seen descending.
pub async fn query_sessions(
    pool: &SqlitePool,
    filter: &SessionFilter,
) -> Result<Vec<SessionRow>, Box<dyn std::error::Error>> {
    let mut sql = String::from(
        "SELECT session_id, first_seen, last_seen, cwd, event_count FROM sessions WHERE 1=1",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(ref since) = filter.since {
        sql.push_str(" AND last_seen >= ?");
        binds.push(since.clone());
    }

    sql.push_str(" ORDER BY last_seen DESC LIMIT ?");
    binds.push(filter.limit.to_string());

    let mut query = sqlx::query_as::<_, SessionRow>(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    Ok(rows)
}

/// Delete events with timestamp older than `before` (ISO 8601 UTC).
/// Returns the number of deleted rows.
#[allow(dead_code)] // Reused by auto-retention (E05-S04)
pub async fn delete_events_before(
    pool: &SqlitePool,
    before: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let result = sqlx::query("DELETE FROM events WHERE timestamp < ?")
        .bind(before)
        .execute(pool)
        .await?;
    Ok(result.rows_affected())
}

/// Delete sessions that have no remaining events.
/// Returns the number of deleted sessions.
#[allow(dead_code)] // Reused by auto-retention (E05-S04)
pub async fn delete_orphaned_sessions(
    pool: &SqlitePool,
) -> Result<u64, Box<dyn std::error::Error>> {
    let result = sqlx::query(
        "DELETE FROM sessions WHERE session_id NOT IN (SELECT DISTINCT session_id FROM events)",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Get a metadata value by key.
pub async fn get_metadata(
    pool: &SqlitePool,
    key: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let row: Option<String> = sqlx::query_scalar("SELECT value FROM _metadata WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// Set a metadata value (upsert).
pub async fn set_metadata(
    pool: &SqlitePool,
    key: &str,
    value: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    sqlx::query(
        "INSERT INTO _metadata (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Database metrics.
pub struct DbStats {
    pub event_count: i64,
    pub session_count: i64,
    pub oldest_event: Option<String>,
    pub newest_event: Option<String>,
}

/// Get database metrics, optionally scoped to events since a given timestamp.
pub async fn get_stats(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<DbStats, Box<dyn std::error::Error>> {
    let (event_count, oldest_event, newest_event) = if let Some(since) = since {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE timestamp >= ?")
            .bind(since)
            .fetch_one(pool)
            .await?;
        let oldest: Option<String> =
            sqlx::query_scalar("SELECT MIN(timestamp) FROM events WHERE timestamp >= ?")
                .bind(since)
                .fetch_one(pool)
                .await?;
        let newest: Option<String> =
            sqlx::query_scalar("SELECT MAX(timestamp) FROM events WHERE timestamp >= ?")
                .bind(since)
                .fetch_one(pool)
                .await?;
        (count, oldest, newest)
    } else {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(pool)
            .await?;
        let oldest: Option<String> = sqlx::query_scalar("SELECT MIN(timestamp) FROM events")
            .fetch_one(pool)
            .await?;
        let newest: Option<String> = sqlx::query_scalar("SELECT MAX(timestamp) FROM events")
            .fetch_one(pool)
            .await?;
        (count, oldest, newest)
    };

    let session_count: i64 = if let Some(since) = since {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE last_seen >= ?")
            .bind(since)
            .fetch_one(pool)
            .await?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(pool)
            .await?
    };

    Ok(DbStats {
        event_count,
        session_count,
        oldest_event,
        newest_event,
    })
}

// ── Extended stats queries (E006) ──

/// Tool usage count for the top-tools breakdown.
#[derive(Debug, Serialize)]
pub struct ToolCount {
    pub tool_name: String,
    pub count: i64,
}

/// Get the most frequently used tools, ranked by call count.
pub async fn top_tools(
    pool: &SqlitePool,
    since: Option<&str>,
    limit: i64,
) -> Result<Vec<ToolCount>, Box<dyn std::error::Error>> {
    let mut sql =
        String::from("SELECT tool_name, COUNT(*) as count FROM events WHERE tool_name IS NOT NULL");
    let mut binds: Vec<String> = Vec::new();

    if let Some(since) = since {
        sql.push_str(" AND timestamp >= ?");
        binds.push(since.to_string());
    }

    sql.push_str(" GROUP BY tool_name ORDER BY count DESC LIMIT ?");
    binds.push(limit.to_string());

    let mut query = sqlx::query(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    let results = rows
        .iter()
        .map(|row| ToolCount {
            tool_name: row.get("tool_name"),
            count: row.get("count"),
        })
        .collect();
    Ok(results)
}

/// Event type distribution count.
#[derive(Debug, Serialize)]
pub struct EventTypeCount {
    pub event_type: String,
    pub count: i64,
}

/// Get event type breakdown (only types with > 0 occurrences).
pub async fn event_type_breakdown(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<Vec<EventTypeCount>, Box<dyn std::error::Error>> {
    let mut sql = String::from("SELECT event_type, COUNT(*) as count FROM events");
    let mut binds: Vec<String> = Vec::new();

    if let Some(since) = since {
        sql.push_str(" WHERE timestamp >= ?");
        binds.push(since.to_string());
    }

    sql.push_str(" GROUP BY event_type ORDER BY count DESC");

    let mut query = sqlx::query(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    let results = rows
        .iter()
        .map(|row| EventTypeCount {
            event_type: row.get("event_type"),
            count: row.get("count"),
        })
        .collect();
    Ok(results)
}

/// A StopFailure error type and its count.
#[derive(Debug, Serialize)]
pub struct StopFailureType {
    pub error_type: String,
    pub count: i64,
}

/// Error summary with failure counts and StopFailure type breakdown.
#[derive(Debug)]
pub struct ErrorSummary {
    pub post_tool_use_failure_count: i64,
    pub stop_failure_count: i64,
    pub stop_failure_types: Vec<StopFailureType>,
}

/// Get error summary: PostToolUseFailure and StopFailure counts, with StopFailure type breakdown.
pub async fn error_summary(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<ErrorSummary, Box<dyn std::error::Error>> {
    // Count PostToolUseFailure events
    let post_tool_use_failure_count: i64 = if let Some(since) = since {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type = 'PostToolUseFailure' AND timestamp >= ?",
        )
        .bind(since)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'PostToolUseFailure'")
            .fetch_one(pool)
            .await?
    };

    // Count StopFailure events
    let stop_failure_count: i64 = if let Some(since) = since {
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE event_type = 'StopFailure' AND timestamp >= ?",
        )
        .bind(since)
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE event_type = 'StopFailure'")
            .fetch_one(pool)
            .await?
    };

    // StopFailure type breakdown via JOIN on stop_event_details (with fallback)
    let stop_failure_types = if stop_failure_count > 0 {
        // Try JOIN-based query first
        let mut sql = String::from(
            "SELECT sed.error as error_type, COUNT(*) as count \
             FROM events e \
             JOIN stop_event_details sed ON sed.event_id = e.id \
             WHERE e.event_type = 'StopFailure' AND sed.error IS NOT NULL",
        );
        let mut binds: Vec<String> = Vec::new();

        if let Some(since) = since {
            sql.push_str(" AND e.timestamp >= ?");
            binds.push(since.to_string());
        }

        sql.push_str(" GROUP BY sed.error ORDER BY count DESC");

        let mut query = sqlx::query(&sql);
        for bind in &binds {
            query = query.bind(bind);
        }

        let rows = query.fetch_all(pool).await?;
        let types: Vec<StopFailureType> = rows
            .iter()
            .map(|row| StopFailureType {
                error_type: row.get("error_type"),
                count: row.get("count"),
            })
            .collect();

        if types.is_empty() {
            // Fallback: detail tables not populated, parse raw_payload
            eprintln!("hint: run 'scribe backfill' to populate detail tables for faster queries");

            let mut fallback_sql =
                String::from("SELECT raw_payload FROM events WHERE event_type = 'StopFailure'");
            let mut fallback_binds: Vec<String> = Vec::new();

            if let Some(since) = since {
                fallback_sql.push_str(" AND timestamp >= ?");
                fallback_binds.push(since.to_string());
            }

            let mut fallback_query = sqlx::query_scalar::<_, String>(&fallback_sql);
            for bind in &fallback_binds {
                fallback_query = fallback_query.bind(bind);
            }

            let payloads = fallback_query.fetch_all(pool).await?;

            let mut type_counts: HashMap<String, i64> = HashMap::new();
            for payload in &payloads {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
                    let error_type = value
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    *type_counts.entry(error_type).or_insert(0) += 1;
                }
            }

            let mut fallback_types: Vec<StopFailureType> = type_counts
                .into_iter()
                .map(|(error_type, count)| StopFailureType { error_type, count })
                .collect();
            fallback_types.sort_by(|a, b| b.count.cmp(&a.count));
            fallback_types
        } else {
            types
        }
    } else {
        Vec::new()
    };

    Ok(ErrorSummary {
        post_tool_use_failure_count,
        stop_failure_count,
        stop_failure_types,
    })
}

/// Directory event count for the top-directories breakdown.
#[derive(Debug, Serialize)]
pub struct DirCount {
    pub cwd: String,
    pub count: i64,
}

/// Get top directories by event count.
pub async fn top_directories(
    pool: &SqlitePool,
    since: Option<&str>,
    limit: i64,
) -> Result<Vec<DirCount>, Box<dyn std::error::Error>> {
    let mut sql = String::from("SELECT cwd, COUNT(*) as count FROM events WHERE cwd IS NOT NULL");
    let mut binds: Vec<String> = Vec::new();

    if let Some(since) = since {
        sql.push_str(" AND timestamp >= ?");
        binds.push(since.to_string());
    }

    sql.push_str(" GROUP BY cwd ORDER BY count DESC LIMIT ?");
    binds.push(limit.to_string());

    let mut query = sqlx::query(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    let results = rows
        .iter()
        .map(|row| DirCount {
            cwd: row.get("cwd"),
            count: row.get("count"),
        })
        .collect();
    Ok(results)
}

/// Get average session duration in seconds, excluding single-event sessions.
/// Returns `None` if no qualifying sessions exist.
pub async fn avg_session_duration(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<Option<f64>, Box<dyn std::error::Error>> {
    let mut sql = String::from(
        "SELECT AVG((julianday(last_seen) - julianday(first_seen)) * 86400) as avg_seconds FROM sessions WHERE first_seen != last_seen",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(since) = since {
        sql.push_str(" AND last_seen >= ?");
        binds.push(since.to_string());
    }

    let mut query = sqlx::query(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let row = query.fetch_one(pool).await?;
    let avg: Option<f64> = row.get("avg_seconds");
    Ok(avg)
}

/// Daily event count for the activity histogram.
#[derive(Debug, Serialize)]
pub struct DailyCount {
    pub date: String,
    pub count: i64,
}

/// Get daily event counts. Defaults to last 14 days when `since` is `None`.
pub async fn daily_activity(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<Vec<DailyCount>, Box<dyn std::error::Error>> {
    let default_since;
    let since_val = if let Some(s) = since {
        s
    } else {
        default_since = (chrono::Utc::now() - chrono::Duration::days(14))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        &default_since
    };

    let rows = sqlx::query(
        "SELECT DATE(timestamp) as day, COUNT(*) as count FROM events WHERE timestamp >= ? GROUP BY day ORDER BY day ASC",
    )
    .bind(since_val)
    .fetch_all(pool)
    .await?;

    let results = rows
        .iter()
        .map(|row| DailyCount {
            date: row.get("day"),
            count: row.get("count"),
        })
        .collect();
    Ok(results)
}

// ── Classification DB operations (E009) ──

/// Count of classifications by risk level.
#[cfg(feature = "guard")]
#[derive(Debug)]
pub struct ClassificationCount {
    pub risk_level: String,
    pub count: i64,
}

#[cfg(feature = "guard")]
/// Check if an event already has a classification.
pub async fn has_classification_for_event(
    pool: &SqlitePool,
    event_id: i64,
) -> Result<bool, Box<dyn std::error::Error>> {
    let row = sqlx::query("SELECT COUNT(*) as cnt FROM classifications WHERE event_id = ?")
        .bind(event_id)
        .fetch_one(pool)
        .await?;
    let count: i64 = row.get("cnt");
    Ok(count > 0)
}

#[cfg(feature = "guard")]
/// Insert a classification result.
pub async fn insert_classification(
    pool: &SqlitePool,
    event_id: Option<i64>,
    classification: &crate::classify::Classification,
) -> Result<i64, Box<dyn std::error::Error>> {
    let result = sqlx::query(
        "INSERT INTO classifications (event_id, tool_name, input_pattern, risk_level, reason, heuristic) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(event_id)
    .bind(&classification.tool_name)
    .bind(&classification.input_pattern)
    .bind(classification.risk_level.as_str())
    .bind(&classification.reason)
    .bind(&classification.heuristic)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

#[cfg(feature = "guard")]
/// Get classification counts by risk level, optionally filtered by time.
pub async fn classification_summary(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<Vec<ClassificationCount>, Box<dyn std::error::Error>> {
    let mut sql =
        String::from("SELECT risk_level, COUNT(*) as count FROM classifications WHERE 1=1");
    let mut binds: Vec<String> = Vec::new();

    if let Some(s) = since {
        sql.push_str(" AND timestamp >= ?");
        binds.push(s.to_string());
    }

    sql.push_str(" GROUP BY risk_level ORDER BY count DESC");

    let mut query = sqlx::query(&sql);
    for b in &binds {
        query = query.bind(b);
    }

    let rows = query.fetch_all(pool).await?;
    let results = rows
        .iter()
        .map(|row| ClassificationCount {
            risk_level: row.get("risk_level"),
            count: row.get("count"),
        })
        .collect();

    Ok(results)
}

// ── Rules & enforcement DB operations (E009) ──

/// A rule row from the rules table.
#[cfg(feature = "guard")]
#[derive(Debug)]
#[allow(dead_code)] // priority used for ordering in DB query, accessed in policy CLI (US-0037)
pub struct RuleRow {
    pub id: i64,
    pub tool_pattern: String,
    pub input_pattern: Option<String>,
    pub action: String,
    pub reason: String,
    pub priority: i64,
}

#[cfg(feature = "guard")]
/// Load all enabled rules, ordered by priority DESC, id DESC.
pub async fn load_enabled_rules(
    pool: &SqlitePool,
) -> Result<Vec<RuleRow>, Box<dyn std::error::Error>> {
    let rows = sqlx::query(
        "SELECT id, tool_pattern, input_pattern, action, reason, priority \
         FROM rules WHERE enabled = 1 ORDER BY priority DESC, id DESC",
    )
    .fetch_all(pool)
    .await?;

    let results = rows
        .iter()
        .map(|row| RuleRow {
            id: row.get("id"),
            tool_pattern: row.get("tool_pattern"),
            input_pattern: row.get("input_pattern"),
            action: row.get("action"),
            reason: row.get("reason"),
            priority: row.get("priority"),
        })
        .collect();

    Ok(results)
}

#[cfg(feature = "guard")]
/// Insert an enforcement record.
#[allow(clippy::too_many_arguments)]
pub async fn insert_enforcement(
    pool: &SqlitePool,
    session_id: &str,
    tool_name: &str,
    tool_input: Option<&str>,
    rule_id: Option<i64>,
    action: &str,
    reason: Option<&str>,
    evaluation_ms: f64,
) -> Result<i64, Box<dyn std::error::Error>> {
    let result = sqlx::query(
        "INSERT INTO enforcements (session_id, tool_name, tool_input, rule_id, action, reason, evaluation_ms) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(tool_name)
    .bind(tool_input)
    .bind(rule_id)
    .bind(action)
    .bind(reason)
    .bind(evaluation_ms)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

// ── Policy CLI DB operations (US-0037) ──

/// A full rule row including metadata fields.
#[cfg(feature = "guard")]
#[derive(Debug)]
#[allow(dead_code)]
pub struct FullRuleRow {
    pub id: i64,
    pub tool_pattern: String,
    pub input_pattern: Option<String>,
    pub action: String,
    pub reason: String,
    pub priority: i64,
    pub enabled: bool,
    pub source: String,
    pub created_at: String,
}

#[cfg(feature = "guard")]
/// A classification row for the promote subcommand.
#[derive(Debug)]
#[allow(dead_code)]
pub struct ClassificationRow {
    pub id: i64,
    pub tool_name: String,
    pub input_pattern: String,
    pub risk_level: String,
    pub reason: String,
    pub heuristic: String,
}

#[cfg(feature = "guard")]
/// Enforcement statistics.
#[derive(Debug)]
pub struct EnforcementStats {
    pub total: i64,
    pub allowed: i64,
    pub denied: i64,
    pub top_denied: Vec<TopDeniedRule>,
}

#[cfg(feature = "guard")]
/// A top denied rule entry for stats display.
#[derive(Debug)]
pub struct TopDeniedRule {
    pub rule_id: i64,
    pub reason: String,
    pub count: i64,
}

#[cfg(feature = "guard")]
/// Insert a new policy rule.
#[allow(clippy::too_many_arguments)]
pub async fn insert_rule(
    pool: &SqlitePool,
    tool_pattern: &str,
    input_pattern: Option<&str>,
    action: &str,
    reason: &str,
    priority: i64,
    source: &str,
) -> Result<i64, Box<dyn std::error::Error>> {
    let result = sqlx::query(
        "INSERT INTO rules (tool_pattern, input_pattern, action, reason, priority, source) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(tool_pattern)
    .bind(input_pattern)
    .bind(action)
    .bind(reason)
    .bind(priority)
    .bind(source)
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

#[cfg(feature = "guard")]
/// Delete a rule by ID. Returns true if a row was deleted.
pub async fn delete_rule(pool: &SqlitePool, id: i64) -> Result<bool, Box<dyn std::error::Error>> {
    let result = sqlx::query("DELETE FROM rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(feature = "guard")]
/// Update the enabled status of a rule. Returns true if a row was updated.
pub async fn update_rule_enabled(
    pool: &SqlitePool,
    id: i64,
    enabled: bool,
) -> Result<bool, Box<dyn std::error::Error>> {
    let result = sqlx::query(
        "UPDATE rules SET enabled = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE id = ?",
    )
    .bind(enabled)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(feature = "guard")]
/// List rules, optionally including disabled ones.
pub async fn list_rules(
    pool: &SqlitePool,
    include_disabled: bool,
) -> Result<Vec<FullRuleRow>, Box<dyn std::error::Error>> {
    let sql = if include_disabled {
        "SELECT id, tool_pattern, input_pattern, action, reason, priority, enabled, source, created_at \
         FROM rules ORDER BY priority DESC, id DESC"
    } else {
        "SELECT id, tool_pattern, input_pattern, action, reason, priority, enabled, source, created_at \
         FROM rules WHERE enabled = 1 ORDER BY priority DESC, id DESC"
    };

    let rows = sqlx::query(sql).fetch_all(pool).await?;

    let results = rows
        .iter()
        .map(|row| {
            let enabled_int: i64 = row.get("enabled");
            FullRuleRow {
                id: row.get("id"),
                tool_pattern: row.get("tool_pattern"),
                input_pattern: row.get("input_pattern"),
                action: row.get("action"),
                reason: row.get("reason"),
                priority: row.get("priority"),
                enabled: enabled_int != 0,
                source: row.get("source"),
                created_at: row.get("created_at"),
            }
        })
        .collect();

    Ok(results)
}

#[cfg(feature = "guard")]
/// Delete all rules. Returns the count deleted.
pub async fn delete_all_rules(pool: &SqlitePool) -> Result<u64, Box<dyn std::error::Error>> {
    let result = sqlx::query("DELETE FROM rules").execute(pool).await?;
    Ok(result.rows_affected())
}

#[cfg(feature = "guard")]
/// Get enforcement statistics, optionally filtered by time.
pub async fn enforcement_stats(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<EnforcementStats, Box<dyn std::error::Error>> {
    // Total count
    let (total_sql, allowed_sql, denied_sql) = if since.is_some() {
        (
            "SELECT COUNT(*) as cnt FROM enforcements WHERE timestamp >= ?",
            "SELECT COUNT(*) as cnt FROM enforcements WHERE action = 'allowed' AND timestamp >= ?",
            "SELECT COUNT(*) as cnt FROM enforcements WHERE action = 'denied' AND timestamp >= ?",
        )
    } else {
        (
            "SELECT COUNT(*) as cnt FROM enforcements",
            "SELECT COUNT(*) as cnt FROM enforcements WHERE action = 'allowed'",
            "SELECT COUNT(*) as cnt FROM enforcements WHERE action = 'denied'",
        )
    };

    let total: i64 = if let Some(s) = since {
        let row = sqlx::query(total_sql).bind(s).fetch_one(pool).await?;
        row.get("cnt")
    } else {
        let row = sqlx::query(total_sql).fetch_one(pool).await?;
        row.get("cnt")
    };

    let allowed: i64 = if let Some(s) = since {
        let row = sqlx::query(allowed_sql).bind(s).fetch_one(pool).await?;
        row.get("cnt")
    } else {
        let row = sqlx::query(allowed_sql).fetch_one(pool).await?;
        row.get("cnt")
    };

    let denied: i64 = if let Some(s) = since {
        let row = sqlx::query(denied_sql).bind(s).fetch_one(pool).await?;
        row.get("cnt")
    } else {
        let row = sqlx::query(denied_sql).fetch_one(pool).await?;
        row.get("cnt")
    };

    // Top denied rules
    let top_sql = if since.is_some() {
        "SELECT e.rule_id, COALESCE(e.reason, '') as reason, COUNT(*) as cnt \
         FROM enforcements e \
         WHERE e.action = 'denied' AND e.timestamp >= ? AND e.rule_id IS NOT NULL \
         GROUP BY e.rule_id \
         ORDER BY cnt DESC \
         LIMIT 10"
    } else {
        "SELECT e.rule_id, COALESCE(e.reason, '') as reason, COUNT(*) as cnt \
         FROM enforcements e \
         WHERE e.action = 'denied' AND e.rule_id IS NOT NULL \
         GROUP BY e.rule_id \
         ORDER BY cnt DESC \
         LIMIT 10"
    };

    let top_rows = if let Some(s) = since {
        sqlx::query(top_sql).bind(s).fetch_all(pool).await?
    } else {
        sqlx::query(top_sql).fetch_all(pool).await?
    };

    let top_denied = top_rows
        .iter()
        .map(|row| TopDeniedRule {
            rule_id: row.get("rule_id"),
            reason: row.get("reason"),
            count: row.get("cnt"),
        })
        .collect();

    Ok(EnforcementStats {
        total,
        allowed,
        denied,
        top_denied,
    })
}

#[cfg(feature = "guard")]
/// Get a classification by ID (for the promote subcommand).
pub async fn get_classification(
    pool: &SqlitePool,
    id: i64,
) -> Result<Option<ClassificationRow>, Box<dyn std::error::Error>> {
    let row = sqlx::query(
        "SELECT id, tool_name, input_pattern, risk_level, reason, heuristic \
         FROM classifications WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ClassificationRow {
        id: r.get("id"),
        tool_name: r.get("tool_name"),
        input_pattern: r.get("input_pattern"),
        risk_level: r.get("risk_level"),
        reason: r.get("reason"),
        heuristic: r.get("heuristic"),
    }))
}

// ── Recent enforcements for TUI (US-0038) ──

/// A single enforcement row for display.
#[cfg(feature = "guard")]
#[derive(Debug)]
#[allow(dead_code)] // id and session_id retained for future use (e.g. drill-down)
pub struct EnforcementRow {
    pub id: i64,
    pub timestamp: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: Option<String>,
    pub action: String,
    pub reason: Option<String>,
    pub rule_id: Option<i64>,
}

#[cfg(feature = "guard")]
/// Fetch recent enforcements ordered by timestamp descending.
pub async fn recent_enforcements(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<EnforcementRow>, Box<dyn std::error::Error>> {
    let rows = sqlx::query(
        "SELECT id, timestamp, session_id, tool_name, tool_input, action, reason, rule_id \
         FROM enforcements ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let results = rows
        .iter()
        .map(|row| EnforcementRow {
            id: row.get("id"),
            timestamp: row.get("timestamp"),
            session_id: row.get("session_id"),
            tool_name: row.get("tool_name"),
            tool_input: row.get("tool_input"),
            action: row.get("action"),
            reason: row.get("reason"),
            rule_id: row.get("rule_id"),
        })
        .collect();

    Ok(results)
}

/// Session count grouped by model name.
#[derive(Debug, Serialize)]
pub struct ModelSessionCount {
    pub model: String,
    pub session_count: i64,
}

/// Tool failure count grouped by tool name and error.
#[derive(Debug, Serialize)]
pub struct ToolFailureCount {
    pub tool_name: String,
    pub error: String,
    pub count: i64,
}

/// Get session counts grouped by model from session_event_details.
pub async fn sessions_by_model(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<Vec<ModelSessionCount>, Box<dyn std::error::Error>> {
    let mut sql = String::from(
        "SELECT sed.model, COUNT(DISTINCT e.session_id) as session_count \
         FROM events e \
         JOIN session_event_details sed ON sed.event_id = e.id \
         WHERE e.event_type = 'SessionStart' AND sed.model IS NOT NULL",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(since) = since {
        sql.push_str(" AND e.timestamp >= ?");
        binds.push(since.to_string());
    }

    sql.push_str(" GROUP BY sed.model ORDER BY session_count DESC");

    let mut query = sqlx::query(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    let results: Vec<ModelSessionCount> = rows
        .iter()
        .map(|row| ModelSessionCount {
            model: row.get("model"),
            session_count: row.get("session_count"),
        })
        .collect();

    Ok(results)
}

/// Get tool failure counts grouped by tool name and error from tool_event_details.
pub async fn tool_failures_by_error(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<Vec<ToolFailureCount>, Box<dyn std::error::Error>> {
    let mut sql = String::from(
        "SELECT e.tool_name, ted.error, COUNT(*) as count \
         FROM events e \
         JOIN tool_event_details ted ON ted.event_id = e.id \
         WHERE e.event_type = 'PostToolUseFailure' AND ted.error IS NOT NULL",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(since) = since {
        sql.push_str(" AND e.timestamp >= ?");
        binds.push(since.to_string());
    }

    sql.push_str(" GROUP BY e.tool_name, ted.error ORDER BY count DESC");

    let mut query = sqlx::query(&sql);
    for bind in &binds {
        query = query.bind(bind);
    }

    let rows = query.fetch_all(pool).await?;
    let results: Vec<ToolFailureCount> = rows
        .iter()
        .map(|row| {
            let tool_name: Option<String> = row.get("tool_name");
            ToolFailureCount {
                tool_name: tool_name.unwrap_or_else(|| "unknown".to_string()),
                error: row.get("error"),
                count: row.get("count"),
            }
        })
        .collect();

    Ok(results)
}

// ── Event detail structs (E010 TUI enhancement) ──

#[derive(Debug)]
pub struct ToolEventDetail {
    pub tool_use_id: Option<String>,
    pub error: Option<String>,
    pub error_details: Option<String>,
    pub is_interrupt: Option<bool>,
    pub permission_suggestions: Option<String>,
}

#[derive(Debug)]
pub struct StopEventDetail {
    pub stop_hook_active: Option<bool>,
    pub last_assistant_message: Option<String>,
    pub error: Option<String>,
    pub error_details: Option<String>,
}

#[derive(Debug)]
pub struct SessionEventDetail {
    pub source: Option<String>,
    pub model: Option<String>,
    pub reason: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug)]
pub struct AgentEventDetail {
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_transcript_path: Option<String>,
}

#[derive(Debug)]
pub struct NotificationEventDetail {
    pub notification_type: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub elicitation_id: Option<String>,
    pub mcp_server_name: Option<String>,
    pub mode: Option<String>,
    pub url: Option<String>,
    pub action: Option<String>,
}

#[derive(Debug)]
pub struct CompactEventDetail {
    pub trigger: Option<String>,
    pub custom_instructions: Option<String>,
    pub compact_summary: Option<String>,
}

#[derive(Debug)]
pub struct InstructionEventDetail {
    pub file_path: Option<String>,
    pub memory_type: Option<String>,
    pub load_reason: Option<String>,
}

#[derive(Debug)]
pub struct TeamEventDetail {
    pub teammate_name: Option<String>,
    pub team_name: Option<String>,
    pub task_id: Option<String>,
}

#[derive(Debug)]
pub struct PromptEventDetail {
    pub prompt: Option<String>,
}

#[derive(Debug)]
pub struct WorktreeEventDetail {
    pub worktree_path: Option<String>,
}

#[derive(Debug)]
pub enum EventDetail {
    Tool(ToolEventDetail),
    Stop(StopEventDetail),
    Session(SessionEventDetail),
    Agent(AgentEventDetail),
    Notification(NotificationEventDetail),
    Compact(CompactEventDetail),
    Instruction(InstructionEventDetail),
    Team(TeamEventDetail),
    Prompt(PromptEventDetail),
    Worktree(WorktreeEventDetail),
    /// SubagentStop: merged from stop + agent detail tables
    StopAgent(StopEventDetail, AgentEventDetail),
}

/// Fetch event-type-specific detail data from the appropriate detail table.
pub async fn fetch_event_detail(
    pool: &SqlitePool,
    event_id: i64,
    event_type: &str,
) -> Result<Option<EventDetail>, Box<dyn std::error::Error>> {
    match event_type {
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" => {
            let row = sqlx::query(
                "SELECT tool_use_id, error, error_details, is_interrupt, permission_suggestions FROM tool_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Tool(ToolEventDetail {
                    tool_use_id: r.get("tool_use_id"),
                    error: r.get("error"),
                    error_details: r.get("error_details"),
                    is_interrupt: r.get::<Option<bool>, _>("is_interrupt"),
                    permission_suggestions: r.get("permission_suggestions"),
                })
            }))
        }
        "Stop" | "StopFailure" => {
            let row = sqlx::query(
                "SELECT stop_hook_active, last_assistant_message, error, error_details FROM stop_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Stop(StopEventDetail {
                    stop_hook_active: r.get::<Option<bool>, _>("stop_hook_active"),
                    last_assistant_message: r.get("last_assistant_message"),
                    error: r.get("error"),
                    error_details: r.get("error_details"),
                })
            }))
        }
        "SubagentStop" => {
            let stop_row = sqlx::query(
                "SELECT stop_hook_active, last_assistant_message, error, error_details FROM stop_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            let agent_row = sqlx::query(
                "SELECT agent_id, agent_type, agent_transcript_path FROM agent_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            match (stop_row, agent_row) {
                (Some(sr), Some(ar)) => Ok(Some(EventDetail::StopAgent(
                    StopEventDetail {
                        stop_hook_active: sr.get::<Option<bool>, _>("stop_hook_active"),
                        last_assistant_message: sr.get("last_assistant_message"),
                        error: sr.get("error"),
                        error_details: sr.get("error_details"),
                    },
                    AgentEventDetail {
                        agent_id: ar.get("agent_id"),
                        agent_type: ar.get("agent_type"),
                        agent_transcript_path: ar.get("agent_transcript_path"),
                    },
                ))),
                (Some(sr), None) => Ok(Some(EventDetail::Stop(StopEventDetail {
                    stop_hook_active: sr.get::<Option<bool>, _>("stop_hook_active"),
                    last_assistant_message: sr.get("last_assistant_message"),
                    error: sr.get("error"),
                    error_details: sr.get("error_details"),
                }))),
                (None, Some(ar)) => Ok(Some(EventDetail::Agent(AgentEventDetail {
                    agent_id: ar.get("agent_id"),
                    agent_type: ar.get("agent_type"),
                    agent_transcript_path: ar.get("agent_transcript_path"),
                }))),
                (None, None) => Ok(None),
            }
        }
        "SubagentStart" => {
            let row = sqlx::query(
                "SELECT agent_id, agent_type, agent_transcript_path FROM agent_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Agent(AgentEventDetail {
                    agent_id: r.get("agent_id"),
                    agent_type: r.get("agent_type"),
                    agent_transcript_path: r.get("agent_transcript_path"),
                })
            }))
        }
        "SessionStart" | "SessionEnd" | "ConfigChange" => {
            let row = sqlx::query(
                "SELECT source, model, reason, file_path FROM session_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Session(SessionEventDetail {
                    source: r.get("source"),
                    model: r.get("model"),
                    reason: r.get("reason"),
                    file_path: r.get("file_path"),
                })
            }))
        }
        "Notification" | "Elicitation" | "ElicitationResult" => {
            let row = sqlx::query(
                "SELECT notification_type, title, message, elicitation_id, mcp_server_name, mode, url, action FROM notification_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Notification(NotificationEventDetail {
                    notification_type: r.get("notification_type"),
                    title: r.get("title"),
                    message: r.get("message"),
                    elicitation_id: r.get("elicitation_id"),
                    mcp_server_name: r.get("mcp_server_name"),
                    mode: r.get("mode"),
                    url: r.get("url"),
                    action: r.get("action"),
                })
            }))
        }
        "PreCompact" | "PostCompact" => {
            let row = sqlx::query(
                "SELECT `trigger`, custom_instructions, compact_summary FROM compact_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Compact(CompactEventDetail {
                    trigger: r.get("trigger"),
                    custom_instructions: r.get("custom_instructions"),
                    compact_summary: r.get("compact_summary"),
                })
            }))
        }
        "InstructionsLoaded" => {
            let row = sqlx::query(
                "SELECT file_path, memory_type, load_reason FROM instruction_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Instruction(InstructionEventDetail {
                    file_path: r.get("file_path"),
                    memory_type: r.get("memory_type"),
                    load_reason: r.get("load_reason"),
                })
            }))
        }
        "TeammateIdle" | "TaskCompleted" => {
            let row = sqlx::query(
                "SELECT teammate_name, team_name, task_id FROM team_event_details WHERE event_id = ?",
            )
            .bind(event_id)
            .fetch_optional(pool)
            .await?;
            Ok(row.map(|r| {
                EventDetail::Team(TeamEventDetail {
                    teammate_name: r.get("teammate_name"),
                    team_name: r.get("team_name"),
                    task_id: r.get("task_id"),
                })
            }))
        }
        "UserPromptSubmit" => {
            let row = sqlx::query("SELECT prompt FROM prompt_event_details WHERE event_id = ?")
                .bind(event_id)
                .fetch_optional(pool)
                .await?;
            Ok(row.map(|r| {
                EventDetail::Prompt(PromptEventDetail {
                    prompt: r.get("prompt"),
                })
            }))
        }
        "WorktreeRemove" => {
            let row =
                sqlx::query("SELECT worktree_path FROM worktree_event_details WHERE event_id = ?")
                    .bind(event_id)
                    .fetch_optional(pool)
                    .await?;
            Ok(row.map(|r| {
                EventDetail::Worktree(WorktreeEventDetail {
                    worktree_path: r.get("worktree_path"),
                })
            }))
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;
    use std::sync::Mutex;

    // Env var tests must run serially since env is process-global
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn test_connect_creates_directory_and_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("dir").join("test.db");
        let db_str = db_path.to_str().unwrap();

        let pool = connect(db_str).await.unwrap();
        assert!(db_path.exists());
        pool.close().await;
    }

    #[tokio::test]
    async fn test_wal_mode_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("wal_test.db");
        let db_str = db_path.to_str().unwrap();

        let pool = connect(db_str).await.unwrap();
        let row = sqlx::query("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: String = row.get(0);
        assert_eq!(mode, "wal");
        pool.close().await;
    }

    #[tokio::test]
    async fn test_auto_vacuum_incremental() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("vacuum_test.db");
        let db_str = db_path.to_str().unwrap();

        let pool = connect(db_str).await.unwrap();
        let row = sqlx::query("PRAGMA auto_vacuum")
            .fetch_one(&pool)
            .await
            .unwrap();
        let mode: i32 = row.get(0);
        // 2 = INCREMENTAL
        assert_eq!(mode, 2);
        pool.close().await;
    }

    #[tokio::test]
    async fn test_busy_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("timeout_test.db");
        let db_str = db_path.to_str().unwrap();

        let pool = connect(db_str).await.unwrap();
        let row = sqlx::query("PRAGMA busy_timeout")
            .fetch_one(&pool)
            .await
            .unwrap();
        let timeout: i32 = row.get(0);
        assert_eq!(timeout, 5000);
        pool.close().await;
    }

    #[tokio::test]
    async fn test_foreign_keys_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("fk_test.db");
        let db_str = db_path.to_str().unwrap();

        let pool = connect(db_str).await.unwrap();
        let row = sqlx::query("PRAGMA foreign_keys")
            .fetch_one(&pool)
            .await
            .unwrap();
        let fk: i32 = row.get(0);
        assert_eq!(fk, 1, "foreign_keys pragma must be ON");
        pool.close().await;
    }

    #[tokio::test]
    async fn test_cascade_delete_removes_detail_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cascade_test.db");
        let db_str = db_path.to_str().unwrap();
        let pool = connect(db_str).await.unwrap();

        // Insert a PreToolUse event (creates events row + tool_event_details row)
        let hook = crate::models::HookInput {
            session_id: "s-cascade".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            cwd: "/tmp".to_string(),
            tool_name: Some("Bash".to_string()),
            tool_use_id: Some("tu-123".to_string()),
            ..Default::default()
        };
        let event_id = insert_event(&pool, &hook, "{}").await.unwrap();

        // Verify detail row exists
        let detail_count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM tool_event_details WHERE event_id = ?")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(detail_count, 1, "detail row should exist before delete");

        // Delete the event
        sqlx::query("DELETE FROM events WHERE id = ?")
            .bind(event_id)
            .execute(&pool)
            .await
            .unwrap();

        // Verify detail row was CASCADE-deleted
        let detail_count_after: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM tool_event_details WHERE event_id = ?")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(
            detail_count_after, 0,
            "detail row should be CASCADE-deleted"
        );

        pool.close().await;
    }

    #[tokio::test]
    async fn test_cascade_delete_with_retain() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("retain_cascade.db");
        let db_str = db_path.to_str().unwrap();
        let pool = connect(db_str).await.unwrap();

        // Insert a SessionStart event
        let hook = crate::models::HookInput {
            session_id: "s-retain".to_string(),
            hook_event_name: "SessionStart".to_string(),
            cwd: "/tmp".to_string(),
            source: Some("startup".to_string()),
            model: Some("claude-sonnet".to_string()),
            ..Default::default()
        };
        let event_id = insert_event(&pool, &hook, "{}").await.unwrap();

        // Verify session_event_details row exists
        let count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM session_event_details WHERE event_id = ?")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(count, 1);

        // Use delete_events_before (same as scribe retain)
        let future = "9999-12-31T23:59:59Z";
        delete_events_before(&pool, future).await.unwrap();

        // Verify cascade cleaned up detail row
        let count_after: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM session_event_details WHERE event_id = ?")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(count_after, 0, "retain should CASCADE-delete detail rows");

        pool.close().await;
    }

    #[tokio::test]
    async fn test_migration_creates_tables_and_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("migration_test.db");
        let db_str = db_path.to_str().unwrap();

        let pool = connect(db_str).await.unwrap();

        // Verify events table exists with expected columns
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('events') ORDER BY cid")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            columns,
            vec![
                "id",
                "timestamp",
                "session_id",
                "event_type",
                "tool_name",
                "tool_input",
                "tool_response",
                "cwd",
                "permission_mode",
                "raw_payload"
            ]
        );

        // Verify sessions table exists with expected columns
        let columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('sessions') ORDER BY cid")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(
            columns,
            vec![
                "session_id",
                "first_seen",
                "last_seen",
                "cwd",
                "event_count"
            ]
        );

        // Verify all five indexes on events (including cwd index from E006 migration)
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='events' AND name LIKE 'idx_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            indexes,
            vec![
                "idx_events_cwd",
                "idx_events_session",
                "idx_events_tool",
                "idx_events_ts",
                "idx_events_type"
            ]
        );

        pool.close().await;
    }

    #[tokio::test]
    async fn test_insert_event_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("roundtrip.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        insert_test_event(
            &pool,
            "sess-1",
            "PreToolUse",
            Some("Bash"),
            Some(r#"{"command":"ls"}"#),
            None,
            "/home/user",
            Some("default"),
            r#"{"session_id":"sess-1","hook_event_name":"PreToolUse"}"#,
        )
        .await
        .unwrap();

        let events = query_events(&pool, &EventFilter::new()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "sess-1");
        assert_eq!(events[0].event_type, "PreToolUse");
        assert_eq!(events[0].tool_name.as_deref(), Some("Bash"));
        assert_eq!(events[0].tool_input.as_deref(), Some(r#"{"command":"ls"}"#));
        assert!(events[0].tool_response.is_none());
        assert_eq!(events[0].cwd.as_deref(), Some("/home/user"));
        assert_eq!(events[0].permission_mode.as_deref(), Some("default"));

        pool.close().await;
    }

    #[tokio::test]
    async fn test_sessions_upsert_same_session() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("upsert.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        // First event
        insert_test_event(
            &pool,
            "sess-1",
            "SessionStart",
            None,
            None,
            None,
            "/home/user/project-a",
            None,
            "{}",
        )
        .await
        .unwrap();

        // Second event with different cwd
        insert_test_event(
            &pool,
            "sess-1",
            "PreToolUse",
            Some("Bash"),
            None,
            None,
            "/home/user/project-b",
            None,
            "{}",
        )
        .await
        .unwrap();

        let row = sqlx::query(
            "SELECT event_count, cwd, first_seen, last_seen FROM sessions WHERE session_id = ?",
        )
        .bind("sess-1")
        .fetch_one(&pool)
        .await
        .unwrap();

        let count: i32 = row.get("event_count");
        let cwd: String = row.get("cwd");
        let first_seen: String = row.get("first_seen");
        let last_seen: String = row.get("last_seen");

        assert_eq!(count, 2);
        assert_eq!(cwd, "/home/user/project-b"); // latest cwd
        assert!(last_seen >= first_seen); // last_seen updated

        pool.close().await;
    }

    #[tokio::test]
    async fn test_sessions_upsert_different_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("multi_session.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        insert_test_event(
            &pool,
            "sess-a",
            "SessionStart",
            None,
            None,
            None,
            "/a",
            None,
            "{}",
        )
        .await
        .unwrap();
        insert_test_event(
            &pool,
            "sess-b",
            "SessionStart",
            None,
            None,
            None,
            "/b",
            None,
            "{}",
        )
        .await
        .unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 2);

        pool.close().await;
    }

    #[tokio::test]
    async fn test_query_events_limit() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("limit.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        for i in 0..5 {
            insert_test_event(
                &pool,
                "sess-1",
                &format!("Event{i}"),
                None,
                None,
                None,
                "/home",
                None,
                "{}",
            )
            .await
            .unwrap();
        }

        let events = query_events(
            &pool,
            &EventFilter {
                limit: 3,
                ..EventFilter::new()
            },
        )
        .await
        .unwrap();
        assert_eq!(events.len(), 3);

        pool.close().await;
    }

    #[tokio::test]
    async fn test_migration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("idempotent_test.db");
        let db_str = db_path.to_str().unwrap();

        // Connect twice — second call should not error
        let pool = connect(db_str).await.unwrap();
        pool.close().await;
        let pool = connect(db_str).await.unwrap();
        pool.close().await;
    }

    #[test]
    fn test_resolve_db_path_cli_overrides_all() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SCRIBE_DB", "/env/path.db") };
        let result = resolve_db_path(Some("/cli/path.db"), Some("/config/path.db")).unwrap();
        assert_eq!(result, "/cli/path.db");
        unsafe { std::env::remove_var("SCRIBE_DB") };
    }

    #[test]
    fn test_resolve_db_path_env_overrides_config() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SCRIBE_DB", "/env/path.db") };
        let result = resolve_db_path(None, Some("/config/path.db")).unwrap();
        assert_eq!(result, "/env/path.db");
        unsafe { std::env::remove_var("SCRIBE_DB") };
    }

    #[test]
    fn test_resolve_db_path_config_overrides_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("SCRIBE_DB") };
        let result = resolve_db_path(None, Some("/config/path.db")).unwrap();
        assert_eq!(result, "/config/path.db");
    }

    #[test]
    fn test_resolve_db_path_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("SCRIBE_DB") };
        let result = resolve_db_path(None, None).unwrap();
        let home = dirs::home_dir().unwrap();
        let expected = home.join(".claude").join("scribe.db");
        assert_eq!(result, expected.to_string_lossy());
    }

    // ── Query filter tests ──

    async fn setup_filtered_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("filter.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        // Insert events with controlled timestamps via direct SQL
        for (i, (session, event_type, tool, cwd, ts)) in [
            (
                "s1",
                "PreToolUse",
                Some("Bash"),
                "/a",
                "2025-01-01T10:00:00.000Z",
            ),
            (
                "s1",
                "PostToolUse",
                Some("Bash"),
                "/a",
                "2025-01-01T11:00:00.000Z",
            ),
            (
                "s2",
                "PreToolUse",
                Some("Write"),
                "/b",
                "2025-01-01T12:00:00.000Z",
            ),
            ("s2", "SessionEnd", None, "/b", "2025-01-01T13:00:00.000Z"),
        ]
        .iter()
        .enumerate()
        {
            let tool_input = if *event_type == "PreToolUse" && *tool == Some("Bash") {
                Some(r#"{"command":"echo hello"}"#)
            } else {
                None
            };
            sqlx::query(
                "INSERT INTO events (id, timestamp, session_id, event_type, tool_name, tool_input, cwd, raw_payload) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(i as i64 + 1)
            .bind(ts)
            .bind(session)
            .bind(event_type)
            .bind(tool)
            .bind(tool_input)
            .bind(cwd)
            .bind("{}")
            .execute(&pool)
            .await
            .unwrap();

            // Upsert session
            sqlx::query(
                "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count) VALUES (?, ?, ?, ?, 1) ON CONFLICT(session_id) DO UPDATE SET last_seen = excluded.last_seen, cwd = excluded.cwd, event_count = event_count + 1",
            )
            .bind(session)
            .bind(ts)
            .bind(ts)
            .bind(cwd)
            .execute(&pool)
            .await
            .unwrap();
        }

        (pool, dir)
    }

    #[tokio::test]
    async fn test_query_events_no_filters() {
        let (pool, _dir) = setup_filtered_db().await;
        let events = query_events(&pool, &EventFilter::new()).await.unwrap();
        assert_eq!(events.len(), 4);
        // Ordered by timestamp DESC
        assert_eq!(events[0].timestamp, "2025-01-01T13:00:00.000Z");
    }

    #[tokio::test]
    async fn test_query_events_since_filter() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            since: Some("2025-01-01T11:30:00.000Z".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert_eq!(events.len(), 2); // 12:00 and 13:00
    }

    #[tokio::test]
    async fn test_query_events_session_filter() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            session_id: Some("s1".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_query_events_event_type_filter() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            event_type: Some("PreToolUse".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_query_events_tool_name_filter() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            tool_name: Some("Write".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].tool_name.as_deref(), Some("Write"));
    }

    #[tokio::test]
    async fn test_query_events_search_filter() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            search: Some("echo hello".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0]
            .tool_input
            .as_ref()
            .unwrap()
            .contains("echo hello"));
    }

    #[tokio::test]
    async fn test_query_events_combined_filters() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            session_id: Some("s1".to_string()),
            event_type: Some("PreToolUse".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].session_id, "s1");
        assert_eq!(events[0].event_type, "PreToolUse");
    }

    #[tokio::test]
    async fn test_query_events_empty_result() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = EventFilter {
            tool_name: Some("NonExistent".to_string()),
            ..EventFilter::new()
        };
        let events = query_events(&pool, &filter).await.unwrap();
        assert!(events.is_empty());
    }

    #[tokio::test]
    async fn test_query_sessions_no_filters() {
        let (pool, _dir) = setup_filtered_db().await;
        let sessions = query_sessions(&pool, &SessionFilter::new()).await.unwrap();
        assert_eq!(sessions.len(), 2);
        // Ordered by last_seen DESC
        assert_eq!(sessions[0].session_id, "s2");
    }

    #[tokio::test]
    async fn test_query_sessions_since_filter() {
        let (pool, _dir) = setup_filtered_db().await;
        let filter = SessionFilter {
            since: Some("2025-01-01T12:30:00.000Z".to_string()),
            ..SessionFilter::new()
        };
        let sessions = query_sessions(&pool, &filter).await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "s2");
    }

    // ── Delete/retain tests ──

    #[tokio::test]
    async fn test_delete_events_before() {
        let (pool, _dir) = setup_filtered_db().await;
        // Delete events before 11:30 — should remove the 10:00 and 11:00 events
        let deleted = delete_events_before(&pool, "2025-01-01T11:30:00.000Z")
            .await
            .unwrap();
        assert_eq!(deleted, 2);

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 2);
    }

    #[tokio::test]
    async fn test_delete_events_before_no_matches() {
        let (pool, _dir) = setup_filtered_db().await;
        let deleted = delete_events_before(&pool, "2020-01-01T00:00:00.000Z")
            .await
            .unwrap();
        assert_eq!(deleted, 0);
    }

    #[tokio::test]
    async fn test_delete_orphaned_sessions() {
        let (pool, _dir) = setup_filtered_db().await;
        // Delete all events for s1 (timestamps 10:00 and 11:00)
        delete_events_before(&pool, "2025-01-01T11:30:00.000Z")
            .await
            .unwrap();
        // s1 should now be orphaned
        let deleted = delete_orphaned_sessions(&pool).await.unwrap();
        assert_eq!(deleted, 1);

        let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 1); // s2 still has events
    }

    #[tokio::test]
    async fn test_delete_orphaned_sessions_no_orphans() {
        let (pool, _dir) = setup_filtered_db().await;
        let deleted = delete_orphaned_sessions(&pool).await.unwrap();
        assert_eq!(deleted, 0);
    }

    // ── Extended stats tests (E006 / US-0021) ──

    /// Set up a DB with diverse data for extended stats tests.
    async fn setup_stats_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("stats.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        let events = [
            (
                "s1",
                "PreToolUse",
                Some("Bash"),
                "/home/user/project-a",
                "2025-01-10T10:00:00.000Z",
                "{}",
            ),
            (
                "s1",
                "PostToolUse",
                Some("Bash"),
                "/home/user/project-a",
                "2025-01-10T10:01:00.000Z",
                "{}",
            ),
            (
                "s1",
                "PreToolUse",
                Some("Read"),
                "/home/user/project-a",
                "2025-01-10T10:02:00.000Z",
                "{}",
            ),
            (
                "s1",
                "PostToolUse",
                Some("Read"),
                "/home/user/project-a",
                "2025-01-10T10:03:00.000Z",
                "{}",
            ),
            (
                "s1",
                "PreToolUse",
                Some("Bash"),
                "/home/user/project-a",
                "2025-01-11T09:00:00.000Z",
                "{}",
            ),
            (
                "s2",
                "PreToolUse",
                Some("Write"),
                "/home/user/project-b",
                "2025-01-12T14:00:00.000Z",
                "{}",
            ),
            (
                "s2",
                "PostToolUseFailure",
                Some("Write"),
                "/home/user/project-b",
                "2025-01-12T14:01:00.000Z",
                "{}",
            ),
            (
                "s2",
                "StopFailure",
                None,
                "/home/user/project-b",
                "2025-01-12T14:02:00.000Z",
                r#"{"error":"rate_limit"}"#,
            ),
            (
                "s2",
                "StopFailure",
                None,
                "/home/user/project-b",
                "2025-01-12T14:03:00.000Z",
                r#"{"error":"rate_limit"}"#,
            ),
            (
                "s2",
                "StopFailure",
                None,
                "/home/user/project-b",
                "2025-01-12T14:04:00.000Z",
                r#"{"error":"server_error"}"#,
            ),
            (
                "s2",
                "SessionEnd",
                None,
                "/home/user/project-b",
                "2025-01-12T15:00:00.000Z",
                "{}",
            ),
        ];

        for (i, (session, event_type, tool, cwd, ts, payload)) in events.iter().enumerate() {
            sqlx::query(
                "INSERT INTO events (id, timestamp, session_id, event_type, tool_name, cwd, raw_payload) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(i as i64 + 1)
            .bind(ts)
            .bind(session)
            .bind(event_type)
            .bind(tool)
            .bind(cwd)
            .bind(payload)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Set up sessions with proper first_seen/last_seen
        sqlx::query(
            "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count) VALUES ('s1', '2025-01-10T10:00:00.000Z', '2025-01-11T09:00:00.000Z', '/home/user/project-a', 5)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count) VALUES ('s2', '2025-01-12T14:00:00.000Z', '2025-01-12T15:00:00.000Z', '/home/user/project-b', 6)",
        )
        .execute(&pool)
        .await
        .unwrap();

        (pool, dir)
    }

    #[tokio::test]
    async fn test_migration_adds_cwd_index() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("cwd_idx.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='events' AND name = 'idx_events_cwd'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(indexes, vec!["idx_events_cwd"]);
        pool.close().await;
    }

    #[tokio::test]
    async fn test_top_tools_populated() {
        let (pool, _dir) = setup_stats_db().await;
        let tools = top_tools(&pool, None, 10).await.unwrap();
        assert!(!tools.is_empty());
        // Bash has 3 events, Read has 1, Write has 1 (PreToolUse only; PostToolUseFailure also has tool_name)
        assert_eq!(tools[0].tool_name, "Bash");
        assert_eq!(tools[0].count, 3);
    }

    #[tokio::test]
    async fn test_top_tools_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        // Only events from 2025-01-12 onwards
        let tools = top_tools(&pool, Some("2025-01-12T00:00:00.000Z"), 10)
            .await
            .unwrap();
        // Write has 2 events (PreToolUse + PostToolUseFailure both have tool_name=Write)
        assert_eq!(tools[0].tool_name, "Write");
        assert_eq!(tools[0].count, 2);
        // No Bash or Read events after 2025-01-12
        assert!(tools.iter().all(|t| t.tool_name != "Bash"));
    }

    #[tokio::test]
    async fn test_top_tools_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("empty.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();
        let tools = top_tools(&pool, None, 10).await.unwrap();
        assert!(tools.is_empty());
    }

    #[tokio::test]
    async fn test_top_tools_limit() {
        let (pool, _dir) = setup_stats_db().await;
        let tools = top_tools(&pool, None, 1).await.unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].tool_name, "Bash");
    }

    #[tokio::test]
    async fn test_event_type_breakdown_populated() {
        let (pool, _dir) = setup_stats_db().await;
        let types = event_type_breakdown(&pool, None).await.unwrap();
        assert!(!types.is_empty());
        // PreToolUse should be the most common (4 events)
        assert_eq!(types[0].event_type, "PreToolUse");
        assert_eq!(types[0].count, 4);
    }

    #[tokio::test]
    async fn test_event_type_breakdown_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        let types = event_type_breakdown(&pool, Some("2025-01-12T00:00:00.000Z"))
            .await
            .unwrap();
        // Should only include events from s2 (6 events)
        let total: i64 = types.iter().map(|t| t.count).sum();
        assert_eq!(total, 6);
    }

    #[tokio::test]
    async fn test_event_type_breakdown_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("empty.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();
        let types = event_type_breakdown(&pool, None).await.unwrap();
        assert!(types.is_empty());
    }

    #[tokio::test]
    async fn test_error_summary_with_errors() {
        let (pool, _dir) = setup_stats_db().await;
        let errors = error_summary(&pool, None).await.unwrap();
        assert_eq!(errors.post_tool_use_failure_count, 1);
        assert_eq!(errors.stop_failure_count, 3);
        assert_eq!(errors.stop_failure_types.len(), 2);
        // rate_limit should be first (count 2)
        assert_eq!(errors.stop_failure_types[0].error_type, "rate_limit");
        assert_eq!(errors.stop_failure_types[0].count, 2);
        assert_eq!(errors.stop_failure_types[1].error_type, "server_error");
        assert_eq!(errors.stop_failure_types[1].count, 1);
    }

    #[tokio::test]
    async fn test_error_summary_no_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("no_errors.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        // Insert a non-error event
        sqlx::query(
            "INSERT INTO events (timestamp, session_id, event_type, cwd, raw_payload) VALUES ('2025-01-01T10:00:00.000Z', 's1', 'PreToolUse', '/tmp', '{}')",
        )
        .execute(&pool)
        .await
        .unwrap();

        let errors = error_summary(&pool, None).await.unwrap();
        assert_eq!(errors.post_tool_use_failure_count, 0);
        assert_eq!(errors.stop_failure_count, 0);
        assert!(errors.stop_failure_types.is_empty());
    }

    #[tokio::test]
    async fn test_error_summary_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        // Before any StopFailure events
        let errors = error_summary(&pool, Some("2025-01-13T00:00:00.000Z"))
            .await
            .unwrap();
        assert_eq!(errors.post_tool_use_failure_count, 0);
        assert_eq!(errors.stop_failure_count, 0);
    }

    #[tokio::test]
    async fn test_top_directories_populated() {
        let (pool, _dir) = setup_stats_db().await;
        let dirs = top_directories(&pool, None, 5).await.unwrap();
        assert_eq!(dirs.len(), 2);
        // project-b has 6 events, project-a has 5
        assert_eq!(dirs[0].cwd, "/home/user/project-b");
        assert_eq!(dirs[0].count, 6);
        assert_eq!(dirs[1].cwd, "/home/user/project-a");
        assert_eq!(dirs[1].count, 5);
    }

    #[tokio::test]
    async fn test_top_directories_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        let dirs = top_directories(&pool, Some("2025-01-12T00:00:00.000Z"), 5)
            .await
            .unwrap();
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].cwd, "/home/user/project-b");
    }

    #[tokio::test]
    async fn test_top_directories_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("empty.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();
        let dirs = top_directories(&pool, None, 5).await.unwrap();
        assert!(dirs.is_empty());
    }

    #[tokio::test]
    async fn test_avg_session_duration_normal() {
        let (pool, _dir) = setup_stats_db().await;
        let avg = avg_session_duration(&pool, None).await.unwrap();
        assert!(avg.is_some());
        let avg = avg.unwrap();
        // s1: 2025-01-10T10:00 to 2025-01-11T09:00 = 23 hours = 82800s
        // s2: 2025-01-12T14:00 to 2025-01-12T15:00 = 1 hour = 3600s
        // avg = (82800 + 3600) / 2 = 43200s
        assert!((avg - 43200.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn test_avg_session_duration_excludes_single_event() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("single_event.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        // Session with same first_seen and last_seen
        sqlx::query(
            "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count) VALUES ('s1', '2025-01-01T10:00:00.000Z', '2025-01-01T10:00:00.000Z', '/tmp', 1)",
        )
        .execute(&pool)
        .await
        .unwrap();

        let avg = avg_session_duration(&pool, None).await.unwrap();
        assert!(avg.is_none());
    }

    #[tokio::test]
    async fn test_avg_session_duration_no_sessions() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("no_sessions.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();

        let avg = avg_session_duration(&pool, None).await.unwrap();
        assert!(avg.is_none());
    }

    #[tokio::test]
    async fn test_avg_session_duration_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        // Only s2 has last_seen >= 2025-01-12
        let avg = avg_session_duration(&pool, Some("2025-01-12T00:00:00.000Z"))
            .await
            .unwrap();
        assert!(avg.is_some());
        // s2: 1 hour = 3600s
        assert!((avg.unwrap() - 3600.0).abs() < 1.0);
    }

    #[tokio::test]
    async fn test_daily_activity_populated() {
        let (pool, _dir) = setup_stats_db().await;
        let activity = daily_activity(&pool, Some("2025-01-10T00:00:00.000Z"))
            .await
            .unwrap();
        assert!(!activity.is_empty());
        // 2025-01-10 should have 4 events, 2025-01-11 should have 1, 2025-01-12 should have 6
        assert_eq!(activity[0].date, "2025-01-10");
        assert_eq!(activity[0].count, 4);
        assert_eq!(activity[1].date, "2025-01-11");
        assert_eq!(activity[1].count, 1);
        assert_eq!(activity[2].date, "2025-01-12");
        assert_eq!(activity[2].count, 6);
    }

    #[tokio::test]
    async fn test_daily_activity_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        let activity = daily_activity(&pool, Some("2025-01-12T00:00:00.000Z"))
            .await
            .unwrap();
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].date, "2025-01-12");
        assert_eq!(activity[0].count, 6);
    }

    #[tokio::test]
    async fn test_daily_activity_empty_db() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("empty.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();
        let activity = daily_activity(&pool, Some("2025-01-01T00:00:00.000Z"))
            .await
            .unwrap();
        assert!(activity.is_empty());
    }

    #[tokio::test]
    async fn test_get_stats_with_since() {
        let (pool, _dir) = setup_stats_db().await;
        let stats = get_stats(&pool, Some("2025-01-12T00:00:00.000Z"))
            .await
            .unwrap();
        assert_eq!(stats.event_count, 6); // only s2 events
        assert_eq!(stats.session_count, 1); // only s2
        assert!(stats.oldest_event.is_some());
        assert!(stats.newest_event.is_some());
    }

    #[tokio::test]
    async fn test_get_stats_without_since() {
        let (pool, _dir) = setup_stats_db().await;
        let stats = get_stats(&pool, None).await.unwrap();
        assert_eq!(stats.event_count, 11);
        assert_eq!(stats.session_count, 2);
    }

    // ── Tier 1 detail table tests ──

    async fn setup_detail_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("detail.db");
        let pool = connect(db_path.to_str().unwrap()).await.unwrap();
        (pool, dir)
    }

    #[tokio::test]
    async fn test_detail_pre_tool_use_tool_use_id() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PreToolUse".into(),
            cwd: "/tmp".into(),
            tool_name: Some("Bash".into()),
            tool_use_id: Some("tu-123".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query("SELECT tool_use_id FROM tool_event_details WHERE event_id = ?")
            .bind(eid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("tool_use_id").as_deref(),
            Some("tu-123")
        );
    }

    #[tokio::test]
    async fn test_detail_post_tool_use_failure_error_fields() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PostToolUseFailure".into(),
            cwd: "/tmp".into(),
            tool_name: Some("Bash".into()),
            error: Some("timeout".into()),
            error_details: Some("command timed out after 30s".into()),
            is_interrupt: Some(true),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT error, error_details, is_interrupt FROM tool_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("error").as_deref(),
            Some("timeout")
        );
        assert_eq!(
            row.get::<Option<String>, _>("error_details").as_deref(),
            Some("command timed out after 30s")
        );
        assert_eq!(row.get::<Option<bool>, _>("is_interrupt"), Some(true));
    }

    #[tokio::test]
    async fn test_detail_permission_request_suggestions() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PermissionRequest".into(),
            cwd: "/tmp".into(),
            tool_name: Some("Bash".into()),
            permission_suggestions: Some(serde_json::json!({"allow": true})),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row =
            sqlx::query("SELECT permission_suggestions FROM tool_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        let ps: Option<String> = row.get("permission_suggestions");
        assert!(ps.is_some());
        let parsed: serde_json::Value = serde_json::from_str(ps.as_ref().unwrap()).unwrap();
        assert_eq!(parsed["allow"], true);
    }

    #[tokio::test]
    async fn test_detail_stop_failure() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "StopFailure".into(),
            cwd: "/tmp".into(),
            error: Some("rate_limit".into()),
            error_details: Some("too many requests".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row =
            sqlx::query("SELECT error, error_details FROM stop_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("error").as_deref(),
            Some("rate_limit")
        );
        assert_eq!(
            row.get::<Option<String>, _>("error_details").as_deref(),
            Some("too many requests")
        );
    }

    #[tokio::test]
    async fn test_detail_stop_event() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "Stop".into(),
            cwd: "/tmp".into(),
            stop_hook_active: Some(false),
            last_assistant_message: Some("done".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT stop_hook_active, last_assistant_message FROM stop_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<Option<bool>, _>("stop_hook_active"), Some(false));
        assert_eq!(
            row.get::<Option<String>, _>("last_assistant_message")
                .as_deref(),
            Some("done")
        );
    }

    #[tokio::test]
    async fn test_detail_subagent_stop() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SubagentStop".into(),
            cwd: "/tmp".into(),
            stop_hook_active: Some(true),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query("SELECT stop_hook_active FROM stop_event_details WHERE event_id = ?")
            .bind(eid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(row.get::<Option<bool>, _>("stop_hook_active"), Some(true));
    }

    #[tokio::test]
    async fn test_detail_session_start() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SessionStart".into(),
            cwd: "/tmp".into(),
            source: Some("startup".into()),
            model: Some("claude-sonnet-4-5-20250514".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query("SELECT source, model FROM session_event_details WHERE event_id = ?")
            .bind(eid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("source").as_deref(),
            Some("startup")
        );
        assert_eq!(
            row.get::<Option<String>, _>("model").as_deref(),
            Some("claude-sonnet-4-5-20250514")
        );
    }

    #[tokio::test]
    async fn test_detail_session_end() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SessionEnd".into(),
            cwd: "/tmp".into(),
            reason: Some("clear".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query("SELECT reason FROM session_event_details WHERE event_id = ?")
            .bind(eid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("reason").as_deref(),
            Some("clear")
        );
    }

    #[tokio::test]
    async fn test_detail_config_change() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "ConfigChange".into(),
            cwd: "/tmp".into(),
            source: Some("user_settings".into()),
            model: Some("claude-opus-4-20250514".into()),
            file_path: Some("/home/.claude/settings.json".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT source, model, file_path FROM session_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("source").as_deref(),
            Some("user_settings")
        );
        assert_eq!(
            row.get::<Option<String>, _>("model").as_deref(),
            Some("claude-opus-4-20250514")
        );
        assert_eq!(
            row.get::<Option<String>, _>("file_path").as_deref(),
            Some("/home/.claude/settings.json")
        );
    }

    #[tokio::test]
    async fn test_detail_unknown_event_no_detail_row() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "Notification".into(),
            cwd: "/tmp".into(),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        // No row in any Tier 1 detail table
        let tool_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tool_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        let stop_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM stop_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        let session_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM session_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(tool_count, 0);
        assert_eq!(stop_count, 0);
        assert_eq!(session_count, 0);
    }

    #[tokio::test]
    async fn test_detail_round_trip_with_join() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PreToolUse".into(),
            cwd: "/project".into(),
            tool_name: Some("Bash".into()),
            tool_use_id: Some("tu-abc".into()),
            tool_input: Some(serde_json::json!({"command": "ls"})),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, r#"{"raw":true}"#).await.unwrap();

        let row = sqlx::query(
            "SELECT e.event_type, e.tool_name, d.tool_use_id
             FROM events e
             JOIN tool_event_details d ON d.event_id = e.id
             WHERE e.id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.get::<String, _>("event_type"), "PreToolUse");
        assert_eq!(
            row.get::<Option<String>, _>("tool_name").as_deref(),
            Some("Bash")
        );
        assert_eq!(
            row.get::<Option<String>, _>("tool_use_id").as_deref(),
            Some("tu-abc")
        );
    }

    #[tokio::test]
    async fn test_insert_event_returns_id() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PreToolUse".into(),
            cwd: "/tmp".into(),
            ..Default::default()
        };
        let id1 = insert_event(&pool, &hook, "{}").await.unwrap();
        let id2 = insert_event(&pool, &hook, "{}").await.unwrap();
        assert!(id1 > 0);
        assert!(id2 > id1);
    }

    // ── Tier 2 / Tier 3 detail insert tests (US-0041) ──

    #[tokio::test]
    async fn test_detail_subagent_start() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SubagentStart".into(),
            cwd: "/tmp".into(),
            agent_id: Some("agent-123".into()),
            agent_type: Some("Explore".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT agent_id, agent_type, agent_transcript_path FROM agent_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("agent_id").as_deref(),
            Some("agent-123")
        );
        assert_eq!(
            row.get::<Option<String>, _>("agent_type").as_deref(),
            Some("Explore")
        );
        assert!(row
            .get::<Option<String>, _>("agent_transcript_path")
            .is_none());
    }

    #[tokio::test]
    async fn test_detail_subagent_stop_dual_insert() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SubagentStop".into(),
            cwd: "/tmp".into(),
            agent_id: Some("agent-456".into()),
            agent_type: Some("Code".into()),
            agent_transcript_path: Some("/transcripts/a.json".into()),
            stop_hook_active: Some(true),
            last_assistant_message: Some("done".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        // Verify stop_event_details
        let stop_row =
            sqlx::query("SELECT stop_hook_active, last_assistant_message FROM stop_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            stop_row.get::<Option<bool>, _>("stop_hook_active"),
            Some(true)
        );
        assert_eq!(
            stop_row
                .get::<Option<String>, _>("last_assistant_message")
                .as_deref(),
            Some("done")
        );

        // Verify agent_event_details
        let agent_row = sqlx::query(
            "SELECT agent_id, agent_type, agent_transcript_path FROM agent_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            agent_row.get::<Option<String>, _>("agent_id").as_deref(),
            Some("agent-456")
        );
        assert_eq!(
            agent_row.get::<Option<String>, _>("agent_type").as_deref(),
            Some("Code")
        );
        assert_eq!(
            agent_row
                .get::<Option<String>, _>("agent_transcript_path")
                .as_deref(),
            Some("/transcripts/a.json")
        );
    }

    #[tokio::test]
    async fn test_detail_notification() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "Notification".into(),
            cwd: "/tmp".into(),
            notification_type: Some("permission_prompt".into()),
            title: Some("Alert".into()),
            message: Some("Please confirm".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT notification_type, title, message FROM notification_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("notification_type").as_deref(),
            Some("permission_prompt")
        );
        assert_eq!(
            row.get::<Option<String>, _>("title").as_deref(),
            Some("Alert")
        );
        assert_eq!(
            row.get::<Option<String>, _>("message").as_deref(),
            Some("Please confirm")
        );
    }

    #[tokio::test]
    async fn test_detail_elicitation() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "Elicitation".into(),
            cwd: "/tmp".into(),
            elicitation_id: Some("e-001".into()),
            mcp_server_name: Some("srv".into()),
            mode: Some("form".into()),
            url: Some("https://example.com".into()),
            requested_schema: Some(serde_json::json!({"type": "object"})),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT elicitation_id, mcp_server_name, mode, url, requested_schema FROM notification_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("elicitation_id").as_deref(),
            Some("e-001")
        );
        assert_eq!(
            row.get::<Option<String>, _>("mcp_server_name").as_deref(),
            Some("srv")
        );
        assert_eq!(
            row.get::<Option<String>, _>("mode").as_deref(),
            Some("form")
        );
        assert_eq!(
            row.get::<Option<String>, _>("url").as_deref(),
            Some("https://example.com")
        );
        assert!(row
            .get::<Option<String>, _>("requested_schema")
            .unwrap()
            .contains("object"));
    }

    #[tokio::test]
    async fn test_detail_elicitation_result() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "ElicitationResult".into(),
            cwd: "/tmp".into(),
            elicitation_id: Some("e-001".into()),
            action: Some("accept".into()),
            content: Some(serde_json::json!({"field": "value"})),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT action, content FROM notification_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("action").as_deref(),
            Some("accept")
        );
        assert!(row
            .get::<Option<String>, _>("content")
            .unwrap()
            .contains("value"));
    }

    #[tokio::test]
    async fn test_detail_pre_compact() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PreCompact".into(),
            cwd: "/tmp".into(),
            trigger: Some("auto".into()),
            custom_instructions: Some("keep it short".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT `trigger`, custom_instructions, compact_summary FROM compact_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("trigger").as_deref(),
            Some("auto")
        );
        assert_eq!(
            row.get::<Option<String>, _>("custom_instructions")
                .as_deref(),
            Some("keep it short")
        );
        assert!(row.get::<Option<String>, _>("compact_summary").is_none());
    }

    #[tokio::test]
    async fn test_detail_post_compact() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PostCompact".into(),
            cwd: "/tmp".into(),
            trigger: Some("manual".into()),
            compact_summary: Some("summarized".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT `trigger`, compact_summary FROM compact_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("trigger").as_deref(),
            Some("manual")
        );
        assert_eq!(
            row.get::<Option<String>, _>("compact_summary").as_deref(),
            Some("summarized")
        );
    }

    #[tokio::test]
    async fn test_detail_instructions_loaded() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "InstructionsLoaded".into(),
            cwd: "/tmp".into(),
            file_path: Some("/project/CLAUDE.md".into()),
            memory_type: Some("project".into()),
            load_reason: Some("session_start".into()),
            globs: Some(vec!["*.md".into(), "*.txt".into()]),
            trigger_file_path: Some("/trigger".into()),
            parent_file_path: Some("/parent".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT file_path, memory_type, load_reason, globs, trigger_file_path, parent_file_path FROM instruction_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("file_path").as_deref(),
            Some("/project/CLAUDE.md")
        );
        assert_eq!(
            row.get::<Option<String>, _>("memory_type").as_deref(),
            Some("project")
        );
        assert_eq!(
            row.get::<Option<String>, _>("load_reason").as_deref(),
            Some("session_start")
        );
        let globs_val = row.get::<Option<String>, _>("globs").unwrap();
        assert!(globs_val.contains("*.md"));
        assert!(globs_val.contains("*.txt"));
        assert_eq!(
            row.get::<Option<String>, _>("trigger_file_path").as_deref(),
            Some("/trigger")
        );
        assert_eq!(
            row.get::<Option<String>, _>("parent_file_path").as_deref(),
            Some("/parent")
        );
    }

    #[tokio::test]
    async fn test_detail_teammate_idle() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "TeammateIdle".into(),
            cwd: "/tmp".into(),
            teammate_name: Some("bob".into()),
            team_name: Some("alpha".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT teammate_name, team_name FROM team_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("teammate_name").as_deref(),
            Some("bob")
        );
        assert_eq!(
            row.get::<Option<String>, _>("team_name").as_deref(),
            Some("alpha")
        );
    }

    #[tokio::test]
    async fn test_detail_task_completed() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "TaskCompleted".into(),
            cwd: "/tmp".into(),
            teammate_name: Some("bob".into()),
            team_name: Some("alpha".into()),
            task_id: Some("t-001".into()),
            task_subject: Some("fix bug".into()),
            task_description: Some("fixed the null pointer".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query(
            "SELECT task_id, task_subject, task_description FROM team_event_details WHERE event_id = ?",
        )
        .bind(eid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("task_id").as_deref(),
            Some("t-001")
        );
        assert_eq!(
            row.get::<Option<String>, _>("task_subject").as_deref(),
            Some("fix bug")
        );
        assert_eq!(
            row.get::<Option<String>, _>("task_description").as_deref(),
            Some("fixed the null pointer")
        );
    }

    #[tokio::test]
    async fn test_detail_user_prompt_submit() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "UserPromptSubmit".into(),
            cwd: "/tmp".into(),
            prompt: Some("hello world".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row = sqlx::query("SELECT prompt FROM prompt_event_details WHERE event_id = ?")
            .bind(eid)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("prompt").as_deref(),
            Some("hello world")
        );
    }

    #[tokio::test]
    async fn test_detail_worktree_remove() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "WorktreeRemove".into(),
            cwd: "/tmp".into(),
            worktree_path: Some("/worktree/feature-x".into()),
            ..Default::default()
        };
        let eid = insert_event(&pool, &hook, "{}").await.unwrap();

        let row =
            sqlx::query("SELECT worktree_path FROM worktree_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            row.get::<Option<String>, _>("worktree_path").as_deref(),
            Some("/worktree/feature-x")
        );
    }

    #[tokio::test]
    async fn test_sessions_by_model() {
        let (pool, _dir) = setup_detail_db().await;

        // Insert SessionStart events with different models
        for i in 0..3 {
            let hook = crate::models::HookInput {
                session_id: format!("sonnet-session-{i}"),
                hook_event_name: "SessionStart".into(),
                cwd: "/tmp".into(),
                model: Some("claude-sonnet-4-20250514".into()),
                ..Default::default()
            };
            insert_event(&pool, &hook, "{}").await.unwrap();
        }
        let hook = crate::models::HookInput {
            session_id: "opus-session-1".into(),
            hook_event_name: "SessionStart".into(),
            cwd: "/tmp".into(),
            model: Some("claude-opus-4-20250514".into()),
            ..Default::default()
        };
        insert_event(&pool, &hook, "{}").await.unwrap();

        let results = sessions_by_model(&pool, None).await.unwrap();
        assert_eq!(results.len(), 2);
        // Sorted by count DESC
        assert_eq!(results[0].model, "claude-sonnet-4-20250514");
        assert_eq!(results[0].session_count, 3);
        assert_eq!(results[1].model, "claude-opus-4-20250514");
        assert_eq!(results[1].session_count, 1);
    }

    #[tokio::test]
    async fn test_sessions_by_model_empty() {
        let (pool, _dir) = setup_detail_db().await;
        let results = sessions_by_model(&pool, None).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn test_tool_failures_by_error() {
        let (pool, _dir) = setup_detail_db().await;

        // Insert PostToolUseFailure events with errors
        for _ in 0..3 {
            let hook = crate::models::HookInput {
                session_id: "s1".into(),
                hook_event_name: "PostToolUseFailure".into(),
                cwd: "/tmp".into(),
                tool_name: Some("Bash".into()),
                error: Some("timeout".into()),
                ..Default::default()
            };
            insert_event(&pool, &hook, "{}").await.unwrap();
        }
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PostToolUseFailure".into(),
            cwd: "/tmp".into(),
            tool_name: Some("Read".into()),
            error: Some("not_found".into()),
            ..Default::default()
        };
        insert_event(&pool, &hook, "{}").await.unwrap();

        let results = tool_failures_by_error(&pool, None).await.unwrap();
        assert_eq!(results.len(), 2);
        // Sorted by count DESC
        assert_eq!(results[0].tool_name, "Bash");
        assert_eq!(results[0].error, "timeout");
        assert_eq!(results[0].count, 3);
        assert_eq!(results[1].tool_name, "Read");
        assert_eq!(results[1].error, "not_found");
        assert_eq!(results[1].count, 1);
    }

    #[tokio::test]
    async fn test_error_summary_with_detail_tables() {
        let (pool, _dir) = setup_detail_db().await;

        // Insert StopFailure events with stop_event_details populated
        for _ in 0..2 {
            let hook = crate::models::HookInput {
                session_id: "s1".into(),
                hook_event_name: "StopFailure".into(),
                cwd: "/tmp".into(),
                error: Some("context_limit".into()),
                ..Default::default()
            };
            insert_event(&pool, &hook, r#"{"error":"context_limit"}"#)
                .await
                .unwrap();
        }
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "StopFailure".into(),
            cwd: "/tmp".into(),
            error: Some("timeout".into()),
            ..Default::default()
        };
        insert_event(&pool, &hook, r#"{"error":"timeout"}"#)
            .await
            .unwrap();

        let summary = error_summary(&pool, None).await.unwrap();
        assert_eq!(summary.stop_failure_count, 3);
        assert_eq!(summary.stop_failure_types.len(), 2);
        // Sorted by count DESC
        assert_eq!(summary.stop_failure_types[0].error_type, "context_limit");
        assert_eq!(summary.stop_failure_types[0].count, 2);
        assert_eq!(summary.stop_failure_types[1].error_type, "timeout");
        assert_eq!(summary.stop_failure_types[1].count, 1);
    }

    #[tokio::test]
    async fn test_error_summary_fallback() {
        let (pool, _dir) = setup_detail_db().await;

        // Insert StopFailure events directly (bypassing detail table population)
        // by inserting into events table only
        sqlx::query(
            "INSERT INTO events (session_id, event_type, cwd, raw_payload) VALUES (?, ?, ?, ?)",
        )
        .bind("s1")
        .bind("StopFailure")
        .bind("/tmp")
        .bind(r#"{"error":"context_limit"}"#)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO events (session_id, event_type, cwd, raw_payload) VALUES (?, ?, ?, ?)",
        )
        .bind("s1")
        .bind("StopFailure")
        .bind("/tmp")
        .bind(r#"{"error":"context_limit"}"#)
        .execute(&pool)
        .await
        .unwrap();

        let summary = error_summary(&pool, None).await.unwrap();
        assert_eq!(summary.stop_failure_count, 2);
        assert_eq!(summary.stop_failure_types.len(), 1);
        assert_eq!(summary.stop_failure_types[0].error_type, "context_limit");
        assert_eq!(summary.stop_failure_types[0].count, 2);
    }

    // ── fetch_event_detail tests (US-0044) ──

    #[tokio::test]
    async fn test_fetch_event_detail_tool() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PreToolUse".into(),
            cwd: "/tmp".into(),
            tool_name: Some("Bash".into()),
            tool_use_id: Some("tu-001".into()),
            ..Default::default()
        };
        let id = insert_event(&pool, &hook, "{}").await.unwrap();

        let detail = fetch_event_detail(&pool, id, "PreToolUse").await.unwrap();
        assert!(detail.is_some());
        match detail.unwrap() {
            EventDetail::Tool(t) => {
                assert_eq!(t.tool_use_id.as_deref(), Some("tu-001"));
                assert!(t.error.is_none());
            }
            other => panic!("expected Tool variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_fetch_event_detail_stop_failure() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "StopFailure".into(),
            cwd: "/tmp".into(),
            error: Some("rate_limit".into()),
            error_details: Some("exceeded quota".into()),
            stop_hook_active: Some(true),
            ..Default::default()
        };
        let id = insert_event(&pool, &hook, "{}").await.unwrap();

        let detail = fetch_event_detail(&pool, id, "StopFailure").await.unwrap();
        assert!(detail.is_some());
        match detail.unwrap() {
            EventDetail::Stop(s) => {
                assert_eq!(s.stop_hook_active, Some(true));
                assert_eq!(s.error.as_deref(), Some("rate_limit"));
                assert_eq!(s.error_details.as_deref(), Some("exceeded quota"));
            }
            other => panic!("expected Stop variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_fetch_event_detail_subagent_stop_dual() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SubagentStop".into(),
            cwd: "/tmp".into(),
            stop_hook_active: Some(true),
            last_assistant_message: Some("Done.".into()),
            agent_id: Some("agent-789".into()),
            agent_type: Some("inner".into()),
            agent_transcript_path: Some("/tmp/transcript.json".into()),
            ..Default::default()
        };
        let id = insert_event(&pool, &hook, "{}").await.unwrap();

        let detail = fetch_event_detail(&pool, id, "SubagentStop").await.unwrap();
        assert!(detail.is_some());
        match detail.unwrap() {
            EventDetail::StopAgent(stop, agent) => {
                assert_eq!(stop.stop_hook_active, Some(true));
                assert_eq!(stop.last_assistant_message.as_deref(), Some("Done."));
                assert_eq!(agent.agent_id.as_deref(), Some("agent-789"));
                assert_eq!(agent.agent_type.as_deref(), Some("inner"));
                assert_eq!(
                    agent.agent_transcript_path.as_deref(),
                    Some("/tmp/transcript.json")
                );
            }
            other => panic!("expected StopAgent variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_fetch_event_detail_missing() {
        let (pool, _dir) = setup_detail_db().await;
        // Query for non-existent event_id
        let detail = fetch_event_detail(&pool, 99999, "PreToolUse")
            .await
            .unwrap();
        assert!(detail.is_none());
    }

    #[tokio::test]
    async fn test_fetch_event_detail_session() {
        let (pool, _dir) = setup_detail_db().await;
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "SessionStart".into(),
            cwd: "/tmp".into(),
            source: Some("startup".into()),
            model: Some("claude-sonnet-4-20250514".into()),
            ..Default::default()
        };
        let id = insert_event(&pool, &hook, "{}").await.unwrap();

        let detail = fetch_event_detail(&pool, id, "SessionStart").await.unwrap();
        assert!(detail.is_some());
        match detail.unwrap() {
            EventDetail::Session(s) => {
                assert_eq!(s.source.as_deref(), Some("startup"));
                assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-20250514"));
                assert!(s.reason.is_none());
            }
            other => panic!("expected Session variant, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_fetch_event_detail_unknown_type() {
        let (pool, _dir) = setup_detail_db().await;
        let detail = fetch_event_detail(&pool, 1, "SomeUnknownType")
            .await
            .unwrap();
        assert!(detail.is_none());
    }
}
