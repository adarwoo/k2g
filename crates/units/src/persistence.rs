//! Part 3 — load and save (serde).
//!
//! This module defines how the typed quantities from [`crate::types`] are
//! serialized to and deserialized from configuration files. The application
//! persists everything as YAML, but these implementations are format-agnostic
//! (they work equally with `serde_json`, used by the tests).
//!
//! # Relationship to `schemas/units.yaml`
//!
//! The YAML schema validates the *textual* form of each field before it ever
//! reaches this code; these implementations then turn that text (or a bare
//! number) into a typed value. The mapping is:
//!
//! | schema `$def` | Rust type          | accepted forms (per the schema pattern)      |
//! |---------------|--------------------|----------------------------------------------|
//! | `size`        | [`Length`]         | `10mm`, `250 um`, `0.125 in`, `1/8"`, `1 1/2 in`, `10 thou` |
//! | `feed`        | [`FeedRate`]       | `1200 mm/min`, `50 in/min`, `50 ipm`         |
//! | `angle`       | [`Angle`]          | `90`, `-45`, `118 deg` (bare number ⇒ degrees) |
//! | `rpm`         | [`RotationalSpeed`]| `12000`, `12000 rpm` (bare number ⇒ rpm)     |
//!
//! # Serialization strategy
//!
//! Values always serialize to their **canonical source string** (via
//! [`std::fmt::Display`]), so a fraction such as `9/8in` round-trips losslessly
//! instead of collapsing to a decimal. Deserialization is permissive: a JSON/
//! YAML **string** is parsed through the quantity's `from_string`, while a raw
//! **number** is accepted directly and assigned that quantity's canonical unit
//! (mm, mm/min, degrees, or rpm) — matching the bare-number rules in the schema.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::types::{Angle, FeedRate, Length, RotationalSpeed, ScalarValue};

/// Serializes a quantity to its canonical source string via `Display`.
macro_rules! serialize_via_display {
    ($ty:ty) => {
        impl Serialize for $ty {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.to_string())
            }
        }
    };
}

serialize_via_display!(Length);
serialize_via_display!(FeedRate);
serialize_via_display!(Angle);
serialize_via_display!(RotationalSpeed);

/// Generates a `Deserialize` impl that accepts either a numeric value (assigned
/// the quantity's canonical unit) or a source string parsed via `from_string`.
///
/// * `$ty`       — the quantity type.
/// * `$visitor`  — a fresh name for the serde visitor struct.
/// * `$expecting`— the human-readable "expecting" message.
macro_rules! deserialize_number_or_string {
    ($ty:ty, $visitor:ident, $expecting:expr) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                struct $visitor;

                impl<'de> Visitor<'de> for $visitor {
                    type Value = $ty;

                    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                        formatter.write_str($expecting)
                    }

                    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        Ok(<$ty>::from_scalar(ScalarValue::Integer(value)))
                    }

                    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        let scalar = match i64::try_from(value) {
                            Ok(integer) => ScalarValue::Integer(integer),
                            Err(_) => ScalarValue::Float(value as f64),
                        };
                        Ok(<$ty>::from_scalar(scalar))
                    }

                    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        Ok(<$ty>::from_scalar(ScalarValue::Float(value)))
                    }

                    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        <$ty>::from_string(value, None).map_err(E::custom)
                    }

                    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
                    where
                        E: de::Error,
                    {
                        self.visit_str(&value)
                    }
                }

                deserializer.deserialize_any($visitor)
            }
        }
    };
}

deserialize_number_or_string!(
    Length,
    LengthVisitor,
    "a length string like '1/3in' or a numeric millimeter value"
);
deserialize_number_or_string!(
    FeedRate,
    FeedRateVisitor,
    "a feed rate string like '96ipm' or a numeric mm/min value"
);
deserialize_number_or_string!(
    Angle,
    AngleVisitor,
    "an angle string like '130deg' or a numeric degree value"
);
deserialize_number_or_string!(
    RotationalSpeed,
    RotationalSpeedVisitor,
    "a spindle speed string like '8000rpm' or a numeric rpm value"
);

#[cfg(test)]
mod tests {
    use crate::types::{FeedRate, Length};

    #[test]
    fn serde_round_trips_native_length_string() {
        let value: Length = serde_json::from_str("\"1 1/8\\\"\"")
            .expect("serde should parse native imperial fractions");
        assert_eq!(value.to_string(), "9/8in");

        let encoded = serde_json::to_string(&value).expect("serde should serialize length");
        assert_eq!(encoded, "\"9/8in\"");
    }

    #[test]
    fn serde_round_trips_native_feedrate_string() {
        let value: FeedRate = serde_json::from_str("\"69/8cm/min\"")
            .expect("serde should parse native feedrate fractions");
        assert_eq!(value.to_string(), "69/8cm/min");
    }

    #[test]
    fn serde_accepts_bare_number_as_canonical_unit() {
        // Bare number ⇒ millimetres for a length, matching the schema's rules.
        let value: Length =
            serde_json::from_str("12").expect("serde should accept a bare integer");
        assert_eq!(value.to_string(), "12mm");
    }
}
