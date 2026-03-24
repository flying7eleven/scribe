use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;

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
#[allow(dead_code)] // Used by cmd_log, cmd_query, etc. — wired in by later stories
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
