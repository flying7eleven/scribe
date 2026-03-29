#[cfg(feature = "guard")]
pub mod classify;
mod cmd_backfill;
#[cfg(feature = "guard")]
mod cmd_classify;
mod cmd_completions;
#[cfg(feature = "guard")]
mod cmd_guard;
mod cmd_init;
mod cmd_log;
#[cfg(feature = "guard")]
mod cmd_policy;
mod cmd_query;
mod cmd_retain;
mod cmd_stats;
mod config;
mod db;
mod format;
mod models;
mod tui;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "scribe",
    version,
    about = "Audit logger for Claude Code hook events"
)]
pub struct Cli {
    /// Path to the SQLite database (overrides SCRIBE_DB env var and default)
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Read hook JSON from stdin and write to SQLite
    Log,
    /// Query the audit log with filters
    Query {
        #[command(subcommand)]
        sub: Option<QuerySub>,

        /// Show events since (duration like "1h" or date like "2025-06-01")
        #[arg(long)]
        since: Option<String>,
        /// Show events until (duration or date)
        #[arg(long)]
        until: Option<String>,
        /// Filter by session ID
        #[arg(long)]
        session: Option<String>,
        /// Filter by event type (e.g., PreToolUse, PostToolUse)
        #[arg(long)]
        event: Option<String>,
        /// Filter by tool name (e.g., Bash, Write, Edit)
        #[arg(long)]
        tool: Option<String>,
        /// Search in tool_input JSON
        #[arg(long)]
        search: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<i64>,
        /// Output as JSON Lines
        #[arg(long, conflicts_with = "csv")]
        json: bool,
        /// Output as CSV
        #[arg(long, conflicts_with = "json")]
        csv: bool,
    },
    /// Generate Claude Code hook configuration
    Init {
        /// Write to .claude/settings.json (project-level)
        #[arg(long, conflicts_with = "global")]
        project: bool,
        /// Write to ~/.claude/settings.json (global)
        #[arg(long, conflicts_with = "project")]
        global: bool,
        /// Include scribe guard on PreToolUse for policy enforcement
        #[cfg(feature = "guard")]
        #[arg(long)]
        with_guard: bool,
    },
    /// Delete events older than the given duration
    Retain {
        /// Duration (e.g. 90d, 30d, 1h)
        duration: String,
    },
    /// Show database metrics
    Stats {
        /// Show stats since a duration (e.g. 7d) or date (e.g. 2025-06-01)
        #[arg(long)]
        since: Option<String>,
        /// Output stats as a single JSON object
        #[arg(long)]
        json: bool,
    },
    /// Print shell completion script to stdout
    Completions {
        /// Shell to generate completions for (bash, zsh, fish, elvish, powershell)
        shell: clap_complete::Shell,
    },
    /// Classify historical tool calls by risk level
    #[cfg(feature = "guard")]
    Classify {
        /// Only classify events after this time (duration or date)
        #[arg(long)]
        since: Option<String>,
        /// Show per-event classification details
        #[arg(long)]
        details: bool,
        /// Filter by risk level: safe, risky, dangerous
        #[arg(long)]
        risk: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Manage policy enforcement rules
    #[cfg(feature = "guard")]
    Policy {
        #[command(subcommand)]
        command: cmd_policy::PolicyCommand,
    },
    /// Evaluate tool call against policy rules (PreToolUse hook)
    #[cfg(feature = "guard")]
    Guard,
    /// Backfill detail tables from existing event raw_payload data
    Backfill {
        /// Show what would be backfilled without modifying the database
        #[arg(long)]
        dry_run: bool,
        /// Number of events per transaction batch
        #[arg(long, default_value = "1000")]
        batch_size: usize,
    },
    /// Launch interactive terminal user interface
    Tui {
        /// Polling interval in ms for the Live tab (default: 1000)
        #[arg(long, default_value = "1000")]
        tick_rate: u64,
        /// Pre-filter initial data load (duration or date)
        #[arg(long)]
        since: Option<String>,
    },
}

#[derive(Subcommand)]
enum QuerySub {
    /// Show session summaries
    Sessions {
        /// Show sessions since (duration or date)
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of results
        #[arg(long)]
        limit: Option<i64>,
        /// Output as JSON Lines
        #[arg(long, conflicts_with = "csv")]
        json: bool,
        /// Output as CSV
        #[arg(long, conflicts_with = "json")]
        csv: bool,
    },
}

/// Returns true for subcommands that are user-interactive (not the `log` hot path).
fn is_interactive_command(cmd: &Commands) -> bool {
    match cmd {
        Commands::Log => false,
        #[cfg(feature = "guard")]
        Commands::Guard => false,
        _ => true,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();

    // Auto-create and migrate config on interactive runs (never on `log` hot path)
    if is_interactive_command(&cli.command) {
        let _ = config::ensure_config_exists();
        if let Ok(Some(report)) = config::migrate_config() {
            eprintln!("{}", config::format_migration_report(&report));
        }
    }

    let config = config::load_config();

    // Commands that don't need a database connection
    match cli.command {
        Commands::Init {
            project,
            global,
            #[cfg(feature = "guard")]
            with_guard,
        } => {
            let target = if project {
                cmd_init::OutputTarget::Project
            } else if global {
                cmd_init::OutputTarget::Global
            } else {
                cmd_init::OutputTarget::Stdout
            };
            #[cfg(feature = "guard")]
            let guard = with_guard;
            #[cfg(not(feature = "guard"))]
            let guard = false;
            if let Err(e) = cmd_init::run(target, None, guard) {
                eprintln!("scribe: init error: {e}");
                std::process::exit(1);
            }
            return;
        }
        Commands::Completions { shell } => {
            cmd_completions::run(shell);
            return;
        }
        _ => {}
    }

    // Commands that need a database connection
    let cli_db = cli.db.as_ref().and_then(|p| p.to_str());
    let db_path = match db::resolve_db_path(cli_db, config.db_path.as_deref()) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("scribe: failed to resolve database path: {e}");
            return;
        }
    };

    let pool = match db::connect(&db_path).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("scribe: db connect error: {e}");
            return;
        }
    };

    match cli.command {
        Commands::Log => {
            let retention = config.retention.as_ref().map(|r| cmd_log::RetentionConfig {
                retention: r.clone(),
                check_interval: config
                    .retention_check_interval
                    .clone()
                    .unwrap_or_else(|| "24h".to_string()),
            });
            if let Err(e) = cmd_log::run(&pool, retention.as_ref()).await {
                eprintln!("scribe: log error: {e}");
            }
        }
        #[cfg(feature = "guard")]
        Commands::Guard => {
            let exit_code = cmd_guard::run(&pool).await;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
        }
        Commands::Query {
            sub,
            since,
            until,
            session,
            event,
            tool,
            search,
            limit,
            json,
            csv,
        } => {
            if let Some(QuerySub::Sessions {
                since: s_since,
                limit: s_limit,
                json: s_json,
                csv: s_csv,
            }) = sub
            {
                let s_since = match s_since.map(|s| cmd_query::parse_time_spec(&s)).transpose() {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("scribe: invalid --since value: {e}");
                        std::process::exit(1);
                    }
                };
                let filter = db::SessionFilter {
                    since: s_since,
                    limit: s_limit.or(config.default_query_limit).unwrap_or(50),
                };
                let format = if s_json {
                    cmd_query::OutputFormat::Json
                } else if s_csv {
                    cmd_query::OutputFormat::Csv
                } else {
                    cmd_query::OutputFormat::Table
                };
                if let Err(e) = cmd_query::run_sessions(&pool, filter, format).await {
                    eprintln!("scribe: query error: {e}");
                    std::process::exit(1);
                }
            } else {
                // Default: event query
                let since = match since.map(|s| cmd_query::parse_time_spec(&s)).transpose() {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("scribe: invalid --since value: {e}");
                        std::process::exit(1);
                    }
                };
                let until = match until.map(|s| cmd_query::parse_time_spec(&s)).transpose() {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("scribe: invalid --until value: {e}");
                        std::process::exit(1);
                    }
                };
                let filter = db::EventFilter {
                    since,
                    until,
                    session_id: session,
                    event_type: event,
                    tool_name: tool,
                    search,
                    limit: limit.or(config.default_query_limit).unwrap_or(50),
                };
                let format = if json {
                    cmd_query::OutputFormat::Json
                } else if csv {
                    cmd_query::OutputFormat::Csv
                } else {
                    cmd_query::OutputFormat::Table
                };
                if let Err(e) = cmd_query::run_events(&pool, filter, format).await {
                    eprintln!("scribe: query error: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Retain { duration } => {
            if let Err(e) = cmd_retain::run(&pool, &duration).await {
                eprintln!("scribe: retain error: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "guard")]
        Commands::Classify {
            since,
            details,
            risk,
            json,
        } => {
            if let Err(e) = cmd_classify::run(&pool, since, details, risk, json).await {
                eprintln!("scribe: classify error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Stats { since, json } => {
            if let Err(e) = cmd_stats::run(
                &pool,
                &db_path,
                since.as_deref(),
                json,
                config.max_session_duration.as_deref(),
            )
            .await
            {
                eprintln!("scribe: stats error: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "guard")]
        Commands::Policy { command } => {
            if let Err(e) = cmd_policy::run(&pool, command).await {
                eprintln!("scribe: policy error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Backfill {
            dry_run,
            batch_size,
        } => {
            if let Err(e) = cmd_backfill::run(&pool, dry_run, batch_size).await {
                eprintln!("scribe: backfill error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Tui { tick_rate, since } => {
            let tick = std::time::Duration::from_millis(tick_rate);
            if let Err(e) = tui::run(&pool, &db_path, tick, since).await {
                eprintln!("scribe: tui error: {e}");
                std::process::exit(1);
            }
        }
        Commands::Init { .. } | Commands::Completions { .. } => unreachable!(),
    }
}
