use std::path::{Path, PathBuf};

use log::warn;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::error::ConfigError;

#[derive(Clone)]
pub struct LoadedYamlDocument {
    pub path: PathBuf,
    pub stem: String,
    pub value: Value,
}

pub fn load_yaml_dir_with_schema_pointer(
    dir: &Path,
    expected_schema: &str,
) -> Result<Vec<LoadedYamlDocument>, ConfigError> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(dir)?;

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if ext != "yaml" && ext != "yml" {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                warn!("Ignoring YAML file '{}': read failed ({e})", path.display());
                continue;
            }
        };

        let yaml_val: serde_yaml::Value = match serde_yaml::from_str(&text) {
            Ok(v) => v,
            Err(e) => {
                warn!("Ignoring YAML file '{}': parse failed ({e})", path.display());
                continue;
            }
        };

        let json_val: Value = match serde_json::to_value(yaml_val) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    "Ignoring YAML file '{}': YAML->JSON conversion failed ({e})",
                    path.display()
                );
                continue;
            }
        };

        if let Err(reason) = validate_schema_pointer(&json_val, expected_schema) {
            warn!("Ignoring YAML file '{}': {reason}", path.display());
            continue;
        }

        out.push(LoadedYamlDocument {
            path,
            stem,
            value: json_val,
        });
    }

    Ok(out)
}

pub fn validate_schema_pointer(document: &Value, expected_schema: &str) -> Result<(), String> {
    let Some(pointer) = schema_pointer(document) else {
        return Err(format!(
            "missing required schema pointer ($schema), expected '{}'",
            expected_schema
        ));
    };

    if schema_pointer_matches(pointer, expected_schema) {
        Ok(())
    } else {
        Err(format!(
            "schema pointer '{}' does not match expected '{}'",
            pointer, expected_schema
        ))
    }
}

fn schema_pointer(document: &Value) -> Option<&str> {
    document
        .get("$schema")
        .and_then(Value::as_str)
        .or_else(|| document.get("schema").and_then(Value::as_str))
}

fn schema_pointer_matches(pointer: &str, expected_schema: &str) -> bool {
    if pointer == expected_schema {
        return true;
    }

    let expected_basename = Path::new(expected_schema)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(expected_schema);

    let pointer_no_fragment = pointer.split('#').next().unwrap_or(pointer);
    let pointer_basename = Path::new(pointer_no_fragment)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(pointer_no_fragment);

    pointer_basename.eq_ignore_ascii_case(expected_basename)
}

/// Parse YAML text, validate schema pointer, optionally normalize, and decode.
pub fn parse_yaml_with_schema<T, F>(
    text: &str,
    expected_schema: &str,
    mut normalize: F,
) -> Result<T, String>
where
    T: DeserializeOwned,
    F: FnMut(&mut Value),
{
    let normalized = text.trim_start_matches('\u{feff}');
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(normalized)
        .map_err(|e| format!("yaml parse failed: {e}"))?;

    let mut json_value: Value = serde_json::to_value(yaml_value)
        .map_err(|e| format!("yaml->json conversion failed: {e}"))?;

    validate_schema_pointer(&json_value, expected_schema)
        .map_err(|e| format!("schema pointer check failed: {e}"))?;

    normalize(&mut json_value);

    serde_json::from_value(json_value)
        .map_err(|e| format!("yaml decode failed: {e}"))
}