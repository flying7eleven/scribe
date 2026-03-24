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
    Query,
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

    match cli.command {
        Commands::Log => {
            let pool = match db::connect(&db_path).await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("scribe: db connect error: {e}");
                    return;
                }
            };
            if let Err(e) = cmd_log::run(&pool).await {
                eprintln!("scribe: log error: {e}");
            }
        }
        Commands::Query => {
            eprintln!("scribe query: not yet implemented (db: {db_path})");
        }
        Commands::Retain { .. } => {
            eprintln!("scribe retain: not yet implemented (db: {db_path})");
        }
        Commands::Stats => {
            eprintln!("scribe stats: not yet implemented (db: {db_path})");
        }
        // Init and Completions handled above
        Commands::Init { .. } | Commands::Completions { .. } => unreachable!(),
    }
}
