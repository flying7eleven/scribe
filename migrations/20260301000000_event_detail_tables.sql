-- Event detail tables (E010): normalize raw_payload fields into queryable columns.
-- Each table has a 1:1 relationship with events via event_id.
-- ON DELETE CASCADE ensures scribe retain cleanup propagates automatically.

-- Tier 1: High Value

-- tool_event_details: PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest
CREATE TABLE IF NOT EXISTS tool_event_details (
    event_id              INTEGER NOT NULL UNIQUE,
    tool_use_id           TEXT,
    error                 TEXT,
    error_details         TEXT,
    is_interrupt          INTEGER,
    permission_suggestions TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_tool_event_details_event ON tool_event_details(event_id);

-- stop_event_details: Stop, StopFailure, SubagentStop
CREATE TABLE IF NOT EXISTS stop_event_details (
    event_id               INTEGER NOT NULL UNIQUE,
    stop_hook_active       INTEGER,
    last_assistant_message TEXT,
    error                  TEXT,
    error_details          TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_stop_event_details_event ON stop_event_details(event_id);

-- session_event_details: SessionStart, SessionEnd, ConfigChange
CREATE TABLE IF NOT EXISTS session_event_details (
    event_id    INTEGER NOT NULL UNIQUE,
    source      TEXT,
    model       TEXT,
    reason      TEXT,
    file_path   TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_session_event_details_event ON session_event_details(event_id);

-- Tier 2: Medium Value

-- agent_event_details: SubagentStart, SubagentStop
CREATE TABLE IF NOT EXISTS agent_event_details (
    event_id              INTEGER NOT NULL UNIQUE,
    agent_id              TEXT,
    agent_type            TEXT,
    agent_transcript_path TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_agent_event_details_event ON agent_event_details(event_id);

-- notification_event_details: Notification, Elicitation, ElicitationResult
CREATE TABLE IF NOT EXISTS notification_event_details (
    event_id          INTEGER NOT NULL UNIQUE,
    notification_type TEXT,
    title             TEXT,
    message           TEXT,
    elicitation_id    TEXT,
    mcp_server_name   TEXT,
    mode              TEXT,
    url               TEXT,
    requested_schema  TEXT,
    action            TEXT,
    content           TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_notification_event_details_event ON notification_event_details(event_id);

-- compact_event_details: PreCompact, PostCompact
CREATE TABLE IF NOT EXISTS compact_event_details (
    event_id            INTEGER NOT NULL UNIQUE,
    trigger             TEXT,
    custom_instructions TEXT,
    compact_summary     TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_compact_event_details_event ON compact_event_details(event_id);

-- Tier 3: Low Value

-- instruction_event_details: InstructionsLoaded
CREATE TABLE IF NOT EXISTS instruction_event_details (
    event_id          INTEGER NOT NULL UNIQUE,
    file_path         TEXT,
    memory_type       TEXT,
    load_reason       TEXT,
    globs             TEXT,
    trigger_file_path TEXT,
    parent_file_path  TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_instruction_event_details_event ON instruction_event_details(event_id);

-- team_event_details: TeammateIdle, TaskCompleted
CREATE TABLE IF NOT EXISTS team_event_details (
    event_id         INTEGER NOT NULL UNIQUE,
    teammate_name    TEXT,
    team_name        TEXT,
    task_id          TEXT,
    task_subject     TEXT,
    task_description TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_team_event_details_event ON team_event_details(event_id);

-- prompt_event_details: UserPromptSubmit
CREATE TABLE IF NOT EXISTS prompt_event_details (
    event_id INTEGER NOT NULL UNIQUE,
    prompt   TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_prompt_event_details_event ON prompt_event_details(event_id);

-- worktree_event_details: WorktreeRemove
CREATE TABLE IF NOT EXISTS worktree_event_details (
    event_id      INTEGER NOT NULL UNIQUE,
    worktree_path TEXT,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE CASCADE
);
CREATE INDEX idx_worktree_event_details_event ON worktree_event_details(event_id);
