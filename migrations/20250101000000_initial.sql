CREATE TABLE IF NOT EXISTS events (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    session_id    TEXT    NOT NULL,
    event_type    TEXT    NOT NULL,
    tool_name     TEXT,
    tool_input    TEXT,
    tool_response TEXT,
    cwd           TEXT,
    permission_mode TEXT,
    raw_payload   TEXT    NOT NULL
);

CREATE INDEX idx_events_session ON events(session_id);
CREATE INDEX idx_events_type    ON events(event_type);
CREATE INDEX idx_events_tool    ON events(tool_name);
CREATE INDEX idx_events_ts      ON events(timestamp);

CREATE TABLE IF NOT EXISTS sessions (
    session_id    TEXT PRIMARY KEY,
    first_seen    TEXT NOT NULL,
    last_seen     TEXT NOT NULL,
    cwd           TEXT,
    event_count   INTEGER NOT NULL DEFAULT 0
);
