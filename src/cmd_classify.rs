//! `scribe classify` subcommand: process historical tool events through the heuristic engine.

use sqlx::SqlitePool;

use crate::classify;
use crate::cmd_query;
use crate::db;
use crate::format::format_count;

/// Run the classify subcommand.
pub async fn run(
    pool: &SqlitePool,
    since: Option<String>,
    details: bool,
    risk_filter: Option<String>,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let resolved_since = since.map(|s| cmd_query::parse_time_spec(&s)).transpose()?;
    let since_ref = resolved_since.as_deref();

    // Query tool events (events with a tool_name)
    let filter = db::EventFilter {
        since: resolved_since.clone(),
        until: None,
        session_id: None,
        event_type: None,
        tool_name: None,
        search: None,
        account: None,
        limit: 100_000, // high limit for classification runs
    };
    let events = db::query_events(pool, &filter).await?;

    let tool_events: Vec<_> = events.iter().filter(|e| e.tool_name.is_some()).collect();

    // Classify and insert (idempotent — skip already-classified)
    let mut classified_count = 0i64;
    let mut skipped_count = 0i64;
    let mut unclassified_count = 0i64;

    for event in &tool_events {
        // Check if already classified
        if db::has_classification_for_event(pool, event.id).await? {
            skipped_count += 1;
            continue;
        }

        let tool_input: Option<serde_json::Value> = event
            .tool_input
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        let result = classify::classify_tool_call(
            event.tool_name.as_deref().unwrap_or(""),
            tool_input.as_ref(),
            event.cwd.as_deref(),
        );

        if let Some(classification) = result {
            db::insert_classification(pool, Some(event.id), &classification).await?;
            classified_count += 1;
        } else {
            unclassified_count += 1;
        }
    }

    // Get summary
    let summary = db::classification_summary(pool, since_ref).await?;

    if json {
        output_json(&summary, unclassified_count, &risk_filter)?;
    } else {
        output_table(&summary, unclassified_count, since_ref, &risk_filter);

        if details {
            output_details(pool, since_ref, &risk_filter).await?;
        }

        // Processing summary
        if classified_count > 0 || skipped_count > 0 {
            println!();
            println!(
                "Processed: {} new, {} skipped (already classified), {} unclassified",
                classified_count, skipped_count, unclassified_count
            );
        }
    }

    Ok(())
}

fn output_table(
    summary: &[db::ClassificationCount],
    unclassified: i64,
    since: Option<&str>,
    risk_filter: &Option<String>,
) {
    let since_label = since
        .map(|s| format!(" (since {})", crate::format::format_timestamp(s)))
        .unwrap_or_default();

    println!("Classification Summary{since_label}");
    println!("{}", "\u{2500}".repeat(40));

    let total: i64 = summary.iter().map(|c| c.count).sum::<i64>() + unclassified;

    for entry in summary {
        if let Some(ref filter) = risk_filter {
            if &entry.risk_level != filter {
                continue;
            }
        }
        let pct = if total > 0 {
            entry.count as f64 / total as f64 * 100.0
        } else {
            0.0
        };
        println!(
            "  {:<12} {:>8}   ({:.1}%)",
            entry.risk_level,
            format_count(entry.count),
            pct
        );
    }

    println!("{}", "\u{2500}".repeat(40));
    println!("  {:<12} {:>8}", "total", format_count(total));
    if unclassified > 0 {
        println!("  {:<12} {:>8}", "unclassified", format_count(unclassified));
    }
}

fn output_json(
    summary: &[db::ClassificationCount],
    unclassified: i64,
    risk_filter: &Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let entries: Vec<serde_json::Value> = summary
        .iter()
        .filter(|c| risk_filter.as_ref().is_none_or(|f| &c.risk_level == f))
        .map(|c| {
            serde_json::json!({
                "risk_level": c.risk_level,
                "count": c.count,
            })
        })
        .collect();

    let total: i64 = summary.iter().map(|c| c.count).sum::<i64>() + unclassified;

    let output = serde_json::json!({
        "summary": entries,
        "total": total,
        "unclassified": unclassified,
    });

    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

async fn output_details(
    pool: &SqlitePool,
    since: Option<&str>,
    risk_filter: &Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut sql = String::from(
        "SELECT tool_name, input_pattern, risk_level, heuristic, reason FROM classifications WHERE 1=1",
    );
    let mut binds: Vec<String> = Vec::new();

    if let Some(s) = since {
        sql.push_str(" AND timestamp >= ?");
        binds.push(s.to_string());
    }
    if let Some(ref r) = risk_filter {
        sql.push_str(" AND risk_level = ?");
        binds.push(r.to_string());
    }
    sql.push_str(" ORDER BY risk_level DESC, timestamp DESC LIMIT 100");

    let mut query = sqlx::query(&sql);
    for b in &binds {
        query = query.bind(b);
    }

    let rows = query.fetch_all(pool).await?;

    if rows.is_empty() {
        return Ok(());
    }

    println!();
    println!(
        "{:<10} {:<8} {:<26} {:<22} Reason",
        "Risk", "Tool", "Pattern", "Heuristic"
    );
    println!(
        "{:<10} {:<8} {:<26} {:<22} {}",
        "\u{2500}".repeat(9),
        "\u{2500}".repeat(7),
        "\u{2500}".repeat(25),
        "\u{2500}".repeat(21),
        "\u{2500}".repeat(30),
    );

    use sqlx::Row;
    for row in &rows {
        let risk: String = row.get("risk_level");
        let tool: String = row.get("tool_name");
        let pattern: String = row.get("input_pattern");
        let heuristic: String = row.get("heuristic");
        let reason: String = row.get("reason");

        let truncated_pattern = if pattern.len() > 25 {
            format!("{}...", &pattern[..22])
        } else {
            pattern
        };
        let truncated_reason = if reason.len() > 30 {
            format!("{}...", &reason[..27])
        } else {
            reason
        };

        println!(
            "{:<10} {:<8} {:<26} {:<22} {}",
            risk, tool, truncated_pattern, heuristic, truncated_reason
        );
    }

    Ok(())
}
