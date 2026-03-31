use crate::db::{self, SessionRow};
use sqlx::SqlitePool;

/// State for the Sessions tab.
pub struct SessionsState {
    pub sessions: Vec<SessionRow>,
    pub selected: usize,
    pub loaded: bool,
}

impl SessionsState {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected: 0,
            loaded: false,
        }
    }

    /// Load sessions from the database.
    pub async fn load(
        &mut self,
        pool: &SqlitePool,
        since: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let filter = db::SessionFilter {
            since: since.map(String::from),
            limit: 500,
        };
        self.sessions = db::query_sessions(pool, &filter).await?;
        self.selected = 0;
        self.loaded = true;
        Ok(())
    }

    /// Move selection down (wraps).
    pub fn next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.sessions.len();
    }

    /// Move selection up (wraps).
    pub fn prev(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.selected = (self.selected + self.sessions.len() - 1) % self.sessions.len();
    }

    /// Jump to top.
    pub fn top(&mut self) {
        self.selected = 0;
    }

    /// Jump to bottom.
    pub fn bottom(&mut self) {
        if !self.sessions.is_empty() {
            self.selected = self.sessions.len() - 1;
        }
    }

    /// Returns the session_id of the currently selected row.
    pub fn selected_session_id(&self) -> Option<&str> {
        self.sessions
            .get(self.selected)
            .map(|s| s.session_id.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_sessions(n: usize) -> Vec<SessionRow> {
        (0..n)
            .map(|i| SessionRow {
                account_id: "default".to_string(),
                session_id: format!("sess-{i}"),
                first_seen: "2025-06-01T10:00:00Z".to_string(),
                last_seen: "2025-06-01T12:00:00Z".to_string(),
                cwd: Some("/tmp".to_string()),
                event_count: (i as i64 + 1) * 10,
                account_email: None,
            })
            .collect()
    }

    #[test]
    fn test_next_wraps() {
        let mut state = SessionsState::new();
        state.sessions = mock_sessions(3);
        assert_eq!(state.selected, 0);
        state.next();
        assert_eq!(state.selected, 1);
        state.next();
        assert_eq!(state.selected, 2);
        state.next(); // wraps
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_prev_wraps() {
        let mut state = SessionsState::new();
        state.sessions = mock_sessions(3);
        assert_eq!(state.selected, 0);
        state.prev(); // wraps to 2
        assert_eq!(state.selected, 2);
        state.prev();
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_next_empty() {
        let mut state = SessionsState::new();
        state.next(); // should not panic
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_top_bottom() {
        let mut state = SessionsState::new();
        state.sessions = mock_sessions(5);
        state.selected = 2;
        state.bottom();
        assert_eq!(state.selected, 4);
        state.top();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_selected_session_id() {
        let mut state = SessionsState::new();
        assert!(state.selected_session_id().is_none());

        state.sessions = mock_sessions(3);
        assert_eq!(state.selected_session_id(), Some("sess-0"));
        state.next();
        assert_eq!(state.selected_session_id(), Some("sess-1"));
    }
}
