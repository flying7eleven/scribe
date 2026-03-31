-- Add account columns to events
ALTER TABLE events ADD COLUMN account_id TEXT NOT NULL DEFAULT 'default';
ALTER TABLE events ADD COLUMN account_email TEXT;

-- Index for account-filtered queries
CREATE INDEX IF NOT EXISTS idx_events_account ON events(account_id);

-- Update dedup index to include account_id
DROP INDEX IF EXISTS idx_events_dedup;
CREATE UNIQUE INDEX IF NOT EXISTS idx_events_dedup
    ON events(account_id, session_id, timestamp, event_type);

-- Recreate sessions table with composite PK (account_id, session_id).
-- SQLite does not support ALTER TABLE to change a primary key, so we
-- must recreate the table.
CREATE TABLE sessions_new (
    account_id    TEXT NOT NULL DEFAULT 'default',
    session_id    TEXT NOT NULL,
    first_seen    TEXT NOT NULL,
    last_seen     TEXT NOT NULL,
    cwd           TEXT,
    event_count   INTEGER NOT NULL DEFAULT 0,
    account_email TEXT,
    PRIMARY KEY (account_id, session_id)
);

INSERT INTO sessions_new (account_id, session_id, first_seen, last_seen, cwd, event_count)
    SELECT 'default', session_id, first_seen, last_seen, cwd, event_count FROM sessions;

DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;
