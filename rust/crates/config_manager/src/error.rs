use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Schema file not found: {0}")]
    SchemaMissing(String),

    #[error("Failed to parse schema: {0}")]
    SchemaParse(String),

    #[error("Failed to parse config file: {0}")]
    ConfigParse(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}