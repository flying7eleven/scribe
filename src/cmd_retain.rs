use chrono::Utc;
use sqlx::SqlitePool;

/// Delete events older than `duration_str` and clean up orphaned sessions.
pub async fn run(pool: &SqlitePool, duration_str: &str) -> Result<(), Box<dyn std::error::Error>> {
    let duration = humantime::parse_duration(duration_str).map_err(|e| {
        format!("invalid duration '{duration_str}': {e} (expected e.g. 90d, 30d, 1w, 24h)")
    })?;

    let cutoff = Utc::now() - chrono::Duration::from_std(duration)?;
    let cutoff_str = cutoff.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();

    // Delete events and orphaned sessions in a single transaction
    let mut tx = pool.begin().await?;

    let events_deleted = sqlx::query("DELETE FROM events WHERE timestamp < ?")
        .bind(&cutoff_str)
        .execute(&mut *tx)
        .await?
        .rows_affected();

    let sessions_deleted = sqlx::query(
        "DELETE FROM sessions WHERE session_id NOT IN (SELECT DISTINCT session_id FROM events)",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit().await?;

    // Report results
    if events_deleted > 0 {
        println!("Deleted {events_deleted} events older than {duration_str}.");
        if sessions_deleted > 0 {
            println!("Removed {sessions_deleted} orphaned sessions.");
        }
    } else {
        println!("No events older than {duration_str} found.");
    }

    // Reclaim disk space
    sqlx::query("PRAGMA incremental_vacuum")
        .execute(pool)
        .await?;
    sqlx::query("PRAGMA journal_size_limit = 0")
        .execute(pool)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();
        (pool, dir)
    }

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
            "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count) VALUES (?, ?, ?, '/tmp', 1) ON CONFLICT(session_id) DO UPDATE SET last_seen = excluded.last_seen, event_count = event_count + 1",
        )
        .bind(session)
        .bind(ts)
        .bind(ts)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_handler_valid_duration() {
        let (pool, _dir) = setup_db().await;

        // Insert old event (2020) and recent event (now)
        insert_event_at(&pool, "s1", "2020-01-01T00:00:00.000Z").await;
        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        insert_event_at(&pool, "s2", &now).await;

        run(&pool, "1d").await.unwrap();

        // Old event deleted, recent kept
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);

        // s1 orphaned and deleted, s2 still has events
        let session_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(session_count, 1);
    }

    #[tokio::test]
    async fn test_handler_no_old_events() {
        let (pool, _dir) = setup_db().await;

        let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        insert_event_at(&pool, "s1", &now).await;

        run(&pool, "1d").await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_handler_invalid_duration() {
        let (pool, _dir) = setup_db().await;
        let result = run(&pool, "not-a-duration").await;
        assert!(result.is_err());
    }
}
