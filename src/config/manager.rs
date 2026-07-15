use std::path::{Path, PathBuf};
use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};
use log::{error, info, warn};
use serde_json::Value;
use uuid::Uuid;

use super::{
    defaults::{populate_defaults, synchronize},
    error::ConfigError,
    validator::SchemaValidator,
};

const SCHEMA_SUFFIX: &str = ".yaml";
const BACKUP_SUFFIX: &str = "bak"; // no leading dot

#[derive(Clone)]
struct QueuedWrite {
    target_path: PathBuf,
    item_hashes: BTreeMap<String, String>,
    yaml_payload: String,
}

#[derive(Default)]
struct QueueState {
    queue: VecDeque<QueuedWrite>,
    persisted_hashes: BTreeMap<String, String>,
    pending_hashes: BTreeMap<String, String>,
}

struct WriteQueueInner {
    state: Mutex<QueueState>,
    wake: Condvar,
    temp_seq: AtomicU64,
}

pub struct PersistenceWriteManager {
    inner: Arc<WriteQueueInner>,
}

impl PersistenceWriteManager {
    fn global() -> &'static Self {
        static INSTANCE: OnceLock<PersistenceWriteManager> = OnceLock::new();
        INSTANCE.get_or_init(|| {
            let inner = Arc::new(WriteQueueInner {
                state: Mutex::new(QueueState::default()),
                wake: Condvar::new(),
                temp_seq: AtomicU64::new(1),
            });

            let worker_inner = inner.clone();
            thread::Builder::new()
                .name("k2g-config-writer".to_string())
                .spawn(move || run_write_worker(worker_inner))
                .expect("Failed to spawn config writer thread");

            Self { inner }
        })
    }

    fn enqueue(
        &self,
        target_path: PathBuf,
        item_values: BTreeMap<String, Value>,
        full_payload: Value,
    ) -> Result<bool, ConfigError> {
        let yaml_payload = value_to_yaml_string(&full_payload)?;

        let mut item_hashes = item_values
            .iter()
            .map(|(key, value)| (key.clone(), canonical_value_hash(value)))
            .collect::<BTreeMap<_, _>>();

        if item_hashes.is_empty() {
            // Fallback key for non-collection documents.
            item_hashes.insert(
                format!("file:{}", target_path.to_string_lossy()),
                canonical_value_hash(&full_payload),
            );
        }

        let mut guard = self
            .inner
            .state
            .lock()
            .expect("config writer mutex poisoned");

        // Bootstrap persisted hashes from disk if this target already matches.
        let has_unseen_item = item_hashes
            .keys()
            .any(|item_key| !guard.persisted_hashes.contains_key(item_key));
        let target_already_queued = guard.queue.iter().any(|q| q.target_path == target_path);
        if has_unseen_item
            && !target_already_queued
            && file_payload_matches(&target_path, &full_payload)
        {
            for (item_key, hash) in &item_hashes {
                guard.persisted_hashes.insert(item_key.clone(), hash.clone());
            }
            return Ok(false);
        }

        let has_change = item_hashes.iter().any(|(item_key, new_hash)| {
            let persisted = guard.persisted_hashes.get(item_key);
            let pending = guard.pending_hashes.get(item_key);
            persisted != Some(new_hash) && pending != Some(new_hash)
        });

        if !has_change {
            return Ok(false);
        }

        // If the same file already has a queued write, drop the oldest one.
        if let Some(idx) = guard
            .queue
            .iter()
            .position(|queued| queued.target_path == target_path)
        {
            if let Some(oldest) = guard.queue.remove(idx) {
                for (item_key, hash) in &oldest.item_hashes {
                    if guard.pending_hashes.get(item_key) == Some(hash) {
                        guard.pending_hashes.remove(item_key);
                    }
                }
            }
        }

        for (item_key, hash) in &item_hashes {
            guard.pending_hashes.insert(item_key.clone(), hash.clone());
        }

        guard.queue.push_back(QueuedWrite {
            target_path,
            item_hashes,
            yaml_payload,
        });

        self.inner.wake.notify_one();
        Ok(true)
    }
}

pub fn queue_persist_document(
    target_path: PathBuf,
    item_values: BTreeMap<String, Value>,
    full_payload: Value,
) -> Result<(), ConfigError> {
    let _ = PersistenceWriteManager::global().enqueue(target_path, item_values, full_payload)?;
    Ok(())
}

fn run_write_worker(inner: Arc<WriteQueueInner>) {
    loop {
        let next = {
            let mut guard = inner
                .state
                .lock()
                .expect("config writer mutex poisoned");

            while guard.queue.is_empty() {
                guard = inner
                    .wake
                    .wait(guard)
                    .expect("config writer condvar poisoned");
            }

            guard.queue.pop_front()
        };

        let Some(queued) = next else {
            continue;
        };

        match write_yaml_atomically(&inner, &queued.target_path, &queued.yaml_payload) {
            Ok(()) => {
                let mut guard = inner
                    .state
                    .lock()
                    .expect("config writer mutex poisoned");
                for (item_key, hash) in &queued.item_hashes {
                    guard.persisted_hashes.insert(item_key.clone(), hash.clone());
                    if guard.pending_hashes.get(item_key) == Some(hash) {
                        guard.pending_hashes.remove(item_key);
                    }
                }
                info!("Persisted config file: {}", queued.target_path.display());
            }
            Err(err) => {
                let mut guard = inner
                    .state
                    .lock()
                    .expect("config writer mutex poisoned");
                for (item_key, hash) in &queued.item_hashes {
                    if guard.pending_hashes.get(item_key) == Some(hash) {
                        guard.pending_hashes.remove(item_key);
                    }
                }
                error!(
                    "Failed to persist config file '{}': {}",
                    queued.target_path.display(),
                    err
                );
            }
        }
    }
}

fn write_yaml_atomically(
    inner: &WriteQueueInner,
    target_path: &Path,
    yaml_payload: &str,
) -> Result<(), ConfigError> {
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::Io)?;
    }

    let temp_path = temp_path_for_target(inner, target_path);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(ConfigError::Io)?;

    file.write_all(yaml_payload.as_bytes()).map_err(ConfigError::Io)?;
    file.flush().map_err(ConfigError::Io)?;
    file.sync_all().map_err(ConfigError::Io)?;
    drop(file);

    // Try atomic rename first. On platforms where replacement is not supported,
    // delete then rename as a fallback.
    match fs::rename(&temp_path, target_path) {
        Ok(()) => Ok(()),
        Err(first_err) => {
            if target_path.exists() {
                fs::remove_file(target_path).map_err(ConfigError::Io)?;
                fs::rename(&temp_path, target_path).map_err(ConfigError::Io)
            } else {
                let _ = fs::remove_file(&temp_path);
                Err(ConfigError::Io(first_err))
            }
        }
    }
}

fn temp_path_for_target(inner: &WriteQueueInner, target_path: &Path) -> PathBuf {
    let parent = target_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = target_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config.yaml");
    let seq = inner.temp_seq.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let temp_name = format!(
        ".k2g.tmp.{}.{}.{}.{}",
        file_name,
        process::id(),
        nanos,
        seq
    );

    parent.join(temp_name)
}

fn value_to_yaml_string(value: &Value) -> Result<String, ConfigError> {
    let yaml_value: serde_yaml::Value = serde_json::from_value(value.clone())
        .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;
    serde_yaml::to_string(&yaml_value)
        .map_err(|e| ConfigError::ConfigParse(e.to_string()))
}

fn canonical_value_hash(value: &Value) -> String {
    let canonical = canonicalize_json(value);
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    format!("{:016x}", fnv1a64(&bytes))
}

fn canonicalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut sorted = serde_json::Map::new();
            for (key, child) in map.iter().collect::<BTreeMap<_, _>>() {
                sorted.insert(key.clone(), canonicalize_json(child));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonicalize_json).collect()),
        _ => value.clone(),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn is_uuid_v7_like(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }

    for &idx in &[8usize, 13, 18, 23] {
        if bytes[idx] != b'-' {
            return false;
        }
    }

    for (i, b) in bytes.iter().enumerate() {
        if i == 8 || i == 13 || i == 18 || i == 23 {
            continue;
        }
        if !b.is_ascii_hexdigit() {
            return false;
        }
    }

    if bytes[14] != b'7' {
        return false;
    }

    matches!(bytes[19] as char, '8' | '9' | 'a' | 'b' | 'A' | 'B')
}

fn migrate_legacy_stock_ids(value: &mut Value) -> bool {
    let Some(items) = value.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };

    let mut changed = false;
    for item in items {
        let current_id = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let current_ref_tool_id = item
            .get("ref")
            .and_then(|v| v.get("tool_id"))
            .and_then(Value::as_str)
            .unwrap_or_default();

        let needs_item_id = !is_uuid_v7_like(current_id);
        let needs_ref_id = !is_uuid_v7_like(current_ref_tool_id);
        if !needs_item_id && !needs_ref_id {
            continue;
        }

        let replacement = Uuid::now_v7().to_string();
        if let Some(obj) = item.as_object_mut() {
            obj.insert("id".to_string(), Value::String(replacement.clone()));
        }
        if let Some(ref_obj) = item.get_mut("ref").and_then(Value::as_object_mut) {
            ref_obj.insert("tool_id".to_string(), Value::String(replacement));
        }
        changed = true;
    }

    changed
}

pub struct YamlConfigManager {
    #[allow(dead_code)]
    section_name: String,
    config_path: PathBuf,
    schema: Value,
    validator: SchemaValidator,
    pub content: Value,
}

impl YamlConfigManager {
    /// Create a new manager for `section_name`.
    /// Schema is loaded from `schema_dir/<section_name>.yaml`.
    /// Config is loaded from `config_dir/<section_name>.yaml`.
    pub fn new(
        section_name: &str,
        schema_dir: &Path,
        config_dir: &Path,
    ) -> Result<Self, ConfigError> {
        let schema_path = Self::resolve_schema_path(schema_dir, section_name);
        let config_path = config_dir.join(format!("{}{}", section_name, SCHEMA_SUFFIX));

        // --- Load & compile schema ---
        let schema = Self::load_schema(&schema_path)?;
        let validator = SchemaValidator::new(&schema)?;

        let mut manager = Self {
            section_name: section_name.to_string(),
            config_path,
            schema,
            validator,
            content: Value::Null,
        };

        manager.load_content();
        Ok(manager)
    }

    fn resolve_schema_path(schema_dir: &Path, section_name: &str) -> PathBuf {
        let mapped = match section_name {
            "global.setting" | "global_settings" => "settings",
            "cnc_profile" => "cnc",
            "fixture_profile" => "fixture",
            "process_profile" => "processing",
            "toolset_profile" => "toolset",
            _ => section_name,
        };

        let candidates = [
            format!("{}{}", mapped, SCHEMA_SUFFIX),
            format!("{}_schema{}", mapped, SCHEMA_SUFFIX),
            format!("{}{}", section_name, SCHEMA_SUFFIX),
            format!("{}_schema{}", section_name, SCHEMA_SUFFIX),
        ];

        for candidate in candidates {
            let path = schema_dir.join(candidate);
            if path.exists() {
                return path;
            }
        }

        schema_dir.join(format!("{}{}", mapped, SCHEMA_SUFFIX))
    }

    /// Load and parse a YAML schema file into a serde_json::Value
    fn load_schema(path: &Path) -> Result<Value, ConfigError> {
        if !path.exists() {
            error!("Missing schema file: {}", path.display());
            return Err(ConfigError::SchemaMissing(path.display().to_string()));
        }

        let text = std::fs::read_to_string(path)?;

        // Parse YAML then round-trip through JSON for jsonschema compatibility
        let yaml_value: serde_yaml::Value = serde_yaml::from_str(&text)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

        let json_value: Value = serde_json::to_value(yaml_value)
            .map_err(|e| ConfigError::SchemaParse(e.to_string()))?;

        Ok(json_value)
    }

    /// Generate default content from schema defaults
    fn generate_default_content(&self) -> Value {
        populate_defaults(&self.schema).unwrap_or(Value::Object(Default::default()))
    }

    /// Load config file, validate, merge with defaults if needed.
    /// Mirrors Python's load_content() behaviour exactly.
    fn load_content(&mut self) {
        let file_exists = self.config_path.exists();
        let mut overwrite = true;

        if file_exists {
            match self.parse_config() {
                Some(parsed) if !parsed.is_null() => {
                    if self.validator.validate(&parsed).is_ok() {
                        // Valid — use as-is
                        self.content = parsed;
                        overwrite = false;
                    } else {
                        let mut migrated_stock = false;
                        if self.section_name == "stock" {
                            let mut migrated = parsed.clone();
                            if migrate_legacy_stock_ids(&mut migrated)
                                && self.validator.validate(&migrated).is_ok()
                            {
                                warn!(
                                    "Migrated legacy stock IDs in '{}' to UUIDv7.",
                                    self.config_path.display()
                                );
                                self.content = migrated;
                                migrated_stock = true;
                            }
                        }

                        if !migrated_stock {
                            // Invalid — merge with defaults
                            warn!(
                                "Config '{}' failed validation; merging with defaults.",
                                self.config_path.display()
                            );
                            let defaults = self.generate_default_content();
                            self.content = synchronize(&parsed, &defaults);

                            // Re-validate merged result
                            if self.validator.validate(&self.content).is_ok() {
                                overwrite = true; // Save the repaired version
                            } else {
                                error!("Merged config still invalid; falling back to defaults.");
                                self.content = self.generate_default_content();
                            }
                        }
                    }
                }
                _ => {
                    error!("Failed to parse '{}'; using defaults.", self.config_path.display());
                    self.content = self.generate_default_content();
                }
            }
        } else {
            info!(
                "Config '{}' not found; creating default.",
                self.config_path.display()
            );
            self.content = self.generate_default_content();
        }

        if overwrite {
            let backed_up = if file_exists { self.backup() } else { true };
            if backed_up {
                self.write_content();
            }
        }
    }

    /// Parse the YAML config file into a JSON Value
    fn parse_config(&self) -> Option<Value> {
        let text = match std::fs::read_to_string(&self.config_path) {
            Ok(t) => t,
            Err(e) => {
                error!("Cannot read '{}': {}", self.config_path.display(), e);
                return None;
            }
        };

        let yaml_value: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                error!("YAML parse error in '{}': {}", self.config_path.display(), e);
                return None;
            }
        };

        match serde_json::to_value(yaml_value) {
            Ok(v) => Some(v),
            Err(e) => {
                error!("JSON conversion error: {}", e);
                None
            }
        }
    }

    /// Write current content back to the config file as YAML
    fn write_content(&self) {
        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    error!("Failed to create directory '{}': {}", parent.display(), e);
                    return;
                }
            }
        }

        let yaml_value: serde_yaml::Value = match serde_json::from_value(self.content.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("Failed to convert content to YAML value: {}", e);
                return;
            }
        };

        match serde_yaml::to_string(&yaml_value) {
            Ok(text) => {
                if let Err(e) = std::fs::write(&self.config_path, text) {
                    error!("Failed to write '{}': {}", self.config_path.display(), e);
                }
            }
            Err(e) => error!("Failed to serialize YAML: {}", e),
        }
    }

    /// Rename the existing config file to .bak
    fn backup(&self) -> bool {
        let backup_path = self.config_path.with_extension(BACKUP_SUFFIX);
        match std::fs::rename(&self.config_path, &backup_path) {
            Ok(_) => {
                warn!("Backed up '{}' to '{}'", self.config_path.display(), backup_path.display());
                true
            }
            Err(e) => {
                error!("Failed to back up '{}': {}", self.config_path.display(), e);
                false
            }
        }
    }

    pub fn get_content(&self) -> &Value {
        &self.content
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn write_file(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    fn minimal_schema() -> &'static str {
        r#"
type: object
properties:
  max_feed_rate:
    type: number
    default: 2000.0
  name:
    type: string
    default: "default"
required: [max_feed_rate, name]
"#
    }

    #[test]
    fn test_loads_defaults_when_no_config() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());

        let mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert_eq!(mgr.content["max_feed_rate"], 2000.0);
        assert_eq!(mgr.content["name"], "default");
    }

    #[test]
    fn test_loads_valid_config() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());
        write_file(
            &config_dir.join("machine.yaml"),
            "max_feed_rate: 1500.0\nname: custom\n",
        );

        let mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert_eq!(mgr.content["max_feed_rate"], 1500.0);
        assert_eq!(mgr.content["name"], "custom");
    }

    #[test]
    fn test_merges_partial_config_with_defaults() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());
        // Only provides one of the two required fields
        write_file(&config_dir.join("machine.yaml"), "max_feed_rate: 999.0\n");

        let mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert_eq!(mgr.content["max_feed_rate"], 999.0); // from file
        assert_eq!(mgr.content["name"], "default");      // from schema default
    }

    #[test]
    fn test_backup_created_on_invalid_config() {
        let tmp = tempdir().unwrap();
        let schema_dir = tmp.path().join("schema");
        let config_dir = tmp.path().join("config");
        fs::create_dir_all(&schema_dir).unwrap();
        fs::create_dir_all(&config_dir).unwrap();

        write_file(&schema_dir.join("machine.yaml"), minimal_schema());
        write_file(&config_dir.join("machine.yaml"), "{ invalid yaml: [[[");

        let _mgr = YamlConfigManager::new("machine", &schema_dir, &config_dir).unwrap();
        assert!(config_dir.join("machine.bak").exists());
    }
}

fn file_payload_matches(target_path: &Path, payload: &Value) -> bool {
    let text = match fs::read_to_string(target_path) {
        Ok(text) => text,
        Err(_) => return false,
        };

    let yaml_value: serde_yaml::Value = match serde_yaml::from_str(&text) {
        Ok(value) => value,
        Err(_) => return false,
    };

    let json_value: Value = match serde_json::to_value(yaml_value) {
        Ok(value) => value,
        Err(_) => return false,
    };

    canonical_value_hash(&json_value) == canonical_value_hash(payload)
}