use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{FromRow, SqlitePool};

/// A row from the `events` table.
#[allow(dead_code)] // Fields read by cmd_query — wired in by E04
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

/// Query events ordered by timestamp descending with a limit.
/// No filtering — full filtering is added in E04.
#[allow(dead_code)] // Wired in by E04 (cmd_query)
pub async fn query_events(
    pool: &SqlitePool,
    limit: i64,
) -> Result<Vec<EventRow>, Box<dyn std::error::Error>> {
    let rows = sqlx::query_as::<_, EventRow>(
        "SELECT id, timestamp, session_id, event_type, tool_name, tool_input, tool_response, cwd, permission_mode, raw_payload
         FROM events ORDER BY timestamp DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
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

        let events = query_events(&pool, 50).await.unwrap();
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

        let events = query_events(&pool, 3).await.unwrap();
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
}
