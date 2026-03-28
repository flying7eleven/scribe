use sqlx::{Row, SqlitePool};

fn get_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|v| v.as_str()).map(String::from)
}

fn get_bool(v: &serde_json::Value, key: &str) -> Option<bool> {
    v.get(key).and_then(|v| v.as_bool())
}

fn get_json_str(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).map(|v| v.to_string())
}

struct Counts {
    tool: u64,
    stop: u64,
    session: u64,
    agent: u64,
    notification: u64,
    compact: u64,
    instruction: u64,
    team: u64,
    prompt: u64,
    worktree: u64,
    skipped: u64,
    parse_errors: u64,
}

impl Counts {
    fn new() -> Self {
        Self {
            tool: 0,
            stop: 0,
            session: 0,
            agent: 0,
            notification: 0,
            compact: 0,
            instruction: 0,
            team: 0,
            prompt: 0,
            worktree: 0,
            skipped: 0,
            parse_errors: 0,
        }
    }

    fn total_inserted(&self) -> u64 {
        self.tool
            + self.stop
            + self.session
            + self.agent
            + self.notification
            + self.compact
            + self.instruction
            + self.team
            + self.prompt
            + self.worktree
    }

    fn summary_line(&self) -> String {
        let mut parts = Vec::new();
        if self.tool > 0 {
            parts.push(format!("tool: {}", self.tool));
        }
        if self.stop > 0 {
            parts.push(format!("stop: {}", self.stop));
        }
        if self.session > 0 {
            parts.push(format!("session: {}", self.session));
        }
        if self.agent > 0 {
            parts.push(format!("agent: {}", self.agent));
        }
        if self.notification > 0 {
            parts.push(format!("notification: {}", self.notification));
        }
        if self.compact > 0 {
            parts.push(format!("compact: {}", self.compact));
        }
        if self.instruction > 0 {
            parts.push(format!("instruction: {}", self.instruction));
        }
        if self.team > 0 {
            parts.push(format!("team: {}", self.team));
        }
        if self.prompt > 0 {
            parts.push(format!("prompt: {}", self.prompt));
        }
        if self.worktree > 0 {
            parts.push(format!("worktree: {}", self.worktree));
        }
        parts.join(", ")
    }
}

struct EventRow {
    id: i64,
    event_type: String,
    raw_payload: String,
}

async fn insert_detail_for_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event_id: i64,
    event_type: &str,
    v: &serde_json::Value,
    counts: &mut Counts,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    match event_type {
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO tool_event_details (event_id, tool_use_id, error, error_details, is_interrupt, permission_suggestions) VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "tool_use_id").as_deref())
                .bind(get_str(v, "error").as_deref())
                .bind(get_str(v, "error_details").as_deref())
                .bind(get_bool(v, "is_interrupt"))
                .bind(get_json_str(v, "permission_suggestions").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.tool += 1;
        }
        "Stop" | "StopFailure" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO stop_event_details (event_id, stop_hook_active, last_assistant_message, error, error_details) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_bool(v, "stop_hook_active"))
                .bind(get_str(v, "last_assistant_message").as_deref())
                .bind(get_str(v, "error").as_deref())
                .bind(get_str(v, "error_details").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.stop += 1;
        }
        "SessionStart" | "SessionEnd" | "ConfigChange" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO session_event_details (event_id, source, model, reason, file_path) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "source").as_deref())
                .bind(get_str(v, "model").as_deref())
                .bind(get_str(v, "reason").as_deref())
                .bind(get_str(v, "file_path").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.session += 1;
        }
        "SubagentStop" => {
            // Dual insert: stop_event_details + agent_event_details
            if !dry_run {
                let stop_rows = sqlx::query(
                    "INSERT OR IGNORE INTO stop_event_details (event_id, stop_hook_active, last_assistant_message, error, error_details) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_bool(v, "stop_hook_active"))
                .bind(get_str(v, "last_assistant_message").as_deref())
                .bind(get_str(v, "error").as_deref())
                .bind(get_str(v, "error_details").as_deref())
                .execute(&mut **tx)
                .await?;
                let agent_rows = sqlx::query(
                    "INSERT OR IGNORE INTO agent_event_details (event_id, agent_id, agent_type, agent_transcript_path) VALUES (?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "agent_id").as_deref())
                .bind(get_str(v, "agent_type").as_deref())
                .bind(get_str(v, "agent_transcript_path").as_deref())
                .execute(&mut **tx)
                .await?;
                if stop_rows.rows_affected() == 0 && agent_rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.stop += 1;
            counts.agent += 1;
        }
        "SubagentStart" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO agent_event_details (event_id, agent_id, agent_type, agent_transcript_path) VALUES (?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "agent_id").as_deref())
                .bind(get_str(v, "agent_type").as_deref())
                .bind(get_str(v, "agent_transcript_path").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.agent += 1;
        }
        "Notification" | "Elicitation" | "ElicitationResult" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO notification_event_details (event_id, notification_type, title, message, elicitation_id, mcp_server_name, mode, url, requested_schema, action, content) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "notification_type").as_deref())
                .bind(get_str(v, "title").as_deref())
                .bind(get_str(v, "message").as_deref())
                .bind(get_str(v, "elicitation_id").as_deref())
                .bind(get_str(v, "mcp_server_name").as_deref())
                .bind(get_str(v, "mode").as_deref())
                .bind(get_str(v, "url").as_deref())
                .bind(get_json_str(v, "requested_schema").as_deref())
                .bind(get_str(v, "action").as_deref())
                .bind(get_json_str(v, "content").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.notification += 1;
        }
        "PreCompact" | "PostCompact" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO compact_event_details (event_id, `trigger`, custom_instructions, compact_summary) VALUES (?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "trigger").as_deref())
                .bind(get_str(v, "custom_instructions").as_deref())
                .bind(get_str(v, "compact_summary").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.compact += 1;
        }
        "InstructionsLoaded" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO instruction_event_details (event_id, file_path, memory_type, load_reason, globs, trigger_file_path, parent_file_path) VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "file_path").as_deref())
                .bind(get_str(v, "memory_type").as_deref())
                .bind(get_str(v, "load_reason").as_deref())
                .bind(get_json_str(v, "globs").as_deref())
                .bind(get_str(v, "trigger_file_path").as_deref())
                .bind(get_str(v, "parent_file_path").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.instruction += 1;
        }
        "TeammateIdle" | "TaskCompleted" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO team_event_details (event_id, teammate_name, team_name, task_id, task_subject, task_description) VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "teammate_name").as_deref())
                .bind(get_str(v, "team_name").as_deref())
                .bind(get_str(v, "task_id").as_deref())
                .bind(get_str(v, "task_subject").as_deref())
                .bind(get_str(v, "task_description").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.team += 1;
        }
        "UserPromptSubmit" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO prompt_event_details (event_id, prompt) VALUES (?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "prompt").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.prompt += 1;
        }
        "WorktreeRemove" => {
            if !dry_run {
                let rows = sqlx::query(
                    "INSERT OR IGNORE INTO worktree_event_details (event_id, worktree_path) VALUES (?, ?)",
                )
                .bind(event_id)
                .bind(get_str(v, "worktree_path").as_deref())
                .execute(&mut **tx)
                .await?;
                if rows.rows_affected() == 0 {
                    counts.skipped += 1;
                    return Ok(());
                }
            }
            counts.worktree += 1;
        }
        _ => {
            // Unknown event type — skip silently
        }
    }
    Ok(())
}

pub async fn run(
    pool: &SqlitePool,
    dry_run: bool,
    batch_size: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    // Fetch all events
    let rows = sqlx::query("SELECT id, event_type, raw_payload FROM events ORDER BY id ASC")
        .fetch_all(pool)
        .await?;

    let total = rows.len();
    if total == 0 {
        eprintln!("No events to backfill.");
        return Ok(());
    }

    let events: Vec<EventRow> = rows
        .into_iter()
        .map(|r| EventRow {
            id: r.get("id"),
            event_type: r.get("event_type"),
            raw_payload: r.get("raw_payload"),
        })
        .collect();

    eprintln!("Backfilling event detail tables...");

    let mut counts = Counts::new();

    for chunk in events.chunks(batch_size) {
        let mut tx = pool.begin().await?;
        let mut batch_ok = true;

        for event in chunk {
            let v: serde_json::Value = match serde_json::from_str(&event.raw_payload) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!(
                        "  Warning: event {} has unparseable raw_payload: {e}",
                        event.id
                    );
                    counts.parse_errors += 1;
                    continue;
                }
            };

            if let Err(e) = insert_detail_for_event(
                &mut tx,
                event.id,
                &event.event_type,
                &v,
                &mut counts,
                dry_run,
            )
            .await
            {
                eprintln!(
                    "  Warning: batch containing event {} failed: {e} — rolling back batch",
                    event.id
                );
                batch_ok = false;
                break;
            }
        }

        if batch_ok && !dry_run {
            tx.commit().await?;
        }
        // If batch_ok is false or dry_run, tx drops and rolls back automatically.

        // Progress reporting: report at end of each batch
        let processed = (chunk.as_ptr() as usize - events.as_ptr() as usize)
            / std::mem::size_of::<EventRow>()
            + chunk.len();
        let pct = (processed * 100) / total;
        let summary = counts.summary_line();
        if !summary.is_empty() {
            eprintln!("  [{processed}/{total}] {pct}% — {summary}");
        } else {
            eprintln!("  [{processed}/{total}] {pct}%");
        }
    }

    let inserted = counts.total_inserted();
    let skipped = counts.skipped;

    if dry_run {
        eprintln!("Dry run — no changes made");
        eprintln!("Events to backfill: {total}");
        if counts.tool > 0 {
            eprintln!("  tool_event_details:         {:>6}", counts.tool);
        }
        if counts.stop > 0 {
            eprintln!("  stop_event_details:         {:>6}", counts.stop);
        }
        if counts.session > 0 {
            eprintln!("  session_event_details:      {:>6}", counts.session);
        }
        if counts.agent > 0 {
            eprintln!(
                "  agent_event_details:        {:>6}  (includes SubagentStop dual-inserts)",
                counts.agent
            );
        }
        if counts.notification > 0 {
            eprintln!("  notification_event_details: {:>6}", counts.notification);
        }
        if counts.compact > 0 {
            eprintln!("  compact_event_details:      {:>6}", counts.compact);
        }
        if counts.instruction > 0 {
            eprintln!("  instruction_event_details:  {:>6}", counts.instruction);
        }
        if counts.team > 0 {
            eprintln!("  team_event_details:         {:>6}", counts.team);
        }
        if counts.prompt > 0 {
            eprintln!("  prompt_event_details:       {:>6}", counts.prompt);
        }
        if counts.worktree > 0 {
            eprintln!("  worktree_event_details:     {:>6}", counts.worktree);
        }
    } else {
        eprintln!("Backfill complete: {total} events processed, {inserted} detail rows inserted, {skipped} skipped");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("backfill_test.db");
        let pool = db::connect(db_path.to_str().unwrap()).await.unwrap();
        (pool, dir)
    }

    /// Insert a raw event directly into the events table (bypassing detail inserts).
    async fn insert_raw_event(
        pool: &SqlitePool,
        session_id: &str,
        event_type: &str,
        raw_payload: &str,
    ) -> i64 {
        let result = sqlx::query(
            "INSERT INTO events (session_id, event_type, raw_payload, cwd) VALUES (?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(event_type)
        .bind(raw_payload)
        .bind("/tmp")
        .execute(pool)
        .await
        .unwrap();

        // Also insert session row
        sqlx::query(
            "INSERT INTO sessions (session_id, first_seen, last_seen, cwd, event_count)
             VALUES (?, datetime('now'), datetime('now'), '/tmp', 1)
             ON CONFLICT(session_id) DO UPDATE SET event_count = event_count + 1",
        )
        .bind(session_id)
        .execute(pool)
        .await
        .unwrap();

        result.last_insert_rowid()
    }

    #[tokio::test]
    async fn test_backfill_idempotent() {
        let (pool, _dir) = setup_db().await;

        // Insert a PreToolUse event with detail via normal path
        let hook = crate::models::HookInput {
            session_id: "s1".into(),
            hook_event_name: "PreToolUse".into(),
            cwd: "/tmp".into(),
            tool_name: Some("Bash".into()),
            tool_use_id: Some("tu-1".into()),
            ..Default::default()
        };
        let eid = db::insert_event(
            &pool,
            &hook,
            r#"{"session_id":"s1","hook_event_name":"PreToolUse","cwd":"/tmp","tool_name":"Bash","tool_use_id":"tu-1"}"#,
        )
        .await
        .unwrap();

        // Verify detail row exists
        let before: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM tool_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(before, 1);

        // Run backfill — should be a no-op for this event
        run(&pool, false, 100).await.unwrap();

        let after: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM tool_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(after, 1);
    }

    #[tokio::test]
    async fn test_backfill_stop_failure() {
        let (pool, _dir) = setup_db().await;

        let payload = r#"{"session_id":"s1","hook_event_name":"StopFailure","cwd":"/tmp","stop_hook_active":true,"error":"timeout","error_details":"took too long"}"#;
        let eid = insert_raw_event(&pool, "s1", "StopFailure", payload).await;

        run(&pool, false, 100).await.unwrap();

        let row =
            sqlx::query("SELECT stop_hook_active, error, error_details FROM stop_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.get::<Option<bool>, _>("stop_hook_active"), Some(true));
        assert_eq!(
            row.get::<Option<String>, _>("error").as_deref(),
            Some("timeout")
        );
        assert_eq!(
            row.get::<Option<String>, _>("error_details").as_deref(),
            Some("took too long")
        );
    }

    #[tokio::test]
    async fn test_backfill_subagent_stop_dual_insert() {
        let (pool, _dir) = setup_db().await;

        let payload = r#"{"session_id":"s1","hook_event_name":"SubagentStop","cwd":"/tmp","agent_id":"a1","agent_type":"code","stop_hook_active":false}"#;
        let eid = insert_raw_event(&pool, "s1", "SubagentStop", payload).await;

        run(&pool, false, 100).await.unwrap();

        let stop_count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM stop_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(stop_count, 1);

        let agent_count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM agent_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(agent_count, 1);

        let agent_row =
            sqlx::query("SELECT agent_id, agent_type FROM agent_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            agent_row.get::<Option<String>, _>("agent_id").as_deref(),
            Some("a1")
        );
        assert_eq!(
            agent_row.get::<Option<String>, _>("agent_type").as_deref(),
            Some("code")
        );
    }

    #[tokio::test]
    async fn test_backfill_unparseable_payload_skipped() {
        let (pool, _dir) = setup_db().await;

        insert_raw_event(&pool, "s1", "PreToolUse", "not valid json").await;

        // Should not crash
        run(&pool, false, 100).await.unwrap();
    }

    #[tokio::test]
    async fn test_backfill_dry_run_no_rows() {
        let (pool, _dir) = setup_db().await;

        let payload =
            r#"{"session_id":"s1","hook_event_name":"Stop","cwd":"/tmp","stop_hook_active":true}"#;
        let eid = insert_raw_event(&pool, "s1", "Stop", payload).await;

        run(&pool, true, 100).await.unwrap();

        // No detail row should exist (dry run)
        let count: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM stop_event_details WHERE event_id = ?")
                .bind(eid)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_backfill_batch_boundary() {
        let (pool, _dir) = setup_db().await;

        // Insert 3 events with batch_size=2 to test boundary
        let payload1 = r#"{"session_id":"s1","hook_event_name":"Stop","cwd":"/tmp"}"#;
        let payload2 = r#"{"session_id":"s1","hook_event_name":"UserPromptSubmit","cwd":"/tmp","prompt":"hello"}"#;
        let payload3 = r#"{"session_id":"s1","hook_event_name":"WorktreeRemove","cwd":"/tmp","worktree_path":"/tmp/wt"}"#;

        let eid1 = insert_raw_event(&pool, "s1", "Stop", payload1).await;
        let eid2 = insert_raw_event(&pool, "s1", "UserPromptSubmit", payload2).await;
        let eid3 = insert_raw_event(&pool, "s1", "WorktreeRemove", payload3).await;

        run(&pool, false, 2).await.unwrap();

        // All 3 should have detail rows
        let c1: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM stop_event_details WHERE event_id = ?")
                .bind(eid1)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        let c2: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM prompt_event_details WHERE event_id = ?")
                .bind(eid2)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");
        let c3: i64 =
            sqlx::query("SELECT COUNT(*) as cnt FROM worktree_event_details WHERE event_id = ?")
                .bind(eid3)
                .fetch_one(&pool)
                .await
                .unwrap()
                .get("cnt");

        assert_eq!(c1, 1);
        assert_eq!(c2, 1);
        assert_eq!(c3, 1);
    }
}
