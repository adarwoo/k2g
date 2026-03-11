//! Strongly-typed unit primitives used by the template context.
//!
//! This module supports scalar inputs from YAML/GUI strings, including fractions
//! like `4/3mm` or `69/8 cm/min`, while preserving the original scalar form.

use std::fmt;

use rhai::{Dynamic, Map};

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

    pub fn from_scalar(value: f64) -> Self {
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

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
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

    pub fn from_cm_per_min(cm_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(cm_per_min),
            unit: FeedRateUnit::CmPerMin,
        }
    }

    pub fn from_m_per_min(m_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(m_per_min),
            unit: FeedRateUnit::MPerMin,
        }
    }

    pub fn from_in_per_min(in_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(in_per_min),
            unit: FeedRateUnit::InPerMin,
        }
    }

    pub fn from_scalar(value: f64) -> Self {
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

    pub fn as_cm_per_min(self) -> f64 {
        self.as_mm_per_min() / FeedRateUnit::CmPerMin.factor_to_mm_per_min()
    }

    pub fn as_m_per_min(self) -> f64 {
        self.as_mm_per_min() / FeedRateUnit::MPerMin.factor_to_mm_per_min()
    }

    pub fn as_in_per_min(self) -> f64 {
        self.as_mm_per_min() / FeedRateUnit::InPerMin.factor_to_mm_per_min()
    }
}

impl fmt::Display for FeedRate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
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

    pub fn from_scalar(value: f64) -> Self {
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

impl fmt::Display for Angle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
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

    pub fn from_scalar(value: f64) -> Self {
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
}

impl fmt::Display for RotationalSpeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
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
    use super::{Angle, FeedRate, Length, ScalarValue};

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
}
