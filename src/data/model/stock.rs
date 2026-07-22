use units::{Angle, FeedRate, Length, RotationalSpeed};
use serde_json::{json, Value};

use super::tool_core::ToolKind;

/// Stock availability status from stock schema.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    InStock,
    OutOfStock,
}

impl ToolStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::InStock => "In stock",
            Self::OutOfStock => "Out of stock",
        }
    }

    pub fn class_name(self) -> &'static str {
        match self {
            Self::InStock => "status-in-stock",
            Self::OutOfStock => "status-out-of-stock",
        }
    }
}

/// Auto-selection preference from stock schema.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ToolPreference {
    Preferred,
    Neutral,
    NotPreferred,
}

impl ToolPreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::Preferred => "Preferred",
            Self::Neutral => "Neutral",
            Self::NotPreferred => "Not preferred",
        }
    }

    pub fn class_name(self) -> &'static str {
        match self {
            Self::Preferred => "status-preferred",
            Self::Neutral => "status-neutral",
            Self::NotPreferred => "status-not-preferred",
        }
    }
}

/// In-memory stock tool (stock.yaml entity mapped for runtime use).
#[allow(dead_code)]
#[derive(Clone)]
pub struct Tool {
    pub id: String,
    pub composite_name: String,
    pub name: String,
    pub kind: String,
    pub diameter: Length,
    pub catalog_diameter: Option<Length>,
    pub point_angle: Angle,
    pub catalog_point_angle: Option<Angle>,
    /// Usable cutting length of the bit (`None` when unspecified). Used by the
    /// tool-selection Z-feasibility check to confirm the bit can reach through
    /// the board; the lossy legacy projection historically dropped it.
    pub flute_length: Option<Length>,
    pub feed_rate: Option<FeedRate>,
    pub catalog_feed_rate: Option<FeedRate>,
    pub spindle_speed: Option<RotationalSpeed>,
    pub catalog_spindle_speed: Option<RotationalSpeed>,
    pub status: ToolStatus,
    pub preference: ToolPreference,
    pub source_catalog: String,
    pub manufacturer: Option<String>,
    pub sku: Option<String>,
}

impl Tool {
    /// The tool's effective display name (the current `name`, falling back to the
    /// original catalog name when unset).
    pub fn display_name(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            self.composite_name.trim().to_string()
        } else {
            name.to_string()
        }
    }
}

/// Builds a `tool_values` object (used for both `base` and `overrides`). Optional
/// unit fields are *omitted* when absent rather than emitted as `null`: both are
/// schema-valid, but the datastore's unit decoder reports a spurious "expected a
/// string or number, got null" on a null unit; an absent key reads back identically
/// (see `value_to_feed`).
fn tool_values_map(
    name: &str,
    kind: &str,
    diameter: Length,
    point_angle: Angle,
    manufacturer: Option<&String>,
    sku: Option<&String>,
    spindle: Option<RotationalSpeed>,
    feed: Option<FeedRate>,
    flute_length: Option<Length>,
) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("name".into(), Value::String(name.to_string()));
    m.insert(
        "kind".into(),
        Value::String(ToolKind::from_kind_label(kind).as_storage_key().to_string()),
    );
    m.insert("diameter".into(), json!(diameter));
    m.insert("point_angle".into(), json!(point_angle));
    if let Some(manufacturer) = manufacturer {
        m.insert("manufacturer".into(), Value::String(manufacturer.clone()));
    }
    if let Some(sku) = sku {
        m.insert("sku".into(), Value::String(sku.clone()));
    }
    if let Some(spindle) = spindle {
        m.insert("spindle".into(), json!(spindle));
    }
    if let Some(feed) = feed {
        m.insert("z_feed".into(), json!(feed));
        m.insert("table_feed".into(), json!(feed));
    }
    if let Some(flute_length) = flute_length {
        m.insert("flute_length".into(), json!(flute_length));
    }
    m
}

/// stock.yaml <-> Tool conversion boundary.
///
/// `base` is the immutable original snapshot (the catalog-derived values captured
/// when the tool was added). `overrides` holds the current, user-editable values.
/// The effective value of a field is its override, falling back to base; a field
/// is "changed" when its override differs from base, and rolling it back reverts
/// the override to the base value. The `catalog_*` members of [`Tool`] carry the
/// base (original) values so the UI can show and revert changes.
pub fn stock_value_from_tools(tools: &[Tool]) -> Value {
    let tool_values = tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            let base = tool_values_map(
                &tool.composite_name,
                &tool.kind,
                tool.catalog_diameter.unwrap_or(tool.diameter),
                tool.catalog_point_angle.unwrap_or(tool.point_angle),
                tool.manufacturer.as_ref(),
                tool.sku.as_ref(),
                tool.catalog_spindle_speed.or(tool.spindle_speed),
                tool.catalog_feed_rate.or(tool.feed_rate),
                tool.flute_length,
            );
            let effective_name = if tool.name.trim().is_empty() {
                tool.composite_name.clone()
            } else {
                tool.name.clone()
            };
            let overrides = tool_values_map(
                &effective_name,
                &tool.kind,
                tool.diameter,
                tool.point_angle,
                tool.manufacturer.as_ref(),
                tool.sku.as_ref(),
                tool.spindle_speed,
                tool.feed_rate,
                tool.flute_length,
            );

            json!({
                "id": tool.id,
                "summary": tool.display_name(),
                "availability": tool_status_to_key(tool.status),
                "preference": tool_preference_to_key(tool.preference),
                "order": index,
                // Catalog tools have no maintained id, so only the catalog *name*
                // is retained (for display), not an id-based reference.
                "source_catalog": tool.source_catalog,
                "base": Value::Object(base),
                "overrides": Value::Object(overrides)
            })
        })
        .collect::<Vec<_>>();

    // `schema_version` is mandatory: stock.yaml declares `x-schema-version: 1`, so
    // the datastore rejects any document lacking a matching `schema_version` during
    // version gating. Emitting it here keeps this projection re-parseable by the
    // AppData writer (the sole writer of stock.yaml).
    json!({ "schema_version": 1, "tools": tool_values })
}

/// Merges `overrides` over `base` (both `tool_values` objects), producing the
/// effective values. A key present in `overrides` wins; otherwise `base` supplies
/// it. Non-object inputs are treated as empty.
fn overlay(base: &Value, overrides: &Value) -> Value {
    let mut merged = base.as_object().cloned().unwrap_or_default();
    if let Some(over) = overrides.as_object() {
        for (key, value) in over {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

/// stock.yaml <-> Tool conversion boundary.
pub fn tools_from_stock_value(stock: &Value) -> Vec<Tool> {
    let Some(items) = stock.get("tools").and_then(Value::as_array) else {
        return Vec::new();
    };

    items
        .iter()
        .enumerate()
        .filter_map(|(idx, item)| {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("tool-{}", idx + 1));

            // `base` is the immutable original; `effective` is base with overrides
            // applied. Current (editable) values come from `effective`; the
            // `catalog_*` originals come from `base` — a field is "changed" when the
            // two differ. An old-format tool (no `base`) falls back to the item body.
            let base = item.get("base").cloned().unwrap_or_else(|| item.clone());
            let overrides = item.get("overrides").cloned().unwrap_or(Value::Null);
            let effective = overlay(&base, &overrides);

            let composite_name = base
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| item.get("summary").and_then(Value::as_str))
                .unwrap_or("Tool")
                .to_string();

            let name = effective
                .get("name")
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(ToString::to_string)
                .unwrap_or_else(|| composite_name.clone());

            let kind = effective
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| ToolKind::from_storage_key(kind).stock_label().to_string())
                .unwrap_or_else(|| ToolKind::Endmill.stock_label().to_string());

            let catalog_diameter = base.get("diameter").and_then(value_to_length);
            let diameter = effective
                .get("diameter")
                .and_then(value_to_length)
                .or(catalog_diameter)
                .unwrap_or_else(|| Length::from_mm(1.0));

            let catalog_point_angle = base.get("point_angle").and_then(value_to_angle);
            let point_angle = effective
                .get("point_angle")
                .and_then(value_to_angle)
                .or(catalog_point_angle)
                .unwrap_or_else(|| Angle::from_degrees(180.0));

            let feed_of = |values: &Value| {
                values
                    .get("table_feed")
                    .and_then(value_to_feed)
                    .or_else(|| values.get("z_feed").and_then(value_to_feed))
            };
            let catalog_feed_rate = feed_of(&base);
            let feed_rate = feed_of(&effective).or(catalog_feed_rate);

            let catalog_spindle_speed = base.get("spindle").and_then(value_to_rpm_speed);
            let spindle_speed = effective
                .get("spindle")
                .and_then(value_to_rpm_speed)
                .or(catalog_spindle_speed);

            let flute_length = effective.get("flute_length").and_then(value_to_length);

            let source_catalog = item
                .get("source_catalog")
                .and_then(Value::as_str)
                // Fallback for any not-yet-migrated value still carrying `ref`.
                .or_else(|| item.pointer("/ref/catalog").and_then(Value::as_str))
                .unwrap_or("Manual")
                .to_string();

            let manufacturer = effective
                .get("manufacturer")
                .and_then(Value::as_str)
                .map(ToString::to_string);

            let sku = effective
                .get("sku")
                .and_then(Value::as_str)
                .map(ToString::to_string);

            Some(Tool {
                id,
                composite_name,
                name,
                kind,
                diameter,
                catalog_diameter,
                point_angle,
                catalog_point_angle,
                flute_length,
                feed_rate,
                catalog_feed_rate,
                spindle_speed,
                catalog_spindle_speed,
                status: item
                    .get("availability")
                    .and_then(Value::as_str)
                    .map(tool_status_from_key)
                    .unwrap_or(ToolStatus::InStock),
                preference: item
                    .get("preference")
                    .and_then(Value::as_str)
                    .map(tool_preference_from_key)
                    .unwrap_or(ToolPreference::Neutral),
                source_catalog,
                manufacturer,
                sku,
            })
        })
        .collect()
}

fn tool_status_to_key(status: ToolStatus) -> &'static str {
    match status {
        ToolStatus::InStock => "in_stock",
        ToolStatus::OutOfStock => "out_of_stock",
    }
}

fn tool_status_from_key(value: &str) -> ToolStatus {
    match value {
        "out_of_stock" => ToolStatus::OutOfStock,
        _ => ToolStatus::InStock,
    }
}

fn tool_preference_to_key(preference: ToolPreference) -> &'static str {
    match preference {
        ToolPreference::Preferred => "preferred",
        ToolPreference::Neutral => "neutral",
        ToolPreference::NotPreferred => "not_preferred",
    }
}

fn tool_preference_from_key(value: &str) -> ToolPreference {
    match value {
        "preferred" => ToolPreference::Preferred,
        "not_preferred" => ToolPreference::NotPreferred,
        _ => ToolPreference::Neutral,
    }
}

fn value_to_length(value: &Value) -> Option<Length> {
    match value {
        Value::String(v) => Length::from_string(v, None).ok(),
        Value::Number(v) => v.as_f64().map(Length::from_mm),
        _ => None,
    }
}

fn value_to_feed(value: &Value) -> Option<FeedRate> {
    match value {
        Value::String(v) => FeedRate::from_string(v, None).ok(),
        Value::Number(v) => v.as_f64().map(FeedRate::from_mm_per_min),
        _ => None,
    }
}

fn value_to_rpm_speed(value: &Value) -> Option<RotationalSpeed> {
    match value {
        Value::String(v) => RotationalSpeed::from_string(v, None).ok(),
        Value::Number(v) => v.as_f64().map(RotationalSpeed::from_rpm),
        _ => None,
    }
}

fn value_to_angle(value: &Value) -> Option<Angle> {
    match value {
        Value::String(v) => Angle::from_string(v, None).ok(),
        Value::Number(v) => v.as_f64().map(Angle::from_degrees),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stock tool whose diameter and feed have been edited away from their
    /// original catalog values (the `catalog_*` fields), everything else unchanged.
    fn tool(current_diameter_mm: f64, original_diameter_mm: f64) -> Tool {
        Tool {
            id: "01903f1a-0000-7000-8000-000000000000".to_string(),
            composite_name: "Drill 2mm".to_string(),
            name: "Drill 2mm".to_string(),
            kind: "Drill bit".to_string(),
            diameter: Length::from_mm(current_diameter_mm),
            catalog_diameter: Some(Length::from_mm(original_diameter_mm)),
            point_angle: Angle::from_degrees(118.0),
            catalog_point_angle: Some(Angle::from_degrees(118.0)),
            flute_length: None,
            feed_rate: Some(FeedRate::from_mm_per_min(300.0)),
            catalog_feed_rate: Some(FeedRate::from_mm_per_min(200.0)),
            spindle_speed: Some(RotationalSpeed::from_rpm(12000.0)),
            catalog_spindle_speed: Some(RotationalSpeed::from_rpm(12000.0)),
            status: ToolStatus::InStock,
            preference: ToolPreference::Neutral,
            source_catalog: "Kyocera".to_string(),
            manufacturer: None,
            sku: None,
        }
    }

    #[test]
    fn overlay_prefers_overrides_then_base() {
        let base = json!({ "diameter": "2mm", "point_angle": "118deg" });
        let overrides = json!({ "diameter": "3mm" });
        let merged = overlay(&base, &overrides);
        assert_eq!(merged.get("diameter").and_then(Value::as_str), Some("3mm")); // override wins
        assert_eq!(merged.get("point_angle").and_then(Value::as_str), Some("118deg")); // base fallback
    }

    #[test]
    fn round_trip_keeps_original_in_base_and_edit_in_overrides() {
        let value = stock_value_from_tools(&[tool(3.0, 2.0)]);

        // base holds the original; overrides hold the current edit.
        let base_dia = value_to_length(&value["tools"][0]["base"]["diameter"]).unwrap();
        let over_dia = value_to_length(&value["tools"][0]["overrides"]["diameter"]).unwrap();
        assert!((base_dia.as_mm() - 2.0).abs() < 1e-6, "base = original");
        assert!((over_dia.as_mm() - 3.0).abs() < 1e-6, "override = edit");
        // The catalog reference object is gone; only the catalog name remains.
        assert!(value["tools"][0].get("ref").is_none());
        assert_eq!(value["tools"][0]["source_catalog"].as_str(), Some("Kyocera"));

        let t = &tools_from_stock_value(&value)[0];
        assert!((t.diameter.as_mm() - 3.0).abs() < 1e-6, "effective diameter = edit");
        assert!((t.catalog_diameter.unwrap().as_mm() - 2.0).abs() < 1e-6, "catalog_* = original");
    }

    #[test]
    fn unchanged_tool_reads_current_equal_to_original() {
        let value = stock_value_from_tools(&[tool(2.0, 2.0)]);
        let t = &tools_from_stock_value(&value)[0];
        assert!((t.diameter.as_mm() - t.catalog_diameter.unwrap().as_mm()).abs() < 1e-6);
    }
}
