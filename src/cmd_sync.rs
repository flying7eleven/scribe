use std::error::Error;

use clap::Subcommand;
use sqlx::SqlitePool;

use crate::sync::crypto;

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

pub async fn handle(cmd: SyncCommand, pool: &SqlitePool) -> Result<(), Box<dyn Error>> {
    match cmd {
        SyncCommand::Keypair { command } => handle_keypair(command, pool).await,
        SyncCommand::Push { .. } => todo!("US-0057: sync push"),
        SyncCommand::Pull { .. } => todo!("US-0057: sync pull"),
        SyncCommand::Status => todo!("US-0058: sync status"),
        SyncCommand::Export { .. } => todo!("US-0055: sync export"),
        SyncCommand::Import => todo!("US-0055: sync import"),
    }
}

async fn handle_keypair(cmd: KeypairCommand, pool: &SqlitePool) -> Result<(), Box<dyn Error>> {
    match cmd {
        KeypairCommand::Generate { force } => {
            let machine_id = crypto::machine_id()?;
            let public_key = crypto::generate_keypair(force)?;

            // Backfill origin_machine_id on existing events
            let updated = crate::db::backfill_origin_machine_id(pool, &machine_id).await?;
            if updated > 0 {
                eprintln!("Backfilled origin_machine_id on {updated} existing events");
            }

            println!("Machine ID: {machine_id}");
            println!("Public key: {public_key}");
            println!();
            println!("Share this public key with peers via:");
            println!("  scribe sync keypair add <name> {public_key}");
        }
        KeypairCommand::Show => {
            let public_key = crypto::local_public_key()?;
            println!("{public_key}");
        }
        KeypairCommand::Add { name, public_key } => {
            crypto::add_peer(&name, &public_key)?;
            println!("Added peer '{name}'");
        }
        KeypairCommand::List => {
            let machine_id = crypto::machine_id()?;
            let local_key = crypto::local_public_key().ok();
            let peers = crypto::list_peers()?;

            println!("Local machine ({machine_id}):");
            if let Some(key) = local_key {
                println!("  {key}");
            } else {
                println!("  (no keypair — run 'scribe sync keypair generate')");
            }
            println!();

            if peers.is_empty() {
                println!("No peers configured.");
            } else {
                // Find max name length for alignment
                let max_name = peers.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
                println!("{:<width$}  Public Key", "Name", width = max_name);
                for (name, key) in &peers {
                    println!("{:<width$}  {key}", name, width = max_name);
                }
            }
        }
        KeypairCommand::Remove { name } => {
            crypto::remove_peer(&name)?;
            println!("Removed peer '{name}'");
        }
    }
    Ok(())
}
