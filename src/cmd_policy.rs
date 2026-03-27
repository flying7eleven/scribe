use clap::Subcommand;
use comfy_table::{ContentArrangement, Table};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::format::format_count;

#[derive(Subcommand)]
pub enum PolicyCommand {
    /// List all rules
    List {
        /// Show disabled rules too
        #[arg(long)]
        all: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Add a new rule
    Add {
        /// Regex pattern for tool name
        #[arg(long)]
        tool: String,
        /// Regex pattern for tool input (optional)
        #[arg(long)]
        input: Option<String>,
        /// Action: allow or deny
        #[arg(long)]
        action: String,
        /// Human-readable reason
        #[arg(long)]
        reason: String,
        /// Priority (higher = evaluated first, default: 0)
        #[arg(long, default_value = "0")]
        priority: i64,
    },
    /// Remove a rule by ID
    Remove {
        /// Rule ID to remove
        id: i64,
    },
    /// Enable a rule by ID
    Enable {
        /// Rule ID to enable
        id: i64,
    },
    /// Disable a rule by ID
    Disable {
        /// Rule ID to disable
        id: i64,
    },
    /// Export rules to TOML file
    Export {
        /// Output file path (default: stdout)
        #[arg(long)]
        file: Option<String>,
    },
    /// Import rules from TOML file
    Import {
        /// Input file path
        file: String,
        /// Replace all existing rules (default: merge)
        #[arg(long)]
        replace: bool,
    },
    /// Show enforcement statistics
    Stats {
        /// Only show stats since this time
        #[arg(long)]
        since: Option<String>,
    },
    /// Promote a classification pattern to a rule
    Promote {
        /// Classification ID to promote
        id: i64,
        /// Override action (default: based on risk level)
        #[arg(long)]
        action: Option<String>,
        /// Override priority
        #[arg(long)]
        priority: Option<i64>,
    },
}

#[derive(Serialize, Deserialize)]
struct PolicyExport {
    rules: Vec<RuleEntry>,
}

#[derive(Serialize, Deserialize)]
struct RuleEntry {
    tool_pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_pattern: Option<String>,
    action: String,
    reason: String,
    priority: i64,
}

/// JSON representation of a rule for --json output.
#[derive(Serialize)]
struct RuleJson {
    id: i64,
    tool_pattern: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    input_pattern: Option<String>,
    action: String,
    reason: String,
    priority: i64,
    enabled: bool,
}

pub async fn run(
    pool: &SqlitePool,
    command: PolicyCommand,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        PolicyCommand::List { all, json } => run_list(pool, all, json).await,
        PolicyCommand::Add {
            tool,
            input,
            action,
            reason,
            priority,
        } => run_add(pool, &tool, input.as_deref(), &action, &reason, priority).await,
        PolicyCommand::Remove { id } => run_remove(pool, id).await,
        PolicyCommand::Enable { id } => run_enable(pool, id).await,
        PolicyCommand::Disable { id } => run_disable(pool, id).await,
        PolicyCommand::Export { file } => run_export(pool, file.as_deref()).await,
        PolicyCommand::Import { file, replace } => run_import(pool, &file, replace).await,
        PolicyCommand::Stats { since } => run_stats(pool, since.as_deref()).await,
        PolicyCommand::Promote {
            id,
            action,
            priority,
        } => run_promote(pool, id, action.as_deref(), priority).await,
    }
}

async fn run_list(
    pool: &SqlitePool,
    include_all: bool,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let rules = crate::db::list_rules(pool, include_all).await?;

    if rules.is_empty() {
        println!("No rules found.");
        return Ok(());
    }

    if json {
        let json_rules: Vec<RuleJson> = rules
            .iter()
            .map(|r| RuleJson {
                id: r.id,
                tool_pattern: r.tool_pattern.clone(),
                input_pattern: r.input_pattern.clone(),
                action: r.action.clone(),
                reason: r.reason.clone(),
                priority: r.priority,
                enabled: r.enabled,
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_rules)?);
        return Ok(());
    }

    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.set_header(vec![
        "ID",
        "Priority",
        "Action",
        "Tool Pattern",
        "Input Pattern",
        "Reason",
        "Enabled",
    ]);

    for rule in &rules {
        table.add_row(vec![
            rule.id.to_string(),
            rule.priority.to_string(),
            rule.action.clone(),
            rule.tool_pattern.clone(),
            rule.input_pattern.clone().unwrap_or_default(),
            rule.reason.clone(),
            if rule.enabled {
                "yes".to_string()
            } else {
                "no".to_string()
            },
        ]);
    }

    println!("{table}");
    Ok(())
}

async fn run_add(
    pool: &SqlitePool,
    tool: &str,
    input: Option<&str>,
    action: &str,
    reason: &str,
    priority: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate tool regex
    if let Err(e) = regex::Regex::new(tool) {
        return Err(format!("invalid --tool regex: {e}").into());
    }

    // Validate input regex if provided
    if let Some(input_pat) = input {
        if let Err(e) = regex::Regex::new(input_pat) {
            return Err(format!("invalid --input regex: {e}").into());
        }
    }

    // Validate action
    if action != "allow" && action != "deny" {
        return Err(format!("invalid --action: must be 'allow' or 'deny', got '{action}'").into());
    }

    let id = crate::db::insert_rule(pool, tool, input, action, reason, priority, "user").await?;
    println!("Rule added (ID: {id})");
    Ok(())
}

async fn run_remove(pool: &SqlitePool, id: i64) -> Result<(), Box<dyn std::error::Error>> {
    if crate::db::delete_rule(pool, id).await? {
        println!("Rule {id} removed.");
    } else {
        return Err(format!("rule {id} not found").into());
    }
    Ok(())
}

async fn run_enable(pool: &SqlitePool, id: i64) -> Result<(), Box<dyn std::error::Error>> {
    if crate::db::update_rule_enabled(pool, id, true).await? {
        println!("Rule {id} enabled.");
    } else {
        return Err(format!("rule {id} not found").into());
    }
    Ok(())
}

async fn run_disable(pool: &SqlitePool, id: i64) -> Result<(), Box<dyn std::error::Error>> {
    if crate::db::update_rule_enabled(pool, id, false).await? {
        println!("Rule {id} disabled.");
    } else {
        return Err(format!("rule {id} not found").into());
    }
    Ok(())
}

async fn run_export(
    pool: &SqlitePool,
    file: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let rules = crate::db::list_rules(pool, false).await?;

    let export = PolicyExport {
        rules: rules
            .iter()
            .map(|r| RuleEntry {
                tool_pattern: r.tool_pattern.clone(),
                input_pattern: r.input_pattern.clone(),
                action: r.action.clone(),
                reason: r.reason.clone(),
                priority: r.priority,
            })
            .collect(),
    };

    let toml_str = toml::to_string_pretty(&export)?;

    if let Some(path) = file {
        std::fs::write(path, &toml_str)?;
        println!("Exported {} rules to {path}", export.rules.len());
    } else {
        print!("{toml_str}");
    }

    Ok(())
}

async fn run_import(
    pool: &SqlitePool,
    file: &str,
    replace: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let contents = std::fs::read_to_string(file)?;
    let export: PolicyExport = toml::from_str(&contents)?;

    if replace {
        let deleted = crate::db::delete_all_rules(pool).await?;
        if deleted > 0 {
            eprintln!("Deleted {deleted} existing rules.");
        }
    }

    let mut count = 0u64;
    for rule in &export.rules {
        crate::db::insert_rule(
            pool,
            &rule.tool_pattern,
            rule.input_pattern.as_deref(),
            &rule.action,
            &rule.reason,
            rule.priority,
            "imported",
        )
        .await?;
        count += 1;
    }

    println!("Imported {count} rules from {file}");
    Ok(())
}

async fn run_stats(
    pool: &SqlitePool,
    since: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let since_ts = since.map(crate::cmd_query::parse_time_spec).transpose()?;

    let stats = crate::db::enforcement_stats(pool, since_ts.as_deref()).await?;

    let label = if let Some(s) = since {
        format!("Enforcement Statistics (since {s})")
    } else {
        "Enforcement Statistics (all time)".to_string()
    };

    println!("{label}");
    println!("{}", "\u{2500}".repeat(label.len()));

    let allowed_pct = if stats.total > 0 {
        format!("{:.1}%", stats.allowed as f64 / stats.total as f64 * 100.0)
    } else {
        "0.0%".to_string()
    };
    let denied_pct = if stats.total > 0 {
        format!("{:.1}%", stats.denied as f64 / stats.total as f64 * 100.0)
    } else {
        "0.0%".to_string()
    };

    println!("  Total enforcements:  {}", format_count(stats.total));
    println!(
        "  Allowed:             {}  ({allowed_pct})",
        format_count(stats.allowed)
    );
    println!(
        "  Denied:              {}  ({denied_pct})",
        format_count(stats.denied)
    );

    if !stats.top_denied.is_empty() {
        println!();
        println!("Top Denied Rules:");

        let mut table = Table::new();
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.set_header(vec!["ID", "Denials", "Reason"]);

        for entry in &stats.top_denied {
            table.add_row(vec![
                entry.rule_id.to_string(),
                format_count(entry.count),
                entry.reason.clone(),
            ]);
        }

        println!("{table}");
    }

    Ok(())
}

async fn run_promote(
    pool: &SqlitePool,
    id: i64,
    action_override: Option<&str>,
    priority_override: Option<i64>,
) -> Result<(), Box<dyn std::error::Error>> {
    let classification = crate::db::get_classification(pool, id)
        .await?
        .ok_or_else(|| format!("classification {id} not found"))?;

    // Map risk level to default action
    let default_action = match classification.risk_level.as_str() {
        "dangerous" | "risky" => "deny",
        "safe" => "allow",
        _ => "deny",
    };

    let action = action_override.unwrap_or(default_action);

    // Validate action override
    if action != "allow" && action != "deny" {
        return Err(format!("invalid --action: must be 'allow' or 'deny', got '{action}'").into());
    }

    // Map risk level to default priority
    let default_priority = match classification.risk_level.as_str() {
        "dangerous" => 100,
        "risky" => 50,
        _ => 0,
    };

    let priority = priority_override.unwrap_or(default_priority);

    // Escape the tool name for regex
    let tool_pattern = regex::escape(&classification.tool_name);

    let rule_id = crate::db::insert_rule(
        pool,
        &tool_pattern,
        Some(&classification.input_pattern),
        action,
        &classification.reason,
        priority,
        "promoted",
    )
    .await?;

    println!("Promoted classification {id} to rule {rule_id}:");
    println!("  Tool:     {tool_pattern}");
    println!("  Input:    {}", classification.input_pattern);
    println!("  Action:   {action}");
    println!("  Priority: {priority}");
    println!("  Source:   promoted");

    Ok(())
}
