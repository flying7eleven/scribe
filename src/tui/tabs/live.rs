use std::collections::VecDeque;
use std::time::Instant;

use crate::db::EventRow;
use sqlx::{Row, SqlitePool};

const MAX_FEED_SIZE: usize = 200;

/// Live stats snapshot.
pub struct LiveStats {
    pub event_count: i64,
    pub session_count: i64,
}

/// State for the Live tab.
pub struct LiveState {
    pub last_seen_id: i64,
    pub feed: VecDeque<EventRow>,
    pub feed_scroll: usize,
    pub auto_scroll: bool,
    pub stats_snapshot: Option<LiveStats>,
    pub events_per_minute: f64,
    pub started_at: Instant,
    pub total_events_received: u64,
    pub initialized: bool,
}

impl LiveState {
    pub fn new() -> Self {
        Self {
            last_seen_id: 0,
            feed: VecDeque::new(),
            feed_scroll: 0,
            auto_scroll: true,
            stats_snapshot: None,
            events_per_minute: 0.0,
            started_at: Instant::now(),
            total_events_received: 0,
            initialized: false,
        }
    }

    /// Initialize by getting the current max ID (avoids flooding with history).
    pub async fn initialize(
        &mut self,
        pool: &SqlitePool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let row = sqlx::query("SELECT COALESCE(MAX(id), 0) as max_id FROM events")
            .fetch_one(pool)
            .await?;
        self.last_seen_id = row.get::<i64, _>("max_id");
        self.started_at = Instant::now();
        self.total_events_received = 0;
        self.events_per_minute = 0.0;
        self.initialized = true;
        Ok(())
    }

    /// Poll for new events since last_seen_id.
    pub async fn poll(&mut self, pool: &SqlitePool) -> Result<(), Box<dyn std::error::Error>> {
        // Fetch new events
        let rows = sqlx::query_as::<_, EventRow>(
            "SELECT id, timestamp, session_id, event_type, tool_name, tool_input, \
             tool_response, cwd, permission_mode, raw_payload \
             FROM events WHERE id > ? ORDER BY id ASC LIMIT 100",
        )
        .bind(self.last_seen_id)
        .fetch_all(pool)
        .await?;

        if let Some(last) = rows.last() {
            self.last_seen_id = last.id;
        }

        let new_count = rows.len() as u64;
        self.total_events_received += new_count;

        for event in rows {
            self.feed.push_back(event);
        }

        // Trim ring buffer
        while self.feed.len() > MAX_FEED_SIZE {
            self.feed.pop_front();
        }

        // Auto-scroll to bottom
        if self.auto_scroll && !self.feed.is_empty() {
            self.feed_scroll = self.feed.len().saturating_sub(1);
        }

        // Update events-per-minute rate
        let elapsed = self.started_at.elapsed().as_secs_f64();
        if elapsed > 0.0 {
            self.events_per_minute = self.total_events_received as f64 / (elapsed / 60.0);
        }

        // Refresh stats snapshot
        let stats_row = sqlx::query(
            "SELECT \
             (SELECT COUNT(*) FROM events) as event_count, \
             (SELECT COUNT(*) FROM sessions) as session_count",
        )
        .fetch_one(pool)
        .await?;

        self.stats_snapshot = Some(LiveStats {
            event_count: stats_row.get::<i64, _>("event_count"),
            session_count: stats_row.get::<i64, _>("session_count"),
        });

        Ok(())
    }

    /// Scroll up in the feed (disables auto-scroll).
    pub fn scroll_up(&mut self) {
        if self.feed_scroll > 0 {
            self.feed_scroll -= 1;
            self.auto_scroll = false;
        }
    }

    /// Scroll down in the feed.
    pub fn scroll_down(&mut self) {
        if !self.feed.is_empty() && self.feed_scroll < self.feed.len() - 1 {
            self.feed_scroll += 1;
        }
    }

    /// Jump to bottom and re-enable auto-scroll.
    pub fn scroll_to_bottom(&mut self) {
        if !self.feed.is_empty() {
            self.feed_scroll = self.feed.len() - 1;
        }
        self.auto_scroll = true;
    }

    /// Number of items in the feed.
    pub fn feed_len(&self) -> usize {
        self.feed.len()
    }

    /// Uptime as a human-readable string.
    pub fn uptime(&self) -> String {
        crate::format::format_duration(self.started_at.elapsed().as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ring_buffer_trimming() {
        let mut state = LiveState::new();

        // Add more than MAX_FEED_SIZE events
        for i in 0..250 {
            state.feed.push_back(EventRow {
                id: i,
                timestamp: "2025-06-01T10:00:00Z".to_string(),
                session_id: "s1".to_string(),
                event_type: "PreToolUse".to_string(),
                tool_name: Some("Bash".to_string()),
                tool_input: None,
                tool_response: None,
                cwd: None,
                permission_mode: None,
                raw_payload: "{}".to_string(),
            });
        }

        while state.feed.len() > MAX_FEED_SIZE {
            state.feed.pop_front();
        }

        assert_eq!(state.feed.len(), MAX_FEED_SIZE);
        // Oldest remaining should be id 50
        assert_eq!(state.feed.front().unwrap().id, 50);
    }

    #[test]
    fn test_auto_scroll_behavior() {
        let mut state = LiveState::new();
        assert!(state.auto_scroll);

        // Add some events
        for i in 0..10 {
            state.feed.push_back(EventRow {
                id: i,
                timestamp: "2025-06-01T10:00:00Z".to_string(),
                session_id: "s1".to_string(),
                event_type: "PreToolUse".to_string(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                cwd: None,
                permission_mode: None,
                raw_payload: "{}".to_string(),
            });
        }
        state.feed_scroll = 9;

        // Scrolling up disables auto-scroll
        state.scroll_up();
        assert!(!state.auto_scroll);
        assert_eq!(state.feed_scroll, 8);

        // scroll_to_bottom re-enables
        state.scroll_to_bottom();
        assert!(state.auto_scroll);
        assert_eq!(state.feed_scroll, 9);
    }

    #[test]
    fn test_events_per_minute() {
        let mut state = LiveState::new();
        state.total_events_received = 60;
        // Pretend 1 minute has elapsed
        let elapsed_secs = 60.0;
        state.events_per_minute = state.total_events_received as f64 / (elapsed_secs / 60.0);
        assert!((state.events_per_minute - 60.0).abs() < 0.01);
    }

    #[test]
    fn test_scroll_empty_feed() {
        let mut state = LiveState::new();
        state.scroll_up(); // should not panic
        state.scroll_down(); // should not panic
        state.scroll_to_bottom(); // should not panic
        assert_eq!(state.feed_scroll, 0);
    }
}
