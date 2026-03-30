use std::error::Error;
use std::io::{self, BufReader, Read};

use clap::Subcommand;
use sqlx::SqlitePool;

use crate::sync::{bundle, crypto, merge};

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
        SyncCommand::Export { since } => handle_export(pool, since).await,
        SyncCommand::Import => handle_import(pool).await,
        SyncCommand::Push { .. } => todo!("US-0057: sync push"),
        SyncCommand::Pull { .. } => todo!("US-0057: sync pull"),
        SyncCommand::Status => todo!("US-0058: sync status"),
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

async fn handle_export(pool: &SqlitePool, since: Option<String>) -> Result<(), Box<dyn Error>> {
    let machine_id = crypto::machine_id()?;

    // Export events to an in-memory plaintext buffer
    let mut plaintext = Vec::new();
    let count = bundle::export_bundles(pool, since.as_deref(), &machine_id, &mut plaintext).await?;

    // Encrypt to stdout
    let stdout = io::stdout();
    let stdout_lock = stdout.lock();
    crypto::encrypt_stream(plaintext.as_slice(), stdout_lock)?;

    eprintln!("Exported {count} events");
    Ok(())
}

async fn handle_import(pool: &SqlitePool) -> Result<(), Box<dyn Error>> {
    // Read encrypted data from stdin
    let mut ciphertext = Vec::new();
    io::stdin().lock().read_to_end(&mut ciphertext)?;

    if ciphertext.is_empty() {
        eprintln!("Imported 0 events");
        return Ok(());
    }

    // Decrypt
    let mut plaintext = Vec::new();
    crypto::decrypt_stream(ciphertext.as_slice(), &mut plaintext)?;

    // Parse JSON Lines and merge
    let reader = BufReader::new(plaintext.as_slice());
    let bundles = bundle::import_bundles(reader);

    let stats = merge::merge_bundles(pool, bundles).await?;

    eprintln!(
        "Imported {} events (skipped {}, errors {})",
        stats.events_imported, stats.events_skipped, stats.errors
    );
    Ok(())
}
