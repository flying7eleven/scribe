use std::error::Error;
use std::process::{Command, Stdio};
use std::time::Instant;

use sqlx::SqlitePool;

pub struct SyncResult {
    pub events_sent: u64,
    pub events_received: u64,
    pub direction: Direction,
    pub duration_secs: f64,
}

pub enum Direction {
    Push,
    Pull,
}

/// Push local events to a remote machine via SSH.
/// Pipeline: scribe sync export [--since <ts>] | ssh <remote> scribe sync import
pub async fn push(
    pool: &SqlitePool,
    remote: &str,
    since: Option<&str>,
) -> Result<SyncResult, Box<dyn Error>> {
    let start = Instant::now();

    // Determine since: explicit > last sync > all
    let effective_since = match since {
        Some(s) => Some(s.to_string()),
        None => get_last_sync_timestamp(pool, remote).await?,
    };

    // Build export command
    let mut export_args = vec!["sync", "export"];
    if let Some(ref s) = effective_since {
        export_args.push("--since");
        export_args.push(s);
    }

    let scribe_bin = std::env::current_exe()?;

    let mut export_proc = Command::new(&scribe_bin)
        .args(&export_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start scribe export: {e}"))?;

    let export_stdout = export_proc
        .stdout
        .take()
        .ok_or("failed to capture export stdout")?;

    // Pipe to ssh <remote> scribe sync import
    let import_proc = Command::new("ssh")
        .arg(remote)
        .args(["scribe", "sync", "import"])
        .stdin(export_stdout)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("SSH connection failed: {e}"))?;

    // Wait for both processes
    let import_output = import_proc
        .wait_with_output()
        .map_err(|e| format!("failed to read import output: {e}"))?;
    let export_output = export_proc
        .wait_with_output()
        .map_err(|e| format!("failed to read export output: {e}"))?;

    // Check export exit code
    if !export_output.status.success() {
        let stderr = String::from_utf8_lossy(&export_output.stderr);
        return Err(format!("export failed: {stderr}").into());
    }

    // Check import (SSH) exit code
    if !import_output.status.success() {
        let stderr = String::from_utf8_lossy(&import_output.stderr);
        let msg = if stderr.contains("command not found") {
            format!("scribe not found on remote '{remote}' — is it installed?")
        } else if stderr.contains("No such file or directory") {
            format!("scribe not found on remote '{remote}' — is it in PATH?")
        } else {
            format!("remote import failed: {stderr}")
        };
        // Log failure
        let _ = insert_sync_log(pool, remote, "push", 0, 0, "error", Some(&msg)).await;
        return Err(msg.into());
    }

    // Parse event counts from stderr
    let export_stderr = String::from_utf8_lossy(&export_output.stderr);
    let events_sent = parse_event_count(&export_stderr, "Exported");

    let import_stderr = String::from_utf8_lossy(&import_output.stderr);
    let events_received = parse_event_count(&import_stderr, "Imported");

    let duration = start.elapsed().as_secs_f64();

    // Log success
    let _ = insert_sync_log(
        pool,
        remote,
        "push",
        events_sent,
        events_received,
        "success",
        None,
    )
    .await;

    Ok(SyncResult {
        events_sent,
        events_received,
        direction: Direction::Push,
        duration_secs: duration,
    })
}

/// Pull events from a remote machine via SSH.
/// Pipeline: ssh <remote> scribe sync export [--since <ts>] | scribe sync import
pub async fn pull(
    pool: &SqlitePool,
    remote: &str,
    since: Option<&str>,
) -> Result<SyncResult, Box<dyn Error>> {
    let start = Instant::now();

    // Determine since: explicit > last sync > all
    let effective_since = match since {
        Some(s) => Some(s.to_string()),
        None => get_last_sync_timestamp(pool, remote).await?,
    };

    // Build remote export command
    let mut remote_args = vec![
        "scribe".to_string(),
        "sync".to_string(),
        "export".to_string(),
    ];
    if let Some(ref s) = effective_since {
        remote_args.push("--since".to_string());
        remote_args.push(s.clone());
    }

    let mut export_proc = Command::new("ssh")
        .arg(remote)
        .args(&remote_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("SSH connection failed: {e}"))?;

    let export_stdout = export_proc
        .stdout
        .take()
        .ok_or("failed to capture SSH stdout")?;

    // Pipe to local scribe sync import
    let scribe_bin = std::env::current_exe()?;

    let import_proc = Command::new(&scribe_bin)
        .args(["sync", "import"])
        .stdin(export_stdout)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start scribe import: {e}"))?;

    // Wait for both processes
    let import_output = import_proc
        .wait_with_output()
        .map_err(|e| format!("failed to read import output: {e}"))?;
    let export_output = export_proc
        .wait_with_output()
        .map_err(|e| format!("failed to read SSH output: {e}"))?;

    // Check SSH (export) exit code
    if !export_output.status.success() {
        let stderr = String::from_utf8_lossy(&export_output.stderr);
        let msg = if stderr.contains("command not found") {
            format!("scribe not found on remote '{remote}' — is it installed?")
        } else if stderr.contains("Connection refused")
            || stderr.contains("Could not resolve hostname")
        {
            format!("SSH connection to '{remote}' failed: {stderr}")
        } else {
            format!("remote export failed: {stderr}")
        };
        let _ = insert_sync_log(pool, remote, "pull", 0, 0, "error", Some(&msg)).await;
        return Err(msg.into());
    }

    // Check local import exit code
    if !import_output.status.success() {
        let stderr = String::from_utf8_lossy(&import_output.stderr);
        let _ = insert_sync_log(
            pool,
            remote,
            "pull",
            0,
            0,
            "error",
            Some(&format!("local import failed: {stderr}")),
        )
        .await;
        return Err(format!("local import failed: {stderr}").into());
    }

    // Parse event counts from stderr
    let export_stderr = String::from_utf8_lossy(&export_output.stderr);
    let events_sent = parse_event_count(&export_stderr, "Exported");

    let import_stderr = String::from_utf8_lossy(&import_output.stderr);
    let events_received = parse_event_count(&import_stderr, "Imported");

    let duration = start.elapsed().as_secs_f64();

    // Log success
    let _ = insert_sync_log(
        pool,
        remote,
        "pull",
        events_sent,
        events_received,
        "success",
        None,
    )
    .await;

    Ok(SyncResult {
        events_sent,
        events_received,
        direction: Direction::Pull,
        duration_secs: duration,
    })
}

/// Parse an event count from stderr output like "Exported 42 events" or "Imported 10 events".
fn parse_event_count(stderr: &str, prefix: &str) -> u64 {
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(prefix) {
            // "Exported 42 events" -> extract "42"
            if let Some(rest) = trimmed.strip_prefix(prefix) {
                let rest = rest.trim();
                if let Some(num_str) = rest.split_whitespace().next() {
                    if let Ok(n) = num_str.parse::<u64>() {
                        return n;
                    }
                }
            }
        }
    }
    0
}

/// Get the last successful sync timestamp for a remote peer.
async fn get_last_sync_timestamp(
    pool: &SqlitePool,
    remote: &str,
) -> Result<Option<String>, Box<dyn Error>> {
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT timestamp FROM sync_log \
         WHERE peer_id = ? AND status = 'success' \
         ORDER BY timestamp DESC LIMIT 1",
    )
    .bind(remote)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.0))
}

/// Insert a sync log entry.
async fn insert_sync_log(
    pool: &SqlitePool,
    peer_id: &str,
    direction: &str,
    events_sent: u64,
    events_received: u64,
    status: &str,
    error_message: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    sqlx::query(
        "INSERT INTO sync_log (peer_id, direction, events_sent, events_received, status, error_message) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(peer_id)
    .bind(direction)
    .bind(events_sent as i64)
    .bind(events_received as i64)
    .bind(status)
    .bind(error_message)
    .execute(pool)
    .await?;
    Ok(())
}

/// Format a sync result as a human-readable summary.
pub fn format_result(result: &SyncResult, remote: &str) -> String {
    let dir = match result.direction {
        Direction::Push => "push",
        Direction::Pull => "pull",
    };
    format!(
        "Synced with {remote}\n  Direction: {dir}\n  Events sent: {}\n  Events received: {}\n  Duration: {:.1}s",
        result.events_sent, result.events_received, result.duration_secs
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event_count_exported() {
        assert_eq!(parse_event_count("Exported 42 events", "Exported"), 42);
        assert_eq!(parse_event_count("Exported 0 events", "Exported"), 0);
        assert_eq!(
            parse_event_count("some noise\nExported 100 events\nmore noise", "Exported"),
            100
        );
    }

    #[test]
    fn test_parse_event_count_imported() {
        assert_eq!(
            parse_event_count("Imported 10 events (skipped 5, errors 0)", "Imported"),
            10
        );
    }

    #[test]
    fn test_parse_event_count_missing() {
        assert_eq!(parse_event_count("no match here", "Exported"), 0);
        assert_eq!(parse_event_count("", "Exported"), 0);
    }

    #[test]
    fn test_format_result() {
        let result = SyncResult {
            events_sent: 42,
            events_received: 10,
            direction: Direction::Push,
            duration_secs: 1.234,
        };
        let output = format_result(&result, "user@host");
        assert!(output.contains("push"));
        assert!(output.contains("42"));
        assert!(output.contains("10"));
        assert!(output.contains("1.2s"));
    }

    #[tokio::test]
    async fn test_sync_log_insert() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::connect(db_path.to_str().unwrap()).await.unwrap();

        // Need to insert a sync_peer first (FK constraint)
        sqlx::query(
            "INSERT INTO sync_peers (machine_id, machine_name, public_key) VALUES ('remote', 'test', 'age1...')",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_sync_log(&pool, "remote", "push", 42, 0, "success", None)
            .await
            .unwrap();

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM sync_log")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count.0, 1);
    }

    #[tokio::test]
    async fn test_last_sync_timestamp() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let pool = crate::db::connect(db_path.to_str().unwrap()).await.unwrap();

        // No sync history
        let ts = get_last_sync_timestamp(&pool, "remote").await.unwrap();
        assert!(ts.is_none());

        // Insert a peer and sync log
        sqlx::query(
            "INSERT INTO sync_peers (machine_id, machine_name, public_key) VALUES ('remote', 'test', 'age1...')",
        )
        .execute(&pool)
        .await
        .unwrap();

        insert_sync_log(&pool, "remote", "push", 10, 0, "success", None)
            .await
            .unwrap();

        let ts = get_last_sync_timestamp(&pool, "remote").await.unwrap();
        assert!(ts.is_some());
    }
}
