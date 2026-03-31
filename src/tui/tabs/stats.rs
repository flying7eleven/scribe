use crate::db::{
    self, DbStats, DirCount, ErrorSummary, EventTypeCount, ModelSessionCount, ToolCount,
    ToolFailureCount,
};
use sqlx::SqlitePool;

/// State for the Stats tab.
pub struct StatsState {
    pub stats: Option<DbStats>,
    pub avg_duration: Option<f64>,
    pub tools: Vec<ToolCount>,
    pub event_types: Vec<EventTypeCount>,
    pub errors: Option<ErrorSummary>,
    pub dirs: Vec<DirCount>,
    pub activity: Vec<(String, i64)>,
    pub models: Vec<ModelSessionCount>,
    pub tool_failures: Vec<ToolFailureCount>,
    pub db_path: String,
    pub db_size: u64,
    pub scroll_offset: u16,
    pub total_lines: u16,
    pub loaded: bool,
}

impl StatsState {
    pub fn new() -> Self {
        Self {
            stats: None,
            avg_duration: None,
            tools: Vec::new(),
            event_types: Vec::new(),
            errors: None,
            dirs: Vec::new(),
            activity: Vec::new(),
            models: Vec::new(),
            tool_failures: Vec::new(),
            db_path: String::new(),
            db_size: 0,
            scroll_offset: 0,
            total_lines: 0,
            loaded: false,
        }
    }

    /// Load all stats from the database.
    pub async fn load(
        &mut self,
        pool: &SqlitePool,
        db_path: &str,
        since: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.stats = Some(db::get_stats(pool, since, None).await?);
        self.avg_duration = db::avg_session_duration(pool, since, None, None).await?;
        self.tools = db::top_tools(pool, since, 10, None).await?;
        self.event_types = db::event_type_breakdown(pool, since, None).await?;
        self.errors = Some(db::error_summary(pool, since, None).await?);
        self.dirs = db::top_directories(pool, since, 5, None).await?;
        let raw_activity = db::daily_activity(pool, since, None).await?;
        self.activity = crate::cmd_stats::fill_zero_days(&raw_activity);
        self.models = db::sessions_by_model(pool, since, None).await?;
        self.tool_failures = db::tool_failures_by_error(pool, since).await?;
        self.db_path = db_path.to_string();
        self.db_size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
        self.scroll_offset = 0;

        // Estimate total lines for scroll clamping
        let mut lines = 8u16; // header lines
        lines += self.tools.len() as u16 + 2;
        lines += self.event_types.len() as u16 + 2;
        lines += 4; // errors section
        lines += self.models.len() as u16 + 2;
        lines += self.tool_failures.len() as u16 + 2;
        lines += self.dirs.len() as u16 + 2;
        lines += self.activity.len() as u16 + 2;
        self.total_lines = lines;

        self.loaded = true;
        Ok(())
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_offset < self.total_lines.saturating_sub(1) {
            self.scroll_offset += 1;
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    pub fn scroll_top(&mut self) {
        self.scroll_offset = 0;
    }

    pub fn scroll_bottom(&mut self) {
        self.scroll_offset = self.total_lines.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_clamping() {
        let mut state = StatsState::new();
        state.total_lines = 10;

        state.scroll_down();
        assert_eq!(state.scroll_offset, 1);

        state.scroll_bottom();
        assert_eq!(state.scroll_offset, 9);

        // Can't scroll past bottom
        state.scroll_down();
        assert_eq!(state.scroll_offset, 9);

        state.scroll_top();
        assert_eq!(state.scroll_offset, 0);

        // Can't scroll above top
        state.scroll_up();
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_scroll_empty() {
        let mut state = StatsState::new();
        state.total_lines = 0;
        state.scroll_down();
        assert_eq!(state.scroll_offset, 0);
        state.scroll_bottom();
        assert_eq!(state.scroll_offset, 0);
    }
}
