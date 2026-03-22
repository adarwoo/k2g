//! Strongly-typed unit primitives used by the template context.
//!
//! This module supports scalar inputs from YAML/GUI strings, including fractions
//! like `4/3mm` or `69/8 cm/min`, while preserving the original scalar form.
#![allow(dead_code)]

use std::fmt;

use rhai::{Dynamic, Map};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const EPS: f64 = 1e-12;

/// Errors raised while parsing scalar/unit strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitParseError {
    InvalidNumberFormat,
    InvalidNumerator,
    InvalidDenominator,
    InvalidUnit,
}

impl fmt::Display for UnitParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumberFormat => write!(f, "invalid number format"),
            Self::InvalidNumerator => write!(f, "invalid numerator"),
            Self::InvalidDenominator => write!(f, "invalid denominator"),
            Self::InvalidUnit => write!(f, "invalid unit"),
        }
    }
}

impl std::error::Error for UnitParseError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserUnitSystem {
    Metric,
    Imperial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDisplay {
    pub user: String,
    pub native: Option<String>,
}

pub trait UserUnitDisplay {
    fn unit_display(&self, user_unit_system: UserUnitSystem) -> UnitDisplay;
    fn user_value(&self, user_unit_system: UserUnitSystem) -> f64;
}

/// Scalar representation preserving whether input was integer, float, or fraction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScalarValue {
    Integer(i64),
    Float(f64),
    Fraction { numerator: f64, denominator: i64 },
}

impl ScalarValue {
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Integer(value) => value as f64,
            Self::Float(value) => value,
            Self::Fraction {
                numerator,
                denominator,
            } => numerator as f64 / denominator as f64,
        }
    }
}

impl fmt::Display for ScalarValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Integer(value) => write!(f, "{value}"),
            Self::Float(value) => write!(f, "{}", trim_float(value)),
            Self::Fraction {
                numerator,
                denominator,
            } => write!(f, "{}/{denominator}", trim_float(numerator)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnit {
    Nm,
    Um,
    Mm,
    Cm,
    Mil,
    Thou,
    Inch,
    In,
}

impl LengthUnit {
    fn name(self) -> &'static str {
        match self {
            Self::Nm => "nm",
            Self::Um => "um",
            Self::Mm => "mm",
            Self::Cm => "cm",
            Self::Mil => "mil",
            Self::Thou => "thou",
            Self::Inch => "inch",
            Self::In => "in",
        }
    }

    fn factor_to_nm(self) -> f64 {
        match self {
            Self::Nm => 1.0,
            Self::Um => 1_000.0,
            Self::Mm => 1_000_000.0,
            Self::Cm => 10_000_000.0,
            Self::Mil | Self::Thou => 25_400.0,
            Self::Inch | Self::In => 25_400_000.0,
        }
    }

    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "nm" => Some(Self::Nm),
            "um" => Some(Self::Um),
            "mm" => Some(Self::Mm),
            "cm" => Some(Self::Cm),
            "mil" => Some(Self::Mil),
            "thou" => Some(Self::Thou),
            "inch" => Some(Self::Inch),
            "in" => Some(Self::In),
            "\"" => Some(Self::In),
            _ => None,
        }
    }
}

/// Length quantity preserving input scalar form and source unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Length {
    scalar: ScalarValue,
    unit: LengthUnit,
}

impl Length {
    pub const fn from_nm(nm: i64) -> Self {
        Self {
            scalar: ScalarValue::Integer(nm),
            unit: LengthUnit::Nm,
        }
    }

    pub fn from_um(um: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(um),
            unit: LengthUnit::Um,
        }
    }

    pub fn from_mm(mm: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(mm),
            unit: LengthUnit::Mm,
        }
    }

    pub fn from_cm(cm: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(cm),
            unit: LengthUnit::Cm,
        }
    }

    pub fn from_mil(mil: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(mil),
            unit: LengthUnit::Mil,
        }
    }

    pub fn from_inch(inch: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(inch),
            unit: LengthUnit::Inch,
        }
    }

    pub fn from_kicad(value: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(value),
            unit: LengthUnit::Mm,
        }
    }

    pub fn from_string(input: &str, default_unit: Option<LengthUnit>) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => LengthUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(LengthUnit::Mm),
        };

        Ok(Self { scalar, unit })
    }

    pub const fn scalar(self) -> ScalarValue {
        self.scalar
    }

    pub const fn unit(self) -> LengthUnit {
        self.unit
    }

    pub fn as_nm(self) -> f64 {
        self.scalar.as_f64() * self.unit.factor_to_nm()
    }

    pub fn as_um(self) -> f64 {
        self.as_nm() / LengthUnit::Um.factor_to_nm()
    }

    pub fn as_mm(self) -> f64 {
        self.as_nm() / LengthUnit::Mm.factor_to_nm()
    }

    pub fn as_cm(self) -> f64 {
        self.as_nm() / LengthUnit::Cm.factor_to_nm()
    }

    pub fn as_mil(self) -> f64 {
        self.as_nm() / LengthUnit::Mil.factor_to_nm()
    }

    pub fn as_inch(self) -> f64 {
        self.as_nm() / LengthUnit::Inch.factor_to_nm()
    }

    pub fn to_rhai_map(self) -> Map {
        let mut map = Map::new();
        insert_number(&mut map, "nm", self.as_nm());
        insert_number(&mut map, "um", self.as_um());
        insert_number(&mut map, "mm", self.as_mm());
        insert_number(&mut map, "cm", self.as_cm());
        insert_number(&mut map, "mil", self.as_mil());
        insert_number(&mut map, "inches", self.as_inch());
        insert_number(&mut map, "inch", self.as_inch());
        map
    }
}

impl UserUnitDisplay for Length {
    fn unit_display(&self, user_unit_system: UserUnitSystem) -> UnitDisplay {
        let user = match user_unit_system {
            UserUnitSystem::Metric => {
                let value = round_to_step(self.as_mm(), 0.001);
                format_with_unit(value, "mm", 3)
            }
            UserUnitSystem::Imperial => {
                let mm = round_to_step(self.as_mm(), 0.001);
                let inch = round_to_step(mm / 25.4, 0.0001);
                format_with_unit(inch, "in", 4)
            }
        };

        let native = match (user_unit_system, self.unit) {
            (UserUnitSystem::Metric, LengthUnit::Mm) => None,
            (UserUnitSystem::Imperial, LengthUnit::In | LengthUnit::Inch) => None,
            _ => Some(format_native_length(*self)),
        };

        UnitDisplay { user, native }
    }

    fn user_value(&self, user_unit_system: UserUnitSystem) -> f64 {
        match user_unit_system {
            UserUnitSystem::Metric => round_to_step(self.as_mm(), 0.001),
            UserUnitSystem::Imperial => {
                let mm = round_to_step(self.as_mm(), 0.001);
                round_to_step(mm / 25.4, 0.0001)
            }
        }
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

impl Serialize for Length {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Length {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct LengthVisitor;

        impl<'de> Visitor<'de> for LengthVisitor {
            type Value = Length;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a length string like '1/3in' or a numeric millimeter value")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Length {
                    scalar: ScalarValue::Integer(value),
                    unit: LengthUnit::Mm,
                })
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let scalar = match i64::try_from(value) {
                    Ok(integer) => ScalarValue::Integer(integer),
                    Err(_) => ScalarValue::Float(value as f64),
                };

                Ok(Length {
                    scalar,
                    unit: LengthUnit::Mm,
                })
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Length {
                    scalar: ScalarValue::Float(value),
                    unit: LengthUnit::Mm,
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Length::from_string(value, None).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(LengthVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedRateUnit {
    MmPerMin,
    CmPerMin,
    MPerMin,
    InPerMin,
    Ipm,
    InchPerMin,
}

impl FeedRateUnit {
    fn name(self) -> &'static str {
        match self {
            Self::MmPerMin => "mm/min",
            Self::CmPerMin => "cm/min",
            Self::MPerMin => "m/min",
            Self::InPerMin => "in/min",
            Self::Ipm => "ipm",
            Self::InchPerMin => "inch/min",
        }
    }

    fn factor_to_mm_per_min(self) -> f64 {
        match self {
            Self::MmPerMin => 1.0,
            Self::CmPerMin => 10.0,
            Self::MPerMin => 1_000.0,
            Self::InPerMin | Self::Ipm | Self::InchPerMin => 25.4,
        }
    }

    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "mm/min" => Some(Self::MmPerMin),
            "cm/min" => Some(Self::CmPerMin),
            "m/min" => Some(Self::MPerMin),
            "in/min" => Some(Self::InPerMin),
            "ipm" => Some(Self::Ipm),
            "inch/min" => Some(Self::InchPerMin),
            _ => None,
        }
    }
}

/// Feedrate quantity preserving input scalar form and source unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedRate {
    scalar: ScalarValue,
    unit: FeedRateUnit,
}

impl FeedRate {
    pub fn from_mm_per_min(mm_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(mm_per_min),
            unit: FeedRateUnit::MmPerMin,
        }
    }

    pub fn from_in_per_min(in_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(in_per_min),
            unit: FeedRateUnit::InPerMin,
        }
    }

    pub fn from_kicad(value: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(value),
            unit: FeedRateUnit::MmPerMin,
        }
    }

    pub fn from_string(input: &str, default_unit: Option<FeedRateUnit>) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => FeedRateUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(FeedRateUnit::MmPerMin),
        };

        Ok(Self { scalar, unit })
    }

    pub fn as_mm_per_min(self) -> f64 {
        self.scalar.as_f64() * self.unit.factor_to_mm_per_min()
    }

    pub fn as_in_per_min(self) -> f64 {
        self.as_mm_per_min() / FeedRateUnit::InPerMin.factor_to_mm_per_min()
    }

    pub const fn unit(self) -> FeedRateUnit {
        self.unit
    }
}

impl UserUnitDisplay for FeedRate {
    fn unit_display(&self, user_unit_system: UserUnitSystem) -> UnitDisplay {
        let user = match user_unit_system {
            UserUnitSystem::Metric => {
                let value = round_to_step(self.as_mm_per_min(), 0.001);
                format_with_unit(value, "mm/min", 3)
            }
            UserUnitSystem::Imperial => {
                let value = round_to_step(self.as_in_per_min(), 0.0001);
                format_with_unit(value, "ipm", 4)
            }
        };

        let native_matches_user = matches!(
            (user_unit_system, self.unit),
            (UserUnitSystem::Metric, FeedRateUnit::MmPerMin)
                | (UserUnitSystem::Imperial, FeedRateUnit::Ipm | FeedRateUnit::InPerMin | FeedRateUnit::InchPerMin)
        );

        let native = if native_matches_user {
            None
        } else {
            Some(format_native_feed_rate(*self))
        };

        UnitDisplay { user, native }
    }

    fn user_value(&self, user_unit_system: UserUnitSystem) -> f64 {
        match user_unit_system {
            UserUnitSystem::Metric => round_to_step(self.as_mm_per_min(), 0.001),
            UserUnitSystem::Imperial => round_to_step(self.as_in_per_min(), 0.0001),
        }
    }
}

impl fmt::Display for FeedRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

impl Serialize for FeedRate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for FeedRate {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct FeedRateVisitor;

        impl<'de> Visitor<'de> for FeedRateVisitor {
            type Value = FeedRate;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a feed rate string like '96ipm' or a numeric mm/min value")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(FeedRate {
                    scalar: ScalarValue::Integer(value),
                    unit: FeedRateUnit::MmPerMin,
                })
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let scalar = match i64::try_from(value) {
                    Ok(integer) => ScalarValue::Integer(integer),
                    Err(_) => ScalarValue::Float(value as f64),
                };

                Ok(FeedRate {
                    scalar,
                    unit: FeedRateUnit::MmPerMin,
                })
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(FeedRate {
                    scalar: ScalarValue::Float(value),
                    unit: FeedRateUnit::MmPerMin,
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                FeedRate::from_string(value, None).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(FeedRateVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleUnit {
    Deg,
    Degree,
}

impl AngleUnit {
    fn name(self) -> &'static str {
        match self {
            Self::Deg => "deg",
            Self::Degree => "degree",
        }
    }

    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "deg" => Some(Self::Deg),
            "degree" => Some(Self::Degree),
            _ => None,
        }
    }
}

/// Angle quantity in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Angle {
    scalar: ScalarValue,
    unit: AngleUnit,
}

impl Angle {
    pub fn from_degrees(degrees: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(degrees),
            unit: AngleUnit::Degree,
        }
    }

    pub fn from_radians(radians: f64) -> Self {
        Self::from_degrees(radians.to_degrees())
    }

    pub fn from_kicad(value: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(value),
            unit: AngleUnit::Degree,
        }
    }

    pub fn from_string(input: &str, default_unit: Option<AngleUnit>) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => AngleUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(AngleUnit::Degree),
        };

        Ok(Self { scalar, unit })
    }

    pub fn as_degrees(self) -> f64 {
        self.scalar.as_f64()
    }

    pub fn as_radians(self) -> f64 {
        self.as_degrees().to_radians()
    }
}

impl UserUnitDisplay for Angle {
    fn unit_display(&self, _user_unit_system: UserUnitSystem) -> UnitDisplay {
        let value = round_to_step(self.as_degrees(), 0.01);
        UnitDisplay {
            user: format_with_unit(value, "deg", 2),
            native: None,
        }
    }

    fn user_value(&self, _user_unit_system: UserUnitSystem) -> f64 {
        round_to_step(self.as_degrees(), 0.01)
    }
}

impl fmt::Display for Angle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

impl Serialize for Angle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Angle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct AngleVisitor;

        impl<'de> Visitor<'de> for AngleVisitor {
            type Value = Angle;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an angle string like '130deg' or a numeric degree value")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Angle {
                    scalar: ScalarValue::Integer(value),
                    unit: AngleUnit::Degree,
                })
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let scalar = match i64::try_from(value) {
                    Ok(integer) => ScalarValue::Integer(integer),
                    Err(_) => ScalarValue::Float(value as f64),
                };

                Ok(Angle {
                    scalar,
                    unit: AngleUnit::Degree,
                })
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(Angle {
                    scalar: ScalarValue::Float(value),
                    unit: AngleUnit::Degree,
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Angle::from_string(value, None).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(AngleVisitor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationalSpeedUnit {
    Rpm,
}

impl RotationalSpeedUnit {
    fn name(self) -> &'static str {
        "rpm"
    }

    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "rpm" => Some(Self::Rpm),
            _ => None,
        }
    }
}

/// Rotational speed quantity in revolutions-per-minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotationalSpeed {
    scalar: ScalarValue,
    unit: RotationalSpeedUnit,
}

impl RotationalSpeed {
    pub fn from_rpm(rpm: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(rpm),
            unit: RotationalSpeedUnit::Rpm,
        }
    }

    pub fn from_kicad(value: f64) -> Self {
        Self::from_rpm(value)
    }

    pub fn from_string(
        input: &str,
        default_unit: Option<RotationalSpeedUnit>,
    ) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => RotationalSpeedUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(RotationalSpeedUnit::Rpm),
        };

        Ok(Self { scalar, unit })
    }

    pub fn as_rpm(self) -> f64 {
        self.scalar.as_f64()
    }

    pub const fn unit(self) -> RotationalSpeedUnit {
        self.unit
    }
}

impl UserUnitDisplay for RotationalSpeed {
    fn unit_display(&self, _user_unit_system: UserUnitSystem) -> UnitDisplay {
        let value = round_to_step(self.as_rpm(), 1.0);
        UnitDisplay {
            user: format_with_unit(value, "rpm", 0),
            native: None,
        }
    }

    fn user_value(&self, _user_unit_system: UserUnitSystem) -> f64 {
        round_to_step(self.as_rpm(), 1.0)
    }
}

impl fmt::Display for RotationalSpeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

impl Serialize for RotationalSpeed {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for RotationalSpeed {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RotationalSpeedVisitor;

        impl<'de> Visitor<'de> for RotationalSpeedVisitor {
            type Value = RotationalSpeed;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a spindle speed string like '8000rpm' or a numeric rpm value")
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RotationalSpeed {
                    scalar: ScalarValue::Integer(value),
                    unit: RotationalSpeedUnit::Rpm,
                })
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let scalar = match i64::try_from(value) {
                    Ok(integer) => ScalarValue::Integer(integer),
                    Err(_) => ScalarValue::Float(value as f64),
                };

                Ok(RotationalSpeed {
                    scalar,
                    unit: RotationalSpeedUnit::Rpm,
                })
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(RotationalSpeed {
                    scalar: ScalarValue::Float(value),
                    unit: RotationalSpeedUnit::Rpm,
                })
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                RotationalSpeed::from_string(value, None).map_err(E::custom)
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                self.visit_str(&value)
            }
        }

        deserializer.deserialize_any(RotationalSpeedVisitor)
    }
}

fn parse_number_with_optional_unit(input: &str) -> Result<(ScalarValue, Option<&str>), UnitParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(UnitParseError::InvalidNumberFormat);
    }

    // Mixed-fraction support: `<whole> <num>/<den><optional-unit>`
    // Examples: `1 1/8 in`, `1 1/8"`.
    if let Some(first_ws) = trimmed.find(char::is_whitespace) {
        let whole_raw = trimmed[..first_ws].trim();
        let rest_raw = trimmed[first_ws..].trim();

        if !whole_raw.is_empty() && !rest_raw.is_empty() {
            if let Ok(whole) = whole_raw.parse::<i64>() {
                let (fraction_raw, mixed_unit_raw) = split_scalar_and_unit(rest_raw);
                if fraction_raw.contains('/') {
                    let fraction = parse_scalar_token(fraction_raw)?;
                    if let ScalarValue::Fraction {
                        numerator,
                        denominator,
                    } = fraction
                    {
                        let sign = if whole < 0 { -1.0 } else { 1.0 };
                        let whole_abs = whole.unsigned_abs() as f64;
                        let combined_numerator = sign * (whole_abs * denominator as f64 + numerator.abs());

                        let unit = if mixed_unit_raw.is_empty() {
                            None
                        } else {
                            Some(mixed_unit_raw)
                        };

                        return Ok((
                            ScalarValue::Fraction {
                                numerator: combined_numerator,
                                denominator,
                            },
                            unit,
                        ));
                    }
                }
            }
        }
    }

    let (number_raw, unit_raw) = split_scalar_and_unit(trimmed);

    if number_raw.is_empty() {
        return Err(UnitParseError::InvalidNumberFormat);
    }

    let scalar = parse_scalar_token(number_raw)?;

    let unit = if unit_raw.is_empty() {
        None
    } else {
        Some(unit_raw)
    };

    Ok((scalar, unit))
}

fn split_scalar_and_unit(input: &str) -> (&str, &str) {
    let boundary = input
        .char_indices()
        .find(|(_, ch)| !is_scalar_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(input.len());

    (input[..boundary].trim(), input[boundary..].trim())
}

fn is_scalar_char(ch: char) -> bool {
    ch.is_ascii_digit() || ch == '.' || ch == '/' || ch == '+' || ch == '-'
}

fn parse_scalar_token(number_raw: &str) -> Result<ScalarValue, UnitParseError> {
    if let Some((n, d)) = number_raw.split_once('/') {
        let numerator = n
            .trim()
            .parse::<f64>()
            .map_err(|_| UnitParseError::InvalidNumerator)?;
        let denominator = d
            .trim()
            .parse::<i64>()
            .map_err(|_| UnitParseError::InvalidDenominator)?;
        if denominator == 0 {
            return Err(UnitParseError::InvalidDenominator);
        }
        Ok(ScalarValue::Fraction {
            numerator,
            denominator,
        })
    } else if number_raw.contains('.') {
        let value = number_raw
            .parse::<f64>()
            .map_err(|_| UnitParseError::InvalidNumberFormat)?;
        Ok(ScalarValue::Float(value))
    } else {
        let value = number_raw
            .parse::<i64>()
            .map_err(|_| UnitParseError::InvalidNumberFormat)?;
        Ok(ScalarValue::Integer(value))
    }
}

fn round_to_step(value: f64, step: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    (value / step).round() * step
}

fn format_with_unit(value: f64, unit_suffix: &str, max_decimals: usize) -> String {
    let mut text = if max_decimals == 0 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.max_decimals$}")
    };

    if max_decimals > 0 {
        while text.contains('.') && text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }

    format!("{text}{unit_suffix}")
}

fn format_native_length(value: Length) -> String {
    match value.unit {
        LengthUnit::Nm => format_with_unit(round_to_step(value.as_nm(), 1_000.0), "nm", 0),
        LengthUnit::Um => format_with_unit(round_to_step(value.as_um(), 1.0), "um", 0),
        LengthUnit::Mm => format_with_unit(round_to_step(value.as_mm(), 0.001), "mm", 3),
        LengthUnit::Cm => format_with_unit(round_to_step(value.as_cm(), 0.0001), "cm", 4),
        LengthUnit::Mil => format_with_unit(round_to_step(value.as_mil(), 0.1), "mil", 1),
        LengthUnit::Thou => format_with_unit(round_to_step(value.as_mil(), 0.1), "thou", 1),
        LengthUnit::Inch | LengthUnit::In => {
            format_with_unit(round_to_step(value.as_inch(), 0.0001), "in", 4)
        }
    }
}

fn format_native_feed_rate(value: FeedRate) -> String {
    match value.unit {
        FeedRateUnit::MmPerMin => format_with_unit(round_to_step(value.as_mm_per_min(), 0.001), "mm/min", 3),
        FeedRateUnit::CmPerMin => format_with_unit(round_to_step(value.as_mm_per_min() / 10.0, 0.0001), "cm/min", 4),
        FeedRateUnit::MPerMin => format_with_unit(round_to_step(value.as_mm_per_min() / 1000.0, 0.000001), "m/min", 6),
        FeedRateUnit::InPerMin => format_with_unit(round_to_step(value.as_in_per_min(), 0.0001), "in/min", 4),
        FeedRateUnit::Ipm => format_with_unit(round_to_step(value.as_in_per_min(), 0.0001), "ipm", 4),
        FeedRateUnit::InchPerMin => {
            format_with_unit(round_to_step(value.as_in_per_min(), 0.0001), "inch/min", 4)
        }
    }
}

fn round_significant(value: f64, digits: usize) -> f64 {
    if value == 0.0 {
        return 0.0;
    }

    let scale = 10f64.powi(digits as i32 - 1 - value.abs().log10().floor() as i32);
    (value * scale).round() / scale
}

fn trim_float(value: f64) -> String {
    let rounded = round_significant(value, 14);
    let mut text = format!("{rounded:.14}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

fn insert_number(map: &mut Map, key: &str, value: f64) {
    let rounded = round_significant(value, 14);
    if (rounded.fract()).abs() < EPS {
        map.insert(key.into(), Dynamic::from_int(rounded.round() as i64));
    } else {
        map.insert(key.into(), Dynamic::from_float(rounded));
    }
}

#[cfg(test)]
mod tests {
    use super::{Angle, FeedRate, Length, ScalarValue, UserUnitDisplay, UserUnitSystem};

    #[test]
    fn stores_fraction_scalar_for_length_string() {
        let value = Length::from_string("4/3mm", None).expect("length should parse");
        assert_eq!(
            value.scalar(),
            ScalarValue::Fraction {
                numerator: 4.0,
                denominator: 3,
            }
        );
        assert_eq!(value.to_string(), "4/3mm");
    }

    #[test]
    fn supports_decimal_fraction_numerator() {
        let value = Length::from_string("1.5/2mm", None).expect("length should parse");
        assert_eq!(value.to_string(), "1.5/2mm");
        assert!((value.as_mm() - 0.75).abs() < 1e-9);
    }

    #[test]
    fn parses_mixed_fraction_imperial() {
        let value = Length::from_string("1 1/8 in", None).expect("length should parse");
        assert_eq!(
            value.scalar(),
            ScalarValue::Fraction {
                numerator: 9.0,
                denominator: 8,
            }
        );
        assert!((value.as_mm() - 28.575).abs() < 1e-9);
    }

    #[test]
    fn parses_mixed_fraction_with_quote_unit() {
        let value = Length::from_string("1 1/8\"", None).expect("length should parse");
        assert!((value.as_inch() - 1.125).abs() < 1e-9);
    }

    #[test]
    fn parses_length_with_unit_alias_and_converts() {
        let value = Length::from_string("8/9 in", None).expect("length should parse");
        assert!((value.as_mm() - 22.577_777_777_8).abs() < 1e-9);
    }

    #[test]
    fn parses_feedrate_fraction() {
        let value = FeedRate::from_string("69/8   cm/min", None).expect("feedrate should parse");
        assert!((value.as_mm_per_min() - 86.25).abs() < 1e-9);
    }

    #[test]
    fn angle_default_unit_matches_python_behavior() {
        let value = Angle::from_string("360", None).expect("angle should parse");
        assert_eq!(value.as_degrees(), 360.0);
    }

    #[test]
    fn length_map_uses_integer_when_exact() {
        let len = Length::from_mm(12.5);
        let map = len.to_rhai_map();
        let nm = map.get("nm").expect("nm should exist").clone().cast::<i64>();
        assert_eq!(nm, 12_500_000);
    }

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
    fn length_user_display_rounds_to_um() {
        let length = Length::from_mm(1.000_02);
        let display = length.unit_display(UserUnitSystem::Metric);
        assert_eq!(display.user, "1mm");

        let length = Length::from_mm(1.100_02);
        let display = length.unit_display(UserUnitSystem::Metric);
        assert_eq!(display.user, "1.1mm");

        let length = Length::from_mm(1.149_992);
        let display = length.unit_display(UserUnitSystem::Metric);
        assert_eq!(display.user, "1.15mm");

        let length = Length::from_mm(0.80000001192093);
        let display = length.unit_display(UserUnitSystem::Metric);
        assert_eq!(display.user, "0.8mm");
    }

    #[test]
    fn length_display_provides_native_when_units_differ() {
        let length = Length::from_string("1/8in", None).expect("length should parse");

        let metric_display = length.unit_display(UserUnitSystem::Metric);
        assert_eq!(metric_display.user, "3.175mm");
        assert_eq!(metric_display.native.as_deref(), Some("0.125in"));

        let imperial_display = length.unit_display(UserUnitSystem::Imperial);
        assert_eq!(imperial_display.user, "0.125in");
        assert_eq!(imperial_display.native, None);
    }
}
