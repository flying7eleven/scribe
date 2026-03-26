use std::path::PathBuf;

use serde::Deserialize;

/// Canonical config template with commented-out defaults.
/// All fields are commented so compiled defaults remain in effect.
/// This constant is also the source of truth for config migration (US-0026-E007).
pub const CONFIG_TEMPLATE: &str = "\
# Path to the SQLite database file.
# Default: ~/.claude/scribe.db
# db_path = \"~/.claude/scribe.db\"

# Automatic retention period. Events older than this are periodically deleted.
# Examples: \"30d\", \"90d\", \"1y\"
# Default: disabled (no automatic deletion)
# retention = \"90d\"

# How often the auto-retention check runs during 'scribe log'.
# Only relevant when 'retention' is set.
# Default: \"24h\"
# retention_check_interval = \"24h\"

# Default number of rows returned by 'scribe query'.
# Can be overridden with --limit.
# Default: 50
# default_query_limit = 50
";

#[derive(Deserialize, Default)]
#[allow(dead_code)] // retention fields consumed by E05-S04 (auto-retention)
pub struct Config {
    pub db_path: Option<String>,
    pub retention: Option<String>,
    pub retention_check_interval: Option<String>,
    pub default_query_limit: Option<i64>,
}

/// Returns the platform-appropriate config file path.
/// `~/.config/claude-scribe/config.toml` on Linux.
pub fn config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("claude-scribe").join("config.toml"))
}

/// Ensures the config file exists. If missing, creates it with the default template.
/// Returns `Ok(true)` if the file was created, `Ok(false)` if it already existed.
/// On failure (permissions, etc.), prints a warning to stderr and returns `Ok(false)`.
pub fn ensure_config_exists() -> Result<bool, Box<dyn std::error::Error>> {
    let Some(path) = config_path() else {
        return Ok(false);
    };
    ensure_config_exists_at(&path)
}

/// Inner implementation that accepts an explicit path (for testing).
pub fn ensure_config_exists_at(path: &std::path::Path) -> Result<bool, Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!(
                "Warning: could not create config directory {}: {e}",
                parent.display()
            );
            return Ok(false);
        }
    }

    if let Err(e) = std::fs::write(path, CONFIG_TEMPLATE) {
        eprintln!(
            "Warning: could not create config file {}: {e}",
            path.display()
        );
        return Ok(false);
    }

    Ok(true)
}

/// Load config from the platform-appropriate config directory.
/// Returns `Config::default()` if the file doesn't exist or cannot be parsed.
pub fn load_config() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };

    load_config_from(&path)
}

/// Load config from a specific path.
/// Returns `Config::default()` if the file doesn't exist or cannot be parsed.
pub fn load_config_from(path: &std::path::Path) -> Config {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!(
                "Warning: could not read config file {}: {e}",
                path.display()
            );
            return Config::default();
        }
    };

    match toml::from_str(&content) {
        Ok(config) => config,
        Err(e) => {
            eprintln!(
                "Warning: failed to parse config file {}: {e}",
                path.display()
            );
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_is_valid_toml() {
        // The template should parse as valid TOML (all fields are comments, so it's an empty doc)
        let doc: toml_edit::DocumentMut = CONFIG_TEMPLATE.parse().unwrap();
        // No active keys — everything is commented out
        assert!(doc.as_table().is_empty());
    }

    #[test]
    fn test_template_contains_all_config_fields() {
        let field_names = [
            "db_path",
            "retention",
            "retention_check_interval",
            "default_query_limit",
        ];
        for field in &field_names {
            let pattern = format!("# {field} =");
            assert!(
                CONFIG_TEMPLATE.contains(&pattern),
                "CONFIG_TEMPLATE is missing commented field: {field}"
            );
        }
    }

    #[test]
    fn test_ensure_config_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("claude-scribe").join("config.toml");

        let created = ensure_config_exists_at(&path).unwrap();
        assert!(created);
        assert!(path.exists());

        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, CONFIG_TEMPLATE);
    }

    #[test]
    fn test_ensure_config_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("deep").join("config.toml");

        let created = ensure_config_exists_at(&path).unwrap();
        assert!(created);
        assert!(path.exists());
    }

    #[test]
    fn test_ensure_config_noop_when_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "db_path = \"/custom.db\"\n").unwrap();

        let created = ensure_config_exists_at(&path).unwrap();
        assert!(!created);

        // Original content preserved
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("/custom.db"));
    }

    #[test]
    fn test_created_config_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        ensure_config_exists_at(&path).unwrap();

        // All fields are commented out, so load_config_from should return defaults
        let config = load_config_from(&path);
        assert!(config.db_path.is_none());
        assert!(config.retention.is_none());
        assert!(config.retention_check_interval.is_none());
        assert!(config.default_query_limit.is_none());
    }

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
db_path = "/custom/scribe.db"
retention = "90d"
retention_check_interval = "12h"
default_query_limit = 100
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.db_path.as_deref(), Some("/custom/scribe.db"));
        assert_eq!(config.retention.as_deref(), Some("90d"));
        assert_eq!(config.retention_check_interval.as_deref(), Some("12h"));
        assert_eq!(config.default_query_limit, Some(100));
    }

    #[test]
    fn test_parse_partial_config() {
        let toml = r#"
db_path = "/custom/scribe.db"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.db_path.as_deref(), Some("/custom/scribe.db"));
        assert!(config.retention.is_none());
        assert!(config.default_query_limit.is_none());
    }

    #[test]
    fn test_parse_empty_file() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.db_path.is_none());
        assert!(config.retention.is_none());
    }

    #[test]
    fn test_parse_invalid_toml() {
        let result: Result<Config, _> = toml::from_str("not valid {{{{ toml");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_from_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            r#"db_path = "/test/path.db"
default_query_limit = 200
"#,
        )
        .unwrap();

        let config = load_config_from(&path);
        assert_eq!(config.db_path.as_deref(), Some("/test/path.db"));
        assert_eq!(config.default_query_limit, Some(200));
    }

    #[test]
    fn test_load_config_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let config = load_config_from(&path);
        assert!(config.db_path.is_none());
    }
}
