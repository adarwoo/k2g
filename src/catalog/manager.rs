//! Catalog manager — loads and indexes all tool catalogs from a directory.
//!
//! Each YAML file in the catalogs directory is validated against the embedded
//! catalog schema before being accepted. Validation errors are collected and
//! returned so that the caller can log them without aborting the entire load.
//!
//! # ID scheme
//! Every tool is addressable by a compound string ID:
//! ```text
//! [<file-stem>] <section-name> / <diameter><unit>
//! ```
//! For example:  `[kyocera] Series 100 (in) / 0.0200in`
#![allow(dead_code)]

use std::path::Path;

use log::{error, warn};
use serde_json::Value;

use crate::config::SchemaValidator;
use super::init::normalize_catalog_fields;
use super::types::{Catalog, ToolEntry, ToolType};

/// The catalog JSON Schema embedded at compile time.
const CATALOG_SCHEMA: &str = include_str!("../../resources/schemas/catalog.schema.yaml");

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("Failed to compile catalog schema: {0}")]
    SchemaCompile(String),

    #[error("Cannot read catalog file '{path}': {error}")]
    Read { path: String, error: String },

    #[error("YAML parse error in '{path}': {error}")]
    YamlParse { path: String, error: String },

    #[error("Schema validation failed for '{path}':\n{error}")]
    Validation { path: String, error: String },
}

// ---------------------------------------------------------------------------
// Loaded catalog (file stem + deserialized data)
// ---------------------------------------------------------------------------

struct LoadedCatalog {
    /// File stem used as the ID prefix, e.g. `"kyocera"`.
    stem: String,
    catalog: Catalog,
}

// ---------------------------------------------------------------------------
// CatalogManager
// ---------------------------------------------------------------------------

/// Holds all successfully loaded catalogs and provides tool lookup by ID.
pub struct CatalogManager {
    validator: SchemaValidator,
    catalogs: Vec<LoadedCatalog>,
}

impl CatalogManager {
    /// Create an empty manager with a compiled schema validator.
    ///
    /// Returns `Err` only if the embedded schema itself is malformed (a
    /// compile-time defect, not a user error).
    pub fn new() -> Result<Self, CatalogError> {
        let schema = compile_schema(CATALOG_SCHEMA)?;
        let validator = SchemaValidator::new(&schema)
            .map_err(|e| CatalogError::SchemaCompile(e.to_string()))?;
        Ok(Self { validator, catalogs: Vec::new() })
    }

    /// Load every `*.yaml` file from `dir`.
    ///
    /// Returns a list of non-fatal per-file errors.  Files that fail to parse
    /// or validate are skipped; successfully loaded files are retained.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<CatalogError> {
        let mut errors = Vec::new();

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                error!("Cannot read catalogs directory '{}': {e}", dir.display());
                return errors;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }

            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();

            match self.load_one(&path, &stem) {
                Ok(loaded) => self.catalogs.push(loaded),
                Err(e) => {
                    warn!("{e}");
                    errors.push(e);
                }
            }
        }

        errors
    }

    // --- Queries ---

    /// Iterator over `(compound_id, &ToolEntry)` for every drill bit across
    /// all loaded catalogs.
    pub fn drillbits(&self) -> impl Iterator<Item = (String, &ToolEntry)> {
        self.all_tools().filter(|(_, t)| t.tool_type == ToolType::Drillbit)
    }

    /// Iterator over `(compound_id, &ToolEntry)` for every router bit across
    /// all loaded catalogs.
    pub fn router_bits(&self) -> impl Iterator<Item = (String, &ToolEntry)> {
        self.all_tools().filter(|(_, t)| t.tool_type == ToolType::Routerbit)
    }

    /// Iterator over `(compound_id, &ToolEntry)` for every tool across all
    /// loaded catalogs in load order.
    pub fn all_tools(&self) -> impl Iterator<Item = (String, &ToolEntry)> {
        self.catalogs.iter().flat_map(|lc| {
            lc.catalog.sections.iter().flat_map(move |sec| {
                sec.tools.iter().map(move |tool| {
                    let id = format!(
                        "[{}] {} / {}",
                        lc.stem,
                        sec.name,
                        tool.diameter_label()
                    );
                    (id, tool)
                })
            })
        })
    }

    /// Look up a tool by its compound ID string.
    ///
    /// Format: `"[stem] Section Name / 0.0200in"`
    pub fn find(&self, id: &str) -> Option<&ToolEntry> {
        self.all_tools().find(|(k, _)| k == id).map(|(_, t)| t)
    }

    /// Names of all loaded catalog files (stems).
    pub fn catalog_stems(&self) -> Vec<&str> {
        self.catalogs.iter().map(|lc| lc.stem.as_str()).collect()
    }

    /// Iterator over loaded catalogs as `(file_stem, catalog)`.
    pub fn catalogs(&self) -> impl Iterator<Item = (&str, &Catalog)> {
        self.catalogs
            .iter()
            .map(|lc| (lc.stem.as_str(), &lc.catalog))
    }

    // --- Internal ---

    fn load_one(&self, path: &Path, stem: &str) -> Result<LoadedCatalog, CatalogError> {
        let text = std::fs::read_to_string(path).map_err(|e| CatalogError::Read {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;

        // Parse YAML → JSON Value for schema validation
        let yaml_val: serde_yaml::Value =
            serde_yaml::from_str(&text).map_err(|e| CatalogError::YamlParse {
                path: path.display().to_string(),
                error: e.to_string(),
            })?;

        let mut json_val: Value = serde_json::to_value(yaml_val).map_err(|e| CatalogError::YamlParse {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;

        normalize_catalog_fields(&mut json_val, stem, false, true);

        self.validator.validate(&json_val).map_err(|e| CatalogError::Validation {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;

        // Deserialize into strongly-typed struct
        let catalog: Catalog =
            serde_json::from_value(json_val).map_err(|e| CatalogError::YamlParse {
                path: path.display().to_string(),
                error: e.to_string(),
            })?;

        Ok(LoadedCatalog { stem: stem.to_string(), catalog })
    }
}

// ---------------------------------------------------------------------------
// Schema compilation helper (YAML → JSON → compiled schema)
// ---------------------------------------------------------------------------

fn compile_schema(yaml_text: &str) -> Result<Value, CatalogError> {
    let yaml_val: serde_yaml::Value =
        serde_yaml::from_str(yaml_text).map_err(|e| CatalogError::SchemaCompile(e.to_string()))?;

    serde_json::to_value(yaml_val).map_err(|e| CatalogError::SchemaCompile(e.to_string()))
}
