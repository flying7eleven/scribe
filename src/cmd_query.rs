use chrono::{NaiveDate, Utc};

/// Parse a time specifier (duration like "1h" or date like "2025-06-01") into a UTC datetime string.
/// Durations are subtracted from `now` to produce an absolute timestamp.
/// Returns an ISO 8601 UTC string matching the DB timestamp format.
#[allow(dead_code)] // Wired in by E04-S02 (cmd_query handler)
pub fn parse_time_spec(input: &str) -> Result<String, Box<dyn std::error::Error>> {
    // Try humantime duration first (e.g., "1h", "7d", "30m")
    if let Ok(duration) = humantime::parse_duration(input) {
        let now = Utc::now();
        let duration_chrono = chrono::Duration::from_std(duration)?;
        let target = now - duration_chrono;
        return Ok(target.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
    }

    // Try ISO 8601 datetime (e.g., "2025-06-01T14:30:00Z")
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input) {
        return Ok(dt
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string());
    }

    // Try date only (e.g., "2025-06-01")
    if let Ok(date) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).ok_or("invalid date")?.and_utc();
        return Ok(dt.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string());
    }

    Err(format!("cannot parse time specifier: '{input}' (expected duration like '1h' or date like '2025-06-01')").into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        let result = parse_time_spec("1h").unwrap();
        // Should be approximately 1 hour before now
        let parsed = chrono::DateTime::parse_from_rfc3339(&result).unwrap();
        let diff = Utc::now() - parsed.with_timezone(&Utc);
        // Allow some tolerance (55-65 minutes)
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
        let result = parse_time_spec("not-a-time");
        assert!(result.is_err());
    }
}
