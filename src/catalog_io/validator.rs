use std::fs;
use std::path::Path;

use jsonschema::{options, Resource, Validator};
use serde_json::Value;
use super::error::ConfigError;

pub struct SchemaValidator {
    compiled: Validator,
}

impl SchemaValidator {
    /// Compile a JSON Schema for reuse
    pub fn new(schema: &Value, schema_dir: &Path) -> Result<Self, ConfigError> {
        let mut opts = options();

        // Register all local schemas so refs like `units.yaml#/$defs/...` and
        // `json-schema:///units.yaml#/$defs/...` resolve without external retrieval.
        if let Ok(entries) = fs::read_dir(schema_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_ascii_lowercase())
                    .unwrap_or_default();
                if ext != "yaml" && ext != "yml" {
                    continue;
                }

                let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };

                let text = fs::read_to_string(&path)
                    .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
                let yaml_value: serde_yaml::Value = serde_yaml::from_str(&text)
                    .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
                let json_value: Value = serde_json::to_value(yaml_value)
                    .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
                let resource = Resource::from_contents(json_value)
                    .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

                opts.with_resource(file_name.to_string(), resource.clone());
                opts.with_resource(format!("json-schema:///{file_name}"), resource);
            }
        }

        let compiled = opts
            .build(schema)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
        Ok(Self { compiled })
    }

    /// Validate a document. Returns all errors joined, or Ok(())
    pub fn validate(&self, document: &Value) -> Result<(), ConfigError> {
        if self.compiled.validate(document).is_err() {
            let messages: Vec<String> = self
                .compiled
                .iter_errors(document)
                .map(|e| e.to_string())
                .collect();
            return Err(ConfigError::Validation(messages.join("\n")));
        }
        Ok(())
    }
}