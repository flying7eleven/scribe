use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

/// A row from the `events` table.
#[allow(dead_code)] // Fields read by cmd_query — wired in by E04-S02
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
    #[allow(dead_code)] // Used by tests + wired in by E04-S02
    pub fn new() -> Self {
        Self {
            limit: 50,
            ..Default::default()
        }
    }
}

/// A row from the `sessions` table.
#[allow(dead_code)] // Fields read by cmd_query — wired in by E04-S02
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
    #[allow(dead_code)] // Used by tests + wired in by E04-S02
    pub fn new() -> Self {
        Self {
            limit: 50,
            ..Default::default()
        }
    }
}

/// Resolve the database path with precedence:
/// 1. `--db <path>` CLI argument (passed as `cli_db`)
/// 2. `SCRIBE_DB` environment variable
/// 3. Default: `~/.claude/scribe.db`
pub fn resolve_db_path(cli_db: Option<&str>) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(path) = cli_db {
        return Ok(path.to_string());
    }

    if let Ok(path) = std::env::var("SCRIBE_DB") {
        if !path.is_empty() {
            return Ok(path);
        }
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

/// Insert an event into `events` and upsert `sessions` in a single transaction.
#[allow(clippy::too_many_arguments)]
pub async fn insert_event(
    pool: &SqlitePool,
    session_id: &str,
    event_type: &str,
    tool_name: Option<&str>,
    tool_input: Option<&str>,
    tool_response: Option<&str>,
    cwd: &str,
    permission_mode: Option<&str>,
    raw_payload: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = chrono::Utc::now()
        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
        .to_string();

    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO events (session_id, event_type, tool_name, tool_input, tool_response, cwd, permission_mode, raw_payload)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(session_id)
    .bind(event_type)
    .bind(tool_name)
    .bind(tool_input)
    .bind(tool_response)
    .bind(cwd)
    .bind(permission_mode)
    .bind(raw_payload)
    .execute(&mut *tx)
    .await?;

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

    tx.commit().await?;
    Ok(())
}

/// Query events with dynamic filters, ordered by timestamp descending.
#[allow(dead_code)] // Wired in by E04-S02 (cmd_query)
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
#[allow(dead_code)] // Wired in by E04-S02 (cmd_query)
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

        // Verify all four indexes on events
        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='events' AND name LIKE 'idx_%' ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            indexes,
            vec![
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

        insert_event(
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
        insert_event(
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
        insert_event(
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

        insert_event(
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
        insert_event(
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
            insert_event(
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
        let result = resolve_db_path(Some("/cli/path.db")).unwrap();
        assert_eq!(result, "/cli/path.db");
        unsafe { std::env::remove_var("SCRIBE_DB") };
    }

    #[test]
    fn test_resolve_db_path_env_overrides_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::set_var("SCRIBE_DB", "/env/path.db") };
        let result = resolve_db_path(None).unwrap();
        assert_eq!(result, "/env/path.db");
        unsafe { std::env::remove_var("SCRIBE_DB") };
    }

    #[test]
    fn test_resolve_db_path_default() {
        let _lock = ENV_LOCK.lock().unwrap();
        unsafe { std::env::remove_var("SCRIBE_DB") };
        let result = resolve_db_path(None).unwrap();
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
}
