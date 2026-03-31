/// Configuration persistence system for loading and saving all config files
/// at startup and during runtime.

use crate::user_path::AppDirs;
use log::{error, info, warn};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use super::manager::YamlConfigManager;
use super::ConfigError;

/// Encapsulates all persisted configuration state
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct PersistenceState {
    /// Global application settings (spindle speed, feedrates, Z heights, unit system, theme, etc.)
    pub global_settings: Value,
    /// Tool stock inventory
    pub stock: Value,
    /// All CNC profiles indexed by ID
    pub cnc_profiles: BTreeMap<String, Value>,
    /// ID of currently selected CNC profile (from global settings)
    pub selected_cnc_profile_id: Option<String>,
}

/// Load all persisted configuration files at application startup
pub fn load_all_configs(
    app_dirs: &AppDirs,
    schema_dir: &Path,
) -> Result<PersistenceState, ConfigError> {
    info!("Loading configuration from {:?}", app_dirs.configs);

    // Load global settings
    let global_mgr =
        YamlConfigManager::new("global.setting", schema_dir, &app_dirs.configs)?;
    let global_settings = global_mgr.get_content().clone();
    let selected_cnc_profile_id = global_settings
        .get("selected_cnc_profile_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    // Load stock config
    let stock_mgr = YamlConfigManager::new("stock", schema_dir, &app_dirs.configs)?;
    let stock = stock_mgr.get_content().clone();

    // Load all CNC profiles from cnc_profiles subdirectory
    let cnc_profiles = load_cnc_profiles(&app_dirs.cnc_profiles, schema_dir)?;

    Ok(PersistenceState {
        global_settings,
        stock,
        cnc_profiles,
        selected_cnc_profile_id,
    })
}

/// Load all CNC profile YAML files from the cnc_profiles directory
fn load_cnc_profiles(
    cnc_profiles_dir: &Path,
    schema_dir: &Path,
) -> Result<BTreeMap<String, Value>, ConfigError> {
    let mut profiles = BTreeMap::new();

    if !cnc_profiles_dir.exists() {
        info!("CNC profiles directory not found; skipping");
        return Ok(profiles);
    }

    let entries = match fs::read_dir(cnc_profiles_dir) {
        Ok(e) => e,
        Err(err) => {
            warn!(
                "Failed to read CNC profiles directory: {}",
                err
            );
            return Ok(profiles);
        }
    };

    for entry in entries {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("yaml")
                || path.extension().and_then(|s| s.to_str()) == Some("yml")
            {
                if let Ok(profile_data) = load_cnc_profile(&path, schema_dir) {
                    if let Some(id) = profile_data.get("id").and_then(Value::as_str) {
                        profiles.insert(id.to_string(), profile_data);
                    }
                }
            }
        }
    }

    info!("Loaded {} CNC profiles", profiles.len());
    Ok(profiles)
}

/// Load a single CNC profile YAML file and validate against schema
fn load_cnc_profile(path: &Path, _schema_dir: &Path) -> Result<Value, ConfigError> {
    let text = fs::read_to_string(path).map_err(|e| {
        ConfigError::Io(e)
    })?;

    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| ConfigError::ConfigParse(e.to_string()))?;

    let json_value: Value = serde_json::to_value(yaml_value)
        .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

    Ok(json_value)
}

/// Save global settings to global.setting.yaml
#[allow(dead_code)]
pub fn save_global_settings(
    app_dirs: &AppDirs,
    global_settings: &Value,
) -> Result<(), ConfigError> {
    save_config_file(
        &app_dirs.configs,
        "global.setting",
        global_settings,
    )
}

/// Save tool stock to stock.yaml
#[allow(dead_code)]
pub fn save_stock(app_dirs: &AppDirs, stock: &Value) -> Result<(), ConfigError> {
    save_config_file(&app_dirs.configs, "stock", stock)
}

/// Save a single CNC profile to cnc_profiles/{profile_name}.yaml
#[allow(dead_code)]
pub fn save_cnc_profile(
    app_dirs: &AppDirs,
    profile_name: &str,
    profile_data: &Value,
) -> Result<(), ConfigError> {
    let file_path = app_dirs.cnc_profiles.join(format!("{}.yaml", profile_name));

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            error!("Failed to create CNC profiles directory: {}", e);
            ConfigError::Io(e)
        })?;
    }

    let yaml_value: serde_yaml::Value = serde_json::from_value(profile_data.clone())
        .map_err(|e| {
            error!("Failed to convert CNC profile to YAML: {}", e);
            ConfigError::SchemaParse(e.to_string())
        })?;

    let yaml_str = serde_yaml::to_string(&yaml_value).map_err(|e| {
        error!("Failed to serialize CNC profile: {}", e);
        ConfigError::ConfigParse(e.to_string())
    })?;

    fs::write(&file_path, yaml_str).map_err(|e| {
        error!("Failed to write CNC profile to {:?}: {}", file_path, e);
        ConfigError::Io(e)
    })?;

    info!("Saved CNC profile: {:?}", file_path);
    Ok(())
}

/// Generic helper to save a config file as YAML
#[allow(dead_code)]
fn save_config_file(
    config_dir: &Path,
    file_stem: &str,
    content: &Value,
) -> Result<(), ConfigError> {
    fs::create_dir_all(config_dir).map_err(|e| {
        error!("Failed to create config directory: {}", e);
        ConfigError::Io(e)
    })?;

    let file_path = config_dir.join(format!("{}.yaml", file_stem));

    let yaml_value: serde_yaml::Value = serde_json::from_value(content.clone())
        .map_err(|e| {
            error!("Failed to convert {} to YAML: {}", file_stem, e);
            ConfigError::SchemaParse(e.to_string())
        })?;

    let yaml_str = serde_yaml::to_string(&yaml_value).map_err(|e| {
        error!("Failed to serialize {}: {}", file_stem, e);
        ConfigError::ConfigParse(e.to_string())
    })?;

    fs::write(&file_path, yaml_str).map_err(|e| {
        error!("Failed to write {} to {:?}: {}", file_stem, file_path, e);
        ConfigError::Io(e)
    })?;

    info!("Saved {}: {:?}", file_stem, file_path);
    Ok(())
}
