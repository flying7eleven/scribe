# scribe

**Audit logger for Claude Code** — silently captures every tool call, session, and system event to a local SQLite database, then lets you query, visualize, and enforce policies on what happened.

## Why scribe?

Claude Code runs tools on your behalf — Bash commands, file writes, edits, web searches. Scribe gives you a complete, queryable audit trail of everything Claude does, without slowing it down.

- **Full visibility**: 21 hook events captured, from tool calls to session lifecycle
- **Zero friction**: One command to set up, then it runs invisibly in the background
- **Fast**: < 10ms logging latency — Claude Code never waits on scribe
- **Local & private**: Everything stays in a SQLite file on your machine
- **Rich querying**: Filter by time, tool, session, event type, or full-text search
- **Interactive dashboard**: A terminal UI for browsing sessions and watching events live

## Installation

```bash
# Build and install from source
cargo install --path .
```

No system dependencies required — SQLite is bundled.

To enable optional features:

```bash
cargo install --path . --features guard        # Policy enforcement
cargo install --path . --features sync         # Multi-machine sync
cargo install --path . --features guard,sync   # Both
```

## Getting Started

### 1. Register hooks with Claude Code

```bash
# Project-level (recommended)
scribe init --project

# Or globally for all projects
scribe init --global
```

This writes the necessary hook configuration to your Claude Code settings. Safe to re-run — it merges cleanly with existing settings.

### 2. Use Claude Code as usual

Every tool call, session start/end, and system event is now logged automatically.

### 3. Query your audit log

```bash
# What happened in the last hour?
scribe query --since 1h

# What Bash commands did Claude run today?
scribe query --since 1d --tool Bash

# Search for specific content
scribe query --search "rm -rf"

# Session overview for the past week
scribe query sessions --since 7d
```

## Commands

### `scribe query`

Browse the audit log with filters and multiple output formats.

```bash
scribe query                              # Recent events (table)
scribe query --since 1h                   # Last hour
scribe query --since 2025-06-01           # Since a specific date
scribe query --tool Bash --limit 20       # Filter by tool, limit results
scribe query --session abc123             # Specific session
scribe query --event PreToolUse           # Specific event type
scribe query --search "pattern"           # Search tool input
scribe query --json                       # JSON Lines output
scribe query --csv > export.csv           # CSV export
```

**Session summaries:**

```bash
scribe query sessions                     # All sessions
scribe query sessions --since 7d          # Recent sessions
scribe query sessions --json              # JSON Lines for scripting
```

### `scribe stats`

Dashboard with database metrics, top tools, activity histograms, and error summaries.

```bash
scribe stats                              # Full dashboard
scribe stats --since 7d                   # Stats for the past week
scribe stats --json                       # JSON output for scripting
```

### `scribe tui`

Interactive terminal UI with live event streaming.

```bash
scribe tui                                # Launch the TUI
scribe tui --since 7d                     # Pre-filter to recent data
```

**Tabs:**
- **Sessions** — browse all sessions with timestamps and event counts
- **Events** — searchable event browser
- **Stats** — live dashboard with all metrics
- **Live** — real-time event stream with auto-scroll

Press `?` for keybindings.

### `scribe retain`

Clean up old data and reclaim disk space.

```bash
scribe retain 90d                         # Delete events older than 90 days
scribe retain 30d                         # 30 days
scribe retain 1w                          # 1 week
```

### `scribe init`

Generate or update the Claude Code hook configuration.

```bash
scribe init                               # Print config to stdout
scribe init --project                     # Write to .claude/settings.json
scribe init --global                      # Write to ~/.claude/settings.json
```

### `scribe completions`

Generate shell completion scripts.

```bash
scribe completions bash > ~/.local/share/bash-completion/completions/scribe
scribe completions zsh > ~/.zfunc/_scribe
scribe completions fish > ~/.config/fish/completions/scribe.fish
```

Supports: bash, zsh, fish, elvish, powershell.

## Configuration

Scribe uses an optional config file at `~/.config/claude-scribe/config.toml`. It is created automatically on first interactive run.

```toml
# Custom database path (default: ~/.claude/scribe.db)
db_path = "/home/user/audit/scribe.db"

# Auto-delete events older than this duration
retention = "90d"

# How often to check for expired events (default: 24h)
retention_check_interval = "24h"

# Default query result limit (default: 50)
default_query_limit = 100

# Exclude stale sessions from average duration calculation
max_session_duration = "8h"
```

All fields are optional. A missing config file is perfectly fine.

### Database path precedence

| Priority | Source |
|----------|--------|
| 1 (highest) | `--db <path>` CLI flag |
| 2 | `SCRIBE_DB` environment variable |
| 3 | `db_path` in config file |
| 4 (default) | `~/.claude/scribe.db` |

## Guard: Policy Enforcement (optional)

When built with `--features guard`, scribe can enforce rules on Claude Code tool calls in real time.

```bash
# Initialize with guard hooks
scribe init --project --with-guard

# Add a policy rule
scribe policy add --name no-force-push \
  --tool Bash \
  --pattern "git push.*--force" \
  --action deny

# List active policies
scribe policy list

# Classify past tool calls by risk level
scribe classify --since 7d
scribe classify --risk dangerous --details
```

Guard uses a fail-open design — if scribe encounters an error, the tool call is allowed through so Claude Code is never blocked.

## Sync: Multi-Machine Audit Logs (optional)

When built with `--features sync`, scribe can synchronize audit events between machines over SSH — no server required. Events are end-to-end encrypted using [age](https://age-encryption.org/) X25519 keys.

```bash
cargo install --path . --features sync
```

### Setup

Both machines need scribe installed with the sync feature. Generate a keypair on each machine, then exchange public keys:

```bash
# On each machine: generate a keypair
scribe sync keypair generate

# Share the printed public key (age1...) with the other machine
scribe sync keypair show

# Add the other machine's public key
scribe sync keypair add workstation age1xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
```

> **SSH PATH note:** `scribe sync push/pull` invokes `scribe sync import/export` on the
> remote via a non-interactive SSH session. Non-interactive shells do **not** source
> `~/.profile` or `~/.bashrc`, so the `scribe` binary must be on the default PATH.
> If you installed via `cargo install`, add `$HOME/.cargo/bin` to your PATH in a file
> that is always sourced:
>
> - **zsh:** `~/.zshenv`
> - **bash:** `~/.bashrc` (before any interactive-only guard)

### Push & Pull

```bash
# Push local events to a remote machine
scribe sync push user@hostname

# Pull events from a remote machine
scribe sync pull user@hostname

# Only sync events from the last 7 days
scribe sync push user@hostname --since 7d
scribe sync pull user@hostname --since 7d
```

The `<remote>` argument accepts `user@hostname` or any host alias from your SSH config.

### Sync Status

```bash
scribe sync status
```

Shows your local machine ID, configured peers, events pending sync, and a log of recent sync operations.

### `scribe sync keypair`

```bash
scribe sync keypair generate [--force]   # Generate new keypair (--force to overwrite)
scribe sync keypair show                 # Print your public key
scribe sync keypair add <name> <key>     # Add a peer's public key
scribe sync keypair list                 # List all peers
scribe sync keypair remove <name>        # Remove a peer
```

### How it works

Sync uses SSH as transport and age for encryption:

1. **Push:** local `scribe sync export` pipes encrypted event bundles over SSH to `scribe sync import` on the remote
2. **Pull:** remote `scribe sync export` pipes encrypted event bundles back to local `scribe sync import`
3. Events are deduplicated on import — syncing the same data twice is safe
4. Each sync is logged with direction, event counts, and status

## Hook Events

Scribe captures 21 Claude Code hook events across these categories:

| Category | Events |
|----------|--------|
| **Tool** | PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest |
| **User** | UserPromptSubmit |
| **Session** | SessionStart, SessionEnd |
| **Agent** | SubagentStart, SubagentStop |
| **Stop** | Stop, StopFailure |
| **System** | Notification, ConfigChange, InstructionsLoaded |
| **Compact** | PreCompact, PostCompact |
| **Worktree** | WorktreeRemove |
| **Elicitation** | Elicitation, ElicitationResult |
| **Team** | TeammateIdle, TaskCompleted |

## Development

```bash
cargo build                               # Build
cargo test                                # Run tests
cargo clippy                              # Lint
cargo fmt --check                         # Check formatting

# Build with guard feature
cargo build --features guard
```

## License

[MIT](LICENSE)
