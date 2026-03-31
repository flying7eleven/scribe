use std::error::Error;

use serde::Serialize;
use sqlx::SqlitePool;

use crate::db::{self, ModelTokenUsage, TokenUsageSummary, ToolTokenUsage};
use crate::format::{format_count, format_percentage, format_token_estimate};

#[derive(Serialize)]
struct UsageJson {
    window: WindowJson,
    weekly: WindowJson,
    by_model: Vec<ModelJson>,
    by_tool: Vec<ToolJson>,
}

#[derive(Serialize)]
struct WindowJson {
    label: String,
    sessions: i64,
    events: i64,
    est_input_tokens: i64,
    est_output_tokens: i64,
    est_total_tokens: i64,
}

#[derive(Serialize)]
struct ModelJson {
    model: String,
    est_tokens: i64,
    percentage: String,
}

#[derive(Serialize)]
struct ToolJson {
    tool_name: String,
    est_tokens: i64,
}

pub async fn run(
    pool: &SqlitePool,
    window: &str,
    weekly: &str,
    account: Option<&str>,
    json: bool,
) -> Result<(), Box<dyn Error>> {
    let now = chrono::Utc::now();

    let window_since = compute_since(&now, window)?;
    let weekly_since = compute_since(&now, weekly)?;

    let summary = db::token_usage_summary(pool, &window_since, account).await?;
    let weekly_summary = db::token_usage_summary(pool, &weekly_since, account).await?;
    let by_model = db::token_usage_by_model(pool, &window_since, account).await?;
    let by_tool = db::token_usage_by_tool(pool, &window_since, 5, account).await?;

    if json {
        print_json(
            &summary,
            &weekly_summary,
            &by_model,
            &by_tool,
            window,
            weekly,
        )?;
    } else {
        print_text(
            &summary,
            &weekly_summary,
            &by_model,
            &by_tool,
            window,
            weekly,
        );
    }

    Ok(())
}

fn compute_since(
    now: &chrono::DateTime<chrono::Utc>,
    duration_str: &str,
) -> Result<String, Box<dyn Error>> {
    let duration = humantime::parse_duration(duration_str)
        .map_err(|e| format!("invalid duration '{duration_str}': {e}"))?;
    let since = *now - chrono::Duration::from_std(duration)?;
    Ok(since.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

fn print_text(
    summary: &TokenUsageSummary,
    weekly: &TokenUsageSummary,
    by_model: &[ModelTokenUsage],
    by_tool: &[ToolTokenUsage],
    window_label: &str,
    weekly_label: &str,
) {
    let total_chars = summary.input_chars + summary.output_chars;

    println!("Token Usage Estimate (last {window_label})");
    println!(
        "  Sessions:           {}",
        format_count(summary.session_count)
    );
    println!(
        "  Events:             {}",
        format_count(summary.event_count)
    );
    println!(
        "  Est. input tokens:  {}",
        format_token_estimate(summary.input_chars)
    );
    println!(
        "  Est. output tokens: {}",
        format_token_estimate(summary.output_chars)
    );
    println!(
        "  Est. total:         {}",
        format_token_estimate(total_chars)
    );
    println!();
    println!("  Note: estimates are approximate lower bounds (conversation context not captured)");
    println!();

    let weekly_total = weekly.input_chars + weekly.output_chars;
    println!("Weekly Summary (last {weekly_label})");
    println!(
        "  Est. total tokens:  {}",
        format_token_estimate(weekly_total)
    );
    println!();

    if !by_model.is_empty() {
        let model_total: i64 = by_model.iter().map(|m| m.total_chars).sum();
        let max_name = by_model
            .iter()
            .map(|m| m.model.len())
            .max()
            .unwrap_or(5)
            .max(5);
        println!("By Model");
        for m in by_model {
            println!(
                "  {:<width$}  {} ({})",
                m.model,
                format_token_estimate(m.total_chars),
                format_percentage(m.total_chars, model_total),
                width = max_name
            );
        }
        println!();
    }

    if !by_tool.is_empty() {
        let max_name = by_tool
            .iter()
            .map(|t| t.tool_name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        println!("By Tool (top {})", by_tool.len());
        for t in by_tool {
            println!(
                "  {:<width$}  {}",
                t.tool_name,
                format_token_estimate(t.total_chars),
                width = max_name
            );
        }
    }
}

fn print_json(
    summary: &TokenUsageSummary,
    weekly: &TokenUsageSummary,
    by_model: &[ModelTokenUsage],
    by_tool: &[ToolTokenUsage],
    window_label: &str,
    weekly_label: &str,
) -> Result<(), Box<dyn Error>> {
    let total_chars = summary.input_chars + summary.output_chars;
    let weekly_total = weekly.input_chars + weekly.output_chars;
    let model_total: i64 = by_model.iter().map(|m| m.total_chars).sum();

    let json = UsageJson {
        window: WindowJson {
            label: window_label.to_string(),
            sessions: summary.session_count,
            events: summary.event_count,
            est_input_tokens: summary.input_chars / 4,
            est_output_tokens: summary.output_chars / 4,
            est_total_tokens: total_chars / 4,
        },
        weekly: WindowJson {
            label: weekly_label.to_string(),
            sessions: weekly.session_count,
            events: weekly.event_count,
            est_input_tokens: weekly.input_chars / 4,
            est_output_tokens: weekly.output_chars / 4,
            est_total_tokens: weekly_total / 4,
        },
        by_model: by_model
            .iter()
            .map(|m| ModelJson {
                model: m.model.clone(),
                est_tokens: m.total_chars / 4,
                percentage: format_percentage(m.total_chars, model_total),
            })
            .collect(),
        by_tool: by_tool
            .iter()
            .map(|t| ToolJson {
                tool_name: t.tool_name.clone(),
                est_tokens: t.total_chars / 4,
            })
            .collect(),
    };

    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}
