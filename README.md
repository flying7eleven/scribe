# scribe

A Rust CLI tool that hooks into Claude Code's lifecycle events, silently logs every tool action to a local SQLite database, and provides a query interface for auditing.

## Quick Start

```bash
# Build from source
cargo build --release

# Register hooks with Claude Code (project-level)
scribe init --project

# Start using Claude Code — all events are now logged

# What happened in the last hour?
scribe query --since 1h

# What Bash commands were run this week?
scribe query --since 7d --tool Bash

# Session overview
scribe query sessions --since 7d

# Database health check
scribe stats
```

## Installation

```bash
# Build and install
cargo install --path .

# Or build in release mode
cargo build --release
# Binary is at target/release/scribe
```

The binary bundles SQLite (via `sqlx-sqlite` with the `bundled` feature), so there are no system dependencies.

## Subcommands

### `scribe log`

The hot path. Called by Claude Code as a command hook for every matching event. Reads hook JSON from stdin, extracts fields, and inserts into SQLite.

```bash
echo '{"session_id":"...","hook_event_name":"PreToolUse",...}' | scribe log
```

- Always exits 0 (never blocks Claude Code)
- Errors go to stderr
- Target latency: < 10ms (stdin read + DB insert + exit)
- Auto-retention: when `retention` is configured, periodically deletes expired events

### `scribe query`

Browse the audit log with filters and multiple output formats.

```bash
scribe query                          # Recent events (table)
scribe query --since 1h               # Last hour
scribe query --since 2025-06-01       # Since a date
scribe query --tool Bash --limit 20   # Bash commands, max 20
scribe query --search "rm -rf"        # Search in tool_input
scribe query --session abc123         # Specific session
scribe query --event PreToolUse       # Specific event type
scribe query --json                   # JSON Lines output
scribe query --csv > export.csv       # CSV export
```

**Session summary:**

```bash
scribe query sessions                 # All sessions
scribe query sessions --since 7d      # Recent sessions
scribe query sessions --json          # JSON Lines with full IDs
```

**Filter flags:** `--since`, `--until`, `--session`, `--event`, `--tool`, `--search`, `--limit`, `--json`, `--csv`

### `scribe init`

Generate the Claude Code `settings.json` hook configuration.

```bash
scribe init                  # Print JSON to stdout
scribe init --project        # Write/merge to .claude/settings.json
scribe init --global         # Write/merge to ~/.claude/settings.json
```

Registers `scribe log` for all 21 supported hook events. When merging into an existing file, preserves all non-scribe hooks and settings. Safe to re-run (idempotent).

### `scribe retain`

Delete events older than a given duration and clean up orphaned sessions.

```bash
scribe retain 90d            # Delete events older than 90 days
scribe retain 30d            # Delete events older than 30 days
scribe retain 1w             # Delete events older than 1 week
```

Runs deletion + orphan cleanup in a single transaction. Reclaims disk space via `PRAGMA incremental_vacuum`.

### `scribe stats`

Show database metrics at a glance.

```bash
scribe stats
```

```
Database:  /home/user/.claude/scribe.db
Size:      1.2 MB
Events:    12,847
Sessions:  42
Oldest:    2025-03-15 08:22:01
Newest:    2025-06-23 17:45:33
```

### `scribe completions`

Generate shell completion scripts.

```bash
scribe completions bash > ~/.local/share/bash-completion/completions/scribe
scribe completions zsh > ~/.zfunc/_scribe
scribe completions fish > ~/.config/fish/completions/scribe.fish
```

Supports: `bash`, `zsh`, `fish`, `elvish`, `powershell`.

## Configuration

### Database Path

Resolved with 4-layer precedence (highest wins):

1. `--db <path>` CLI flag
2. `SCRIBE_DB` environment variable
3. Config file `db_path`
4. Default: `~/.claude/scribe.db`

### Config File

Optional config at `~/.config/claude-scribe/config.toml`:

```toml
# Database path (overrides default, overridden by --db and SCRIBE_DB)
db_path = "/home/user/audit/scribe.db"

# Auto-retention: delete events older than this duration
# When set, `scribe log` periodically enforces this
retention = "90d"

# How often to check for expired events (default: 24h)
retention_check_interval = "24h"

# Default query limit (overrides the compiled default of 50)
default_query_limit = 100
```

All fields are optional. A missing config file is normal (not an error).

## Hook Events

scribe logs 21 of 22 Claude Code hook events:

| Event | Matcher | Category |
|-------|---------|----------|
| PreToolUse | tool name | Tool |
| PostToolUse | tool name | Tool |
| PostToolUseFailure | tool name | Tool |
| PermissionRequest | tool name | Tool |
| UserPromptSubmit | - | User |
| SessionStart | session source | Session |
| SessionEnd | exit reason | Session |
| SubagentStart | agent type | Agent |
| SubagentStop | agent type | Agent |
| Stop | - | Stop |
| StopFailure | error type | Stop |
| Notification | notification type | System |
| PreCompact | trigger | Compact |
| PostCompact | trigger | Compact |
| InstructionsLoaded | load reason | Config |
| ConfigChange | config source | Config |
| WorktreeRemove | - | Worktree |
| Elicitation | MCP server | Elicit |
| ElicitationResult | MCP server | Elicit |
| TeammateIdle | - | Team |
| TaskCompleted | - | Team |

**WorktreeCreate** is intentionally excluded (its stdout is used for worktree path communication).

## Database Schema

SQLite with WAL mode, stored at `~/.claude/scribe.db` by default.

**`events`** — one row per hook invocation:
`id`, `timestamp`, `session_id`, `event_type`, `tool_name`, `tool_input`, `tool_response`, `cwd`, `permission_mode`, `raw_payload`

**`sessions`** — one row per session (auto-upserted):
`session_id`, `first_seen`, `last_seen`, `cwd`, `event_count`

**`_metadata`** — internal key-value store (auto-retention tracking)

Indexed on: `session_id`, `event_type`, `tool_name`, `timestamp`.

## Project Structure

```
src/
  main.rs              # CLI dispatch + tokio runtime
  db.rs                # SQLite connection, migrations, queries
  models.rs            # HookInput struct (serde deserialization)
  config.rs            # Config file loading
  cmd_log.rs           # `log` subcommand handler
  cmd_query.rs         # `query` subcommand handler + formatters
  cmd_init.rs          # `init` subcommand handler + merge logic
  cmd_retain.rs        # `retain` subcommand handler
  cmd_stats.rs         # `stats` subcommand handler
  cmd_completions.rs   # `completions` subcommand handler
migrations/
  20250101000000_initial.sql    # events + sessions tables
  20250201000000_metadata.sql   # _metadata table
tests/
  log_integration.rs
  init_integration.rs
  query_integration.rs
  retain_integration.rs
  stats_integration.rs
  completions_integration.rs
```

## Development

```bash
cargo build              # Build
cargo test               # Run all 182 tests
cargo clippy             # Lint
cargo fmt --check        # Check formatting
```

## Design Constraints

- **Always exit 0** on `scribe log` — never block Claude Code
- **< 10ms** target latency for the log hot path
- **WAL mode** for concurrent writes from multiple sessions
- **Resilience-first** JSON parsing — unknown fields are silently ignored
- **`raw_payload`** stores the complete original JSON for lossless auditing

## License

[MIT](LICENSE)
