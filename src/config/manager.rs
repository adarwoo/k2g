use std::path::{Path, PathBuf};
use log::{error, info, warn};
use serde_json::Value;

use super::{
    defaults::{populate_defaults, synchronize},
    error::ConfigError,
    validator::SchemaValidator,
};

const SCHEMA_SUFFIX: &str = ".yaml";
const BACKUP_SUFFIX: &str = "bak"; // no leading dot

pub struct YamlConfigManager {
    section_name: String,
    config_path: PathBuf,
    schema: Value,
    validator: SchemaValidator,
    pub content: Value,
}

impl YamlConfigManager {
    /// Create a new manager for `section_name`.
    /// Schema is loaded from `schema_dir/<section_name>.yaml`.
    /// Config is loaded from `config_dir/<section_name>.yaml`.
    pub fn new(
        section_name: &str,
        schema_dir: &Path,
        config_dir: &Path,
    ) -> Result<Self, ConfigError> {
        let schema_path = schema_dir.join(format!("{}_schema{}", section_name, SCHEMA_SUFFIX));
        let config_path = config_dir.join(format!("{}{}", section_name, SCHEMA_SUFFIX));

        // --- Load & compile schema ---
        let schema = Self::load_schema(&schema_path)?;
        let validator = SchemaValidator::new(&schema)?;

        let mut manager = Self {
            section_name: section_name.to_string(),
            config_path,
            schema,
            validator,
            content: Value::Null,
        };

        manager.load_content();
        Ok(manager)
    }

    /// Load and parse a YAML schema file into a serde_json::Value
    fn load_schema(path: &Path) -> Result<Value, ConfigError> {
        if !path.exists() {
            error!("Missing schema file: {}", path.display());
            return Err(ConfigError::SchemaMissing(path.display().to_string()));
        }

        let text = std::fs::read_to_string(path)?;

        // Parse YAML then round-trip through JSON for jsonschema compatibility
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(&text)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

        let json_value: Value = serde_json::to_value(yaml_value)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

        Ok(json_value)
    }

    /// Generate default content from schema defaults
    fn generate_default_content(&self) -> Value {
        populate_defaults(&self.schema).unwrap_or(Value::Object(Default::default()))
    }

    /// Load config file, validate, merge with defaults if needed.
    /// Mirrors Python's load_content() behaviour exactly.
    fn load_content(&mut self) {
        let file_exists = self.config_path.exists();
        let mut overwrite = true;

        if file_exists {
            match self.parse_config() {
                Some(parsed) if !parsed.is_null() => {
                    if self.validator.validate(&parsed).is_ok() {
                        // Valid — use as-is
                        self.content = parsed;
                        overwrite = false;
                    } else {
                        // Invalid — merge with defaults
                        warn!(
                            "Config '{}' failed validation; merging with defaults.",
                            self.config_path.display()
                        );
                        let defaults = self.generate_default_content();
                        self.content = synchronize(&parsed, &defaults);

                        // Re-validate merged result
                        if self.validator.validate(&self.content).is_ok() {
                            overwrite = true; // Save the repaired version
                        } else {
                            error!("Merged config still invalid; falling back to defaults.");
                            self.content = self.generate_default_content();
                        }
                    }
                }
                _ => {
                    error!("Failed to parse '{}'; using defaults.", self.config_path.display());
                    self.content = self.generate_default_content();
                }
            }
        } else {
            info!(
                "Config '{}' not found; creating default.",
                self.config_path.display()
            );
            self.content = self.generate_default_content();
        }

        if overwrite {
            let backed_up = if file_exists { self.backup() } else { true };
            if backed_up {
                self.write_content();
            }
        }
    }

    /// Parse the YAML config file into a JSON Value
    fn parse_config(&self) -> Option<Value> {
        let text = match std::fs::read_to_string(&self.config_path) {
            Ok(t) => t,
            Err(e) => {
                error!("Cannot read '{}': {}", self.config_path.display(), e);
                return None;
            }
        };

        let yaml_value: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                error!("YAML parse error in '{}': {}", self.config_path.display(), e);
                return None;
            }
        };

        match serde_json::to_value(yaml_value) {
            Ok(v) => Some(v),
            Err(e) => {
                error!("JSON conversion error: {}", e);
                None
            }
        }
    }

    /// Write current content back to the config file as YAML
    fn write_content(&self) {
        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    error!("Failed to create directory '{}': {}", parent.display(), e);
                    return;
                }
            }
        }

        let yaml_value: serde_yaml::Value = match serde_json::from_value(self.content.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to convert content to YAML value: {}", e);
                return;
            }
        };

        match serde_yaml::to_string(&yaml_value) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.config_path, text) {
                    error!("Failed to write '{}': {}", self.config_path.display(), e);
                }
            }
            Err(e) => error!("Failed to serialize YAML: {}", e),
        }
    }

    /// Rename the existing config file to .bak
    fn backup(&self) -> bool {
        let backup_path = self.config_path.with_extension(BACKUP_SUFFIX);
        match std::fs::rename(&self.config_path, &backup_path) {
            Ok(_) => {
                warn!("Backed up '{}' to '{}'", self.config_path.display(), backup_path.display());
                true
            }
            Err(e) => {
                error!("Failed to back up '{}': {}", self.config_path.display(), e);
                false
            }
        }
    }

    pub fn get_content(&self) -> &Value {
        &self.content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_file(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    fn minimal_schema() -> &'static str {
        r#"
type: object
properties:
  max_feed_rate:
    type: number
    default: 2000.0
  name:
    type: string
    default: "default"
required: [max_feed_rate, name]
"#
    }

    #[test]
    fn test_loads_defaults_when_no_config() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());

        let mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert_eq!(mgr.content["max_feed_rate"], 2000.0);
        assert_eq!(mgr.content["name"], "default");
    }

    #[test]
    fn test_loads_valid_config() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());
        write_file(
            &config_dir.join("machine.yaml"),
            "max_feed_rate: 1500.0\nname: custom\n",
        );

        let mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert_eq!(mgr.content["max_feed_rate"], 1500.0);
        assert_eq!(mgr.content["name"], "custom");
    }

    #[test]
    fn test_merges_partial_config_with_defaults() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());
        // Only provides one of the two required fields
        write_file(&config_dir.join("machine.yaml"), "max_feed_rate: 999.0\n");

        let mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert_eq!(mgr.content["max_feed_rate"], 999.0); // from file
        assert_eq!(mgr.content["name"], "default");      // from schema default
    }

    #[test]
    fn test_backup_created_on_invalid_config() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());
        write_file(&config_dir.join("machine.yaml"), "{ invalid yaml: [[[");

        let _mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert!(config_dir.join("machine.bak").exists());
    }
}