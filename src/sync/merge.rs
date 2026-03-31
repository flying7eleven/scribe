#![allow(dead_code)] // Functions used by cmd_sync import handler
use std::error::Error;

use sqlx::SqlitePool;

use super::bundle::*;

/// Merge result statistics.
pub struct MergeStats {
    pub events_imported: u64,
    pub events_skipped: u64,
    pub classifications_imported: u64,
    pub enforcements_imported: u64,
    pub errors: u64,
}

/// Merge a stream of EventBundles into the local database.
/// Returns statistics about the merge operation.
pub async fn merge_bundles(
    pool: &SqlitePool,
    bundles: impl Iterator<Item = Result<EventBundle, Box<dyn Error>>>,
) -> Result<MergeStats, Box<dyn Error>> {
    let mut stats = MergeStats {
        events_imported: 0,
        events_skipped: 0,
        classifications_imported: 0,
        enforcements_imported: 0,
        errors: 0,
    };

    for result in bundles {
        let bundle = match result {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Warning: skipping malformed bundle: {e}");
                stats.errors += 1;
                continue;
            }
        };

        match merge_single_bundle(pool, bundle, &mut stats).await {
            Ok(()) => {}
            Err(e) => {
                eprintln!("Warning: failed to merge bundle: {e}");
                stats.errors += 1;
            }
        }
    }

    // Rebuild sessions table after merge
    crate::db::rebuild_sessions(pool).await?;

    Ok(stats)
}

/// Merge a single EventBundle into the database.
async fn merge_single_bundle(
    pool: &SqlitePool,
    bundle: EventBundle,
    stats: &mut MergeStats,
) -> Result<(), Box<dyn Error>> {
    // Dedup check
    let existing = crate::db::check_event_exists(
        pool,
        bundle.event.account_id.as_deref().unwrap_or("default"),
        &bundle.event.session_id,
        &bundle.event.timestamp,
        &bundle.event.event_type,
    )
    .await?;

    if existing.is_some() {
        stats.events_skipped += 1;
        return Ok(());
    }

    // Insert event
    let event_id = crate::db::insert_synced_event(pool, &bundle.event).await?;
    stats.events_imported += 1;

    // Insert detail rows
    insert_detail_row(pool, event_id, &bundle).await?;

    // Insert classifications with remapped event_id
    for classification in &bundle.classifications {
        crate::db::insert_synced_classification(pool, event_id, classification).await?;
        stats.classifications_imported += 1;
    }

    // Insert enforcements (rule_id = NULL)
    for enforcement in &bundle.enforcements {
        crate::db::insert_synced_enforcement(pool, enforcement).await?;
        stats.enforcements_imported += 1;
    }

    Ok(())
}

/// Insert the appropriate detail row based on event type.
async fn insert_detail_row(
    pool: &SqlitePool,
    event_id: i64,
    bundle: &EventBundle,
) -> Result<(), Box<dyn Error>> {
    let event_type = bundle.event.event_type.as_str();

    match event_type {
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" | "PermissionRequest" => {
            if let Some(ref d) = bundle.tool_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO tool_event_details \
                     (event_id, tool_use_id, error, error_details, is_interrupt, permission_suggestions) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(&d.tool_use_id)
                .bind(&d.error)
                .bind(&d.error_details)
                .bind(d.is_interrupt.map(|b| b as i32))
                .bind(&d.permission_suggestions)
                .execute(pool)
                .await?;
            }
        }
        "Stop" | "StopFailure" => {
            if let Some(ref d) = bundle.stop_details {
                insert_stop_detail(pool, event_id, d).await?;
            }
        }
        "SubagentStop" => {
            // Dual insert: stop + agent
            if let Some(ref d) = bundle.stop_details {
                insert_stop_detail(pool, event_id, d).await?;
            }
            if let Some(ref d) = bundle.agent_details {
                insert_agent_detail(pool, event_id, d).await?;
            }
        }
        "SubagentStart" => {
            if let Some(ref d) = bundle.agent_details {
                insert_agent_detail(pool, event_id, d).await?;
            }
        }
        "SessionStart" | "SessionEnd" | "ConfigChange" => {
            if let Some(ref d) = bundle.session_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO session_event_details \
                     (event_id, source, model, reason, file_path) \
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(&d.source)
                .bind(&d.model)
                .bind(&d.reason)
                .bind(&d.file_path)
                .execute(pool)
                .await?;
            }
        }
        "Notification" | "Elicitation" | "ElicitationResult" => {
            if let Some(ref d) = bundle.notification_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO notification_event_details \
                     (event_id, notification_type, title, message, elicitation_id, \
                      mcp_server_name, mode, url, requested_schema, action, content) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(&d.notification_type)
                .bind(&d.title)
                .bind(&d.message)
                .bind(&d.elicitation_id)
                .bind(&d.mcp_server_name)
                .bind(&d.mode)
                .bind(&d.url)
                .bind(&d.requested_schema)
                .bind(&d.action)
                .bind(&d.content)
                .execute(pool)
                .await?;
            }
        }
        "PreCompact" | "PostCompact" => {
            if let Some(ref d) = bundle.compact_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO compact_event_details \
                     (event_id, `trigger`, custom_instructions, compact_summary) \
                     VALUES (?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(&d.trigger)
                .bind(&d.custom_instructions)
                .bind(&d.compact_summary)
                .execute(pool)
                .await?;
            }
        }
        "InstructionsLoaded" => {
            if let Some(ref d) = bundle.instruction_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO instruction_event_details \
                     (event_id, file_path, memory_type, load_reason, globs, trigger_file_path, parent_file_path) \
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(&d.file_path)
                .bind(&d.memory_type)
                .bind(&d.load_reason)
                .bind(&d.globs)
                .bind(&d.trigger_file_path)
                .bind(&d.parent_file_path)
                .execute(pool)
                .await?;
            }
        }
        "TeammateIdle" | "TaskCompleted" | "TaskCreated" => {
            if let Some(ref d) = bundle.team_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO team_event_details \
                     (event_id, teammate_name, team_name, task_id, task_subject, task_description) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(event_id)
                .bind(&d.teammate_name)
                .bind(&d.team_name)
                .bind(&d.task_id)
                .bind(&d.task_subject)
                .bind(&d.task_description)
                .execute(pool)
                .await?;
            }
        }
        "UserPromptSubmit" => {
            if let Some(ref d) = bundle.prompt_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO prompt_event_details (event_id, prompt) VALUES (?, ?)",
                )
                .bind(event_id)
                .bind(&d.prompt)
                .execute(pool)
                .await?;
            }
        }
        "WorktreeRemove" | "WorktreeCreate" => {
            if let Some(ref d) = bundle.worktree_details {
                sqlx::query(
                    "INSERT OR IGNORE INTO worktree_event_details (event_id, worktree_path) VALUES (?, ?)",
                )
                .bind(event_id)
                .bind(&d.worktree_path)
                .execute(pool)
                .await?;
            }
        }
        _ => {} // CwdChanged, FileChanged — no detail table
    }

    Ok(())
}

async fn insert_stop_detail(
    pool: &SqlitePool,
    event_id: i64,
    d: &StopEventDetails,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "INSERT OR IGNORE INTO stop_event_details \
         (event_id, stop_hook_active, last_assistant_message, error, error_details) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(event_id)
    .bind(d.stop_hook_active.map(|b| b as i32))
    .bind(&d.last_assistant_message)
    .bind(&d.error)
    .bind(&d.error_details)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_agent_detail(
    pool: &SqlitePool,
    event_id: i64,
    d: &AgentEventDetails,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "INSERT OR IGNORE INTO agent_event_details \
         (event_id, agent_id, agent_type, agent_transcript_path) \
         VALUES (?, ?, ?, ?)",
    )
    .bind(event_id)
    .bind(&d.agent_id)
    .bind(&d.agent_type)
    .bind(&d.agent_transcript_path)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_merge_db() -> (SqlitePool, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("merge_test.db");
        let pool = crate::db::connect(db_path.to_str().unwrap()).await.unwrap();
        (pool, dir)
    }

    fn make_event_bundle(session_id: &str, event_type: &str, timestamp: &str) -> EventBundle {
        EventBundle {
            event: EventRow {
                timestamp: timestamp.into(),
                session_id: session_id.into(),
                event_type: event_type.into(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                cwd: Some("/tmp".into()),
                permission_mode: None,
                raw_payload: "{}".into(),
                origin_machine_id: Some("remote-machine".into()),
                account_id: Some("default".into()),
                account_email: None,
            },
            tool_details: None,
            stop_details: None,
            session_details: None,
            agent_details: None,
            notification_details: None,
            compact_details: None,
            instruction_details: None,
            team_details: None,
            prompt_details: None,
            worktree_details: None,
            classifications: vec![],
            enforcements: vec![],
        }
    }

    #[tokio::test]
    async fn test_merge_new_events() {
        let (pool, _dir) = setup_merge_db().await;

        let bundles = vec![
            Ok(make_event_bundle("s1", "Stop", "2026-01-01T00:00:00.000Z")),
            Ok(make_event_bundle("s2", "Stop", "2026-01-01T00:00:01.000Z")),
        ];

        let stats = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 2);
        assert_eq!(stats.events_skipped, 0);
        assert_eq!(stats.errors, 0);
    }

    #[tokio::test]
    async fn test_merge_duplicates_skipped() {
        let (pool, _dir) = setup_merge_db().await;

        // Insert first
        let bundles = vec![Ok(make_event_bundle(
            "s1",
            "Stop",
            "2026-01-01T00:00:00.000Z",
        ))];
        let stats = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 1);

        // Insert same again — should be skipped
        let bundles = vec![Ok(make_event_bundle(
            "s1",
            "Stop",
            "2026-01-01T00:00:00.000Z",
        ))];
        let stats = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 0);
        assert_eq!(stats.events_skipped, 1);
    }

    #[tokio::test]
    async fn test_merge_mixed() {
        let (pool, _dir) = setup_merge_db().await;

        // Insert one event
        let bundles = vec![Ok(make_event_bundle(
            "s1",
            "Stop",
            "2026-01-01T00:00:00.000Z",
        ))];
        merge_bundles(&pool, bundles.into_iter()).await.unwrap();

        // Now merge: one duplicate + one new
        let bundles = vec![
            Ok(make_event_bundle("s1", "Stop", "2026-01-01T00:00:00.000Z")), // dup
            Ok(make_event_bundle("s2", "Stop", "2026-01-01T00:00:01.000Z")), // new
        ];
        let stats = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 1);
        assert_eq!(stats.events_skipped, 1);
    }

    #[tokio::test]
    async fn test_merge_with_tool_details() {
        let (pool, _dir) = setup_merge_db().await;

        let mut bundle = make_event_bundle("s1", "PreToolUse", "2026-01-01T00:00:00.000Z");
        bundle.event.tool_name = Some("Bash".into());
        bundle.tool_details = Some(ToolEventDetails {
            tool_use_id: Some("tu-001".into()),
            error: None,
            error_details: None,
            is_interrupt: Some(false),
            permission_suggestions: None,
        });

        let stats = merge_bundles(&pool, vec![Ok(bundle)].into_iter())
            .await
            .unwrap();
        assert_eq!(stats.events_imported, 1);

        // Verify detail row exists
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM tool_event_details WHERE tool_use_id = 'tu-001'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, 1);
    }

    #[tokio::test]
    async fn test_merge_subagent_stop_dual_insert() {
        let (pool, _dir) = setup_merge_db().await;

        let mut bundle = make_event_bundle("s1", "SubagentStop", "2026-01-01T00:00:00.000Z");
        bundle.stop_details = Some(StopEventDetails {
            stop_hook_active: Some(true),
            last_assistant_message: Some("done".into()),
            error: None,
            error_details: None,
        });
        bundle.agent_details = Some(AgentEventDetails {
            agent_id: Some("a1".into()),
            agent_type: Some("Bash".into()),
            agent_transcript_path: None,
        });

        let stats = merge_bundles(&pool, vec![Ok(bundle)].into_iter())
            .await
            .unwrap();
        assert_eq!(stats.events_imported, 1);

        // Verify both detail tables have a row
        let stop_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM stop_event_details")
            .fetch_one(&pool)
            .await
            .unwrap();
        let agent_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM agent_event_details")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stop_count.0, 1);
        assert_eq!(agent_count.0, 1);
    }

    #[tokio::test]
    async fn test_merge_with_classifications() {
        let (pool, _dir) = setup_merge_db().await;

        let mut bundle = make_event_bundle("s1", "PreToolUse", "2026-01-01T00:00:00.000Z");
        bundle.classifications = vec![ClassificationRow {
            timestamp: "2026-01-01T00:00:01.000Z".into(),
            tool_name: "Bash".into(),
            input_pattern: "rm -rf".into(),
            risk_level: "dangerous".into(),
            reason: "destructive".into(),
            heuristic: "bash_destructive".into(),
        }];

        let stats = merge_bundles(&pool, vec![Ok(bundle)].into_iter())
            .await
            .unwrap();
        assert_eq!(stats.events_imported, 1);
        assert_eq!(stats.classifications_imported, 1);

        // Verify classification has the new event_id
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM classifications WHERE event_id IS NOT NULL")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, 1);
    }

    #[tokio::test]
    async fn test_merge_sessions_rebuilt() {
        let (pool, _dir) = setup_merge_db().await;

        let bundles = vec![
            Ok(make_event_bundle("s1", "Stop", "2026-01-01T00:00:00.000Z")),
            Ok(make_event_bundle(
                "s1",
                "PreToolUse",
                "2026-01-01T00:00:01.000Z",
            )),
            Ok(make_event_bundle("s2", "Stop", "2026-01-01T00:01:00.000Z")),
        ];

        merge_bundles(&pool, bundles.into_iter()).await.unwrap();

        // Check sessions
        let sessions: Vec<(String,)> =
            sqlx::query_as("SELECT session_id FROM sessions ORDER BY session_id")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].0, "s1");
        assert_eq!(sessions[1].0, "s2");

        // Check s1 event_count
        let count: (i64,) =
            sqlx::query_as("SELECT event_count FROM sessions WHERE session_id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count.0, 2);
    }

    #[tokio::test]
    async fn test_merge_empty_stream() {
        let (pool, _dir) = setup_merge_db().await;
        let bundles: Vec<Result<EventBundle, Box<dyn Error>>> = vec![];
        let stats = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 0);
        assert_eq!(stats.events_skipped, 0);
        assert_eq!(stats.errors, 0);
    }

    #[tokio::test]
    async fn test_merge_error_continues() {
        let (pool, _dir) = setup_merge_db().await;

        let bundles: Vec<Result<EventBundle, Box<dyn Error>>> = vec![
            Err("simulated error".into()),
            Ok(make_event_bundle("s1", "Stop", "2026-01-01T00:00:00.000Z")),
        ];

        let stats = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 1);
        assert_eq!(stats.errors, 1);
    }
}
