use chrono::NaiveDate;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::cmd_query;
use crate::db;

/// JSON output structure for `scribe stats --json`.
#[derive(Serialize)]
struct StatsJson {
    db_path: String,
    db_size_bytes: u64,
    event_count: i64,
    session_count: i64,
    oldest_event: Option<String>,
    newest_event: Option<String>,
    avg_session_duration_seconds: Option<f64>,
    top_tools: Vec<db::ToolCount>,
    event_types: Vec<db::EventTypeCount>,
    errors: ErrorsJson,
    top_directories: Vec<db::DirCount>,
    daily_activity: Vec<DailyActivityEntry>,
}

#[derive(Serialize)]
struct ErrorsJson {
    post_tool_use_failure: i64,
    stop_failure: i64,
    stop_failure_types: Vec<db::StopFailureType>,
}

#[derive(Serialize)]
struct DailyActivityEntry {
    date: String,
    count: i64,
}

/// Display database metrics with extended stats dashboard.
pub async fn run(
    pool: &SqlitePool,
    db_path: &str,
    since: Option<&str>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // Resolve --since to ISO 8601 UTC timestamp
    let resolved_since = since.map(cmd_query::parse_time_spec).transpose()?;
    let since_ref = resolved_since.as_deref();

    // Gather all stats
    let stats = db::get_stats(pool, since_ref).await?;
    let avg_dur = db::avg_session_duration(pool, since_ref).await?;
    let tools = db::top_tools(pool, since_ref, 10).await?;
    let event_types = db::event_type_breakdown(pool, since_ref).await?;
    let errors = db::error_summary(pool, since_ref).await?;
    let dirs = db::top_directories(pool, since_ref, 5).await?;
    let activity = db::daily_activity(pool, since_ref).await?;
    let filled = fill_zero_days(&activity);

    if json {
        return run_json(
            db_path,
            &stats,
            avg_dur,
            tools,
            event_types,
            errors,
            dirs,
            &filled,
        );
    }

    run_text(
        db_path,
        since_ref,
        &stats,
        avg_dur,
        &tools,
        &event_types,
        &errors,
        &dirs,
        &filled,
    )
}

/// JSON output mode — single JSON object to stdout.
#[allow(clippy::too_many_arguments)]
fn run_json(
    db_path: &str,
    stats: &db::DbStats,
    avg_dur: Option<f64>,
    tools: Vec<db::ToolCount>,
    event_types: Vec<db::EventTypeCount>,
    errors: db::ErrorSummary,
    dirs: Vec<db::DirCount>,
    filled: &[(String, i64)],
) -> Result<(), Box<dyn std::error::Error>> {
    let db_size_bytes = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);

    let daily_activity: Vec<DailyActivityEntry> = filled
        .iter()
        .map(|(date, count)| DailyActivityEntry {
            date: date.clone(),
            count: *count,
        })
        .collect();

    let output = StatsJson {
        db_path: db_path.to_string(),
        db_size_bytes,
        event_count: stats.event_count,
        session_count: stats.session_count,
        oldest_event: stats.oldest_event.clone(),
        newest_event: stats.newest_event.clone(),
        avg_session_duration_seconds: avg_dur,
        top_tools: tools,
        event_types,
        errors: ErrorsJson {
            post_tool_use_failure: errors.post_tool_use_failure_count,
            stop_failure: errors.stop_failure_count,
            stop_failure_types: errors.stop_failure_types,
        },
        top_directories: dirs,
        daily_activity,
    };

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

/// Text output mode — human-readable dashboard.
#[allow(clippy::too_many_arguments)]
fn run_text(
    db_path: &str,
    since_ref: Option<&str>,
    stats: &db::DbStats,
    avg_dur: Option<f64>,
    tools: &[db::ToolCount],
    event_types: &[db::EventTypeCount],
    errors: &db::ErrorSummary,
    dirs: &[db::DirCount],
    filled: &[(String, i64)],
) -> Result<(), Box<dyn std::error::Error>> {
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

    // ── Header ──
    println!("Database:  {db_path}");
    println!("Size:      {file_size}");
    if let Some(since_ts) = since_ref {
        let period = format_period(since_ts);
        println!("Period:    {period}");
    }
    println!("Events:    {}", format_count(stats.event_count));
    println!("Sessions:  {}", format_count(stats.session_count));

    if let Some(avg) = avg_dur {
        println!("Avg duration:  {}", format_duration(avg));
    }

    println!("Oldest:    {oldest}");
    println!("Newest:    {newest}");

    // Skip extended sections if DB is empty
    if stats.event_count == 0 {
        return Ok(());
    }

    // ── Top tools ──
    if !tools.is_empty() {
        println!();
        println!("Top tools:");
        let max_count = tools.iter().map(|t| t.count).max().unwrap_or(0);
        let count_width = format_count(max_count).len();
        for (i, tool) in tools.iter().enumerate() {
            println!(
                "  {:>2}. {:<20} {:>width$}",
                i + 1,
                tool.tool_name,
                format_count(tool.count),
                width = count_width
            );
        }
    }

    // ── Event types ──
    if !event_types.is_empty() {
        println!();
        println!("Event types:");
        let max_count = event_types.iter().map(|t| t.count).max().unwrap_or(0);
        let count_width = format_count(max_count).len();
        for et in event_types {
            println!(
                "  {:<24} {:>width$}",
                et.event_type,
                format_count(et.count),
                width = count_width
            );
        }
    }

    // ── Errors ──
    println!();
    if errors.post_tool_use_failure_count == 0 && errors.stop_failure_count == 0 {
        println!("Errors:              none");
    } else {
        println!("Errors:");
        if errors.post_tool_use_failure_count > 0 {
            println!(
                "  {:<24} {:>6}",
                "PostToolUseFailure",
                format_count(errors.post_tool_use_failure_count)
            );
        }
        if errors.stop_failure_count > 0 {
            println!(
                "  {:<24} {:>6}",
                "StopFailure",
                format_count(errors.stop_failure_count)
            );
            for sf in &errors.stop_failure_types {
                println!("    {:<22} {:>6}", sf.error_type, format_count(sf.count));
            }
        }
    }

    // ── Top directories ──
    if !dirs.is_empty() {
        println!();
        println!("Top directories:");
        let max_count = dirs.iter().map(|d| d.count).max().unwrap_or(0);
        let count_width = format_count(max_count).len();
        for (i, dir) in dirs.iter().enumerate() {
            let path = truncate_path(&dir.cwd, 40);
            println!(
                "  {:>2}. {:<40} {:>width$}",
                i + 1,
                path,
                format_count(dir.count),
                width = count_width
            );
        }
    }

    // ── Activity histogram ──
    if !filled.is_empty() {
        println!();
        if let Some(s) = since_ref {
            println!("Activity (since {}):", format_timestamp(s));
        } else {
            println!("Activity (last 14 days):");
        }

        let max_count = filled.iter().map(|(_, c)| *c).max().unwrap_or(0);
        let count_width = format_count(max_count).len();

        for (date_str, count) in filled {
            let label = format_date_label(date_str);
            let bar = histogram_bar(*count, max_count, 40);
            println!(
                "  {label}  {:<40} {:>width$}",
                bar,
                format_count(*count),
                width = count_width
            );
        }
    }

    Ok(())
}

/// Format the --since period line, e.g. "since 2025-06-17 (7 days)"
fn format_period(since_ts: &str) -> String {
    let date_part = format_timestamp(since_ts);
    let now = chrono::Utc::now();

    // Try to parse the since timestamp and compute relative days
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(since_ts, "%Y-%m-%dT%H:%M:%S%.fZ") {
        let days = (now.naive_utc() - dt).num_days();
        if days > 0 {
            return format!("since {date_part} ({days} days)");
        }
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(since_ts) {
        let days = (now - dt.with_timezone(&chrono::Utc)).num_days();
        if days > 0 {
            return format!("since {date_part} ({days} days)");
        }
    }

    format!("since {date_part}")
}

/// Format a date string (YYYY-MM-DD) as "Mon DD" for histogram labels.
fn format_date_label(date_str: &str) -> String {
    if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        return date.format("%b %d").to_string();
    }
    date_str.to_string()
}

/// Fill in zero-count days between first and last date in the activity data.
fn fill_zero_days(activity: &[db::DailyCount]) -> Vec<(String, i64)> {
    if activity.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let first = &activity[0].date;
    let last = &activity[activity.len() - 1].date;

    let Ok(start_date) = NaiveDate::parse_from_str(first, "%Y-%m-%d") else {
        return activity.iter().map(|d| (d.date.clone(), d.count)).collect();
    };
    let Ok(end_date) = NaiveDate::parse_from_str(last, "%Y-%m-%d") else {
        return activity.iter().map(|d| (d.date.clone(), d.count)).collect();
    };

    let count_map: std::collections::HashMap<&str, i64> = activity
        .iter()
        .map(|d| (d.date.as_str(), d.count))
        .collect();

    let mut current = start_date;
    while current <= end_date {
        let key = current.format("%Y-%m-%d").to_string();
        let count = count_map.get(key.as_str()).copied().unwrap_or(0);
        result.push((key, count));
        current += chrono::Duration::days(1);
    }

    result
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

/// Render a histogram bar of `value` relative to `max_value`, capped at `max_width` characters.
pub fn histogram_bar(value: i64, max_value: i64, max_width: usize) -> String {
    if value == 0 || max_value == 0 {
        return String::new();
    }
    let width = (value as f64 / max_value as f64 * max_width as f64).round() as usize;
    let width = width.max(1); // at least 1 char for non-zero values
    "\u{2588}".repeat(width)
}

/// Truncate a path to fit within `max_width` characters.
/// Preserves the last meaningful path segments, replacing leading segments with `...`.
pub fn truncate_path(path: &str, max_width: usize) -> String {
    if path.len() <= max_width {
        return path.to_string();
    }

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let prefix = "...";

    // Try progressively fewer trailing segments
    for start in 1..segments.len() {
        let tail = segments[start..].join("/");
        let candidate = format!("{prefix}/{tail}");
        if candidate.len() <= max_width {
            return candidate;
        }
    }

    // Even the last segment + prefix is too long — hard truncate from the left
    let truncated = &path[path.len().saturating_sub(max_width.saturating_sub(3))..];
    format!("{prefix}{truncated}")
}

/// Format a duration in seconds into a human-friendly string.
pub fn format_duration(seconds: f64) -> String {
    if seconds <= 0.0 {
        return "< 1s".to_string();
    }

    let total_secs = seconds.round() as u64;

    if total_secs == 0 {
        return "< 1s".to_string();
    }

    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;

    if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        if secs > 0 && mins < 10 {
            format!("{mins}m {secs}s")
        } else {
            format!("{mins}m")
        }
    } else {
        format!("{secs}s")
    }
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

    // ── Histogram bar tests ──

    #[test]
    fn test_histogram_bar_full() {
        let bar = histogram_bar(500, 500, 40);
        assert_eq!(bar.chars().count(), 40);
        assert!(bar.chars().all(|c| c == '\u{2588}'));
    }

    #[test]
    fn test_histogram_bar_half() {
        let bar = histogram_bar(250, 500, 40);
        assert_eq!(bar.chars().count(), 20);
    }

    #[test]
    fn test_histogram_bar_zero_value() {
        assert_eq!(histogram_bar(0, 500, 40), "");
    }

    #[test]
    fn test_histogram_bar_zero_max() {
        assert_eq!(histogram_bar(100, 0, 40), "");
    }

    #[test]
    fn test_histogram_bar_small_value_at_least_one() {
        let bar = histogram_bar(1, 500, 40);
        assert!(!bar.is_empty());
        assert!(bar.chars().count() >= 1);
    }

    // ── Path truncation tests ──

    #[test]
    fn test_truncate_path_short() {
        assert_eq!(truncate_path("/short", 20), "/short");
    }

    #[test]
    fn test_truncate_path_long() {
        let result = truncate_path("/home/user/projects/api", 20);
        assert!(result.len() <= 20);
        assert!(result.starts_with("..."));
        assert!(result.ends_with("/api"));
    }

    #[test]
    fn test_truncate_path_preserves_trailing() {
        let result = truncate_path("/home/user/projects/frontend/src/components", 30);
        assert!(result.len() <= 30);
        assert!(result.starts_with("..."));
        assert!(result.contains("components"));
    }

    #[test]
    fn test_truncate_path_exact_fit() {
        let path = "/a/b";
        assert_eq!(truncate_path(path, 4), "/a/b");
    }

    #[test]
    fn test_truncate_path_very_narrow() {
        let result = truncate_path("/home/user/very/long/path", 10);
        assert!(result.len() <= 10 || result.starts_with("..."));
    }

    // ── Duration formatting tests ──

    #[test]
    fn test_format_duration_hours_minutes() {
        assert_eq!(format_duration(7440.0), "2h 4m");
    }

    #[test]
    fn test_format_duration_minutes_only() {
        assert_eq!(format_duration(2700.0), "45m");
    }

    #[test]
    fn test_format_duration_minutes_seconds() {
        assert_eq!(format_duration(192.0), "3m 12s");
    }

    #[test]
    fn test_format_duration_seconds_only() {
        assert_eq!(format_duration(42.0), "42s");
    }

    #[test]
    fn test_format_duration_zero() {
        assert_eq!(format_duration(0.0), "< 1s");
    }

    #[test]
    fn test_format_duration_negative() {
        assert_eq!(format_duration(-5.0), "< 1s");
    }

    #[test]
    fn test_format_duration_large_minutes_no_seconds() {
        // >= 10 minutes: don't show seconds
        assert_eq!(format_duration(630.0), "10m");
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
