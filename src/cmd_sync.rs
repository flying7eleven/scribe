use std::error::Error;

use clap::Subcommand;
use sqlx::SqlitePool;

#[derive(Subcommand)]
pub enum SyncCommand {
    /// Generate, show, or manage age encryption keypairs
    Keypair {
        #[command(subcommand)]
        command: KeypairCommand,
    },
    /// Push local events to a remote machine
    Push {
        /// Remote host (e.g., user@hostname or SSH config alias)
        remote: String,
        /// Only export events after this time (duration or date)
        #[arg(long)]
        since: Option<String>,
    },
    /// Pull events from a remote machine
    Pull {
        /// Remote host (e.g., user@hostname or SSH config alias)
        remote: String,
        /// Only import events after this time (duration or date)
        #[arg(long)]
        since: Option<String>,
    },
    /// Show sync status and peer information
    Status,
    /// Export encrypted sync bundle to stdout
    #[clap(hide = true)]
    Export {
        /// Only export events after this timestamp
        #[arg(long)]
        since: Option<String>,
    },
    /// Import encrypted sync bundle from stdin
    #[clap(hide = true)]
    Import,
}

#[derive(Subcommand)]
pub enum KeypairCommand {
    /// Generate a new age keypair for this machine
    Generate {
        /// Overwrite existing keypair
        #[arg(long)]
        force: bool,
    },
    /// Show this machine's public key
    Show,
    /// Add a peer's public key
    Add {
        /// Display name for the peer
        name: String,
        /// The peer's age public key (starts with age1...)
        public_key: String,
    },
    /// List known peers and their public keys
    List,
    /// Remove a peer's public key
    Remove {
        /// Name of the peer to remove
        name: String,
    },
}

pub async fn handle(cmd: SyncCommand, _pool: &SqlitePool) -> Result<(), Box<dyn Error>> {
    match cmd {
        SyncCommand::Keypair { command } => match command {
            KeypairCommand::Generate { .. } => todo!("US-0053: keypair generate"),
            KeypairCommand::Show => todo!("US-0053: keypair show"),
            KeypairCommand::Add { .. } => todo!("US-0053: keypair add"),
            KeypairCommand::List => todo!("US-0053: keypair list"),
            KeypairCommand::Remove { .. } => todo!("US-0053: keypair remove"),
        },
        SyncCommand::Push { .. } => todo!("US-0057: sync push"),
        SyncCommand::Pull { .. } => todo!("US-0057: sync pull"),
        SyncCommand::Status => todo!("US-0058: sync status"),
        SyncCommand::Export { .. } => todo!("US-0055: sync export"),
        SyncCommand::Import => todo!("US-0055: sync import"),
    }
}
