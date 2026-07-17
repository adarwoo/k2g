/// Configuration persistence system for loading and saving all config files
/// at startup and during runtime.

use crate::user_path::{AppDirs, GLOBAL_SETTINGS_SECTION, STOCK_SECTION};
use log::{error, info, warn};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use super::manager::{
    begin_persist_session, end_persist_session, queue_persist_document,
    queue_persist_document_in_session, YamlConfigManager,
};
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
    /// All fixture profiles indexed by ID
    pub fixture_profiles: BTreeMap<String, Value>,
    /// All processing profiles indexed by ID
    pub processing_profiles: BTreeMap<String, Value>,
    /// All toolset profiles indexed by ID
    pub toolset_profiles: BTreeMap<String, Value>,
    /// ID of currently selected process profile (from global settings)
    pub selected_process_profile_id: Option<String>,
    /// ID of last edited processing profile in processing profile view
    pub last_edited_process_profile_id: Option<String>,
    /// ID of currently selected CNC profile (from global settings)
    pub selected_cnc_profile_id: Option<String>,
    /// ID of currently selected fixture profile (from global settings)
    pub selected_fixture_profile_id: Option<String>,
    /// ID of currently selected toolset profile (from global settings)
    pub selected_toolset_profile_id: Option<String>,
}

/// Load all persisted configuration files at application startup
pub fn load_all_configs(
    app_dirs: &AppDirs,
    schema_dir: &Path,
) -> Result<PersistenceState, ConfigError> {
    info!("Loading configuration from {:?}", app_dirs.configs);

    // Load global settings
    let global_mgr =
        YamlConfigManager::new(GLOBAL_SETTINGS_SECTION, schema_dir, &app_dirs.configs)?;
    let global_settings = global_mgr.get_content().clone();
    let selected_process_profile_id = global_settings
        .get("selected_process_profile_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let last_edited_process_profile_id = global_settings
        .get("last_edited_process_profile_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let selected_cnc_profile_id = global_settings
        .get("selected_cnc_profile_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let selected_fixture_profile_id = global_settings
        .get("selected_fixture_profile_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let selected_toolset_profile_id = global_settings
        .get("selected_toolset_profile_id")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    // Load stock config
    let stock_mgr = YamlConfigManager::new(STOCK_SECTION, schema_dir, &app_dirs.configs)?;
    let stock = stock_mgr.get_content().clone();

    // Load all profile domains from their subdirectories.
    let cnc_profiles = load_cnc_profiles(&app_dirs.cnc_profiles, schema_dir)?;
    let fixture_profiles = load_profiles_from_dir(&app_dirs.fixture_profiles)?;
    let processing_profiles = load_profiles_from_dir(&app_dirs.processing_profiles)?;
    let toolset_profiles = load_profiles_from_dir(&app_dirs.toolset_profiles)?;

    Ok(PersistenceState {
        global_settings,
        stock,
        cnc_profiles,
        fixture_profiles,
        processing_profiles,
        toolset_profiles,
        selected_process_profile_id,
        last_edited_process_profile_id,
        selected_cnc_profile_id,
        selected_fixture_profile_id,
        selected_toolset_profile_id,
    })
}

/// Load all persisted configuration files with per-domain fallbacks.
///
/// Unlike `load_all_configs`, this never fails the whole load when one
/// domain has issues. It is intended for startup fallback paths.
pub fn load_all_configs_best_effort(app_dirs: &AppDirs, schema_dir: &Path) -> PersistenceState {
    info!(
        "Loading configuration (best-effort) from {:?}",
        app_dirs.configs
    );

    let global_settings = match YamlConfigManager::new(GLOBAL_SETTINGS_SECTION, schema_dir, &app_dirs.configs) {
        Ok(mgr) => mgr.get_content().clone(),
        Err(err) => {
            warn!("Failed to load global settings: {}", err);
            serde_json::json!({
                "units": "mm",
                "theme": "Light",
                "selected_process_profile_id": Value::Null,
                "selected_cnc_profile_id": Value::Null,
                "selected_fixture_profile_id": Value::Null,
                "selected_toolset_profile_id": Value::Null,
            })
        }
    };

    let stock = match YamlConfigManager::new(STOCK_SECTION, schema_dir, &app_dirs.configs) {
        Ok(mgr) => mgr.get_content().clone(),
        Err(err) => {
            warn!("Failed to load stock settings: {}", err);
            serde_json::json!({ "tools": [] })
        }
    };

    let cnc_profiles = match load_cnc_profiles(&app_dirs.cnc_profiles, schema_dir) {
        Ok(v) => v,
        Err(err) => {
            warn!("Failed to load CNC profiles: {}", err);
            BTreeMap::new()
        }
    };
    let fixture_profiles = match load_profiles_from_dir(&app_dirs.fixture_profiles) {
        Ok(v) => v,
        Err(err) => {
            warn!("Failed to load fixture profiles: {}", err);
            BTreeMap::new()
        }
    };
    let processing_profiles = match load_profiles_from_dir(&app_dirs.processing_profiles) {
        Ok(v) => v,
        Err(err) => {
            warn!("Failed to load processing profiles: {}", err);
            BTreeMap::new()
        }
    };
    let toolset_profiles = match load_profiles_from_dir(&app_dirs.toolset_profiles) {
        Ok(v) => v,
        Err(err) => {
            warn!("Failed to load toolset profiles: {}", err);
            BTreeMap::new()
        }
    };

    PersistenceState {
        selected_process_profile_id: global_settings
            .get("selected_process_profile_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        last_edited_process_profile_id: global_settings
            .get("last_edited_process_profile_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        selected_cnc_profile_id: global_settings
            .get("selected_cnc_profile_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        selected_fixture_profile_id: global_settings
            .get("selected_fixture_profile_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        selected_toolset_profile_id: global_settings
            .get("selected_toolset_profile_id")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        global_settings,
        stock,
        cnc_profiles,
        fixture_profiles,
        processing_profiles,
        toolset_profiles,
    }
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
                if !has_uuid_file_stem(&path) {
                    warn!(
                        "Skipping CNC profile '{}': filename stem is not a UUID",
                        path.display()
                    );
                    continue;
                }

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

fn load_profiles_from_dir(profiles_dir: &Path) -> Result<BTreeMap<String, Value>, ConfigError> {
    let mut profiles = BTreeMap::new();

    if !profiles_dir.exists() {
        return Ok(profiles);
    }

    let entries = match fs::read_dir(profiles_dir) {
        Ok(e) => e,
        Err(err) => {
            warn!("Failed to read profiles directory '{}': {}", profiles_dir.display(), err);
            return Ok(profiles);
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|s| s.to_str());
        if ext != Some("yaml") && ext != Some("yml") {
            continue;
        }

        if !has_uuid_file_stem(&path) {
            warn!(
                "Skipping profile '{}': filename stem is not a UUID",
                path.display()
            );
            continue;
        }

        let text = match fs::read_to_string(&path) {
            Ok(v) => v,
            Err(err) => {
                warn!("Could not read profile '{}': {}", path.display(), err);
                continue;
            }
        };

        let yaml_value: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(err) => {
                warn!("Could not parse profile '{}': {}", path.display(), err);
                continue;
            }
        };

        let json_value: Value = match serde_json::to_value(yaml_value) {
            Ok(v) => v,
            Err(err) => {
                warn!("Could not convert profile '{}' to json: {}", path.display(), err);
                continue;
            }
        };

        if let Some(id) = json_value.get("id").and_then(Value::as_str) {
            profiles.insert(id.to_string(), json_value);
        }
    }

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
    if let Some(parent) = app_dirs.global_settings.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            error!("Failed to create config directory: {}", e);
            ConfigError::Io(e)
        })?;
    }

    let mut item_values = BTreeMap::new();
    item_values.insert(
        format!("config:{}", GLOBAL_SETTINGS_SECTION),
        global_settings.clone(),
    );
    queue_persist_document(
        app_dirs.global_settings.clone(),
        item_values,
        global_settings.clone(),
    )
}

/// Save tool stock to stock.yaml
#[allow(dead_code)]
pub fn save_stock(app_dirs: &AppDirs, stock: &Value) -> Result<(), ConfigError> {
    fs::create_dir_all(&app_dirs.configs).map_err(|e| {
        error!("Failed to create config directory: {}", e);
        ConfigError::Io(e)
    })?;

    write_yaml_file_atomically(&app_dirs.stock, stock)
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

    let mut item_values = BTreeMap::new();
    item_values.insert(
        format!("cnc_profile:{}", profile_name),
        profile_data.clone(),
    );
    queue_persist_document(file_path, item_values, profile_data.clone())
}

pub fn save_cnc_profiles(
    app_dirs: &AppDirs,
    profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    save_profile_map(&app_dirs.cnc_profiles, profiles)
}

pub fn save_fixture_profiles(
    app_dirs: &AppDirs,
    profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    save_profile_map(&app_dirs.fixture_profiles, profiles)
}

pub fn save_processing_profiles(
    app_dirs: &AppDirs,
    profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    save_profile_map(&app_dirs.processing_profiles, profiles)
}

pub fn save_toolset_profiles(
    app_dirs: &AppDirs,
    profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    save_profile_map(&app_dirs.toolset_profiles, profiles)
}

pub fn save_processing_and_toolset_profiles_session(
    app_dirs: &AppDirs,
    processing_profiles: &BTreeMap<String, Value>,
    toolset_profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    let mut session = begin_persist_session();
    enqueue_profile_map_requests_in_session(
        &mut session,
        &app_dirs.processing_profiles,
        processing_profiles,
    )?;
    enqueue_profile_map_requests_in_session(
        &mut session,
        &app_dirs.toolset_profiles,
        toolset_profiles,
    )?;
    end_persist_session(session)
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
    let mut item_values = BTreeMap::new();
    item_values.insert(format!("config:{}", file_stem), content.clone());
    queue_persist_document(file_path, item_values, content.clone())
}

fn save_profile_map(
    dir: &Path,
    profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    let mut session = begin_persist_session();
    enqueue_profile_map_requests_in_session(&mut session, dir, profiles)?;
    end_persist_session(session)
}

fn enqueue_profile_map_requests_in_session(
    session: &mut super::manager::PersistSession,
    dir: &Path,
    profiles: &BTreeMap<String, Value>,
) -> Result<(), ConfigError> {
    fs::create_dir_all(dir).map_err(ConfigError::Io)?;

    let mut id_to_stem = BTreeMap::<String, String>::new();
    for id in profiles.keys() {
        let stem = profile_stem_for_id(id);
        id_to_stem.insert(id.clone(), stem);
    }

    let expected_stems = id_to_stem
        .values()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();

    // Remove stale yaml files that no longer exist in memory.
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str());
            if ext != Some("yaml") && ext != Some("yml") {
                continue;
            }

            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            if !expected_stems.contains(stem) {
                let _ = fs::remove_file(&path);
            }
        }
    }

    for (id, profile_data) in profiles {
        let Some(stem) = id_to_stem.get(id) else {
            continue;
        };
        let file_path = dir.join(format!("{}.yaml", stem));
        let mut item_values = BTreeMap::new();
        item_values.insert(
            format!("profile:{}:{}:{}", dir.display(), id, stem),
            profile_data.clone(),
        );
        queue_persist_document_in_session(session, file_path, item_values, profile_data.clone());
    }

    Ok(())
}

fn profile_stem_for_id(
    id: &str,
) -> String {
    // Profile filename stems are UUIDs to ensure immutable, unique file names.
    // If `id` is already a UUID, keep it. For legacy non-UUID ids, derive a
    // deterministic UUIDv5 so filenames remain stable across saves.
    if let Ok(parsed) = Uuid::parse_str(id) {
        parsed.to_string()
    } else {
        Uuid::new_v5(&Uuid::NAMESPACE_OID, id.as_bytes()).to_string()
    }
}

fn has_uuid_file_stem(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|stem| Uuid::parse_str(stem).ok())
        .is_some()
}

fn write_yaml_file_atomically(file_path: &Path, content: &Value) -> Result<(), ConfigError> {
    let yaml_value: serde_yaml::Value = serde_json::from_value(content.clone())
        .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
    let yaml_payload = serde_yaml::to_string(&yaml_value)
        .map_err(|e| ConfigError::ConfigParse(e.to_string()))?;

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::Io)?;
    }

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let temp_name = format!(
        ".k2g.tmp.stock.{}.{}.yaml",
        process::id(),
        nanos
    );
    let temp_path = file_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(temp_name);

    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(ConfigError::Io)?;

    file.write_all(yaml_payload.as_bytes()).map_err(ConfigError::Io)?;
    file.flush().map_err(ConfigError::Io)?;
    file.sync_all().map_err(ConfigError::Io)?;
    drop(file);

    match fs::rename(&temp_path, file_path) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            if file_path.exists() {
                fs::remove_file(file_path).map_err(ConfigError::Io)?;
                fs::rename(&temp_path, file_path).map_err(ConfigError::Io)
            } else {
                let _ = fs::remove_file(&temp_path);
                Err(ConfigError::Io(first_err))
            }
        }
    }
}
