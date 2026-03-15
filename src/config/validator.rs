use jsonschema::{validator_for, Validator};
use serde_json::Value;
use super::error::ConfigError;

pub struct SchemaValidator {
    compiled: Validator,
}

impl SchemaValidator {
    /// Compile a JSON Schema for reuse
    pub fn new(schema: &Value) -> Result<Self, ConfigError> {
        let compiled = validator_for(schema)
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