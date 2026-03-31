use comfy_table::{ContentArrangement, Table};
use serde_json::json;
use sqlx::SqlitePool;

use crate::db;
use crate::format::format_count;

/// Display all known accounts with session/event counts and last seen.
pub async fn run_list(pool: &SqlitePool, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let accounts = db::account_list(pool).await?;

    if accounts.is_empty() {
        eprintln!("No accounts found.");
        return Ok(());
    }

    if json {
        print_json(&accounts);
    } else {
        print_table(&accounts);
    }

    Ok(())
}

fn print_table(accounts: &[db::AccountListRow]) {
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec!["ACCOUNT", "SESSIONS", "EVENTS", "LAST SEEN"]);

    for a in accounts {
        let label = a.account_email.as_deref().unwrap_or(&a.account_id);
        let ago = format_relative_time(&a.last_seen);
        table.add_row(vec![
            label.to_string(),
            format_count(a.session_count),
            format_count(a.event_count),
            ago,
        ]);
    }

    println!("{table}");
}

fn print_json(accounts: &[db::AccountListRow]) {
    let arr: Vec<_> = accounts
        .iter()
        .map(|a| {
            json!({
                "account_id": a.account_id,
                "account_email": a.account_email,
                "session_count": a.session_count,
                "event_count": a.event_count,
                "last_seen": a.last_seen,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&arr).unwrap());
}

/// Format an ISO 8601 timestamp as a relative "ago" string.
fn format_relative_time(ts: &str) -> String {
    let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.fZ") else {
        return ts.to_string();
    };
    let now = chrono::Utc::now().naive_utc();
    let diff = now - dt;

    let minutes = diff.num_minutes();
    let hours = diff.num_hours();
    let days = diff.num_days();

    if minutes < 1 {
        "just now".to_string()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if hours < 24 {
        format!("{hours}h ago")
    } else {
        format!("{days}d ago")
    }
}
