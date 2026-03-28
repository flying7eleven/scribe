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
        .pragma("busy_timeout", "5000");

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
        "Stop" | "StopFailure" | "SubagentStop" => {
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
        _ => {} // Tier 2/3 handled in US-0041
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

    // Application-side parsing of StopFailure error types from raw_payload
    let stop_failure_types = if stop_failure_count > 0 {
        let mut sql =
            String::from("SELECT raw_payload FROM events WHERE event_type = 'StopFailure'");
        let mut binds: Vec<String> = Vec::new();

        if let Some(since) = since {
            sql.push_str(" AND timestamp >= ?");
            binds.push(since.to_string());
        }

        let mut query = sqlx::query_scalar::<_, String>(&sql);
        for bind in &binds {
            query = query.bind(bind);
        }

        let payloads = query.fetch_all(pool).await?;

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

        let mut types: Vec<StopFailureType> = type_counts
            .into_iter()
            .map(|(error_type, count)| StopFailureType { error_type, count })
            .collect();
        types.sort_by(|a, b| b.count.cmp(&a.count));
        types
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
#[derive(Debug)]
pub struct ClassificationCount {
    pub risk_level: String,
    pub count: i64,
}

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

/// Enforcement statistics.
#[derive(Debug)]
pub struct EnforcementStats {
    pub total: i64,
    pub allowed: i64,
    pub denied: i64,
    pub top_denied: Vec<TopDeniedRule>,
}

/// A top denied rule entry for stats display.
#[derive(Debug)]
pub struct TopDeniedRule {
    pub rule_id: i64,
    pub reason: String,
    pub count: i64,
}

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

/// Delete a rule by ID. Returns true if a row was deleted.
pub async fn delete_rule(pool: &SqlitePool, id: i64) -> Result<bool, Box<dyn std::error::Error>> {
    let result = sqlx::query("DELETE FROM rules WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

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

/// Delete all rules. Returns the count deleted.
pub async fn delete_all_rules(pool: &SqlitePool) -> Result<u64, Box<dyn std::error::Error>> {
    let result = sqlx::query("DELETE FROM rules").execute(pool).await?;
    Ok(result.rows_affected())
}

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
}
