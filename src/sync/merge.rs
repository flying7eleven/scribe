#![allow(dead_code)] // Functions used by cmd_sync import handler
use std::collections::HashSet;
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
///
/// Uses `INSERT OR IGNORE` with the existing unique dedup index
/// `(session_id, timestamp, event_type)` to skip duplicates without
/// per-event SELECT queries. Returns merge statistics and the set of
/// `(account_id, session_id)` pairs that received new events (for
/// incremental sessions update in US-0075).
pub async fn merge_bundles(
    pool: &SqlitePool,
    bundles: impl Iterator<Item = Result<EventBundle, Box<dyn Error>>>,
) -> Result<(MergeStats, HashSet<(String, String)>), Box<dyn Error>> {
    let mut stats = MergeStats {
        events_imported: 0,
        events_skipped: 0,
        classifications_imported: 0,
        enforcements_imported: 0,
        errors: 0,
    };
    let mut affected_sessions: HashSet<(String, String)> = HashSet::new();

    let mut tx = pool.begin().await?;

    for result in bundles {
        let bundle = match result {
            Ok(b) => b,
            Err(e) => {
                eprintln!("Warning: skipping malformed bundle: {e}");
                stats.errors += 1;
                continue;
            }
        };

        // INSERT OR IGNORE — let the unique index handle dedup
        let insert_result = sqlx::query(
            "INSERT OR IGNORE INTO events (timestamp, session_id, event_type, tool_name, \
             tool_input, tool_response, cwd, permission_mode, raw_payload, \
             origin_machine_id, account_id, account_email) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&bundle.event.timestamp)
        .bind(&bundle.event.session_id)
        .bind(&bundle.event.event_type)
        .bind(&bundle.event.tool_name)
        .bind(&bundle.event.tool_input)
        .bind(&bundle.event.tool_response)
        .bind(&bundle.event.cwd)
        .bind(&bundle.event.permission_mode)
        .bind(&bundle.event.raw_payload)
        .bind(&bundle.event.origin_machine_id)
        .bind(bundle.event.account_id.as_deref().unwrap_or("default"))
        .bind(bundle.event.account_email.as_deref())
        .execute(&mut *tx)
        .await;

        let insert_result = match insert_result {
            Ok(r) => r,
            Err(e) => {
                eprintln!("Warning: failed to insert event: {e}");
                stats.errors += 1;
                continue;
            }
        };

        if insert_result.rows_affected() == 0 {
            // Duplicate — unique constraint triggered INSERT OR IGNORE
            stats.events_skipped += 1;
            continue;
        }

        // New event inserted successfully
        let event_id = insert_result.last_insert_rowid();
        stats.events_imported += 1;
        affected_sessions.insert((
            bundle
                .event
                .account_id
                .clone()
                .unwrap_or_else(|| "default".to_string()),
            bundle.event.session_id.clone(),
        ));

        // Insert detail rows
        if let Err(e) = insert_detail_row_tx(&mut tx, event_id, &bundle).await {
            eprintln!("Warning: failed to insert detail row: {e}");
            // Don't increment errors — the event itself was inserted successfully
        }

        // Insert classifications with remapped event_id
        for classification in &bundle.classifications {
            if let Err(e) = insert_synced_classification_tx(&mut tx, event_id, classification).await
            {
                eprintln!("Warning: failed to insert classification: {e}");
            } else {
                stats.classifications_imported += 1;
            }
        }

        // Insert enforcements (rule_id = NULL)
        for enforcement in &bundle.enforcements {
            if let Err(e) = insert_synced_enforcement_tx(&mut tx, enforcement).await {
                eprintln!("Warning: failed to insert enforcement: {e}");
            } else {
                stats.enforcements_imported += 1;
            }
        }
    }

    tx.commit().await?;

    // Incrementally update only affected sessions (US-0075)
    crate::db::update_sessions_incremental(pool, &affected_sessions).await?;

    Ok((stats, affected_sessions))
}

/// Insert a classification within a transaction.
async fn insert_synced_classification_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    event_id: i64,
    c: &ClassificationRow,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "INSERT INTO classifications (timestamp, event_id, tool_name, input_pattern, \
         risk_level, reason, heuristic) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&c.timestamp)
    .bind(event_id)
    .bind(&c.tool_name)
    .bind(&c.input_pattern)
    .bind(&c.risk_level)
    .bind(&c.reason)
    .bind(&c.heuristic)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert an enforcement within a transaction (rule_id = NULL).
async fn insert_synced_enforcement_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    e: &EnforcementRow,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "INSERT INTO enforcements (timestamp, session_id, tool_name, tool_input, \
         action, reason, evaluation_ms) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&e.timestamp)
    .bind(&e.session_id)
    .bind(&e.tool_name)
    .bind(&e.tool_input)
    .bind(&e.action)
    .bind(&e.reason)
    .bind(e.evaluation_ms)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Insert the appropriate detail row based on event type (within a transaction).
async fn insert_detail_row_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
                .execute(&mut **tx)
                .await?;
            }
        }
        "Stop" | "StopFailure" => {
            if let Some(ref d) = bundle.stop_details {
                insert_stop_detail_tx(tx, event_id, d).await?;
            }
        }
        "SubagentStop" => {
            // Dual insert: stop + agent
            if let Some(ref d) = bundle.stop_details {
                insert_stop_detail_tx(tx, event_id, d).await?;
            }
            if let Some(ref d) = bundle.agent_details {
                insert_agent_detail_tx(tx, event_id, d).await?;
            }
        }
        "SubagentStart" => {
            if let Some(ref d) = bundle.agent_details {
                insert_agent_detail_tx(tx, event_id, d).await?;
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
                .execute(&mut **tx)
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
                .execute(&mut **tx)
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
                .execute(&mut **tx)
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
                .execute(&mut **tx)
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
                .execute(&mut **tx)
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
                .execute(&mut **tx)
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
                .execute(&mut **tx)
                .await?;
            }
        }
        _ => {} // CwdChanged, FileChanged — no detail table
    }

    Ok(())
}

async fn insert_stop_detail_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_agent_detail_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
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
    .execute(&mut **tx)
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

        let (stats, _affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
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
        let (stats, _affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 1);

        // Insert same again — should be skipped
        let bundles = vec![Ok(make_event_bundle(
            "s1",
            "Stop",
            "2026-01-01T00:00:00.000Z",
        ))];
        let (stats, _affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
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
        let (stats, _affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
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

        let (stats, _affected) = merge_bundles(&pool, vec![Ok(bundle)].into_iter())
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

        let (stats, _affected) = merge_bundles(&pool, vec![Ok(bundle)].into_iter())
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

        let (stats, _affected) = merge_bundles(&pool, vec![Ok(bundle)].into_iter())
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
        let (stats, _affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
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

        let (stats, _affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 1);
        assert_eq!(stats.errors, 1);
    }

    #[tokio::test]
    async fn test_merge_large_scale_dedup() {
        // US-0074: Import 1000 bundles where 900 are duplicates
        let (pool, _dir) = setup_merge_db().await;

        // First: insert 900 unique events
        let initial_bundles: Vec<Result<EventBundle, Box<dyn Error>>> = (0..900)
            .map(|i| {
                Ok(make_event_bundle(
                    &format!("s-{}", i),
                    "Stop",
                    &format!("2026-01-01T00:{:02}:{:02}.000Z", i / 60, i % 60),
                ))
            })
            .collect();

        let (stats, affected) = merge_bundles(&pool, initial_bundles.into_iter())
            .await
            .unwrap();
        assert_eq!(stats.events_imported, 900);
        assert_eq!(stats.events_skipped, 0);
        assert_eq!(affected.len(), 900);

        // Now: import 1000 bundles — 900 duplicates + 100 new
        let mixed_bundles: Vec<Result<EventBundle, Box<dyn Error>>> = (0..1000)
            .map(|i| {
                if i < 900 {
                    // Duplicate — same session/timestamp/event_type as initial
                    Ok(make_event_bundle(
                        &format!("s-{}", i),
                        "Stop",
                        &format!("2026-01-01T00:{:02}:{:02}.000Z", i / 60, i % 60),
                    ))
                } else {
                    // New event
                    Ok(make_event_bundle(
                        &format!("s-new-{}", i),
                        "PreToolUse",
                        &format!(
                            "2026-01-02T00:{:02}:{:02}.000Z",
                            (i - 900) / 60,
                            (i - 900) % 60
                        ),
                    ))
                }
            })
            .collect();

        let (stats, affected) = merge_bundles(&pool, mixed_bundles.into_iter())
            .await
            .unwrap();
        assert_eq!(stats.events_imported, 100);
        assert_eq!(stats.events_skipped, 900);
        assert_eq!(stats.errors, 0);
        // Only the 100 new sessions should be affected
        assert_eq!(affected.len(), 100);
    }

    #[tokio::test]
    async fn test_merge_affected_sessions_tracked() {
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

        let (_stats, affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();

        // Two distinct sessions affected
        assert_eq!(affected.len(), 2);
        assert!(affected.contains(&("default".to_string(), "s1".to_string())));
        assert!(affected.contains(&("default".to_string(), "s2".to_string())));
    }

    #[tokio::test]
    async fn test_incremental_sessions_only_updates_affected() {
        // US-0075: Insert events into 1000 sessions, then sync 50 events spanning
        // 5 new sessions — verify only those 5 are added, the other 1000 unchanged.
        let (pool, _dir) = setup_merge_db().await;

        // Pre-populate 1000 sessions via direct event inserts
        for i in 0..1000 {
            let payload = serde_json::json!({
                "session_id": format!("existing-{}", i),
                "hook_event_name": "SessionStart",
                "cwd": format!("/project/{}", i),
                "source": "startup",
                "permission_mode": "default"
            });
            let hook_input: crate::models::HookInput =
                serde_json::from_value(payload.clone()).unwrap();
            crate::db::insert_event(
                &pool,
                &hook_input,
                &serde_json::to_string(&payload).unwrap(),
                "default",
                None,
            )
            .await
            .unwrap();
        }

        // Verify 1000 sessions exist
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1000);

        // Record a session's event_count before merge (should be 1)
        let before: (i64,) =
            sqlx::query_as("SELECT event_count FROM sessions WHERE session_id = 'existing-500'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before.0, 1);

        // Now merge 50 events spanning 5 NEW sessions (10 events each)
        let bundles: Vec<Result<EventBundle, Box<dyn Error>>> = (0..50)
            .map(|i| {
                let session_idx = i / 10; // 5 sessions, 10 events each
                Ok(make_event_bundle(
                    &format!("new-session-{}", session_idx),
                    "PreToolUse",
                    &format!("2026-02-01T00:{:02}:{:02}.000Z", session_idx, i % 10),
                ))
            })
            .collect();

        let (stats, affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 50);
        assert_eq!(affected.len(), 5);

        // Verify we now have 1005 sessions total
        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1005);

        // Verify the 5 new sessions have correct event_count
        for i in 0..5 {
            let row: (i64,) =
                sqlx::query_as("SELECT event_count FROM sessions WHERE session_id = ?")
                    .bind(format!("new-session-{}", i))
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(row.0, 10, "new-session-{} should have 10 events", i);
        }

        // Verify existing sessions are untouched (still event_count = 1)
        let after: (i64,) =
            sqlx::query_as("SELECT event_count FROM sessions WHERE session_id = 'existing-500'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(after.0, 1);
    }

    #[tokio::test]
    async fn test_incremental_sessions_skips_when_all_duplicates() {
        let (pool, _dir) = setup_merge_db().await;

        // Insert one event
        let bundles = vec![Ok(make_event_bundle(
            "s1",
            "Stop",
            "2026-01-01T00:00:00.000Z",
        ))];
        let (stats, _) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_imported, 1);

        // Record session state
        let before: (String,) =
            sqlx::query_as("SELECT last_seen FROM sessions WHERE session_id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();

        // Merge same event again (duplicate)
        let bundles = vec![Ok(make_event_bundle(
            "s1",
            "Stop",
            "2026-01-01T00:00:00.000Z",
        ))];
        let (stats, affected) = merge_bundles(&pool, bundles.into_iter()).await.unwrap();
        assert_eq!(stats.events_skipped, 1);
        assert_eq!(stats.events_imported, 0);
        assert!(affected.is_empty());

        // Session should be unchanged
        let after: (String,) =
            sqlx::query_as("SELECT last_seen FROM sessions WHERE session_id = 's1'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(before.0, after.0);
    }
}
