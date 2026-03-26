/// State for the filter bar (used in Sessions and Events tabs).
pub struct FilterState {
    pub active: bool,
    pub input: String,
}

impl FilterState {
    pub fn new() -> Self {
        Self {
            active: false,
            input: String::new(),
        }
    }

    /// Activate the filter bar (clears any previous input).
    pub fn activate(&mut self) {
        self.active = true;
        self.input.clear();
    }

    /// Deactivate the filter bar.
    pub fn deactivate(&mut self) {
        self.active = false;
        self.input.clear();
    }

    /// Add a character to the filter input.
    pub fn push_char(&mut self, c: char) {
        self.input.push(c);
    }

    /// Delete the last character (backspace).
    pub fn delete_char(&mut self) {
        self.input.pop();
    }

    /// Returns true if the input is empty (no active filter text).
    #[allow(dead_code)] // available for use by tab implementations
    pub fn is_empty(&self) -> bool {
        self.input.is_empty()
    }

    /// Case-insensitive substring match.
    pub fn matches(&self, text: &str) -> bool {
        if self.input.is_empty() {
            return true;
        }
        text.to_lowercase().contains(&self.input.to_lowercase())
    }
}

/// Compute filtered indices for a list of sessions.
pub fn filter_sessions(filter: &FilterState, sessions: &[crate::db::SessionRow]) -> Vec<usize> {
    if filter.input.is_empty() {
        return (0..sessions.len()).collect();
    }
    sessions
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let searchable = format!(
                "{} {} {} {}",
                s.session_id,
                s.first_seen,
                s.last_seen,
                s.cwd.as_deref().unwrap_or("")
            );
            filter.matches(&searchable)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Compute filtered indices for a list of events.
pub fn filter_events(filter: &FilterState, events: &[crate::db::EventRow]) -> Vec<usize> {
    if filter.input.is_empty() {
        return (0..events.len()).collect();
    }
    events
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let searchable = format!(
                "{} {} {} {}",
                e.event_type,
                e.tool_name.as_deref().unwrap_or(""),
                e.session_id,
                e.timestamp,
            );
            filter.matches(&searchable)
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{EventRow, SessionRow};

    #[test]
    fn test_matches_case_insensitive() {
        let mut f = FilterState::new();
        f.input = "bash".to_string();
        assert!(f.matches("PreToolUse Bash"));
        assert!(f.matches("BASH command"));
        assert!(!f.matches("Read file"));
    }

    #[test]
    fn test_matches_empty_input() {
        let f = FilterState::new();
        assert!(f.matches("anything"));
    }

    #[test]
    fn test_push_delete_char() {
        let mut f = FilterState::new();
        f.push_char('h');
        f.push_char('i');
        assert_eq!(f.input, "hi");
        f.delete_char();
        assert_eq!(f.input, "h");
        f.delete_char();
        assert_eq!(f.input, "");
        f.delete_char(); // no panic on empty
        assert_eq!(f.input, "");
    }

    #[test]
    fn test_activate_deactivate() {
        let mut f = FilterState::new();
        f.input = "old".to_string();
        f.activate();
        assert!(f.active);
        assert!(f.input.is_empty());
        f.push_char('n');
        f.deactivate();
        assert!(!f.active);
        assert!(f.input.is_empty());
    }

    #[test]
    fn test_filter_sessions() {
        let sessions = vec![
            SessionRow {
                session_id: "sess-abc".to_string(),
                first_seen: "2025-06-01T10:00:00Z".to_string(),
                last_seen: "2025-06-01T12:00:00Z".to_string(),
                cwd: Some("/home/user/project".to_string()),
                event_count: 10,
            },
            SessionRow {
                session_id: "sess-xyz".to_string(),
                first_seen: "2025-06-02T10:00:00Z".to_string(),
                last_seen: "2025-06-02T12:00:00Z".to_string(),
                cwd: Some("/tmp".to_string()),
                event_count: 5,
            },
        ];

        let mut f = FilterState::new();
        f.input = "project".to_string();
        let indices = filter_sessions(&f, &sessions);
        assert_eq!(indices, vec![0]);

        f.input = "xyz".to_string();
        let indices = filter_sessions(&f, &sessions);
        assert_eq!(indices, vec![1]);

        f.input = "".to_string();
        let indices = filter_sessions(&f, &sessions);
        assert_eq!(indices, vec![0, 1]);
    }

    #[test]
    fn test_filter_events() {
        let events = vec![
            EventRow {
                id: 1,
                timestamp: "2025-06-01T10:00:00Z".to_string(),
                session_id: "s1".to_string(),
                event_type: "PreToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                tool_input: None,
                tool_response: None,
                cwd: None,
                permission_mode: None,
                raw_payload: "{}".to_string(),
            },
            EventRow {
                id: 2,
                timestamp: "2025-06-01T10:01:00Z".to_string(),
                session_id: "s1".to_string(),
                event_type: "PostToolUse".to_string(),
                tool_name: Some("Read".to_string()),
                tool_input: None,
                tool_response: None,
                cwd: None,
                permission_mode: None,
                raw_payload: "{}".to_string(),
            },
        ];

        let mut f = FilterState::new();
        f.input = "bash".to_string();
        let indices = filter_events(&f, &events);
        assert_eq!(indices, vec![0]);

        f.input = "PostTool".to_string();
        let indices = filter_events(&f, &events);
        assert_eq!(indices, vec![1]);
    }
}
