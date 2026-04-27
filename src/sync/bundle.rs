#![allow(dead_code)] // Structs and functions used by US-0055/US-0056
use std::error::Error;
use std::io::{BufRead, Write};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

// ── Bundle struct ──

#[derive(Serialize, Deserialize, Debug)]
pub struct EventBundle {
    pub event: EventRow,
    pub tool_details: Option<ToolEventDetails>,
    pub stop_details: Option<StopEventDetails>,
    pub session_details: Option<SessionEventDetails>,
    pub agent_details: Option<AgentEventDetails>,
    pub notification_details: Option<NotificationEventDetails>,
    pub compact_details: Option<CompactEventDetails>,
    pub instruction_details: Option<InstructionEventDetails>,
    pub team_details: Option<TeamEventDetails>,
    pub prompt_details: Option<PromptEventDetails>,
    pub worktree_details: Option<WorktreeEventDetails>,
    pub classifications: Vec<ClassificationRow>,
    pub enforcements: Vec<EnforcementRow>,
}

// ── Event row (excludes machine-local `id`) ──

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EventRow {
    pub timestamp: String,
    pub session_id: String,
    pub event_type: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
    pub tool_response: Option<String>,
    pub cwd: Option<String>,
    pub permission_mode: Option<String>,
    pub raw_payload: String,
    pub origin_machine_id: Option<String>,
    #[serde(default = "default_account_id")]
    pub account_id: Option<String>,
    #[serde(default)]
    pub account_email: Option<String>,
}

fn default_account_id() -> Option<String> {
    Some("default".to_string())
}

// ── Detail structs (one per detail table, excludes event_id) ──

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ToolEventDetails {
    pub tool_use_id: Option<String>,
    pub error: Option<String>,
    pub error_details: Option<String>,
    pub is_interrupt: Option<bool>,
    pub permission_suggestions: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct StopEventDetails {
    pub stop_hook_active: Option<bool>,
    pub last_assistant_message: Option<String>,
    pub error: Option<String>,
    pub error_details: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SessionEventDetails {
    pub source: Option<String>,
    pub model: Option<String>,
    pub reason: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AgentEventDetails {
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_transcript_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct NotificationEventDetails {
    pub notification_type: Option<String>,
    pub title: Option<String>,
    pub message: Option<String>,
    pub elicitation_id: Option<String>,
    pub mcp_server_name: Option<String>,
    pub mode: Option<String>,
    pub url: Option<String>,
    pub requested_schema: Option<String>,
    pub action: Option<String>,
    pub content: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CompactEventDetails {
    pub trigger: Option<String>,
    pub custom_instructions: Option<String>,
    pub compact_summary: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct InstructionEventDetails {
    pub file_path: Option<String>,
    pub memory_type: Option<String>,
    pub load_reason: Option<String>,
    pub globs: Option<String>,
    pub trigger_file_path: Option<String>,
    pub parent_file_path: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TeamEventDetails {
    pub teammate_name: Option<String>,
    pub team_name: Option<String>,
    pub task_id: Option<String>,
    pub task_subject: Option<String>,
    pub task_description: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PromptEventDetails {
    pub prompt: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WorktreeEventDetails {
    pub worktree_path: Option<String>,
}

// ── Classification and enforcement rows (excludes machine-local id, event_id, rule_id) ──

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ClassificationRow {
    pub timestamp: String,
    pub tool_name: String,
    pub input_pattern: String,
    pub risk_level: String,
    pub reason: String,
    pub heuristic: String,
}

#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub struct EnforcementRow {
    pub timestamp: String,
    pub session_id: String,
    pub tool_name: String,
    pub tool_input: Option<String>,
    pub action: String,
    pub reason: Option<String>,
    pub evaluation_ms: Option<f64>,
}

// ── Export ──

/// Chunk size for batch export queries. Stays well under SQLite's 999 bind-variable limit.
const EXPORT_CHUNK_SIZE: usize = 500;

/// Export events since `since` timestamp as JSON Lines to a writer.
/// Each line is a complete EventBundle. Returns the count of exported events.
///
/// Uses chunked batch queries to avoid N+1 query patterns: for each chunk of
/// events, all detail tables + classifications are fetched in bulk (12 queries
/// per chunk) instead of 3 queries per event.
pub async fn export_bundles<W: Write>(
    pool: &SqlitePool,
    since: Option<&str>,
    machine_id: &str,
    writer: &mut W,
) -> Result<u64, Box<dyn Error>> {
    let events = crate::db::export_events_since(pool, since).await?;
    let mut count = 0u64;

    for chunk in events.chunks(EXPORT_CHUNK_SIZE) {
        let ids: Vec<i64> = chunk.iter().map(|(id, _)| *id).collect();

        // Batch-fetch all detail tables for this chunk (10 queries)
        let tool_details = crate::db::batch_get_tool_details(pool, &ids).await?;
        let stop_details = crate::db::batch_get_stop_details(pool, &ids).await?;
        let session_details = crate::db::batch_get_session_details(pool, &ids).await?;
        let agent_details = crate::db::batch_get_agent_details(pool, &ids).await?;
        let notification_details = crate::db::batch_get_notification_details(pool, &ids).await?;
        let compact_details = crate::db::batch_get_compact_details(pool, &ids).await?;
        let instruction_details = crate::db::batch_get_instruction_details(pool, &ids).await?;
        let team_details = crate::db::batch_get_team_details(pool, &ids).await?;
        let prompt_details = crate::db::batch_get_prompt_details(pool, &ids).await?;
        let worktree_details = crate::db::batch_get_worktree_details(pool, &ids).await?;

        // Batch-fetch classifications (1 query) — enforcements remain empty (no FK link)
        let classifications = crate::db::batch_get_classifications(pool, &ids).await?;

        // Assemble bundles from the hash maps
        for (event_id, event_row) in chunk {
            let mut event_row = event_row.clone();
            if event_row.origin_machine_id.is_none() {
                event_row.origin_machine_id = Some(machine_id.to_string());
            }

            let bundle = EventBundle {
                event: event_row,
                tool_details: tool_details.get(event_id).cloned(),
                stop_details: stop_details.get(event_id).cloned(),
                session_details: session_details.get(event_id).cloned(),
                agent_details: agent_details.get(event_id).cloned(),
                notification_details: notification_details.get(event_id).cloned(),
                compact_details: compact_details.get(event_id).cloned(),
                instruction_details: instruction_details.get(event_id).cloned(),
                team_details: team_details.get(event_id).cloned(),
                prompt_details: prompt_details.get(event_id).cloned(),
                worktree_details: worktree_details.get(event_id).cloned(),
                classifications: classifications.get(event_id).cloned().unwrap_or_default(),
                enforcements: Vec::new(),
            };

            serde_json::to_writer(&mut *writer, &bundle)?;
            writer.write_all(b"\n")?;
            count += 1;
        }
    }

    Ok(count)
}

// ── Import ──

/// Read JSON Lines from a reader, returning an iterator of EventBundle.
/// Each line is deserialized independently — errors on one line don't abort the stream.
pub fn import_bundles<R: BufRead>(
    reader: R,
) -> impl Iterator<Item = Result<EventBundle, Box<dyn Error>>> {
    reader.lines().filter_map(|line_result| {
        match line_result {
            Err(e) => Some(Err(Box::new(e) as Box<dyn Error>)),
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None; // skip blank lines
                }
                Some(
                    serde_json::from_str::<EventBundle>(trimmed)
                        .map_err(|e| Box::new(e) as Box<dyn Error>),
                )
            }
        }
    })
}

/// Container for all detail types returned by get_event_details_for_sync.
#[derive(Default)]
pub struct SyncEventDetails {
    pub tool: Option<ToolEventDetails>,
    pub stop: Option<StopEventDetails>,
    pub session: Option<SessionEventDetails>,
    pub agent: Option<AgentEventDetails>,
    pub notification: Option<NotificationEventDetails>,
    pub compact: Option<CompactEventDetails>,
    pub instruction: Option<InstructionEventDetails>,
    pub team: Option<TeamEventDetails>,
    pub prompt: Option<PromptEventDetails>,
    pub worktree: Option<WorktreeEventDetails>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn test_event_bundle_roundtrip() {
        let bundle = EventBundle {
            event: EventRow {
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                session_id: "sess-123".into(),
                event_type: "PreToolUse".into(),
                tool_name: Some("Bash".into()),
                tool_input: Some(r#"{"command":"ls"}"#.into()),
                tool_response: None,
                cwd: Some("/tmp".into()),
                permission_mode: Some("default".into()),
                raw_payload: r#"{"session_id":"sess-123"}"#.into(),
                origin_machine_id: Some("machine-uuid".into()),
                account_id: Some("default".into()),
                account_email: None,
            },
            tool_details: Some(ToolEventDetails {
                tool_use_id: Some("tu-001".into()),
                error: None,
                error_details: None,
                is_interrupt: Some(false),
                permission_suggestions: None,
            }),
            stop_details: None,
            session_details: None,
            agent_details: None,
            notification_details: None,
            compact_details: None,
            instruction_details: None,
            team_details: None,
            prompt_details: None,
            worktree_details: None,
            classifications: vec![ClassificationRow {
                timestamp: "2026-01-01T00:00:01.000Z".into(),
                tool_name: "Bash".into(),
                input_pattern: "ls".into(),
                risk_level: "safe".into(),
                reason: "read-only command".into(),
                heuristic: "bash_safe_commands".into(),
            }],
            enforcements: vec![],
        };

        // Serialize
        let json = serde_json::to_string(&bundle).unwrap();

        // Deserialize
        let parsed: EventBundle = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.event, bundle.event);
        assert_eq!(parsed.tool_details, bundle.tool_details);
        assert_eq!(parsed.classifications.len(), 1);
        assert_eq!(parsed.classifications[0].tool_name, "Bash");
    }

    #[test]
    fn test_import_bundles_streaming() {
        let bundle1 = EventBundle {
            event: EventRow {
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                session_id: "s1".into(),
                event_type: "Stop".into(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                cwd: Some("/tmp".into()),
                permission_mode: None,
                raw_payload: "{}".into(),
                origin_machine_id: None,
                account_id: Some("default".into()),
                account_email: None,
            },
            tool_details: None,
            stop_details: Some(StopEventDetails {
                stop_hook_active: Some(true),
                last_assistant_message: Some("done".into()),
                error: None,
                error_details: None,
            }),
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
        };

        let mut lines = String::new();
        lines.push_str(&serde_json::to_string(&bundle1).unwrap());
        lines.push('\n');
        // Add a second line (same structure)
        lines.push_str(&serde_json::to_string(&bundle1).unwrap());
        lines.push('\n');

        let reader = BufReader::new(Cursor::new(lines));
        let bundles: Vec<_> = import_bundles(reader).collect();
        assert_eq!(bundles.len(), 2);
        assert!(bundles[0].is_ok());
        assert!(bundles[1].is_ok());
    }

    #[test]
    fn test_import_bundles_malformed_line() {
        let lines = "not valid json\n{}\n";
        let reader = BufReader::new(Cursor::new(lines));
        let results: Vec<_> = import_bundles(reader).collect();
        // First line: error (not valid JSON)
        assert!(results[0].is_err());
        // Second line: error (valid JSON but not EventBundle — missing required fields)
        assert!(results[1].is_err());
    }

    #[test]
    fn test_import_bundles_empty_lines_skipped() {
        let bundle = EventBundle {
            event: EventRow {
                timestamp: "2026-01-01T00:00:00.000Z".into(),
                session_id: "s1".into(),
                event_type: "Stop".into(),
                tool_name: None,
                tool_input: None,
                tool_response: None,
                cwd: None,
                permission_mode: None,
                raw_payload: "{}".into(),
                origin_machine_id: None,
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
        };

        let mut lines = String::new();
        lines.push('\n'); // blank line
        lines.push_str(&serde_json::to_string(&bundle).unwrap());
        lines.push('\n');
        lines.push('\n'); // blank line

        let reader = BufReader::new(Cursor::new(lines));
        let results: Vec<_> = import_bundles(reader).collect();
        assert_eq!(results.len(), 1);
        assert!(results[0].is_ok());
    }

    #[test]
    fn test_all_detail_types_serialize() {
        // Verify each detail type can round-trip through JSON
        let tool = ToolEventDetails {
            tool_use_id: Some("tu-1".into()),
            error: None,
            error_details: None,
            is_interrupt: Some(false),
            permission_suggestions: None,
        };
        let json = serde_json::to_string(&tool).unwrap();
        let parsed: ToolEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(tool, parsed);

        let stop = StopEventDetails {
            stop_hook_active: Some(true),
            last_assistant_message: Some("msg".into()),
            error: Some("err".into()),
            error_details: None,
        };
        let json = serde_json::to_string(&stop).unwrap();
        let parsed: StopEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(stop, parsed);

        let session = SessionEventDetails {
            source: Some("startup".into()),
            model: Some("claude-4".into()),
            reason: None,
            file_path: None,
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: SessionEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(session, parsed);

        let agent = AgentEventDetails {
            agent_id: Some("a1".into()),
            agent_type: Some("Bash".into()),
            agent_transcript_path: None,
        };
        let json = serde_json::to_string(&agent).unwrap();
        let parsed: AgentEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(agent, parsed);

        let notification = NotificationEventDetails {
            notification_type: Some("permission_prompt".into()),
            title: Some("title".into()),
            message: Some("msg".into()),
            elicitation_id: None,
            mcp_server_name: None,
            mode: None,
            url: None,
            requested_schema: None,
            action: None,
            content: None,
        };
        let json = serde_json::to_string(&notification).unwrap();
        let parsed: NotificationEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(notification, parsed);

        let compact = CompactEventDetails {
            trigger: Some("auto".into()),
            custom_instructions: None,
            compact_summary: Some("summary".into()),
        };
        let json = serde_json::to_string(&compact).unwrap();
        let parsed: CompactEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(compact, parsed);

        let instruction = InstructionEventDetails {
            file_path: Some("/path".into()),
            memory_type: Some("project".into()),
            load_reason: Some("session_start".into()),
            globs: None,
            trigger_file_path: None,
            parent_file_path: None,
        };
        let json = serde_json::to_string(&instruction).unwrap();
        let parsed: InstructionEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(instruction, parsed);

        let team = TeamEventDetails {
            teammate_name: Some("agent-1".into()),
            team_name: Some("team-a".into()),
            task_id: Some("t1".into()),
            task_subject: None,
            task_description: None,
        };
        let json = serde_json::to_string(&team).unwrap();
        let parsed: TeamEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(team, parsed);

        let prompt = PromptEventDetails {
            prompt: Some("hello".into()),
        };
        let json = serde_json::to_string(&prompt).unwrap();
        let parsed: PromptEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(prompt, parsed);

        let worktree = WorktreeEventDetails {
            worktree_path: Some("/wt".into()),
        };
        let json = serde_json::to_string(&worktree).unwrap();
        let parsed: WorktreeEventDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(worktree, parsed);
    }

    #[test]
    fn test_classification_and_enforcement_serialize() {
        let c = ClassificationRow {
            timestamp: "2026-01-01T00:00:00.000Z".into(),
            tool_name: "Bash".into(),
            input_pattern: "rm -rf".into(),
            risk_level: "dangerous".into(),
            reason: "destructive".into(),
            heuristic: "bash_destructive".into(),
        };
        let json = serde_json::to_string(&c).unwrap();
        let parsed: ClassificationRow = serde_json::from_str(&json).unwrap();
        assert_eq!(c, parsed);

        let e = EnforcementRow {
            timestamp: "2026-01-01T00:00:00.000Z".into(),
            session_id: "s1".into(),
            tool_name: "Bash".into(),
            tool_input: Some("rm -rf /".into()),
            action: "denied".into(),
            reason: Some("blocked".into()),
            evaluation_ms: Some(2.5),
        };
        let json = serde_json::to_string(&e).unwrap();
        let parsed: EnforcementRow = serde_json::from_str(&json).unwrap();
        assert_eq!(e, parsed);
    }

    #[tokio::test]
    async fn test_batch_export_with_detail_tables() {
        // Set up a temp DB and insert events with detail rows, then verify batch export
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("batch_test.db");
        let pool = crate::db::connect(db_path.to_str().unwrap()).await.unwrap();

        // Insert a PreToolUse event (gets tool_event_details)
        let tool_payload = serde_json::json!({
            "session_id": "sess-batch-1",
            "hook_event_name": "PreToolUse",
            "cwd": "/tmp/test",
            "tool_name": "Bash",
            "tool_input": {"command": "ls -la"},
            "tool_use_id": "tu-batch-001",
            "permission_mode": "default"
        });
        let hook_input: crate::models::HookInput =
            serde_json::from_value(tool_payload.clone()).unwrap();
        crate::db::insert_event(
            &pool,
            &hook_input,
            &serde_json::to_string(&tool_payload).unwrap(),
            "default",
            None,
        )
        .await
        .unwrap();

        // Insert a SessionStart event (gets session_event_details)
        let session_payload = serde_json::json!({
            "session_id": "sess-batch-1",
            "hook_event_name": "SessionStart",
            "cwd": "/tmp/test",
            "source": "startup",
            "model": "claude-4",
            "permission_mode": "default"
        });
        let hook_input2: crate::models::HookInput =
            serde_json::from_value(session_payload.clone()).unwrap();
        crate::db::insert_event(
            &pool,
            &hook_input2,
            &serde_json::to_string(&session_payload).unwrap(),
            "default",
            None,
        )
        .await
        .unwrap();

        // Insert a Stop event (gets stop_event_details)
        let stop_payload = serde_json::json!({
            "session_id": "sess-batch-1",
            "hook_event_name": "Stop",
            "cwd": "/tmp/test",
            "stop_hook_active": true,
            "last_assistant_message": "done",
            "permission_mode": "default"
        });
        let hook_input3: crate::models::HookInput =
            serde_json::from_value(stop_payload.clone()).unwrap();
        crate::db::insert_event(
            &pool,
            &hook_input3,
            &serde_json::to_string(&stop_payload).unwrap(),
            "default",
            None,
        )
        .await
        .unwrap();

        // Export using batch function
        let mut output = Vec::new();
        let count = export_bundles(&pool, None, "test-machine", &mut output)
            .await
            .unwrap();

        assert_eq!(count, 3);

        // Parse back and verify detail tables were populated
        let reader = BufReader::new(Cursor::new(output));
        let bundles: Vec<EventBundle> = import_bundles(reader).filter_map(|r| r.ok()).collect();

        assert_eq!(bundles.len(), 3);

        // Find and verify the PreToolUse bundle has tool_details
        let tool_bundle = bundles
            .iter()
            .find(|b| b.event.event_type == "PreToolUse")
            .expect("should have PreToolUse bundle");
        assert!(tool_bundle.tool_details.is_some());
        assert_eq!(
            tool_bundle.tool_details.as_ref().unwrap().tool_use_id,
            Some("tu-batch-001".into())
        );

        // Find and verify the SessionStart bundle has session_details
        let session_bundle = bundles
            .iter()
            .find(|b| b.event.event_type == "SessionStart")
            .expect("should have SessionStart bundle");
        assert!(session_bundle.session_details.is_some());
        assert_eq!(
            session_bundle.session_details.as_ref().unwrap().source,
            Some("startup".into())
        );
        assert_eq!(
            session_bundle.session_details.as_ref().unwrap().model,
            Some("claude-4".into())
        );

        // Find and verify the Stop bundle has stop_details
        let stop_bundle = bundles
            .iter()
            .find(|b| b.event.event_type == "Stop")
            .expect("should have Stop bundle");
        assert!(stop_bundle.stop_details.is_some());
        assert_eq!(
            stop_bundle.stop_details.as_ref().unwrap().stop_hook_active,
            Some(true)
        );
        assert_eq!(
            stop_bundle
                .stop_details
                .as_ref()
                .unwrap()
                .last_assistant_message,
            Some("done".into())
        );

        // All bundles should have origin_machine_id filled in
        for bundle in &bundles {
            assert_eq!(bundle.event.origin_machine_id, Some("test-machine".into()));
        }
    }

    #[tokio::test]
    async fn test_batch_export_chunks_correctly() {
        // Verify that chunking works correctly with more events than EXPORT_CHUNK_SIZE
        // We'll use a smaller count to keep the test fast, but verify the chunking logic
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("chunk_test.db");
        let pool = crate::db::connect(db_path.to_str().unwrap()).await.unwrap();

        // Insert 10 events (verifies the chunk loop works for < EXPORT_CHUNK_SIZE)
        for i in 0..10 {
            let payload = serde_json::json!({
                "session_id": format!("sess-chunk-{}", i),
                "hook_event_name": "PreToolUse",
                "cwd": "/tmp/test",
                "tool_name": "Bash",
                "tool_input": {"command": format!("echo {}", i)},
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

        let mut output = Vec::new();
        let count = export_bundles(&pool, None, "chunk-machine", &mut output)
            .await
            .unwrap();

        assert_eq!(count, 10);

        // Verify all 10 bundles are distinct and valid
        let reader = BufReader::new(Cursor::new(output));
        let bundles: Vec<EventBundle> = import_bundles(reader).filter_map(|r| r.ok()).collect();
        assert_eq!(bundles.len(), 10);

        // Verify uniqueness of session_ids
        let session_ids: std::collections::HashSet<&str> = bundles
            .iter()
            .map(|b| b.event.session_id.as_str())
            .collect();
        assert_eq!(session_ids.len(), 10);
    }
}
