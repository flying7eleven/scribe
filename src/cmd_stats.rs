use chrono::NaiveDate;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::cmd_query;
use crate::db;
use crate::format::{
    format_count, format_date_label, format_duration, format_period, format_size, format_timestamp,
    histogram_bar, truncate_path,
};

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
    sessions_by_model: Vec<db::ModelSessionCount>,
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
    let models = db::sessions_by_model(pool, since_ref).await?;

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
            models,
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
        &models,
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
    models: Vec<db::ModelSessionCount>,
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
        sessions_by_model: models,
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
    models: &[db::ModelSessionCount],
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

    // ── Sessions by model ──
    if !models.is_empty() {
        println!();
        println!("Sessions by model:");
        let max_count = models.iter().map(|m| m.session_count).max().unwrap_or(0);
        let count_width = format_count(max_count).len();
        for m in models {
            println!(
                "  {:<30} {:>width$}",
                m.model,
                format_count(m.session_count),
                width = count_width
            );
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

/// Fill in zero-count days between first and last date in the activity data.
pub fn fill_zero_days(activity: &[db::DailyCount]) -> Vec<(String, i64)> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_stats_populated() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();

        db::insert_test_event(
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
        db::insert_test_event(
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
