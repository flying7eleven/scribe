use std::io;

use chrono::{DateTime, NaiveDate, Utc};
use comfy_table::{ContentArrangement, Table};
use serde_json::json;
use sqlx::SqlitePool;

use crate::db::{self, EventFilter, EventRow, SessionFilter, SessionRow};

pub enum OutputFormat {
    Table,
    Json,
    Csv,
}

/// Parse a time specifier (duration like "1h" or date like "2025-06-01") into a UTC datetime string.
pub fn parse_time_spec(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(duration) = humantime::parse_duration(input) {
        let now = Utc::now();
        let duration_chrono = chrono::Duration::from_std(duration)?;
        let target = now - duration_chrono;
        return Ok(target.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
    }

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input) {
        return Ok(dt
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string());
    }

    if let Ok(date) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).ok_or("invalid date")?.and_utc();
        return Ok(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
    }

    Err(format!(
        "cannot parse time specifier: '{input}' (expected duration like '1h' or date like '2025-06-01')"
    )
    .into())
}

/// Query and display events.
pub async fn run_events(
    pool: &SqlitePool,
    filter: EventFilter,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let events = db::query_events(pool, &filter).await?;

    if events.is_empty() {
        eprintln!("No events found.");
        return Ok(());
    }

    match format {
        OutputFormat::Table => print_events_table(&events),
        OutputFormat::Json => print_events_json(&events),
        OutputFormat::Csv => print_events_csv(&events)?,
    }

    Ok(())
}

/// Query and display sessions.
pub async fn run_sessions(
    pool: &SqlitePool,
    filter: SessionFilter,
    format: OutputFormat,
) -> Result<(), Box<dyn std::error::Error>> {
    let sessions = db::query_sessions(pool, &filter).await?;

    if sessions.is_empty() {
        eprintln!("No sessions found.");
        return Ok(());
    }

    match format {
        OutputFormat::Table => print_sessions_table(&sessions),
        OutputFormat::Json => print_sessions_json(&sessions),
        OutputFormat::Csv => print_sessions_csv(&sessions)?,
    }

    Ok(())
}

// ── Event output formatters ──

fn print_events_table(events: &[EventRow]) {
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["TIMESTAMP", "EVENT", "TOOL", "SUMMARY"]);

    for event in events {
        table.add_row(vec![
            format_timestamp(&event.timestamp),
            event.event_type.clone(),
            event.tool_name.clone().unwrap_or_default(),
            truncate_summary(event.tool_input.as_deref(), 60),
        ]);
    }

    println!("{table}");
}

fn print_events_json(events: &[EventRow]) {
    for event in events {
        let obj = json!({
            "id": event.id,
            "timestamp": event.timestamp,
            "session_id": event.session_id,
            "event_type": event.event_type,
            "tool_name": event.tool_name,
            "tool_input": event.tool_input,
            "tool_response": event.tool_response,
            "cwd": event.cwd,
            "permission_mode": event.permission_mode,
            "raw_payload": event.raw_payload,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    }
}

fn print_events_csv(events: &[EventRow]) -> Result<(), Box<dyn std::error::Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());
    wtr.write_record([
        "id",
        "timestamp",
        "session_id",
        "event_type",
        "tool_name",
        "tool_input",
        "tool_response",
        "cwd",
        "permission_mode",
        "raw_payload",
    ])?;
    for event in events {
        wtr.write_record([
            &event.id.to_string(),
            &event.timestamp,
            &event.session_id,
            &event.event_type,
            event.tool_name.as_deref().unwrap_or(""),
            event.tool_input.as_deref().unwrap_or(""),
            event.tool_response.as_deref().unwrap_or(""),
            event.cwd.as_deref().unwrap_or(""),
            event.permission_mode.as_deref().unwrap_or(""),
            &event.raw_payload,
        ])?;
    }
    wtr.flush()?;
    Ok(())
}

// ── Session output formatters ──

fn print_sessions_table(sessions: &[SessionRow]) {
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "SESSION",
        "FIRST SEEN",
        "LAST SEEN",
        "DURATION",
        "EVENTS",
        "CWD",
    ]);

    for session in sessions {
        table.add_row(vec![
            truncate_session_id(&session.session_id),
            format_timestamp(&session.first_seen),
            format_timestamp(&session.last_seen),
            format_duration(&session.first_seen, &session.last_seen),
            session.event_count.to_string(),
            truncate_cwd(session.cwd.as_deref(), 40),
        ]);
    }

    println!("{table}");
}

fn print_sessions_json(sessions: &[SessionRow]) {
    for session in sessions {
        let obj = json!({
            "session_id": session.session_id,
            "first_seen": session.first_seen,
            "last_seen": session.last_seen,
            "duration": format_duration(&session.first_seen, &session.last_seen),
            "event_count": session.event_count,
            "cwd": session.cwd,
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    }
}

fn print_sessions_csv(sessions: &[SessionRow]) -> Result<(), Box<dyn std::error::Error>> {
    let mut wtr = csv::Writer::from_writer(io::stdout());
    wtr.write_record([
        "session_id",
        "first_seen",
        "last_seen",
        "duration",
        "event_count",
        "cwd",
    ])?;
    for session in sessions {
        wtr.write_record([
            &session.session_id,
            &session.first_seen,
            &session.last_seen,
            &format_duration(&session.first_seen, &session.last_seen),
            &session.event_count.to_string(),
            session.cwd.as_deref().unwrap_or(""),
        ])?;
    }
    wtr.flush()?;
    Ok(())
}

// ── Helpers ──

fn format_timestamp(ts: &str) -> String {
    // Parse ISO 8601 and reformat for readability
    if let Ok(dt) = DateTime::parse_from_rfc3339(ts) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    // Fallback: try parsing the DB format with milliseconds
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.fZ") {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    ts.to_string()
}

fn format_duration(first_seen: &str, last_seen: &str) -> String {
    let parse = |s: &str| -> Option<DateTime<Utc>> {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .ok()
            .or_else(|| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.fZ")
                    .map(|ndt| ndt.and_utc())
                    .ok()
            })
    };

    let (Some(start), Some(end)) = (parse(first_seen), parse(last_seen)) else {
        return "?".to_string();
    };

    let diff = end - start;
    let total_minutes = diff.num_minutes();
    let total_hours = diff.num_hours();
    let total_days = diff.num_days();

    if total_minutes < 1 {
        "< 1m".to_string()
    } else if total_hours < 1 {
        format!("{total_minutes}m")
    } else if total_days < 1 {
        let hours = total_hours;
        let minutes = total_minutes - hours * 60;
        format!("{hours}h {minutes}m")
    } else {
        let days = total_days;
        let hours = total_hours - days * 24;
        format!("{days}d {hours}h")
    }
}

fn truncate_session_id(id: &str) -> String {
    if id.len() > 8 {
        id[..8].to_string()
    } else {
        id.to_string()
    }
}

fn truncate_summary(tool_input: Option<&str>, max_len: usize) -> String {
    let Some(input) = tool_input else {
        return String::new();
    };

    // Try to extract a recognizable value from the JSON
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(input) {
        // Common tool input keys
        for key in ["command", "file_path", "pattern", "content", "query"] {
            if let Some(s) = val.get(key).and_then(|v| v.as_str()) {
                return truncate_str(s, max_len);
            }
        }
    }

    truncate_str(input, max_len)
}

fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

fn truncate_cwd(cwd: Option<&str>, max_len: usize) -> String {
    let Some(path) = cwd else {
        return String::new();
    };
    if path.len() <= max_len {
        path.to_string()
    } else {
        format!("...{}", &path[path.len() - max_len + 3..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Time parsing tests (from S01) ──

    #[test]
    fn test_parse_duration() {
        let result = parse_time_spec("1h").unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&result).unwrap();
        let diff = Utc::now() - parsed.with_timezone(&Utc);
        assert!(diff.num_minutes() >= 55 && diff.num_minutes() <= 65);
    }

    #[test]
    fn test_parse_duration_days() {
        let result = parse_time_spec("7d").unwrap();
        let parsed = chrono::DateTime::parse_from_rfc3339(&result).unwrap();
        let diff = Utc::now() - parsed.with_timezone(&Utc);
        assert!(diff.num_days() >= 6 && diff.num_days() <= 8);
    }

    #[test]
    fn test_parse_absolute_date() {
        let result = parse_time_spec("2025-06-01").unwrap();
        assert_eq!(result, "2025-06-01T00:00:00.000Z");
    }

    #[test]
    fn test_parse_absolute_datetime() {
        let result = parse_time_spec("2025-06-01T14:30:00Z").unwrap();
        assert_eq!(result, "2025-06-01T14:30:00.000Z");
    }

    #[test]
    fn test_parse_invalid_input() {
        assert!(parse_time_spec("not-a-time").is_err());
    }

    // ── Duration formatting ──

    #[test]
    fn test_duration_less_than_1m() {
        assert_eq!(
            format_duration("2025-01-01T10:00:00.000Z", "2025-01-01T10:00:30.000Z"),
            "< 1m"
        );
    }

    #[test]
    fn test_duration_minutes() {
        assert_eq!(
            format_duration("2025-01-01T10:00:00.000Z", "2025-01-01T10:14:00.000Z"),
            "14m"
        );
    }

    #[test]
    fn test_duration_hours_minutes() {
        assert_eq!(
            format_duration("2025-01-01T10:00:00.000Z", "2025-01-01T12:14:00.000Z"),
            "2h 14m"
        );
    }

    #[test]
    fn test_duration_days_hours() {
        assert_eq!(
            format_duration("2025-01-01T10:00:00.000Z", "2025-01-04T11:00:00.000Z"),
            "3d 1h"
        );
    }

    // ── Truncation helpers ──

    #[test]
    fn test_truncate_session_id_long() {
        assert_eq!(truncate_session_id("abcdefghijklmnop"), "abcdefgh");
    }

    #[test]
    fn test_truncate_session_id_short() {
        assert_eq!(truncate_session_id("abc"), "abc");
    }

    #[test]
    fn test_truncate_summary_none() {
        assert_eq!(truncate_summary(None, 60), "");
    }

    #[test]
    fn test_truncate_summary_short() {
        assert_eq!(truncate_summary(Some(r#"{"command":"ls"}"#), 60), "ls");
    }

    #[test]
    fn test_truncate_summary_long() {
        let long = "a".repeat(100);
        let result = truncate_summary(Some(&long), 20);
        assert!(result.len() <= 20);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_truncate_summary_bash_command() {
        assert_eq!(
            truncate_summary(Some(r#"{"command":"echo hello world"}"#), 60),
            "echo hello world"
        );
    }

    #[test]
    fn test_truncate_summary_file_path() {
        assert_eq!(
            truncate_summary(Some(r#"{"file_path":"/home/user/file.rs"}"#), 60),
            "/home/user/file.rs"
        );
    }

    #[test]
    fn test_truncate_cwd_short() {
        assert_eq!(truncate_cwd(Some("/home/user"), 40), "/home/user");
    }

    #[test]
    fn test_truncate_cwd_long() {
        let long = format!("/very/long/path/{}", "sub/".repeat(20));
        let result = truncate_cwd(Some(&long), 40);
        assert!(result.len() <= 40);
        assert!(result.starts_with("..."));
    }

    #[test]
    fn test_truncate_cwd_none() {
        assert_eq!(truncate_cwd(None, 40), "");
    }

    // ── Format timestamp ──

    #[test]
    fn test_format_timestamp() {
        assert_eq!(
            format_timestamp("2025-06-01T14:30:05.123Z"),
            "2025-06-01 14:30:05"
        );
    }
}
