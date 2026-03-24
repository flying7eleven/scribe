use serde::Deserialize;

#[derive(Deserialize, Default)]
#[allow(dead_code)] // retention fields consumed by E05-S04 (auto-retention)
pub struct Config {
    pub db_path: Option<String>,
    pub retention: Option<String>,
    pub retention_check_interval: Option<String>,
    pub default_query_limit: Option<i64>,
}

/// Load config from the platform-appropriate config directory.
/// Returns `Config::default()` if the file doesn't exist or cannot be parsed.
pub fn load_config() -> Config {
    let Some(config_dir) = dirs::config_dir() else {
        return Config::default();
    };

    let path = config_dir.join("claude-scribe").join("config.toml");

    let content = match std::fs::read_to_string(&path) {
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

/// Load config from a specific path (for testing).
#[cfg(test)]
pub fn load_config_from(path: &std::path::Path) -> Config {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Config::default(),
    };

    toml::from_str(&content).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

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
