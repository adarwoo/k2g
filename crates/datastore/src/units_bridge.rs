//! Bridge from `units.yaml` `$def` names to the `units` crate types.
//!
//! A schema marks a unit-bearing field with `$ref: "units.yaml#/$defs/<name>"`.
//! [`UnitKind`] enumerates those `<name>`s. Four of them map to a concrete
//! `units` type and are decoded into a [`crate::model::UnitValue`]; the rest
//! (`feed_rev`, `feed_tooth`, `speed`, `percent*`) have no `units` type yet, so
//! the parser keeps their raw scalar form and records the kind in metadata only.

use serde_json::Value;
use units::{Angle, FeedRate, Length, RotationalSpeed};

use crate::model::UnitValue;

/// A unit category declared in `units.yaml`'s `$defs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitKind {
    /// Linear dimension — decodes to [`units::Length`].
    Size,
    /// Feed rate (distance/min) — decodes to [`units::FeedRate`].
    Feed,
    /// Feed per revolution — no `units` type; kept as its raw string.
    FeedRev,
    /// Feed per tooth — no `units` type; kept as its raw string.
    FeedTooth,
    /// Spindle speed — decodes to [`units::RotationalSpeed`].
    Rpm,
    /// Angle — decodes to [`units::Angle`].
    Angle,
    /// Percentage — no `units` type; kept as a number.
    Percent,
    /// Surface/cutting speed — no `units` type; kept as its raw string.
    Speed,
}

impl UnitKind {
    /// Maps a `units.yaml#/$defs/<name>` fragment to a [`UnitKind`].
    pub fn from_def_name(name: &str) -> Option<Self> {
        Some(match name {
            "size" => Self::Size,
            "feed" => Self::Feed,
            "feed_rev" => Self::FeedRev,
            "feed_tooth" => Self::FeedTooth,
            "rpm" => Self::Rpm,
            "angle" => Self::Angle,
            "percent" | "percent_0_100" | "percent_50_100" => Self::Percent,
            "speed" => Self::Speed,
            _ => return None,
        })
    }

    /// Whether this kind decodes into a typed [`UnitValue`] (vs. staying a raw
    /// scalar in the model).
    pub fn is_typed(self) -> bool {
        matches!(self, Self::Size | Self::Feed | Self::Rpm | Self::Angle)
    }
}

/// Decodes a JSON scalar into a typed [`UnitValue`] for the four kinds backed by
/// the `units` crate. Returns `Ok(None)` for kinds with no `units` type (the
/// caller keeps the raw scalar), and `Err` when a typed decode fails.
pub fn decode_unit(kind: UnitKind, value: &Value) -> Result<Option<UnitValue>, String> {
    if !kind.is_typed() {
        return Ok(None);
    }

    let text = scalar_to_string(value)
        .ok_or_else(|| format!("expected a string or number, got {}", type_name(value)))?;

    let decoded = match kind {
        UnitKind::Size => UnitValue::Length(Length::from_string(&text, None).map_err(|e| e.to_string())?),
        UnitKind::Feed => UnitValue::Feed(FeedRate::from_string(&text, None).map_err(|e| e.to_string())?),
        UnitKind::Rpm => {
            UnitValue::Rpm(RotationalSpeed::from_string(&text, None).map_err(|e| e.to_string())?)
        }
        UnitKind::Angle => UnitValue::Angle(Angle::from_string(&text, None).map_err(|e| e.to_string())?),
        _ => unreachable!("guarded by is_typed"),
    };

    Ok(Some(decoded))
}

/// Renders a JSON scalar as the string the `units` parsers expect. Numbers are
/// stringified (so a bare `12000` becomes `"12000"`).
fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
