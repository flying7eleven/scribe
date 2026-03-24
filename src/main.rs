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
        #[arg(long)]
        project: bool,
        /// Write to ~/.claude/settings.json (global)
        #[arg(long)]
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

    match cli.command {
        Commands::Log => {
            eprintln!("scribe log: not yet implemented");
        }
        Commands::Query => {
            eprintln!("scribe query: not yet implemented");
        }
        Commands::Init { .. } => {
            eprintln!("scribe init: not yet implemented");
        }
        Commands::Retain { .. } => {
            eprintln!("scribe retain: not yet implemented");
        }
        Commands::Stats => {
            eprintln!("scribe stats: not yet implemented");
        }
        Commands::Completions { .. } => {
            eprintln!("scribe completions: not yet implemented");
        }
    }
}
