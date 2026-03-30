-- Origin tracking: which machine generated each event
ALTER TABLE events ADD COLUMN origin_machine_id TEXT;

-- Sync metadata: track peers and sync state
CREATE TABLE IF NOT EXISTS sync_peers (
    machine_id    TEXT PRIMARY KEY,
    machine_name  TEXT NOT NULL,
    public_key    TEXT NOT NULL,
    first_synced  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    last_synced   TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE IF NOT EXISTS sync_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    timestamp       TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    peer_id         TEXT NOT NULL,
    direction       TEXT NOT NULL,  -- 'push' | 'pull'
    events_sent     INTEGER NOT NULL DEFAULT 0,
    events_received INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL,  -- 'success' | 'error'
    error_message   TEXT,
    FOREIGN KEY (peer_id) REFERENCES sync_peers(machine_id) ON DELETE CASCADE
);

-- Dedup index for merge operations
CREATE UNIQUE INDEX IF NOT EXISTS idx_events_dedup
    ON events(session_id, timestamp, event_type);
