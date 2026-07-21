#![allow(dead_code)]

use std::path::Path;

use log::{error, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::catalog_io::normalize_catalog_fields;
use crate::catalog_io::yaml_service::load_yaml_dir_with_schema_pointer;
use crate::catalog_io::SchemaValidator;
use crate::data::model::tool_core::{ToolCore, ToolKind};
use units::{Angle, FeedRate, Length, RotationalSpeed};
use crate::paths::{ensure_app_dirs, UserPathError};

const CATALOGS: &[(&str, &str)] = &[
    ("kyocera.yaml", include_str!("../../../assets/catalogs/kyocera.yaml")),
    ("unionfab.yaml", include_str!("../../../assets/catalogs/unionfab.yaml")),
    ("generic.yaml", include_str!("../../../assets/catalogs/generic.yaml")),
];

const CATALOG_SCHEMA: &str = include_str!("../../../schemas/catalog.yaml");
const CATALOG_SCHEMA_POINTER: &str = "catalog.yaml";

pub fn default_catalogs() -> &'static [(&'static str, &'static str)] {
    CATALOGS
}

#[allow(dead_code)]
pub fn catalog_dir() -> Result<std::path::PathBuf, UserPathError> {
    ensure_app_dirs().map(|d| d.catalogs)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Drillbit,
    Routerbit,
    Engraver,
    Vbit,
    Endmill,
}

impl ToolType {
    pub fn to_tool_kind(self) -> ToolKind {
        match self {
            Self::Drillbit => ToolKind::Drillbit,
            Self::Routerbit => ToolKind::Routerbit,
            Self::Engraver => ToolKind::Engraver,
            Self::Vbit => ToolKind::Vbit,
            Self::Endmill => ToolKind::Endmill,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LinearUnit {
    #[serde(rename = "mm")]
    Mm,
    #[serde(rename = "in")]
    In,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type")]
    pub tool_type: ToolType,
    pub diameter: Length,
    pub flute_length: Option<Length>,
    #[serde(default, alias = "sku_name")]
    pub sku: Option<String>,
    pub point_angle: Angle,
    pub z_min_depth: Length,
    pub spindle_rpm: Option<RotationalSpeed>,
    pub z_feed: Option<FeedRate>,
    pub table_feed: Option<FeedRate>,
    pub max_hits: Option<u32>,
    pub notes: Option<String>,
}

impl ToolEntry {
    pub fn to_tool_core(&self) -> ToolCore {
        ToolCore {
            kind: self.tool_type.to_tool_kind(),
            diameter: self.diameter,
            point_angle: self.point_angle,
            feed_rate: self.table_feed.or(self.z_feed),
            spindle_speed: self.spindle_rpm,
            sku: self.sku.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogSection {
    pub name: String,
    pub default_flute_length_unit: Option<LinearUnit>,
    pub description: Option<String>,
    pub tools: Vec<ToolEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub name: String,
    pub description: Option<String>,
    pub sections: Vec<CatalogSection>,
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("Failed to compile catalog schema: {0}")]
    SchemaCompile(String),
    #[error("YAML parse error in '{path}': {error}")]
    YamlParse { path: String, error: String },
    #[error("Schema validation failed for '{path}':\n{error}")]
    Validation { path: String, error: String },
}

struct LoadedCatalog {
    stem: String,
    catalog: Catalog,
}

pub struct CatalogManager {
    validator: SchemaValidator,
    catalogs: Vec<LoadedCatalog>,
}

impl CatalogManager {
    pub fn new() -> Result<Self, CatalogError> {
        let schema = compile_schema(CATALOG_SCHEMA)?;
        let validator = SchemaValidator::new(&schema, Path::new("schemas"))
            .map_err(|e| CatalogError::SchemaCompile(e.to_string()))?;
        Ok(Self {
            validator,
            catalogs: Vec::new(),
        })
    }

    pub fn load_dir(&mut self, dir: &Path) -> Vec<CatalogError> {
        let mut errors = Vec::new();

        let documents = match load_yaml_dir_with_schema_pointer(dir, CATALOG_SCHEMA_POINTER) {
            Ok(documents) => documents,
            Err(e) => {
                error!("Cannot read catalogs directory '{}': {}", dir.display(), e);
                return errors;
            }
        };

        for doc in documents {
            match self.load_one(doc.value, &doc.path, &doc.stem) {
                Ok(loaded) => self.catalogs.push(loaded),
                Err(e) => {
                    warn!("{e}");
                    errors.push(e);
                }
            }
        }

        errors
    }

    pub fn catalogs(&self) -> impl Iterator<Item = (&str, &Catalog)> {
        self.catalogs
            .iter()
            .map(|lc| (lc.stem.as_str(), &lc.catalog))
    }

    fn load_one(&self, mut json_val: Value, path: &Path, stem: &str) -> Result<LoadedCatalog, CatalogError> {
        normalize_catalog_fields(&mut json_val, stem, false, true);

        self.validator
            .validate(&json_val)
            .map_err(|e| CatalogError::Validation {
                path: path.display().to_string(),
                error: e.to_string(),
            })?;

        let catalog: Catalog = serde_json::from_value(json_val).map_err(|e| CatalogError::YamlParse {
            path: path.display().to_string(),
            error: e.to_string(),
        })?;

        Ok(LoadedCatalog {
            stem: stem.to_string(),
            catalog,
        })
    }
}

fn compile_schema(yaml_text: &str) -> Result<Value, CatalogError> {
    let yaml_val: serde_yaml::Value =
        serde_yaml::from_str(yaml_text).map_err(|e| CatalogError::SchemaCompile(e.to_string()))?;

    serde_json::to_value(yaml_val).map_err(|e| CatalogError::SchemaCompile(e.to_string()))
}
// -----------------------------------------------------------------------------
// Runtime catalog projections shown in the stock "add from catalog" UI.
// (Relocated out of the former `ui::model` facade.)
// -----------------------------------------------------------------------------

/// Runtime catalog tool item shown in stock import UI.
#[derive(Clone)]
pub struct CatalogStockTool {
    pub key: String,
    pub display_name: String,
    pub kind: String,
    pub diameter: Length,
    pub point_angle: Angle,
    pub feed_rate: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub sku: Option<String>,
}

/// Runtime catalog section shown in stock import UI.
#[derive(Clone)]
pub struct CatalogStockSection {
    pub key: String,
    pub name: String,
    pub tools: Vec<CatalogStockTool>,
}

/// Runtime catalog index node shown in stock import UI.
#[derive(Clone)]
pub struct CatalogStockCatalog {
    pub key: String,
    pub name: String,
    pub built_in: bool,
    pub sections: Vec<CatalogStockSection>,
}
