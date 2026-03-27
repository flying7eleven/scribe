-- Policy enforcement tables (E009)

CREATE TABLE IF NOT EXISTS classifications (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    event_id      INTEGER,
    tool_name     TEXT    NOT NULL,
    input_pattern TEXT    NOT NULL,
    risk_level    TEXT    NOT NULL,  -- 'safe' | 'risky' | 'dangerous'
    reason        TEXT    NOT NULL,
    heuristic     TEXT    NOT NULL,
    FOREIGN KEY (event_id) REFERENCES events(id) ON DELETE SET NULL
);

CREATE INDEX idx_classifications_event    ON classifications(event_id);
CREATE INDEX idx_classifications_risk     ON classifications(risk_level);
CREATE INDEX idx_classifications_tool     ON classifications(tool_name);

CREATE TABLE IF NOT EXISTS rules (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    created_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    updated_at    TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    tool_pattern  TEXT    NOT NULL,
    input_pattern TEXT,
    action        TEXT    NOT NULL,   -- 'allow' | 'deny'
    reason        TEXT    NOT NULL,
    priority      INTEGER NOT NULL DEFAULT 0,
    enabled       INTEGER NOT NULL DEFAULT 1,
    source        TEXT    NOT NULL DEFAULT 'user'
);

CREATE INDEX idx_rules_enabled ON rules(enabled);

CREATE TABLE IF NOT EXISTS enforcements (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp     TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    session_id    TEXT    NOT NULL,
    tool_name     TEXT    NOT NULL,
    tool_input    TEXT,
    rule_id       INTEGER,
    action        TEXT    NOT NULL,   -- 'allowed' | 'denied'
    reason        TEXT,
    evaluation_ms REAL,
    FOREIGN KEY (rule_id) REFERENCES rules(id) ON DELETE SET NULL
);

CREATE INDEX idx_enforcements_session  ON enforcements(session_id);
CREATE INDEX idx_enforcements_ts       ON enforcements(timestamp);
CREATE INDEX idx_enforcements_action   ON enforcements(action);
