use serde::Deserialize;

/// Hook event payload from Claude Code.
///
/// Minimal skeleton with common fields needed by `insert_event()`.
/// The full struct with all event-specific fields is implemented in E02.
#[allow(dead_code)] // Used by cmd_log — wired in by E02
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct HookInput {
    pub session_id: String,
    pub hook_event_name: String,
    pub cwd: String,
    pub permission_mode: Option<String>,
}
