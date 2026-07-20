//! Part 1 — internal data-type management.
//!
//! This module owns the strongly-typed unit primitives and everything needed to
//! construct, convert, and canonically render them. It has **no** knowledge of
//! how values are displayed to a user (see [`crate::display`]) or how they are
//! serialized to configuration files (see [`crate::persistence`]).
//!
//! # Model
//!
//! Every quantity ([`Length`], [`FeedRate`], [`Angle`], [`RotationalSpeed`])
//! stores two things:
//!
//! * a [`ScalarValue`] — the raw magnitude, preserving whether the source was an
//!   integer, a float, or a fraction (`4/3`); and
//! * the source unit it was written in (e.g. [`LengthUnit::Mm`]).
//!
//! Preserving the original scalar form and unit means a value read as `1 1/8"`
//! round-trips back to an imperial fraction instead of being flattened to
//! `28.575mm`. Conversions ([`Length::as_mm`], [`FeedRate::as_mm_per_min`], …)
//! compute on demand through a canonical base unit (nanometres for length,
//! mm/min for feed rate).
//!
//! # Parsing
//!
//! [`Length::from_string`] and friends accept the human/GUI/YAML forms the
//! application uses, including bare numbers, `10mm`, `0.125 in`, `1/8"`, and
//! mixed fractions such as `1 1/8 in`. The shared parser lives at the bottom of
//! this module ([`parse_number_with_optional_unit`]).

use std::fmt;

/// Errors raised while parsing a scalar/unit string (see `parse_number_with_optional_unit`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitParseError {
    /// The numeric portion was empty or not a valid integer/float.
    InvalidNumberFormat,
    /// The numerator of a fraction failed to parse.
    InvalidNumerator,
    /// The denominator of a fraction was zero or failed to parse.
    InvalidDenominator,
    /// The trailing unit token was not recognised for the target quantity.
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

/// Scalar magnitude that remembers how it was written.
///
/// Keeping the original shape lets a value serialize back to the exact form the
/// user typed (e.g. a fraction stays a fraction) while still converting to a
/// plain `f64` on demand via [`ScalarValue::as_f64`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScalarValue {
    /// A whole number such as `10`.
    Integer(i64),
    /// A decimal such as `0.125`.
    Float(f64),
    /// A fraction such as `4/3`; the numerator may itself be fractional.
    Fraction { numerator: f64, denominator: i64 },
}

impl ScalarValue {
    /// Collapses the scalar to a plain floating-point magnitude.
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
    /// Renders the scalar in its canonical form (`10`, `0.125`, `4/3`).
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

// ---------------------------------------------------------------------------
// Length
// ---------------------------------------------------------------------------

/// Linear-dimension units understood by [`Length`].
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
    /// Canonical suffix used when rendering the source form (e.g. `mm`).
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

    /// Multiplier that converts one unit of `self` to nanometres, the canonical
    /// base used for all length conversions.
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

    /// Parses a unit suffix; accepts the `"` alias for inches.
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
    /// Builds an exact length from an integer number of nanometres.
    pub const fn from_nm(nm: i64) -> Self {
        Self {
            scalar: ScalarValue::Integer(nm),
            unit: LengthUnit::Nm,
        }
    }

    /// Builds a length from micrometres.
    pub fn from_um(um: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(um),
            unit: LengthUnit::Um,
        }
    }

    /// Builds a length from millimetres.
    pub fn from_mm(mm: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(mm),
            unit: LengthUnit::Mm,
        }
    }

    /// Builds a length from centimetres.
    pub fn from_cm(cm: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(cm),
            unit: LengthUnit::Cm,
        }
    }

    /// Builds a length from mils (thou).
    pub fn from_mil(mil: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(mil),
            unit: LengthUnit::Mil,
        }
    }

    /// Builds a length from inches.
    pub fn from_inch(inch: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(inch),
            unit: LengthUnit::Inch,
        }
    }

    /// Builds a length from a KiCad IPC value, which is always integer nanometres.
    ///
    /// The KiCad IPC API expresses every dimension in nanometres (`Vector2Nm`,
    /// `*_nm` fields), so the value is taken as an exact nm count rather than a
    /// millimetre float.
    pub const fn from_kicad(nm: i64) -> Self {
        Self::from_nm(nm)
    }

    /// Parses a length string such as `10mm`, `0.125 in`, `1/8"`, or `1 1/8 in`.
    ///
    /// When the string carries no unit, `default_unit` is used (falling back to
    /// millimetres). Returns [`UnitParseError`] on malformed input.
    pub fn from_string(input: &str, default_unit: Option<LengthUnit>) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => LengthUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(LengthUnit::Mm),
        };

        Ok(Self { scalar, unit })
    }

    /// Constructs a length from a raw scalar, defaulting to the millimetre unit.
    ///
    /// Used by the persistence layer when a bare number is deserialized.
    pub(crate) const fn from_scalar(scalar: ScalarValue) -> Self {
        Self {
            scalar,
            unit: LengthUnit::Mm,
        }
    }

    /// The original scalar magnitude, in its source form.
    pub const fn scalar(self) -> ScalarValue {
        self.scalar
    }

    /// The unit the length was authored in.
    pub const fn unit(self) -> LengthUnit {
        self.unit
    }

    /// Value in nanometres (the canonical base).
    pub fn as_nm(self) -> f64 {
        self.scalar.as_f64() * self.unit.factor_to_nm()
    }

    /// Value in micrometres.
    pub fn as_um(self) -> f64 {
        self.as_nm() / LengthUnit::Um.factor_to_nm()
    }

    /// Value in millimetres.
    pub fn as_mm(self) -> f64 {
        self.as_nm() / LengthUnit::Mm.factor_to_nm()
    }

    /// Value in centimetres.
    pub fn as_cm(self) -> f64 {
        self.as_nm() / LengthUnit::Cm.factor_to_nm()
    }

    /// Value in mils (thou).
    pub fn as_mil(self) -> f64 {
        self.as_nm() / LengthUnit::Mil.factor_to_nm()
    }

    /// Value in inches.
    pub fn as_inch(self) -> f64 {
        self.as_nm() / LengthUnit::Inch.factor_to_nm()
    }
}

impl fmt::Display for Length {
    /// Renders the canonical source form, e.g. `4/3mm` or `9/8in`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

// ---------------------------------------------------------------------------
// FeedRate
// ---------------------------------------------------------------------------

/// Feed-rate units understood by [`FeedRate`].
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
    /// Canonical suffix used when rendering the source form (e.g. `mm/min`).
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

    /// Multiplier that converts one unit of `self` to mm/min, the canonical base
    /// used for all feed-rate conversions.
    fn factor_to_mm_per_min(self) -> f64 {
        match self {
            Self::MmPerMin => 1.0,
            Self::CmPerMin => 10.0,
            Self::MPerMin => 1_000.0,
            Self::InPerMin | Self::Ipm | Self::InchPerMin => 25.4,
        }
    }

    /// Parses a feed-rate unit suffix.
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

/// Feed-rate quantity preserving input scalar form and source unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FeedRate {
    scalar: ScalarValue,
    unit: FeedRateUnit,
}

impl FeedRate {
    /// Builds a feed rate from millimetres per minute.
    pub fn from_mm_per_min(mm_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(mm_per_min),
            unit: FeedRateUnit::MmPerMin,
        }
    }

    /// Builds a feed rate from inches per minute.
    pub fn from_in_per_min(in_per_min: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(in_per_min),
            unit: FeedRateUnit::InPerMin,
        }
    }

    /// Builds a feed rate from a KiCad value, which is always mm/min.
    pub fn from_kicad(value: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(value),
            unit: FeedRateUnit::MmPerMin,
        }
    }

    /// Parses a feed-rate string such as `1200mm/min`, `96ipm`, or `69/8 cm/min`.
    ///
    /// When the string carries no unit, `default_unit` is used (falling back to
    /// mm/min).
    pub fn from_string(input: &str, default_unit: Option<FeedRateUnit>) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => FeedRateUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(FeedRateUnit::MmPerMin),
        };

        Ok(Self { scalar, unit })
    }

    /// Constructs a feed rate from a raw scalar, defaulting to the mm/min unit.
    ///
    /// Used by the persistence layer when a bare number is deserialized.
    pub(crate) const fn from_scalar(scalar: ScalarValue) -> Self {
        Self {
            scalar,
            unit: FeedRateUnit::MmPerMin,
        }
    }

    /// Value in millimetres per minute (the canonical base).
    pub fn as_mm_per_min(self) -> f64 {
        self.scalar.as_f64() * self.unit.factor_to_mm_per_min()
    }

    /// Value in inches per minute.
    pub fn as_in_per_min(self) -> f64 {
        self.as_mm_per_min() / FeedRateUnit::InPerMin.factor_to_mm_per_min()
    }

    /// The unit the feed rate was authored in.
    pub const fn unit(self) -> FeedRateUnit {
        self.unit
    }
}

impl fmt::Display for FeedRate {
    /// Renders the canonical source form, e.g. `69/8cm/min`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

// ---------------------------------------------------------------------------
// Angle
// ---------------------------------------------------------------------------

/// Angle units understood by [`Angle`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleUnit {
    Deg,
    Degree,
}

impl AngleUnit {
    /// Canonical suffix used when rendering the source form.
    fn name(self) -> &'static str {
        match self {
            Self::Deg => "deg",
            Self::Degree => "degree",
        }
    }

    /// Parses an angle unit suffix.
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
    /// Builds an angle from degrees.
    pub fn from_degrees(degrees: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(degrees),
            unit: AngleUnit::Deg,
        }
    }

    /// Builds an angle from radians (converted to degrees).
    pub fn from_radians(radians: f64) -> Self {
        Self::from_degrees(radians.to_degrees())
    }

    /// Builds an angle from a KiCad value, which is always degrees.
    pub fn from_kicad(value: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(value),
            unit: AngleUnit::Deg,
        }
    }

    /// Parses an angle string such as `130deg`; bare numbers default to degrees.
    pub fn from_string(input: &str, default_unit: Option<AngleUnit>) -> Result<Self, UnitParseError> {
        let (scalar, unit_str) = parse_number_with_optional_unit(input)?;
        let unit = match unit_str {
            Some(raw) => AngleUnit::parse(raw).ok_or(UnitParseError::InvalidUnit)?,
            None => default_unit.unwrap_or(AngleUnit::Deg),
        };

        Ok(Self { scalar, unit })
    }

    /// Constructs an angle from a raw scalar, defaulting to the degree unit.
    ///
    /// Used by the persistence layer when a bare number is deserialized.
    pub(crate) const fn from_scalar(scalar: ScalarValue) -> Self {
        Self {
            scalar,
            unit: AngleUnit::Deg,
        }
    }

    /// Value in degrees.
    pub fn as_degrees(self) -> f64 {
        self.scalar.as_f64()
    }

    /// Value in radians.
    pub fn as_radians(self) -> f64 {
        self.as_degrees().to_radians()
    }
}

impl fmt::Display for Angle {
    /// Renders the canonical source form, e.g. `130deg`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

// ---------------------------------------------------------------------------
// RotationalSpeed
// ---------------------------------------------------------------------------

/// Rotational-speed units understood by [`RotationalSpeed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationalSpeedUnit {
    Rpm,
}

impl RotationalSpeedUnit {
    /// Canonical suffix used when rendering the source form.
    fn name(self) -> &'static str {
        "rpm"
    }

    /// Parses a rotational-speed unit suffix.
    fn parse(unit: &str) -> Option<Self> {
        match unit {
            "rpm" => Some(Self::Rpm),
            _ => None,
        }
    }
}

/// Rotational-speed quantity in revolutions-per-minute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RotationalSpeed {
    scalar: ScalarValue,
    unit: RotationalSpeedUnit,
}

impl RotationalSpeed {
    /// Builds a rotational speed from revolutions per minute.
    pub fn from_rpm(rpm: f64) -> Self {
        Self {
            scalar: ScalarValue::Float(rpm),
            unit: RotationalSpeedUnit::Rpm,
        }
    }

    /// Builds a rotational speed from a KiCad value, which is always rpm.
    pub fn from_kicad(value: f64) -> Self {
        Self::from_rpm(value)
    }

    /// Parses a spindle-speed string such as `8000rpm`; bare numbers default to rpm.
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

    /// Constructs a rotational speed from a raw scalar, defaulting to the rpm unit.
    ///
    /// Used by the persistence layer when a bare number is deserialized.
    pub(crate) const fn from_scalar(scalar: ScalarValue) -> Self {
        Self {
            scalar,
            unit: RotationalSpeedUnit::Rpm,
        }
    }

    /// Value in revolutions per minute.
    pub fn as_rpm(self) -> f64 {
        self.scalar.as_f64()
    }

    /// The unit the speed was authored in.
    pub const fn unit(self) -> RotationalSpeedUnit {
        self.unit
    }
}

impl fmt::Display for RotationalSpeed {
    /// Renders the canonical source form, e.g. `8000rpm`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.scalar, self.unit.name())
    }
}

// ---------------------------------------------------------------------------
// Shared scalar/unit parser
// ---------------------------------------------------------------------------

/// Splits a raw input string into a [`ScalarValue`] and an optional unit token.
///
/// Handles three input shapes:
///
/// * mixed fractions — `1 1/8 in`, `1 1/8"`;
/// * simple fractions — `4/3mm`, `1.5/2mm`; and
/// * plain integers/decimals — `10mm`, `0.125 in`, `360`.
///
/// The unit token, when present, is returned verbatim for the caller's
/// quantity-specific parser to validate.
pub(crate) fn parse_number_with_optional_unit(
    input: &str,
) -> Result<(ScalarValue, Option<&str>), UnitParseError> {
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

/// Splits leading numeric characters from a trailing unit token.
fn split_scalar_and_unit(input: &str) -> (&str, &str) {
    let boundary = input
        .char_indices()
        .find(|(_, ch)| !is_scalar_char(*ch))
        .map(|(idx, _)| idx)
        .unwrap_or(input.len());

    (input[..boundary].trim(), input[boundary..].trim())
}

/// Characters that may appear in the numeric portion of a scalar token.
fn is_scalar_char(ch: char) -> bool {
    ch.is_ascii_digit() || ch == '.' || ch == '/' || ch == '+' || ch == '-'
}

/// Parses a bare numeric token into a [`ScalarValue`], choosing the variant that
/// preserves the written form (fraction / float / integer).
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

// ---------------------------------------------------------------------------
// Numeric formatting helpers (shared with the display layer)
// ---------------------------------------------------------------------------

/// Rounds `value` to `digits` significant figures, guarding against zero.
pub(crate) fn round_significant(value: f64, digits: usize) -> f64 {
    if value == 0.0 {
        return 0.0;
    }

    let scale = 10f64.powi(digits as i32 - 1 - value.abs().log10().floor() as i32);
    (value * scale).round() / scale
}

/// Formats a float without trailing zeros, used for canonical scalar rendering.
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
}
