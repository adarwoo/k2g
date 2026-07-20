pub mod defaults;
pub mod error;
pub mod manager;
pub mod validator;
pub mod persistence;
pub mod yaml_service;
pub mod bootstrap;
pub mod catalog_normalizer;

#[allow(unused_imports)]
pub use manager::YamlConfigManager;
pub use error::ConfigError;
pub use validator::SchemaValidator;
pub use bootstrap::write_embedded_schemas;
pub use bootstrap::ensure_default_files;
pub use catalog_normalizer::{backfill_catalog_fields, normalize_catalog_fields};
#[allow(unused_imports)]
pub use persistence::{
	load_all_configs,
	load_all_configs_best_effort,
	save_global_settings,
	save_stock,
	save_cnc_profile,
	save_processing_profiles,
	PersistenceState,
};
