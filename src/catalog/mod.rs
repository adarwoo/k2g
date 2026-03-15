//! Tool catalog module.
//!
//! Provides types, loading, validation, and first-run initialization for
//! the tool catalog system.  All catalogs are stored as YAML files in the
//! user data directory under `catalogs/`.  Each file is validated against
//! the embedded `catalog_schema.yaml` on load.
//!
//! # Compound tool IDs
//! To prevent name clashes when multiple catalog files are loaded, every tool
//! is identified by a compound string of the form:
//! ```text
//! [<file-stem>] <section-name> / <diameter>mm
//! ```
//! For example: `[kyocera] Series 100 / 0.20mm`

pub mod init;
pub mod manager;
pub mod types;

pub use manager::{CatalogError, CatalogManager};
pub use types::{Catalog, CatalogSection, ToolEntry, ToolType};
