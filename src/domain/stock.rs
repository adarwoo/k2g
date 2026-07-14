use crate::units::{Angle, FeedRate, Length, RotationalSpeed};
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
    /// Display name resolved from base name and optional user nickname.
    pub fn display_name(&self) -> String {
        let composite = self.composite_name.trim();
        let nickname = self.name.trim();

        if nickname.is_empty() {
            composite.to_string()
        } else {
            format!("{} - {}", composite, nickname)
        }
    }
}

/// stock.yaml <-> Tool conversion boundary.
pub fn stock_value_from_tools(tools: &[Tool]) -> Value {
    let tool_values = tools
        .iter()
        .enumerate()
        .map(|(index, tool)| {
            json!({
                "id": tool.id,
                "summary": tool.display_name(),
                "availability": tool_status_to_key(tool.status),
                "preference": tool_preference_to_key(tool.preference),
                "order": index,
                "ref": {
                    "catalog": tool.source_catalog,
                    "tool_id": tool.id,
                    "section": Value::Null,
                    "sku": tool.sku,
                },
                "base": {
                    "name": tool.composite_name,
                    "kind": ToolKind::from_kind_label(&tool.kind).as_storage_key(),
                    "manufacturer": tool.manufacturer,
                    "sku": tool.sku,
                    "diameter": tool.diameter,
                    "point_angle": tool.point_angle,
                    "spindle": tool.spindle_speed,
                    "z_feed": tool.feed_rate,
                    "table_feed": tool.feed_rate,
                },
                "overrides": {
                    "name": if tool.name.trim().is_empty() { Value::Null } else { Value::String(tool.name.clone()) },
                }
            })
        })
        .collect::<Vec<_>>();

    json!({ "tools": tool_values })
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

            let base = item.get("base").unwrap_or(item);
            let overrides = item.get("overrides").unwrap_or(&Value::Null);

            let composite_name = base
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| item.get("summary").and_then(Value::as_str))
                .unwrap_or("Tool")
                .to_string();

            let name = overrides
                .get("name")
                .and_then(Value::as_str)
                .or_else(|| item.get("name").and_then(Value::as_str))
                .unwrap_or("")
                .to_string();

            let kind = base
                .get("kind")
                .and_then(Value::as_str)
                .map(|kind| ToolKind::from_storage_key(kind).stock_label().to_string())
                .unwrap_or_else(|| ToolKind::Endmill.stock_label().to_string());

            let diameter = base
                .get("diameter")
                .and_then(value_to_length)
                .or_else(|| item.get("diameter").and_then(value_to_length))
                .unwrap_or_else(|| Length::from_mm(1.0));

            let point_angle = base
                .get("point_angle")
                .and_then(value_to_angle)
                .or_else(|| item.get("point_angle").and_then(value_to_angle))
                .unwrap_or_else(|| Angle::from_degrees(180.0));

            let feed_rate = base
                .get("table_feed")
                .and_then(value_to_feed)
                .or_else(|| base.get("z_feed").and_then(value_to_feed))
                .or_else(|| item.get("feed_rate").and_then(value_to_feed));

            let spindle_speed = base
                .get("spindle")
                .and_then(value_to_rpm_speed)
                .or_else(|| item.get("spindle_speed").and_then(value_to_rpm_speed));

            let source_catalog = item
                .pointer("/ref/catalog")
                .and_then(Value::as_str)
                .or_else(|| item.get("source_catalog").and_then(Value::as_str))
                .unwrap_or("Manual")
                .to_string();

            let manufacturer = base
                .get("manufacturer")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| {
                    item.get("manufacturer")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                });

            let sku = base
                .get("sku")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .or_else(|| item.get("sku").and_then(Value::as_str).map(ToString::to_string));

            Some(Tool {
                id,
                composite_name,
                name,
                kind,
                diameter,
                catalog_diameter: Some(diameter),
                point_angle,
                catalog_point_angle: Some(point_angle),
                feed_rate,
                catalog_feed_rate: feed_rate,
                spindle_speed,
                catalog_spindle_speed: spindle_speed,
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
