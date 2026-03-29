use serde::Deserialize;

/// Hook event payload from Claude Code.
///
/// Flat struct with `Option<T>` for all event-specific fields. Only the common
/// fields (`session_id`, `hook_event_name`, `cwd`) are required; everything
/// else is optional. Unknown fields are silently ignored by serde.
#[derive(Default, Deserialize)]
#[serde(default)]
pub struct HookInput {
    // ── Common fields (present in all events) ──
    pub session_id: String,
    pub hook_event_name: String,
    pub cwd: String,
    pub permission_mode: Option<String>,
    pub transcript_path: Option<String>,

    // ── Tool events (PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest) ──
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
    pub tool_response: Option<serde_json::Value>,
    pub tool_use_id: Option<String>,

    // ── UserPromptSubmit ──
    pub prompt: Option<String>,

    // ── Stop / SubagentStop ──
    pub stop_hook_active: Option<bool>,
    pub last_assistant_message: Option<String>,

    // ── StopFailure / PostToolUseFailure ──
    pub error: Option<String>,
    pub error_details: Option<String>,

    // ── Common (subagent/agent context) ──
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,

    // ── SubagentStop extras ──
    pub agent_transcript_path: Option<String>,

    // ── SessionStart / ConfigChange ──
    pub source: Option<String>,
    pub model: Option<String>,

    // ── SessionEnd ──
    pub reason: Option<String>,

    // ── PostToolUseFailure ──
    pub is_interrupt: Option<bool>,

    // ── Notification ──
    pub message: Option<String>,
    pub title: Option<String>,
    pub notification_type: Option<String>,

    // ── PreCompact / PostCompact ──
    pub trigger: Option<String>,
    pub custom_instructions: Option<String>,
    pub compact_summary: Option<String>,

    // ── InstructionsLoaded ──
    pub file_path: Option<String>,
    pub memory_type: Option<String>,
    pub load_reason: Option<String>,
    pub globs: Option<Vec<String>>,
    pub trigger_file_path: Option<String>,
    pub parent_file_path: Option<String>,

    // ── WorktreeRemove ──
    pub worktree_path: Option<String>,

    // ── Elicitation / ElicitationResult ──
    pub elicitation_id: Option<String>,
    pub mcp_server_name: Option<String>,
    pub mode: Option<String>,
    pub url: Option<String>,
    pub requested_schema: Option<serde_json::Value>,
    pub action: Option<String>,
    pub content: Option<serde_json::Value>,

    // ── TeammateIdle / TaskCompleted ──
    pub teammate_name: Option<String>,
    pub team_name: Option<String>,

    // ── TaskCompleted ──
    pub task_id: Option<String>,
    pub task_subject: Option<String>,
    pub task_description: Option<String>,

    // ── PermissionRequest (extra) ──
    pub permission_suggestions: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> HookInput {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn test_pre_tool_use() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_input":{"command":"ls"}}"#,
        );
        assert_eq!(h.hook_event_name, "PreToolUse");
        assert_eq!(h.tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn test_post_tool_use() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PostToolUse","cwd":"/tmp","tool_name":"Bash","tool_response":{"output":"file.txt"}}"#,
        );
        assert_eq!(h.hook_event_name, "PostToolUse");
        assert!(h.tool_response.is_some());
    }

    #[test]
    fn test_post_tool_use_failure() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PostToolUseFailure","cwd":"/tmp","tool_name":"Bash","error":"timeout","is_interrupt":true}"#,
        );
        assert_eq!(h.error.as_deref(), Some("timeout"));
        assert_eq!(h.is_interrupt, Some(true));
    }

    #[test]
    fn test_user_prompt_submit() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/tmp","prompt":"hello"}"#,
        );
        assert_eq!(h.prompt.as_deref(), Some("hello"));
    }

    #[test]
    fn test_permission_request() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PermissionRequest","cwd":"/tmp","tool_name":"Bash","permission_suggestions":{"allow":true}}"#,
        );
        assert!(h.permission_suggestions.is_some());
    }

    #[test]
    fn test_session_start() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"SessionStart","cwd":"/tmp","source":"startup","model":"claude-sonnet-4-5-20250514"}"#,
        );
        assert_eq!(h.source.as_deref(), Some("startup"));
        assert_eq!(h.model.as_deref(), Some("claude-sonnet-4-5-20250514"));
    }

    #[test]
    fn test_session_end() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"SessionEnd","cwd":"/tmp","reason":"clear"}"#,
        );
        assert_eq!(h.reason.as_deref(), Some("clear"));
    }

    #[test]
    fn test_subagent_start() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"SubagentStart","cwd":"/tmp","agent_id":"a1","agent_type":"Bash"}"#,
        );
        assert_eq!(h.agent_id.as_deref(), Some("a1"));
        assert_eq!(h.agent_type.as_deref(), Some("Bash"));
    }

    #[test]
    fn test_subagent_stop() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"SubagentStop","cwd":"/tmp","agent_id":"a1","agent_transcript_path":"/path","stop_hook_active":true,"last_assistant_message":"done"}"#,
        );
        assert_eq!(h.agent_transcript_path.as_deref(), Some("/path"));
        assert_eq!(h.stop_hook_active, Some(true));
    }

    #[test]
    fn test_stop() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"Stop","cwd":"/tmp","stop_hook_active":false,"last_assistant_message":"bye"}"#,
        );
        assert_eq!(h.last_assistant_message.as_deref(), Some("bye"));
    }

    #[test]
    fn test_stop_failure() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"StopFailure","cwd":"/tmp","error":"rate_limit","error_details":"429"}"#,
        );
        assert_eq!(h.error.as_deref(), Some("rate_limit"));
        assert_eq!(h.error_details.as_deref(), Some("429"));
    }

    #[test]
    fn test_notification() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"Notification","cwd":"/tmp","message":"hi","title":"Alert","notification_type":"permission_prompt"}"#,
        );
        assert_eq!(h.notification_type.as_deref(), Some("permission_prompt"));
    }

    #[test]
    fn test_pre_compact() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PreCompact","cwd":"/tmp","trigger":"auto","custom_instructions":"keep it short"}"#,
        );
        assert_eq!(h.trigger.as_deref(), Some("auto"));
        assert_eq!(h.custom_instructions.as_deref(), Some("keep it short"));
    }

    #[test]
    fn test_post_compact() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PostCompact","cwd":"/tmp","trigger":"manual","compact_summary":"summarized"}"#,
        );
        assert_eq!(h.compact_summary.as_deref(), Some("summarized"));
    }

    #[test]
    fn test_instructions_loaded() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"InstructionsLoaded","cwd":"/tmp","file_path":"/a/CLAUDE.md","memory_type":"project","load_reason":"session_start","globs":["*.md"],"trigger_file_path":"/b","parent_file_path":"/c"}"#,
        );
        assert_eq!(h.file_path.as_deref(), Some("/a/CLAUDE.md"));
        assert_eq!(h.globs.as_ref().unwrap(), &vec!["*.md".to_string()]);
    }

    #[test]
    fn test_config_change() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"ConfigChange","cwd":"/tmp","source":"user_settings","file_path":"/settings.json"}"#,
        );
        assert_eq!(h.source.as_deref(), Some("user_settings"));
    }

    #[test]
    fn test_worktree_remove() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"WorktreeRemove","cwd":"/tmp","worktree_path":"/worktree"}"#,
        );
        assert_eq!(h.worktree_path.as_deref(), Some("/worktree"));
    }

    #[test]
    fn test_elicitation() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"Elicitation","cwd":"/tmp","elicitation_id":"e1","mcp_server_name":"srv","mode":"form","message":"fill this","requested_schema":{"type":"object"}}"#,
        );
        assert_eq!(h.elicitation_id.as_deref(), Some("e1"));
        assert_eq!(h.mode.as_deref(), Some("form"));
    }

    #[test]
    fn test_elicitation_result() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"ElicitationResult","cwd":"/tmp","elicitation_id":"e1","action":"accept","content":{"field":"value"}}"#,
        );
        assert_eq!(h.action.as_deref(), Some("accept"));
        assert!(h.content.is_some());
    }

    #[test]
    fn test_teammate_idle() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"TeammateIdle","cwd":"/tmp","teammate_name":"bob","team_name":"alpha"}"#,
        );
        assert_eq!(h.teammate_name.as_deref(), Some("bob"));
    }

    #[test]
    fn test_task_completed() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"TaskCompleted","cwd":"/tmp","task_id":"t1","task_subject":"fix bug","task_description":"fixed it","teammate_name":"bob","team_name":"alpha"}"#,
        );
        assert_eq!(h.task_id.as_deref(), Some("t1"));
        assert_eq!(h.task_subject.as_deref(), Some("fix bug"));
    }

    #[test]
    fn test_cwd_changed() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"CwdChanged","cwd":"/new/working/dir"}"#,
        );
        assert_eq!(h.hook_event_name, "CwdChanged");
        assert_eq!(h.cwd, "/new/working/dir");
    }

    #[test]
    fn test_task_created() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"TaskCreated","cwd":"/tmp","task_id":"t1","task_subject":"implement feature","task_description":"add auth","teammate_name":"bob","team_name":"alpha"}"#,
        );
        assert_eq!(h.hook_event_name, "TaskCreated");
        assert_eq!(h.task_id.as_deref(), Some("t1"));
        assert_eq!(h.task_subject.as_deref(), Some("implement feature"));
        assert_eq!(h.task_description.as_deref(), Some("add auth"));
    }

    #[test]
    fn test_unknown_fields_ignored() {
        let h = parse(
            r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","brand_new_field":"surprise","another":42}"#,
        );
        assert_eq!(h.hook_event_name, "PreToolUse");
    }

    #[test]
    fn test_missing_optional_fields() {
        let h = parse(r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp"}"#);
        assert!(h.tool_name.is_none());
        assert!(h.tool_input.is_none());
        assert!(h.permission_mode.is_none());
    }
}
