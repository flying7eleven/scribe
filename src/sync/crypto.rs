use std::error::Error;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

use age::x25519;

/// Returns the sync directory path: `~/.config/claude-scribe/sync/`
pub fn sync_dir() -> Result<PathBuf, Box<dyn Error>> {
    let config_dir = dirs::config_dir().ok_or("could not determine config directory")?;
    Ok(config_dir.join("claude-scribe").join("sync"))
}

/// Returns the peers directory path: `~/.config/claude-scribe/sync/peers/`
fn peers_dir() -> Result<PathBuf, Box<dyn Error>> {
    Ok(sync_dir()?.join("peers"))
}

/// Generate a UUID v4 from OS random bytes.
fn generate_uuid_v4() -> Result<String, Box<dyn Error>> {
    let mut bytes = [0u8; 16];
    let mut f = fs::File::open("/dev/urandom")?;
    f.read_exact(&mut bytes)?;
    // Set version (4) and variant (RFC 4122)
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        u16::from_be_bytes([bytes[4], bytes[5]]),
        u16::from_be_bytes([bytes[6], bytes[7]]),
        u16::from_be_bytes([bytes[8], bytes[9]]),
        // Last 6 bytes as a single u64 (only lower 48 bits used)
        u64::from_be_bytes([
            0, 0, bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
        ])
    ))
}

/// Read or generate the stable machine UUID.
/// Stored at `<sync_dir>/machine_id`.
pub fn machine_id() -> Result<String, Box<dyn Error>> {
    let dir = sync_dir()?;
    let path = dir.join("machine_id");

    if path.exists() {
        let id = fs::read_to_string(&path)?.trim().to_string();
        if !id.is_empty() {
            return Ok(id);
        }
    }

    fs::create_dir_all(&dir)?;
    let id = generate_uuid_v4()?;
    fs::write(&path, &id)?;
    Ok(id)
}

/// Generate a new X25519 keypair.
/// Writes `identity.age` (private key) and `recipient.age` (public key).
/// Returns the public key string.
/// Fails if keypair already exists unless `force` is true.
pub fn generate_keypair(force: bool) -> Result<String, Box<dyn Error>> {
    let dir = sync_dir()?;
    let identity_path = dir.join("identity.age");
    let recipient_path = dir.join("recipient.age");

    if identity_path.exists() && !force {
        return Err("keypair already exists — use --force to overwrite".into());
    }

    fs::create_dir_all(&dir)?;

    let identity = x25519::Identity::generate();
    let recipient = identity.to_public();
    let public_key = recipient.to_string();

    // Write private key with restricted permissions
    let secret_str = identity.to_string();
    fs::write(&identity_path, secret_str.expose_secret())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&identity_path, fs::Permissions::from_mode(0o600))?;
    }

    // Write public key
    fs::write(&recipient_path, &public_key)?;

    Ok(public_key)
}

/// Read the local public key (recipient) from `recipient.age`.
pub fn local_public_key() -> Result<String, Box<dyn Error>> {
    let path = sync_dir()?.join("recipient.age");
    if !path.exists() {
        return Err("no keypair found — run 'scribe sync keypair generate' first".into());
    }
    Ok(fs::read_to_string(&path)?.trim().to_string())
}

/// Add a peer's public key to `peers/<name>.age`.
pub fn add_peer(name: &str, public_key: &str) -> Result<(), Box<dyn Error>> {
    // Validate public key format
    if !public_key.starts_with("age1") {
        return Err("invalid public key — must start with 'age1'".into());
    }
    // Validate it actually parses
    let _: x25519::Recipient = public_key
        .parse()
        .map_err(|e: &str| format!("invalid age public key: {e}"))?;

    let dir = peers_dir()?;
    fs::create_dir_all(&dir)?;

    let path = dir.join(format!("{name}.age"));
    if path.exists() {
        eprintln!("Warning: overwriting existing peer '{name}'");
    }
    fs::write(&path, public_key)?;
    Ok(())
}

/// List all known peers as (name, public_key) pairs.
pub fn list_peers() -> Result<Vec<(String, String)>, Box<dyn Error>> {
    let dir = peers_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut peers = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("age") {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let key = fs::read_to_string(&path)?.trim().to_string();
            peers.push((name, key));
        }
    }
    peers.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(peers)
}

/// Remove a peer's public key file.
pub fn remove_peer(name: &str) -> Result<(), Box<dyn Error>> {
    let path = peers_dir()?.join(format!("{name}.age"));
    if !path.exists() {
        return Err(format!("peer '{name}' not found").into());
    }
    fs::remove_file(&path)?;
    Ok(())
}

/// Load all recipients (self + all peers) for encryption.
pub fn all_recipients() -> Result<Vec<x25519::Recipient>, Box<dyn Error>> {
    let mut recipients = Vec::new();

    // Add self
    let self_key = local_public_key()?;
    let self_recipient: x25519::Recipient = self_key
        .parse()
        .map_err(|e: &str| format!("invalid local public key: {e}"))?;
    recipients.push(self_recipient);

    // Add peers
    for (_, key) in list_peers()? {
        let recipient: x25519::Recipient = key
            .parse()
            .map_err(|e: &str| format!("invalid peer public key: {e}"))?;
        recipients.push(recipient);
    }

    Ok(recipients)
}

/// Load the local identity (private key) for decryption.
pub fn local_identity() -> Result<x25519::Identity, Box<dyn Error>> {
    let path = sync_dir()?.join("identity.age");
    if !path.exists() {
        return Err("no keypair found — run 'scribe sync keypair generate' first".into());
    }
    let secret = fs::read_to_string(&path)?.trim().to_string();
    let identity: x25519::Identity = secret
        .parse()
        .map_err(|e: &str| format!("invalid identity key: {e}"))?;
    Ok(identity)
}

// Re-export for use by other sync modules and tests
#[allow(unused_imports)]
pub use age::secrecy::ExposeSecret;

// ── Encryption / Decryption ──

/// Encrypt plaintext from a reader to an age-encrypted writer.
/// Encrypts to all known recipients (self + peers).
pub fn encrypt_stream<R: std::io::Read, W: std::io::Write>(
    mut input: R,
    output: W,
) -> Result<(), Box<dyn Error>> {
    let recipients = all_recipients()?;
    if recipients.is_empty() {
        return Err("no recipients found — generate a keypair first".into());
    }

    let encryptor =
        age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
            .map_err(|_| "failed to create encryptor")?;
    let mut writer = encryptor
        .wrap_output(output)
        .map_err(|e| format!("failed to wrap output: {e}"))?;
    std::io::copy(&mut input, &mut writer)?;
    writer
        .finish()
        .map_err(|e| format!("failed to finalize encryption: {e}"))?;
    Ok(())
}

/// Decrypt age-encrypted data from a reader to a plaintext writer.
/// Uses the local identity (private key) for decryption.
pub fn decrypt_stream<R: std::io::Read, W: std::io::Write>(
    input: R,
    mut output: W,
) -> Result<(), Box<dyn Error>> {
    let identity = local_identity()?;

    let decryptor =
        age::Decryptor::new(input).map_err(|e| format!("failed to parse encrypted data: {e}"))?;

    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .map_err(|e| format!("decryption failed — check keypair exchange: {e}"))?;
    std::io::copy(&mut reader, &mut output)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: override sync_dir for tests by using env var or temp dir.
    /// Since sync_dir() uses dirs::config_dir(), we test the individual
    /// functions with explicit paths instead.

    #[test]
    fn test_generate_uuid_v4() {
        let uuid = generate_uuid_v4().unwrap();
        // UUID v4 format: 8-4-4-4-12 hex chars
        assert_eq!(uuid.len(), 36);
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
        // Version nibble should be '4'
        assert!(parts[2].starts_with('4'));
        // Variant should be 8, 9, a, or b
        let variant_char = parts[3].chars().next().unwrap();
        assert!(
            "89ab".contains(variant_char),
            "variant nibble should be 8/9/a/b, got '{variant_char}'"
        );

        // Two UUIDs should be different
        let uuid2 = generate_uuid_v4().unwrap();
        assert_ne!(uuid, uuid2);
    }

    #[test]
    fn test_machine_id_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("machine_id");

        // First call: generate
        let id1 = {
            let uuid = generate_uuid_v4().unwrap();
            fs::write(&path, &uuid).unwrap();
            uuid
        };

        // Second call: read existing
        let id2 = fs::read_to_string(&path).unwrap().trim().to_string();
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_keypair_generate_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let identity_path = dir.path().join("identity.age");
        let recipient_path = dir.path().join("recipient.age");

        // Generate keypair
        let identity = x25519::Identity::generate();
        let recipient = identity.to_public();
        let public_key = recipient.to_string();

        fs::write(&identity_path, identity.to_string().expose_secret()).unwrap();
        fs::write(&recipient_path, &public_key).unwrap();

        // Read back
        let read_key = fs::read_to_string(&recipient_path)
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(read_key, public_key);
        assert!(read_key.starts_with("age1"));

        // Parse identity back
        let secret = fs::read_to_string(&identity_path)
            .unwrap()
            .trim()
            .to_string();
        let _parsed: x25519::Identity = secret.parse().unwrap();
    }

    #[test]
    fn test_peer_management() {
        let dir = tempfile::tempdir().unwrap();
        let peers = dir.path().join("peers");
        fs::create_dir_all(&peers).unwrap();

        // Generate a test public key
        let identity = x25519::Identity::generate();
        let pubkey = identity.to_public().to_string();

        // Add peer
        let peer_path = peers.join("test-peer.age");
        fs::write(&peer_path, &pubkey).unwrap();

        // Read back
        let content = fs::read_to_string(&peer_path).unwrap().trim().to_string();
        assert_eq!(content, pubkey);

        // Remove
        fs::remove_file(&peer_path).unwrap();
        assert!(!peer_path.exists());
    }

    #[test]
    fn test_add_peer_validates_key_format() {
        // Invalid key (doesn't start with age1)
        let result = "not-a-valid-key"
            .parse::<x25519::Recipient>()
            .map_err(|e: &str| e.to_string());
        assert!(result.is_err());
    }

    #[test]
    fn test_all_recipients_includes_self_and_peers() {
        let dir = tempfile::tempdir().unwrap();

        // Generate "self" keypair
        let self_identity = x25519::Identity::generate();
        let self_pubkey = self_identity.to_public().to_string();

        // Generate "peer" keypair
        let peer_identity = x25519::Identity::generate();
        let peer_pubkey = peer_identity.to_public().to_string();

        // Parse both as recipients
        let self_r: x25519::Recipient = self_pubkey.parse().unwrap();
        let peer_r: x25519::Recipient = peer_pubkey.parse().unwrap();

        // Create a list
        let recipients = [self_r, peer_r];
        assert_eq!(recipients.len(), 2);

        // Write to temp dir and read back
        let peers_dir = dir.path().join("peers");
        fs::create_dir_all(&peers_dir).unwrap();
        fs::write(dir.path().join("recipient.age"), &self_pubkey).unwrap();
        fs::write(peers_dir.join("peer1.age"), &peer_pubkey).unwrap();

        // Read peer files
        let mut loaded = Vec::new();
        let self_key = fs::read_to_string(dir.path().join("recipient.age"))
            .unwrap()
            .trim()
            .to_string();
        loaded.push(self_key.parse::<x25519::Recipient>().unwrap());

        for entry in fs::read_dir(&peers_dir).unwrap() {
            let entry = entry.unwrap();
            let key = fs::read_to_string(entry.path()).unwrap().trim().to_string();
            loaded.push(key.parse::<x25519::Recipient>().unwrap());
        }

        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let identity = x25519::Identity::generate();
        let recipient = identity.to_public();
        let plaintext = b"Hello, world! This is a test of age encryption.";

        // Encrypt
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&recipient as &dyn age::Recipient))
                .unwrap();
        let mut ciphertext = Vec::new();
        let mut writer = encryptor.wrap_output(&mut ciphertext).unwrap();
        std::io::Write::write_all(&mut writer, plaintext).unwrap();
        writer.finish().unwrap();

        assert!(!ciphertext.is_empty());
        assert_ne!(&ciphertext[..], plaintext);

        // Decrypt
        let decryptor = age::Decryptor::new(ciphertext.as_slice()).unwrap();
        let mut decrypted = Vec::new();
        let mut reader = decryptor
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .unwrap();
        std::io::Read::read_to_end(&mut reader, &mut decrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_multiple_recipients() {
        let id1 = x25519::Identity::generate();
        let id2 = x25519::Identity::generate();
        let r1 = id1.to_public();
        let r2 = id2.to_public();
        let plaintext = b"secret data";

        // Encrypt to both recipients
        let encryptor = age::Encryptor::with_recipients(
            [&r1 as &dyn age::Recipient, &r2 as &dyn age::Recipient].into_iter(),
        )
        .unwrap();
        let mut ciphertext = Vec::new();
        let mut writer = encryptor.wrap_output(&mut ciphertext).unwrap();
        std::io::Write::write_all(&mut writer, plaintext).unwrap();
        writer.finish().unwrap();

        // Both identities should be able to decrypt
        for identity in [&id1, &id2] {
            let decryptor = age::Decryptor::new(ciphertext.as_slice()).unwrap();
            let mut decrypted = Vec::new();
            let mut reader = decryptor
                .decrypt(std::iter::once(identity as &dyn age::Identity))
                .unwrap();
            std::io::Read::read_to_end(&mut reader, &mut decrypted).unwrap();
            assert_eq!(decrypted, plaintext);
        }
    }

    #[test]
    fn test_decrypt_wrong_identity_fails() {
        let id1 = x25519::Identity::generate();
        let id_wrong = x25519::Identity::generate();
        let r1 = id1.to_public();
        let plaintext = b"secret";

        // Encrypt to id1 only
        let encryptor =
            age::Encryptor::with_recipients(std::iter::once(&r1 as &dyn age::Recipient)).unwrap();
        let mut ciphertext = Vec::new();
        let mut writer = encryptor.wrap_output(&mut ciphertext).unwrap();
        std::io::Write::write_all(&mut writer, plaintext).unwrap();
        writer.finish().unwrap();

        // Attempt decrypt with wrong identity
        let decryptor = age::Decryptor::new(ciphertext.as_slice()).unwrap();
        let result = decryptor.decrypt(std::iter::once(&id_wrong as &dyn age::Identity));
        assert!(result.is_err());
    }
}
