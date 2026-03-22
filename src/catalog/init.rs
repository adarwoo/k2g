//! First-run initialization — writes embedded catalog and schema files to the
//! user data directory the first time the application executes.
//!
//! Catalogs are only written if they do not already exist so that user edits
//! are preserved across upgrades.  Schema files are always overwritten because
//! they are reference-only and should track the application version.

use log::{info, warn};
use serde_json::Value;
use std::path::Path;

use crate::units::{Angle, AngleUnit, FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed, RotationalSpeedUnit};
use crate::user_path::{ensure_app_dirs, UserPathError};

// ---------------------------------------------------------------------------
// Embedded resource files (compiled into the binary)
// ---------------------------------------------------------------------------

// --- Schemas (always overwritten on startup) ---
const SCHEMAS: &[(&str, &str)] = &[
    ("catalog_schema", include_str!("../../resources/schemas/catalog.schema.yaml")),
    (
        "global.setting_schema",
        include_str!("../../resources/schemas/global_settings.schema.yaml"),
    ),
    (
        "cnc_profile_schema",
        include_str!("../../resources/schemas/cnc_profile.schema.yaml"),
    ),
    ("rack_schema", include_str!("../../resources/schemas/rack.schema.yaml")),
    ("stock_schema", include_str!("../../resources/schemas/stock.schema.yaml")),
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
        let dest = dirs.schemas.join(format!("{}.yaml", name));
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

pub(crate) fn parse_catalog_with_backfill(
    text: &str,
    stem: &str,
) -> Result<crate::catalog::types::Catalog, String> {
    let normalized = text.trim_start_matches('\u{feff}');
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(normalized)
        .map_err(|e| format!("yaml parse failed: {e}"))?;

    let mut json_value: Value = serde_json::to_value(yaml_value)
        .map_err(|e| format!("yaml->json conversion failed: {e}"))?;

    normalize_catalog_fields(&mut json_value, stem, true, true);

    serde_json::from_value(json_value)
        .map_err(|e| format!("catalog decode failed: {e}"))
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

    if !normalize_catalog_fields(&mut json_value, &stem, true, false) {
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

pub(crate) fn normalize_catalog_fields(
    root: &mut Value,
    stem: &str,
    inject_missing: bool,
    canonicalize_typed_values: bool,
) -> bool {
    let mut changed = false;

    let Some(sections) = root
        .get_mut("sections")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };

    for section in sections {
        if inject_missing && !section.get("default_flute_length_unit").is_some() {
            if let Some(obj) = section.as_object_mut() {
                obj.insert("default_flute_length_unit".to_string(), Value::String("mm".to_string()));
                changed = true;
            }
        }

        let section_linear_unit = "mm".to_string();
        let section_flute_unit = section
            .get("default_flute_length_unit")
            .and_then(Value::as_str)
            .unwrap_or(&section_linear_unit)
            .to_string();
        let section_feed_unit = "mm_min".to_string();

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
            let diameter_unit = obj
                .get("diameter_unit")
                .and_then(Value::as_str)
                .unwrap_or(&section_linear_unit)
                .to_string();
            let flute_length_unit = obj
                .get("flute_length_unit")
                .and_then(Value::as_str)
                .unwrap_or(&section_flute_unit)
                .to_string();

            let diameter = obj
                .get("diameter")
                .and_then(|value| parse_length_value(value, &diameter_unit));
            let diameter_mm = diameter.map(|value| value.as_mm()).unwrap_or(0.0);
            let point_angle = obj
                .get("point_angle")
                .and_then(parse_angle_value)
                .unwrap_or_else(|| default_point_angle(stem, &tool_type, diameter_mm) as f64);

            if canonicalize_typed_values {
                changed |= normalize_typed_value(obj, "diameter", &diameter_unit, normalize_length_value);
                changed |= normalize_typed_value(obj, "flute_length", &flute_length_unit, normalize_length_value);
                changed |= normalize_typed_value(obj, "z_feed", &section_feed_unit, normalize_feed_value);
                changed |= normalize_typed_value(obj, "table_feed", &section_feed_unit, normalize_feed_value);
            }

            if canonicalize_typed_values {
                if obj.remove("diameter_unit").is_some() {
                    changed = true;
                }
                if obj.remove("flute_length_unit").is_some() {
                    changed = true;
                }
            }

            if inject_missing && !obj.contains_key("sku_name") {
                let tool_kind = if tool_type == "routerbit" { "R" } else { "D" };
                let sku = format!(
                    "{}-{}-{}{}",
                    stem,
                    section_slug,
                    tool_kind,
                    format_diameter_token(diameter_mm)
                );
                obj.insert("sku_name".to_string(), Value::String(sku));
                changed = true;
            }

            if inject_missing && !obj.contains_key("point_angle") {
                let angle = default_point_angle(stem, &tool_type, diameter_mm);
                obj.insert("point_angle".to_string(), Value::from(angle));
                changed = true;
            }

            if !obj.contains_key("z_min_depth") {
                if inject_missing {
                    let z_min_depth = diameter
                        .map(|value| default_z_min_depth(value, point_angle))
                        .unwrap_or_else(|| format_length_with_unit(0.0, &diameter_unit));
                    obj.insert(
                        "z_min_depth".to_string(),
                        Value::String(z_min_depth),
                    );
                    changed = true;
                }
            } else if let Some(raw_value) = obj.get("z_min_depth") {
                if let Some(normalized) = normalize_length_value(raw_value, &diameter_unit) {
                    let needs_update = !matches!(raw_value, Value::String(current) if current == &normalized);
                    if canonicalize_typed_values && needs_update {
                        obj.insert("z_min_depth".to_string(), Value::String(normalized));
                        changed = true;
                    }
                }
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

fn default_z_min_depth(diameter: Length, point_angle: f64) -> String {
    let diameter_mm = diameter.as_mm();
    if diameter_mm <= 0.0 || point_angle >= 179.999 {
        return match diameter.unit() {
            LengthUnit::In | LengthUnit::Inch => Length::from_inch(0.0).to_string(),
            _ => Length::from_mm(0.0).to_string(),
        };
    }

    let half_angle_deg = (point_angle / 2.0).clamp(1.0, 89.999);
    let tip_depth_mm = (diameter_mm * 0.5) / half_angle_deg.to_radians().tan();

    if tip_depth_mm.is_finite() && tip_depth_mm > 0.0 {
        match diameter.unit() {
            LengthUnit::In | LengthUnit::Inch => Length::from_inch(tip_depth_mm / 25.4).to_string(),
            _ => Length::from_mm(tip_depth_mm).to_string(),
        }
    } else {
        match diameter.unit() {
            LengthUnit::In | LengthUnit::Inch => Length::from_inch(0.0).to_string(),
            _ => Length::from_mm(0.0).to_string(),
        }
    }
}

fn normalize_typed_value(
    obj: &mut serde_json::Map<String, Value>,
    key: &str,
    default_unit: &str,
    normalize: fn(&Value, &str) -> Option<String>,
) -> bool {
    let Some(raw_value) = obj.get(key) else {
        return false;
    };

    let Some(normalized) = normalize(raw_value, default_unit) else {
        return false;
    };

    let needs_update = !matches!(raw_value, Value::String(current) if current == &normalized);
    if needs_update {
        obj.insert(key.to_string(), Value::String(normalized));
        true
    } else {
        false
    }
}

fn parse_length_value(value: &Value, default_unit: &str) -> Option<Length> {
    let default_unit = linear_unit_from_str(default_unit);

    match value {
        Value::Number(number) => {
            let input = format!("{}{}", number, default_unit_suffix(default_unit));
            Length::from_string(&input, None).ok()
        }
        Value::String(text) => Length::from_string(text, Some(default_unit)).ok(),
        _ => None,
    }
}

fn parse_feed_value(value: &Value, default_unit: &str) -> Option<FeedRate> {
    let default_unit = feed_rate_unit_from_str(default_unit);

    match value {
        Value::Number(number) => {
            let input = format!("{}{}", number, default_feed_unit_suffix(default_unit));
            FeedRate::from_string(&input, None).ok()
        }
        Value::String(text) => FeedRate::from_string(text, Some(default_unit)).ok(),
        _ => None,
    }
}

fn parse_angle_value(value: &Value) -> Option<f64> {
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => Angle::from_string(text, Some(AngleUnit::Degree))
            .ok()
            .map(|angle| angle.as_degrees()),
        _ => None,
    }
}

fn normalize_length_value(value: &Value, default_unit: &str) -> Option<String> {
    parse_length_value(value, default_unit).map(|length| length.to_string())
}

fn normalize_feed_value(value: &Value, default_unit: &str) -> Option<String> {
    parse_feed_value(value, default_unit).map(|feed| feed.to_string())
}

fn format_length_with_unit(value: f64, default_unit: &str) -> String {
    normalize_length_value(&Value::from(value), default_unit)
        .unwrap_or_else(|| format!("{value}{default_unit}"))
}

fn default_unit_suffix(unit: LengthUnit) -> &'static str {
    match unit {
        LengthUnit::In | LengthUnit::Inch => "in",
        _ => "mm",
    }
}

fn linear_unit_from_str(unit: &str) -> LengthUnit {
    match unit {
        "in" => LengthUnit::In,
        _ => LengthUnit::Mm,
    }
}

fn feed_rate_unit_from_str(unit: &str) -> FeedRateUnit {
    match unit {
        "ipm" => FeedRateUnit::Ipm,
        _ => FeedRateUnit::MmPerMin,
    }
}

fn default_feed_unit_suffix(unit: FeedRateUnit) -> &'static str {
    match unit {
        FeedRateUnit::Ipm | FeedRateUnit::InPerMin | FeedRateUnit::InchPerMin => "ipm",
        FeedRateUnit::CmPerMin => "cm/min",
        FeedRateUnit::MPerMin => "m/min",
        FeedRateUnit::MmPerMin => "mm/min",
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

fn _parse_rpm_value(value: &Value) -> Option<RotationalSpeed> {
    match value {
        Value::Number(number) => number.as_f64().map(RotationalSpeed::from_rpm),
        Value::String(text) => RotationalSpeed::from_string(text, Some(RotationalSpeedUnit::Rpm)).ok(),
        _ => None,
    }
}
