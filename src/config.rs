use std::path::{Path, PathBuf};

use serde::Deserialize;
use toml_edit::DocumentMut;

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

# Maximum session duration for stats. Sessions longer than this
# are considered stale and excluded from average duration.
# Examples: \"4h\", \"8h\", \"1d\"
# Default: disabled (all sessions included)
# max_session_duration = \"8h\"

# [sync]
# Machine display name for sync identification.
# Default: system hostname
# machine_name = \"work-laptop\"
";

#[derive(Deserialize, Default)]
#[allow(dead_code)] // retention fields consumed by E05-S04 (auto-retention)
pub struct Config {
    pub db_path: Option<String>,
    pub retention: Option<String>,
    pub retention_check_interval: Option<String>,
    pub default_query_limit: Option<i64>,
    pub max_session_duration: Option<String>,
    pub sync: Option<SyncConfig>,
}

#[derive(Deserialize, Default)]
#[allow(dead_code)] // consumed by sync feature (E012)
pub struct SyncConfig {
    pub machine_name: Option<String>,
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

/// Fields that have been removed from the config schema.
/// Migration will delete these from the user's config file.
const OBSOLETE_FIELDS: &[&str] = &[
    // Currently empty. When a field is renamed or removed in a future version,
    // add its old name here. Example:
    // "old_field_name",
];

/// Result of a config migration operation.
pub struct MigrationReport {
    pub fields_added: Vec<String>,
    pub fields_removed: Vec<String>,
}

impl MigrationReport {
    pub fn has_changes(&self) -> bool {
        !self.fields_added.is_empty() || !self.fields_removed.is_empty()
    }
}

/// Migrate the config file at the platform-appropriate path.
/// Returns `Ok(None)` if no migration was needed or file doesn't exist.
pub fn migrate_config() -> Result<Option<MigrationReport>, Box<dyn std::error::Error>> {
    let Some(path) = config_path() else {
        return Ok(None);
    };
    migrate_config_at(&path)
}

/// Migrate a config file at an explicit path (for testing).
/// Adds missing fields from CONFIG_TEMPLATE, removes obsolete fields.
/// Preserves user values, comments, and unknown fields.
pub fn migrate_config_at(
    path: &Path,
) -> Result<Option<MigrationReport>, Box<dyn std::error::Error>> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            eprintln!(
                "Warning: could not read config file for migration {}: {e}",
                path.display()
            );
            return Ok(None);
        }
    };

    let mut doc: DocumentMut = match content.parse() {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "Warning: could not parse config file for migration {}: {e}",
                path.display()
            );
            return Ok(None);
        }
    };

    let mut fields_added = Vec::new();
    let mut fields_removed = Vec::new();

    // Extract the known field names from the template by scanning for "# field_name ="
    let template_fields: Vec<&str> = CONFIG_TEMPLATE
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") && trimmed.contains(" = ") {
                // Extract field name from "# field_name = ..."
                trimmed
                    .strip_prefix("# ")
                    .and_then(|rest| rest.split(" = ").next())
            } else {
                None
            }
        })
        .collect();

    // Check for missing fields — collect blocks to append
    let mut append_blocks = String::new();
    for field in &template_fields {
        let active = doc.as_table().contains_key(field);
        let commented = field_present_in_text(&content, field);

        if !active && !commented {
            let block = extract_template_block(field);
            append_blocks.push('\n');
            append_blocks.push_str(&block);
            fields_added.push((*field).to_string());
        }
    }

    // Remove obsolete fields
    for field in OBSOLETE_FIELDS {
        if doc.as_table().contains_key(field) {
            doc.remove(field);
            fields_removed.push((*field).to_string());
        }
    }

    let report = MigrationReport {
        fields_added,
        fields_removed,
    };

    if !report.has_changes() {
        return Ok(None);
    }

    // Build final content: serialized document + appended comment blocks
    let mut output = doc.to_string();
    if !append_blocks.is_empty() {
        if !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&append_blocks);
    }

    if let Err(e) = std::fs::write(path, output) {
        eprintln!(
            "Warning: could not write migrated config file {}: {e}",
            path.display()
        );
        return Ok(None);
    }

    Ok(Some(report))
}

/// Check if a field name appears as a commented-out line in the raw text.
/// Matches lines like `# field_name = ...` with optional leading whitespace.
fn field_present_in_text(text: &str, field: &str) -> bool {
    let pattern = format!("# {field} =");
    text.lines().any(|line| line.trim().starts_with(&pattern))
}

/// Extract the comment block + commented-out value for a field from CONFIG_TEMPLATE.
/// Returns the block as a string ending with a newline.
fn extract_template_block(field: &str) -> String {
    let target_line = format!("# {field} =");
    let lines: Vec<&str> = CONFIG_TEMPLATE.lines().collect();

    // Find the line with "# field_name = ..."
    let Some(field_idx) = lines
        .iter()
        .position(|l| l.trim().starts_with(&target_line))
    else {
        return format!("{target_line}\n");
    };

    // Walk backwards to find the start of the comment block
    let mut start = field_idx;
    while start > 0 && lines[start - 1].starts_with('#') {
        start -= 1;
    }

    // Include a leading blank line for separation if we're not at the start
    let mut block = String::new();
    for line in &lines[start..=field_idx] {
        block.push_str(line);
        block.push('\n');
    }
    block
}

/// Format a migration report as a stderr message.
pub fn format_migration_report(report: &MigrationReport) -> String {
    let mut parts = Vec::new();
    if !report.fields_added.is_empty() {
        let names: Vec<String> = report
            .fields_added
            .iter()
            .map(|f| format!("'{f}'"))
            .collect();
        parts.push(format!("added {}", names.join(", ")));
    }
    if !report.fields_removed.is_empty() {
        let names: Vec<String> = report
            .fields_removed
            .iter()
            .map(|f| format!("'{f}'"))
            .collect();
        parts.push(format!("removed {}", names.join(", ")));
    }
    format!("scribe: config updated — {}", parts.join(", "))
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
            "machine_name",
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

    // --- Migration tests (US-0026-E007) ---

    #[test]
    fn test_migrate_adds_missing_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Config with only db_path set — other fields missing
        std::fs::write(&path, "db_path = \"/custom.db\"\n").unwrap();

        let report = migrate_config_at(&path).unwrap().unwrap();
        assert!(report.has_changes());
        assert!(report.fields_added.contains(&"retention".to_string()));
        assert!(report
            .fields_added
            .contains(&"retention_check_interval".to_string()));
        assert!(report
            .fields_added
            .contains(&"default_query_limit".to_string()));
        // db_path was already present, should not be added
        assert!(!report.fields_added.contains(&"db_path".to_string()));

        // Verify the file now contains the missing fields as comments
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# retention ="));
        assert!(content.contains("# retention_check_interval ="));
        assert!(content.contains("# default_query_limit ="));
        // User value preserved
        assert!(content.contains("db_path = \"/custom.db\""));
    }

    #[test]
    fn test_migrate_preserves_user_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "db_path = \"/my/path.db\"\ndefault_query_limit = 200\n",
        )
        .unwrap();

        let report = migrate_config_at(&path).unwrap().unwrap();
        // retention, retention_check_interval, max_session_duration, and machine_name should be added
        assert_eq!(report.fields_added.len(), 4);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("db_path = \"/my/path.db\""));
        assert!(content.contains("default_query_limit = 200"));
    }

    #[test]
    fn test_migrate_preserves_user_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# My custom comment about this config\ndb_path = \"/custom.db\"\n",
        )
        .unwrap();

        migrate_config_at(&path).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# My custom comment about this config"));
        assert!(content.contains("db_path = \"/custom.db\""));
    }

    #[test]
    fn test_migrate_empty_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "").unwrap();

        let report = migrate_config_at(&path).unwrap().unwrap();
        assert_eq!(report.fields_added.len(), 6);

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("# db_path ="));
        assert!(content.contains("# retention ="));
        assert!(content.contains("# retention_check_interval ="));
        assert!(content.contains("# default_query_limit ="));
    }

    #[test]
    fn test_migrate_unparseable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let bad_content = "not valid {{{{ toml ]]]]";
        std::fs::write(&path, bad_content).unwrap();

        let result = migrate_config_at(&path).unwrap();
        assert!(result.is_none());

        // File should not be modified
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, bad_content);
    }

    #[test]
    fn test_migrate_noop_when_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Write the full template — all fields present as comments
        std::fs::write(&path, CONFIG_TEMPLATE).unwrap();

        let result = migrate_config_at(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_migrate_commented_out_field_not_readded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // db_path is active, retention is commented out — both should be considered present
        std::fs::write(&path, "db_path = \"/custom.db\"\n# retention = \"90d\"\n").unwrap();

        let report = migrate_config_at(&path).unwrap().unwrap();
        // Only retention_check_interval and default_query_limit should be added
        assert!(!report.fields_added.contains(&"db_path".to_string()));
        assert!(!report.fields_added.contains(&"retention".to_string()));
        assert!(report
            .fields_added
            .contains(&"retention_check_interval".to_string()));
        assert!(report
            .fields_added
            .contains(&"default_query_limit".to_string()));
    }

    #[test]
    fn test_migrate_mix_active_and_commented() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Mix of active and commented-out fields covering all 6
        std::fs::write(
            &path,
            "db_path = \"/custom.db\"\n# retention = \"90d\"\n# retention_check_interval = \"12h\"\ndefault_query_limit = 100\n# max_session_duration = \"8h\"\n# machine_name = \"laptop\"\n",
        )
        .unwrap();

        let result = migrate_config_at(&path).unwrap();
        // All fields present — no migration needed
        assert!(result.is_none());
    }

    #[test]
    fn test_migrate_unknown_fields_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // Config with all known fields plus an unknown one
        std::fs::write(
            &path,
            "db_path = \"/custom.db\"\nunknown_setting = true\n# retention = \"90d\"\n# retention_check_interval = \"24h\"\n# default_query_limit = 50\n# max_session_duration = \"8h\"\n# machine_name = \"laptop\"\n",
        )
        .unwrap();

        let result = migrate_config_at(&path).unwrap();
        assert!(result.is_none());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("unknown_setting = true"));
    }

    #[test]
    fn test_migrate_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");

        let result = migrate_config_at(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_migration_report_format() {
        let report = MigrationReport {
            fields_added: vec!["retention".to_string(), "default_query_limit".to_string()],
            fields_removed: vec![],
        };
        let msg = format_migration_report(&report);
        assert_eq!(
            msg,
            "scribe: config updated — added 'retention', 'default_query_limit'"
        );

        let report2 = MigrationReport {
            fields_added: vec!["new_field".to_string()],
            fields_removed: vec!["old_field".to_string()],
        };
        let msg2 = format_migration_report(&report2);
        assert_eq!(
            msg2,
            "scribe: config updated — added 'new_field', removed 'old_field'"
        );
    }
}
