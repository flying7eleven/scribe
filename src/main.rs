mod cmd_completions;
mod cmd_init;
mod cmd_log;
mod cmd_query;
mod cmd_retain;
mod cmd_stats;
mod db;
mod models;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "scribe",
    version,
    about = "Audit logger for Claude Code hook events"
)]
struct Cli {
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
        /// Maximum number of results (default: 50)
        #[arg(long, default_value_t = 50)]
        limit: i64,
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
    },
    /// Delete events older than the given duration
    Retain {
        /// Duration (e.g. 90d, 30d, 1h)
        duration: String,
    },
    /// Show database metrics
    Stats,
    /// Print shell completion script to stdout
    Completions {
        /// Shell to generate completions for (bash, zsh, fish)
        shell: String,
    },
}

#[derive(Subcommand)]
enum QuerySub {
    /// Show session summaries
    Sessions {
        /// Show sessions since (duration or date)
        #[arg(long)]
        since: Option<String>,
        /// Maximum number of results (default: 50)
        #[arg(long, default_value_t = 50)]
        limit: i64,
        /// Output as JSON Lines
        #[arg(long, conflicts_with = "csv")]
        json: bool,
        /// Output as CSV
        #[arg(long, conflicts_with = "json")]
        csv: bool,
    },
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let cli = Cli::parse();

    // Commands that don't need a database connection
    match cli.command {
        Commands::Init { project, global } => {
            let target = if project {
                cmd_init::OutputTarget::Project
            } else if global {
                cmd_init::OutputTarget::Global
            } else {
                cmd_init::OutputTarget::Stdout
            };
            if let Err(e) = cmd_init::run(target, None) {
                eprintln!("scribe: init error: {e}");
                std::process::exit(1);
            }
            return;
        }
        Commands::Completions { .. } => {
            eprintln!("scribe completions: not yet implemented");
            return;
        }
        _ => {}
    }

    // Commands that need a database connection
    let cli_db = cli.db.as_ref().and_then(|p| p.to_str());
    let db_path = match db::resolve_db_path(cli_db) {
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
            if let Err(e) = cmd_log::run(&pool).await {
                eprintln!("scribe: log error: {e}");
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
                    limit: s_limit,
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
                    limit,
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
        Commands::Stats => {
            eprintln!("scribe stats: not yet implemented (db: {db_path})");
        }
        Commands::Init { .. } | Commands::Completions { .. } => unreachable!(),
    }
}
