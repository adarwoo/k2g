#![allow(dead_code)]

use std::path::Path;

use log::{error, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::normalize_catalog_fields;
use crate::config::yaml_service::load_yaml_dir_with_schema_pointer;
use crate::config::SchemaValidator;
use crate::domain::tool_core::{ToolCore, ToolKind};
use crate::units::{Angle, FeedRate, Length, RotationalSpeed};
use crate::user_path::{ensure_app_dirs, UserPathError};

const CATALOGS: &[(&str, &str)] = &[
    ("kyocera.yaml", include_str!("../../resources/catalogs/kyocera.yaml")),
    ("unionfab.yaml", include_str!("../../resources/catalogs/unionfab.yaml")),
    ("generic.yaml", include_str!("../../resources/catalogs/generic.yaml")),
];

const CATALOG_SCHEMA: &str = include_str!("../../resources/schemas/catalog.yaml");
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

impl LinearUnit {
    #[allow(dead_code)]
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Mm => "mm",
            Self::In => "in",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeedUnit {
    #[serde(rename = "mm_min")]
    MmMin,
    #[serde(rename = "ipm")]
    Ipm,
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
    #[allow(dead_code)]
    pub fn diameter_label(&self) -> String {
        self.diameter.to_string()
    }

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
    #[error("Cannot read catalog file '{path}': {error}")]
    Read { path: String, error: String },
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
        let validator = SchemaValidator::new(&schema)
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

    pub fn drillbits(&self) -> impl Iterator<Item = (String, &ToolEntry)> {
        self.all_tools()
            .filter(|(_, t)| t.tool_type == ToolType::Drillbit)
    }

    pub fn router_bits(&self) -> impl Iterator<Item = (String, &ToolEntry)> {
        self.all_tools()
            .filter(|(_, t)| t.tool_type == ToolType::Routerbit)
    }

    pub fn all_tools(&self) -> impl Iterator<Item = (String, &ToolEntry)> {
        self.catalogs.iter().flat_map(|lc| {
            lc.catalog.sections.iter().flat_map(move |sec| {
                sec.tools.iter().map(move |tool| {
                    let id = format!("[{}] {} / {}", lc.stem, sec.name, tool.diameter_label());
                    (id, tool)
                })
            })
        })
    }

    pub fn find(&self, id: &str) -> Option<&ToolEntry> {
        self.all_tools().find(|(k, _)| k == id).map(|(_, t)| t)
    }

    pub fn catalog_stems(&self) -> Vec<&str> {
        self.catalogs.iter().map(|lc| lc.stem.as_str()).collect()
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