//! Shared formatting helpers used by cmd_stats and the TUI.

use chrono::NaiveDate;

/// Format a byte count as a human-readable size string.
pub fn format_size(bytes: u64) -> String {
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

/// Format an integer with comma-separated thousands.
pub fn format_count(n: i64) -> String {
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

/// Format an ISO 8601 timestamp as "YYYY-MM-DD HH:MM:SS".
pub fn format_timestamp(ts: &str) -> String {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.fZ") {
        return dt.format("%Y-%m-%d %H:%M:%S").to_string();
    }
    ts.to_string()
}

/// Format the --since period line, e.g. "since 2025-06-17 (7 days)"
pub fn format_period(since_ts: &str) -> String {
    let date_part = format_timestamp(since_ts);
    let now = chrono::Utc::now();

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
pub fn format_date_label(date_str: &str) -> String {
    if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        return date.format("%b %d").to_string();
    }
    date_str.to_string()
}

/// Convert a character count to an estimated token count string.
/// Uses the ~4 chars/token heuristic, rounds to nearest 1000, adds "~" prefix.
pub fn format_token_estimate(chars: i64) -> String {
    if chars == 0 {
        return "~0".to_string();
    }
    let tokens = chars / 4;
    if tokens < 1000 {
        return format!("~{tokens}");
    }
    // Round to nearest 1000
    let rounded = ((tokens + 500) / 1000) * 1000;
    format!("~{}", format_count(rounded))
}

/// Format a percentage from part/total.
pub fn format_percentage(part: i64, total: i64) -> String {
    if total == 0 {
        return "0%".to_string();
    }
    let pct = (part as f64 / total as f64 * 100.0).round() as i64;
    format!("{pct}%")
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
        assert_eq!(format_duration(630.0), "10m");
    }

    #[test]
    fn test_format_token_estimate_zero() {
        assert_eq!(format_token_estimate(0), "~0");
    }

    #[test]
    fn test_format_token_estimate_small() {
        // 100 chars / 4 = 25 tokens, below 1000 threshold
        assert_eq!(format_token_estimate(100), "~25");
    }

    #[test]
    fn test_format_token_estimate_medium() {
        // 20000 chars / 4 = 5000 tokens, rounds to 5000
        assert_eq!(format_token_estimate(20_000), "~5,000");
    }

    #[test]
    fn test_format_token_estimate_large() {
        // 1824000 chars / 4 = 456000 tokens
        assert_eq!(format_token_estimate(1_824_000), "~456,000");
    }

    #[test]
    fn test_format_percentage_normal() {
        assert_eq!(format_percentage(77, 100), "77%");
    }

    #[test]
    fn test_format_percentage_zero_total() {
        assert_eq!(format_percentage(10, 0), "0%");
    }

    #[test]
    fn test_format_percentage_zero_part() {
        assert_eq!(format_percentage(0, 100), "0%");
    }
}
