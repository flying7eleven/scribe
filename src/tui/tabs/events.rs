use crate::db::{self, EventRow};
use sqlx::SqlitePool;

/// Detail view mode for the expanded event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailMode {
    Structured,
    RawJson,
}

/// State for the Events tab.
pub struct EventsState {
    pub events: Vec<EventRow>,
    pub selected: usize,
    pub expanded: Option<usize>,
    pub detail_mode: DetailMode,
    pub session_filter: Option<String>,
    pub loaded: bool,
    /// Cached detail for the currently expanded event: (event_id, detail).
    pub cached_detail: Option<(i64, Option<db::EventDetail>)>,
}

impl EventsState {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            selected: 0,
            expanded: None,
            detail_mode: DetailMode::Structured,
            session_filter: None,
            loaded: false,
            cached_detail: None,
        }
    }

    /// Load events from the database.
    pub async fn load(
        &mut self,
        pool: &SqlitePool,
        since: Option<&str>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let filter = db::EventFilter {
            since: since.map(String::from),
            until: None,
            session_id: self.session_filter.clone(),
            event_type: None,
            tool_name: None,
            search: None,
            account: None,
            limit: 500,
        };
        self.events = db::query_events(pool, &filter).await?;
        self.selected = 0;
        self.expanded = None;
        self.loaded = true;
        Ok(())
    }

    /// Move selection down (wraps).
    pub fn next(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.events.len();
        // Collapse detail when moving
        self.expanded = None;
        self.cached_detail = None;
    }

    /// Move selection up (wraps).
    pub fn prev(&mut self) {
        if self.events.is_empty() {
            return;
        }
        self.selected = (self.selected + self.events.len() - 1) % self.events.len();
        self.expanded = None;
        self.cached_detail = None;
    }

    /// Jump to top.
    pub fn top(&mut self) {
        self.selected = 0;
        self.expanded = None;
        self.cached_detail = None;
    }

    /// Jump to bottom.
    pub fn bottom(&mut self) {
        if !self.events.is_empty() {
            self.selected = self.events.len() - 1;
        }
        self.expanded = None;
        self.cached_detail = None;
    }

    /// Toggle expand/collapse on the selected row.
    /// Returns `true` if an event was newly expanded (caller should fetch detail).
    pub fn toggle_expand(&mut self) -> bool {
        if self.events.is_empty() {
            return false;
        }
        if self.expanded == Some(self.selected) {
            self.expanded = None;
            self.cached_detail = None;
            false
        } else {
            self.expanded = Some(self.selected);
            self.cached_detail = None;
            true
        }
    }

    /// Toggle between Structured and RawJson detail modes.
    pub fn toggle_detail_mode(&mut self) {
        self.detail_mode = match self.detail_mode {
            DetailMode::Structured => DetailMode::RawJson,
            DetailMode::RawJson => DetailMode::Structured,
        };
    }

    /// Clear the session filter and mark as unloaded (forces reload).
    pub fn clear_session_filter(&mut self) {
        self.session_filter = None;
        self.loaded = false;
        self.expanded = None;
        self.cached_detail = None;
    }

    /// Set session filter (from Sessions tab drill-down).
    pub fn set_session_filter(&mut self, session_id: String) {
        self.session_filter = Some(session_id);
        self.loaded = false;
        self.expanded = None;
        self.cached_detail = None;
    }
}

/// Format an EventRow as structured key-value lines for the detail view.
pub fn format_structured_detail(event: &EventRow) -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(format!(
        "  Timestamp:     {}",
        crate::format::format_timestamp(&event.timestamp)
    ));
    lines.push(format!("  Event Type:    {}", event.event_type));
    if let Some(ref tool) = event.tool_name {
        lines.push(format!("  Tool Name:     {tool}"));
    }
    lines.push(format!("  Session:       {}", &event.session_id));
    if let Some(ref cwd) = event.cwd {
        lines.push(format!("  CWD:           {cwd}"));
    }
    if let Some(ref mode) = event.permission_mode {
        lines.push(format!("  Permission:    {mode}"));
    }
    if let Some(ref input) = event.tool_input {
        lines.push("  Tool Input:".to_string());
        for line in pretty_json_truncated(input, 10) {
            lines.push(format!("    {line}"));
        }
    }
    if let Some(ref response) = event.tool_response {
        lines.push("  Tool Response:".to_string());
        for line in pretty_json_truncated(response, 10) {
            lines.push(format!("    {line}"));
        }
    }

    lines
}

/// Truncate a string to max_len chars, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len])
    }
}

/// Format event-type-specific detail fields as display lines.
/// Only non-None fields are included.
pub fn format_detail_lines(detail: &db::EventDetail) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(String::new());
    lines.push("  ── Detail ──".to_string());
    match detail {
        db::EventDetail::Tool(t) => {
            if let Some(ref v) = t.tool_use_id {
                lines.push(format!("  Tool Use ID:   {v}"));
            }
            if let Some(ref v) = t.error {
                lines.push(format!("  Error:         {v}"));
            }
            if let Some(ref v) = t.error_details {
                lines.push(format!("  Error Details: {v}"));
            }
            if let Some(v) = t.is_interrupt {
                lines.push(format!("  Is Interrupt:  {v}"));
            }
            if let Some(ref v) = t.permission_suggestions {
                lines.push(format!("  Permissions:   {v}"));
            }
        }
        db::EventDetail::Stop(s) => {
            format_stop_lines(&mut lines, s);
        }
        db::EventDetail::Session(s) => {
            if let Some(ref v) = s.source {
                lines.push(format!("  Source:        {v}"));
            }
            if let Some(ref v) = s.model {
                lines.push(format!("  Model:         {v}"));
            }
            if let Some(ref v) = s.reason {
                lines.push(format!("  Reason:        {v}"));
            }
            if let Some(ref v) = s.file_path {
                lines.push(format!("  File Path:     {v}"));
            }
        }
        db::EventDetail::Agent(a) => {
            format_agent_lines(&mut lines, a);
        }
        db::EventDetail::Notification(n) => {
            if let Some(ref v) = n.notification_type {
                lines.push(format!("  Type:          {v}"));
            }
            if let Some(ref v) = n.title {
                lines.push(format!("  Title:         {v}"));
            }
            if let Some(ref v) = n.message {
                lines.push(format!("  Message:       {v}"));
            }
            if let Some(ref v) = n.elicitation_id {
                lines.push(format!("  Elicitation:   {v}"));
            }
            if let Some(ref v) = n.mcp_server_name {
                lines.push(format!("  MCP Server:    {v}"));
            }
            if let Some(ref v) = n.mode {
                lines.push(format!("  Mode:          {v}"));
            }
            if let Some(ref v) = n.url {
                lines.push(format!("  URL:           {v}"));
            }
            if let Some(ref v) = n.action {
                lines.push(format!("  Action:        {v}"));
            }
        }
        db::EventDetail::Compact(c) => {
            if let Some(ref v) = c.trigger {
                lines.push(format!("  Trigger:       {v}"));
            }
            if let Some(ref v) = c.custom_instructions {
                lines.push(format!("  Custom Instr:  {}", truncate_str(v, 200)));
            }
            if let Some(ref v) = c.compact_summary {
                lines.push(format!("  Summary:       {}", truncate_str(v, 200)));
            }
        }
        db::EventDetail::Instruction(i) => {
            if let Some(ref v) = i.file_path {
                lines.push(format!("  File Path:     {v}"));
            }
            if let Some(ref v) = i.memory_type {
                lines.push(format!("  Memory Type:   {v}"));
            }
            if let Some(ref v) = i.load_reason {
                lines.push(format!("  Load Reason:   {v}"));
            }
        }
        db::EventDetail::Team(t) => {
            if let Some(ref v) = t.teammate_name {
                lines.push(format!("  Teammate:      {v}"));
            }
            if let Some(ref v) = t.team_name {
                lines.push(format!("  Team:          {v}"));
            }
            if let Some(ref v) = t.task_id {
                lines.push(format!("  Task ID:       {v}"));
            }
        }
        db::EventDetail::Prompt(p) => {
            if let Some(ref v) = p.prompt {
                lines.push(format!("  Prompt:        {}", truncate_str(v, 500)));
            }
        }
        db::EventDetail::Worktree(w) => {
            if let Some(ref v) = w.worktree_path {
                lines.push(format!("  Worktree:      {v}"));
            }
        }
        db::EventDetail::StopAgent(stop, agent) => {
            format_stop_lines(&mut lines, stop);
            format_agent_lines(&mut lines, agent);
        }
    }
    lines
}

fn format_stop_lines(lines: &mut Vec<String>, s: &db::StopEventDetail) {
    if let Some(v) = s.stop_hook_active {
        lines.push(format!("  Hook Active:   {v}"));
    }
    if let Some(ref v) = s.last_assistant_message {
        lines.push(format!("  Last Message:  {}", truncate_str(v, 200)));
    }
    if let Some(ref v) = s.error {
        lines.push(format!("  Error:         {v}"));
    }
    if let Some(ref v) = s.error_details {
        lines.push(format!("  Error Details: {v}"));
    }
}

fn format_agent_lines(lines: &mut Vec<String>, a: &db::AgentEventDetail) {
    if let Some(ref v) = a.agent_id {
        lines.push(format!("  Agent ID:      {v}"));
    }
    if let Some(ref v) = a.agent_type {
        lines.push(format!("  Agent Type:    {v}"));
    }
    if let Some(ref v) = a.agent_transcript_path {
        lines.push(format!("  Transcript:    {v}"));
    }
}

/// Format raw_payload as pretty-printed JSON lines.
pub fn format_raw_json(event: &EventRow) -> Vec<String> {
    match serde_json::from_str::<serde_json::Value>(&event.raw_payload) {
        Ok(val) => serde_json::to_string_pretty(&val)
            .unwrap_or_else(|_| event.raw_payload.clone())
            .lines()
            .map(|l| format!("  {l}"))
            .collect(),
        Err(_) => event
            .raw_payload
            .lines()
            .map(|l| format!("  {l}"))
            .collect(),
    }
}

/// Pretty-print a JSON string, truncated to max_lines.
fn pretty_json_truncated(json_str: &str, max_lines: usize) -> Vec<String> {
    let pretty = match serde_json::from_str::<serde_json::Value>(json_str) {
        Ok(val) => serde_json::to_string_pretty(&val).unwrap_or_else(|_| json_str.to_string()),
        Err(_) => json_str.to_string(),
    };

    let lines: Vec<&str> = pretty.lines().collect();
    if lines.len() <= max_lines {
        lines.iter().map(|l| l.to_string()).collect()
    } else {
        let mut result: Vec<String> = lines[..max_lines].iter().map(|l| l.to_string()).collect();
        result.push(format!("... ({} more lines)", lines.len() - max_lines));
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mock_event(id: i64, tool: Option<&str>) -> EventRow {
        EventRow {
            id,
            timestamp: "2025-06-01T10:00:00.000Z".to_string(),
            session_id: "sess-abc123".to_string(),
            event_type: "PreToolUse".to_string(),
            tool_name: tool.map(String::from),
            tool_input: Some(r#"{"command":"ls -la"}"#.to_string()),
            tool_response: None,
            cwd: Some("/home/user/project".to_string()),
            permission_mode: Some("default".to_string()),
            raw_payload:
                r#"{"session_id":"sess-abc123","hook_event_name":"PreToolUse","tool_name":"Bash"}"#
                    .to_string(),
            origin_machine_id: None,
            account_id: "default".to_string(),
            account_email: None,
        }
    }

    #[test]
    fn test_toggle_expand() {
        let mut state = EventsState::new();
        state.events = vec![mock_event(1, Some("Bash")), mock_event(2, Some("Read"))];

        // Expand row 0
        state.toggle_expand();
        assert_eq!(state.expanded, Some(0));

        // Collapse row 0
        state.toggle_expand();
        assert_eq!(state.expanded, None);

        // Move to row 1 and expand
        state.next();
        assert_eq!(state.selected, 1);
        state.toggle_expand();
        assert_eq!(state.expanded, Some(1));
    }

    #[test]
    fn test_toggle_detail_mode() {
        let mut state = EventsState::new();
        assert_eq!(state.detail_mode, DetailMode::Structured);
        state.toggle_detail_mode();
        assert_eq!(state.detail_mode, DetailMode::RawJson);
        state.toggle_detail_mode();
        assert_eq!(state.detail_mode, DetailMode::Structured);
    }

    #[test]
    fn test_clear_session_filter() {
        let mut state = EventsState::new();
        state.session_filter = Some("sess-123".to_string());
        state.loaded = true;
        state.expanded = Some(0);

        state.clear_session_filter();
        assert!(state.session_filter.is_none());
        assert!(!state.loaded);
        assert!(state.expanded.is_none());
    }

    #[test]
    fn test_set_session_filter() {
        let mut state = EventsState::new();
        state.loaded = true;

        state.set_session_filter("sess-456".to_string());
        assert_eq!(state.session_filter.as_deref(), Some("sess-456"));
        assert!(!state.loaded);
    }

    #[test]
    fn test_navigation_collapses_detail() {
        let mut state = EventsState::new();
        state.events = vec![mock_event(1, Some("Bash")), mock_event(2, Some("Read"))];
        state.toggle_expand();
        assert_eq!(state.expanded, Some(0));

        state.next(); // moving collapses
        assert!(state.expanded.is_none());
    }

    #[test]
    fn test_structured_detail_contains_fields() {
        let event = mock_event(1, Some("Bash"));
        let lines = format_structured_detail(&event);
        let joined = lines.join("\n");

        assert!(joined.contains("Timestamp:"));
        assert!(joined.contains("Event Type:"));
        assert!(joined.contains("Tool Name:"));
        assert!(joined.contains("Bash"));
        assert!(joined.contains("Session:"));
        assert!(joined.contains("CWD:"));
        assert!(joined.contains("Tool Input:"));
    }

    #[test]
    fn test_raw_json_format() {
        let event = mock_event(1, Some("Bash"));
        let lines = format_raw_json(&event);
        let joined = lines.join("\n");
        assert!(joined.contains("session_id"));
        assert!(joined.contains("hook_event_name"));
    }

    #[test]
    fn test_pretty_json_truncated() {
        let long_json =
            r#"{"a":1,"b":2,"c":3,"d":4,"e":5,"f":6,"g":7,"h":8,"i":9,"j":10,"k":11,"l":12}"#;
        let lines = pretty_json_truncated(long_json, 5);
        assert!(lines.len() <= 6); // 5 + "... (N more lines)"
        assert!(lines.last().unwrap().contains("more lines"));
    }
}
