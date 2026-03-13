use jsonschema::JSONSchema;
use serde_json::Value;
use crate::error::ConfigError;

pub struct SchemaValidator {
    compiled: JSONSchema,
}

impl SchemaValidator {
    /// Compile a JSON Schema for reuse
    pub fn new(schema: &Value) -> Result<Self, ConfigError> {
        let compiled = JSONSchema::compile(schema)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
        Ok(Self { compiled })
    }

    /// Validate a document. Returns all errors joined, or Ok(())
    pub fn validate(&self, document: &Value) -> Result<(), ConfigError> {
        let result = self.compiled.validate(document);
        if let Err(errors) = result {
            let messages: Vec<String> = errors.map(|e| e.to_string()).collect();
            return Err(ConfigError::Validation(messages.join("\n")));
        }
        Ok(())
    }
}