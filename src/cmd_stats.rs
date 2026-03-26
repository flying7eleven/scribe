use sqlx::SqlitePool;

use crate::db;

/// Display database metrics.
pub async fn run(pool: &SqlitePool, db_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let stats = db::get_stats(pool, None).await?;

    let file_size = std::fs::metadata(db_path)
        .map(|m| format_size(m.len()))
        .unwrap_or_else(|_| "unknown".to_string());

    let oldest = stats
        .oldest_event
        .as_deref()
        .map(format_timestamp)
        .unwrap_or_else(|| "\u{2014}".to_string()); // em dash
    let newest = stats
        .newest_event
        .as_deref()
        .map(format_timestamp)
        .unwrap_or_else(|| "\u{2014}".to_string());

    println!("Database:  {db_path}");
    println!("Size:      {file_size}");
    println!("Events:    {}", format_count(stats.event_count));
    println!("Sessions:  {}", format_count(stats.session_count));
    println!("Oldest:    {oldest}");
    println!("Newest:    {newest}");

    Ok(())
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn format_count(n: i64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

fn format_timestamp(ts: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.fZ") {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    ts.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
    }

    #[test]
    fn test_format_size_kb() {
        assert_eq!(format_size(1234), "1.2 KB");
    }

    #[test]
    fn test_format_size_mb() {
        assert_eq!(format_size(1_500_000), "1.4 MB");
    }

    #[test]
    fn test_format_size_gb() {
        assert_eq!(format_size(2_500_000_000), "2.3 GB");
    }

    #[test]
    fn test_format_count_small() {
        assert_eq!(format_count(42), "42");
    }

    #[test]
    fn test_format_count_thousands() {
        assert_eq!(format_count(12847), "12,847");
    }

    #[test]
    fn test_format_count_millions() {
        assert_eq!(format_count(1_234_567), "1,234,567");
    }

    #[test]
    fn test_format_count_zero() {
        assert_eq!(format_count(0), "0");
    }

    #[test]
    fn test_format_timestamp_iso() {
        assert_eq!(
            format_timestamp("2025-06-01T14:30:05.123Z"),
            "2025-06-01 14:30:05"
        );
    }

    #[tokio::test]
    async fn test_get_stats_populated() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        db::insert_event(
            &pool,
            "s1",
            "PreToolUse",
            Some("Bash"),
            None,
            None,
            "/tmp",
            None,
            "{}",
        )
        .await
        .unwrap();
        db::insert_event(
            &pool,
            "s2",
            "SessionStart",
            None,
            None,
            None,
            "/tmp",
            None,
            "{}",
        )
        .await
        .unwrap();

        let stats = db::get_stats(&pool, None).await.unwrap();
        assert_eq!(stats.event_count, 2);
        assert_eq!(stats.session_count, 2);
        assert!(stats.oldest_event.is_some());
        assert!(stats.newest_event.is_some());
    }

    #[tokio::test]
    async fn test_get_stats_empty() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        let stats = db::get_stats(&pool, None).await.unwrap();
        assert_eq!(stats.event_count, 0);
        assert_eq!(stats.session_count, 0);
        assert!(stats.oldest_event.is_none());
        assert!(stats.newest_event.is_none());
    }
}
