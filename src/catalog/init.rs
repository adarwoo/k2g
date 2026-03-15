//! First-run initialization — writes embedded catalog and schema files to the
//! user data directory the first time the application executes.
//!
//! Catalogs are only written if they do not already exist so that user edits
//! are preserved across upgrades.  Schema files are always overwritten because
//! they are reference-only and should track the application version.

use log::{info, warn};
use serde_json::Value;
use std::path::Path;

use crate::user_path::{ensure_app_dirs, UserPathError};

// ---------------------------------------------------------------------------
// Embedded resource files (compiled into the binary)
// ---------------------------------------------------------------------------

// --- Schemas (always overwritten on startup) ---
const SCHEMAS: &[(&str, &str)] = &[
    ("catalog_schema.yaml",       include_str!("../../resources/schemas/catalog_schema.yaml")),
    ("global_settings_schema.yaml", include_str!("../../resources/schemas/global_settings_schema.yaml")),
    ("machine.yaml",              include_str!("../../resources/schemas/machine.yaml")),
    ("machining_data_schema.yaml", include_str!("../../resources/schemas/machining_data_schema.yaml")),
    ("masso.yaml",                include_str!("../../resources/schemas/masso.yaml")),
    ("rack_schema.yaml",          include_str!("../../resources/schemas/rack_schema.yaml")),
    ("stock_schema.yaml",         include_str!("../../resources/schemas/stock_schema.yaml")),
];

// --- Catalogs (written only on first run; user edits are preserved) ---
const CATALOGS: &[(&str, &str)] = &[
    ("kyocera.yaml",  include_str!("../../resources/catalogs/kyocera.yaml")),
    ("unionfab.yaml", include_str!("../../resources/catalogs/unionfab.yaml")),
    ("generic.yaml",  include_str!("../../resources/catalogs/generic.yaml")),
];

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Ensure the user data directory tree exists and populate it with the
/// built-in files.
///
/// Errors are returned rather than panicked so that the caller can decide
/// whether to abort or continue with degraded functionality.
pub fn first_run_init() -> Result<(), UserPathError> {
    let dirs = ensure_app_dirs()?;

    // Schemas — always refresh from the embedded copy.
    for (name, content) in SCHEMAS {
        let dest = dirs.schemas.join(name);
        match std::fs::write(&dest, content) {
            Ok(_) => info!("Wrote schema reference: {}", dest.display()),
            Err(e) => warn!("Could not write schema '{}': {e}", dest.display()),
        }
    }

    // Catalogs — write only if the file does not yet exist.
    for (name, content) in CATALOGS {
        let dest = dirs.catalogs.join(name);
        if dest.exists() {
            if let Err(e) = backfill_catalog_fields(&dest) {
                warn!("Could not backfill catalog '{}': {e}", dest.display());
            }
            continue;
        }
        match std::fs::write(&dest, content) {
            Ok(_) => info!("Created default catalog: {}", dest.display()),
            Err(e) => warn!("Could not write catalog '{}': {e}", dest.display()),
        }

        if let Err(e) = backfill_catalog_fields(&dest) {
            warn!("Could not backfill catalog '{}': {e}", dest.display());
        }
    }

    Ok(())
}

/// Return the path to the user catalogs directory, creating the directory
/// tree first if needed.
pub fn catalog_dir() -> Result<std::path::PathBuf, UserPathError> {
    ensure_app_dirs().map(|d| d.catalogs)
}

fn backfill_catalog_fields(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read failed: {e}"))?;

    let yaml_value: serde_yaml::Value = serde_yaml::from_str(&text)
        .map_err(|e| format!("yaml parse failed: {e}"))?;

    let mut json_value: Value = serde_json::to_value(yaml_value)
        .map_err(|e| format!("yaml->json conversion failed: {e}"))?;

    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("catalog")
        .to_string();

    if !inject_missing_catalog_fields(&mut json_value, &stem) {
        return Ok(());
    }

    let out_yaml: serde_yaml::Value = serde_json::from_value(json_value)
        .map_err(|e| format!("json->yaml conversion failed: {e}"))?;

    let out_text = serde_yaml::to_string(&out_yaml)
        .map_err(|e| format!("yaml serialization failed: {e}"))?;

    std::fs::write(path, out_text)
        .map_err(|e| format!("write failed: {e}"))?;

    info!("Backfilled catalog metadata: {}", path.display());
    Ok(())
}

fn inject_missing_catalog_fields(root: &mut Value, stem: &str) -> bool {
    let mut changed = false;

    let Some(sections) = root
        .get_mut("sections")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };

    for section in sections {
        if !section.get("default_diameter_unit").is_some() {
            if let Some(obj) = section.as_object_mut() {
                obj.insert("default_diameter_unit".to_string(), Value::String("mm".to_string()));
                changed = true;
            }
        }

        if !section.get("default_feed_unit").is_some() {
            let diameter_unit = section
                .get("default_diameter_unit")
                .and_then(Value::as_str)
                .unwrap_or("mm");
            let feed_unit = if diameter_unit == "in" { "ipm" } else { "mm_min" };
            if let Some(obj) = section.as_object_mut() {
                obj.insert("default_feed_unit".to_string(), Value::String(feed_unit.to_string()));
                changed = true;
            }
        }

        if !section.get("default_flute_length_unit").is_some() {
            if let Some(obj) = section.as_object_mut() {
                obj.insert("default_flute_length_unit".to_string(), Value::String("mm".to_string()));
                changed = true;
            }
        }

        let section_name = section
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Section");
        let section_slug = slugify(section_name);

        let Some(tools) = section
            .get_mut("tools")
            .and_then(Value::as_array_mut)
        else {
            continue;
        };

        for tool in tools {
            let Some(obj) = tool.as_object_mut() else {
                continue;
            };

            let tool_type = obj
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let diameter = obj
                .get("diameter")
                .and_then(as_f64_flexible)
                .unwrap_or(0.0);

            if !obj.contains_key("sku_name") {
                let tool_kind = if tool_type == "routerbit" { "R" } else { "D" };
                let sku = format!(
                    "{}-{}-{}{}",
                    stem,
                    section_slug,
                    tool_kind,
                    format_diameter_token(diameter)
                );
                obj.insert("sku_name".to_string(), Value::String(sku));
                changed = true;
            }

            if !obj.contains_key("point_angle") {
                let angle = default_point_angle(stem, &tool_type, diameter);
                obj.insert("point_angle".to_string(), Value::from(angle));
                changed = true;
            }
        }
    }

    changed
}

fn default_point_angle(stem: &str, tool_type: &str, diameter: f64) -> i64 {
    if tool_type == "routerbit" {
        return 180;
    }

    if stem == "generic" {
        if diameter >= 4.0 {
            150
        } else if diameter >= 3.0 {
            145
        } else if diameter >= 2.0 {
            140
        } else if diameter >= 1.5 {
            135
        } else if diameter >= 1.2 {
            132
        } else {
            130
        }
    } else {
        130
    }
}

fn format_diameter_token(value: f64) -> String {
    let raw = format!("{value:.3}");
    raw.trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

fn slugify(input: &str) -> String {
    let mut out = String::new();
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        }
    }

    if out.is_empty() {
        "Section".to_string()
    } else {
        out
    }
}

fn as_f64_flexible(value: &Value) -> Option<f64> {
    if let Some(n) = value.as_f64() {
        return Some(n);
    }

    value
        .as_str()
        .and_then(|s| s.trim().parse::<f64>().ok())
}
