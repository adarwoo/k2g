//! Catalog file I/O: schema-validated loading and normalization of the tool
//! catalogs, plus seeding of the bundled default catalog files.
//!
//! This was formerly the `config` / `legacy_config` module, which also hosted the
//! legacy persistence layer (config loading and profile/settings/stock saving).
//! That layer has been retired — the AppData datastore ([`crate::data`]) is now the
//! single reader and writer of every persisted realm — leaving only the
//! catalog-loading support here.

pub mod error;
pub mod validator;
pub mod yaml_service;
pub mod bootstrap;
pub mod catalog_normalizer;

pub use validator::SchemaValidator;
pub use bootstrap::ensure_default_files;
pub use catalog_normalizer::{backfill_catalog_fields, normalize_catalog_fields};
