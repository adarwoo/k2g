use super::model::UnitSystem;
use crate::units::{
    Angle, FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed, ScalarValue,
    UnitParseError,
};

const MM_PRECISION: f64 = 0.001;
const IN_PRECISION: f64 = 0.00001;
const MIL_PRECISION: f64 = 0.1;

fn round_to_step(value: f64, step: f64) -> f64 {
    if step <= 0.0 {
        return value;
    }
    (value / step).round() * step
}

fn format_trimmed(mut value: f64, precision: f64, digits: usize) -> String {
    value = round_to_step(value, precision);
    if value.abs() < precision / 2.0 {
        value = 0.0;
    }
    let mut out = format!("{value:.digits$}");
    while out.contains('.') && out.ends_with('0') {
        out.pop();
    }
    if out.ends_with('.') {
        out.pop();
    }
    out
}

fn length_precision(unit_system: UnitSystem) -> (f64, usize) {
    match unit_system {
        UnitSystem::Metric => (MM_PRECISION, 3),
        UnitSystem::Imperial => (IN_PRECISION, 5),
        UnitSystem::Mil => (MIL_PRECISION, 1),
    }
}

fn feed_precision(unit_system: UnitSystem) -> (f64, usize) {
    match unit_system {
        UnitSystem::Metric => (MM_PRECISION, 3),
        UnitSystem::Imperial | UnitSystem::Mil => (IN_PRECISION, 5),
    }
}

fn preferred_length_matches(length: Length, unit_system: UnitSystem) -> bool {
    match unit_system {
        UnitSystem::Metric => matches!(length.unit(), LengthUnit::Mm),
        UnitSystem::Imperial => matches!(length.unit(), LengthUnit::In | LengthUnit::Inch),
        UnitSystem::Mil => matches!(length.unit(), LengthUnit::Mil | LengthUnit::Thou),
    }
}

fn preferred_feed_matches(feed_rate: FeedRate, unit_system: UnitSystem) -> bool {
    match unit_system {
        UnitSystem::Metric => matches!(feed_rate.unit(), FeedRateUnit::MmPerMin),
        UnitSystem::Imperial | UnitSystem::Mil => matches!(feed_rate.unit(), FeedRateUnit::InPerMin),
    }
}

fn strip_suffix<'a>(value: &'a str, suffix: &str) -> Option<&'a str> {
    value.strip_suffix(suffix).map(str::trim_end)
}

fn symbolized_inch_value(length: Length) -> String {
    let text = length.to_string();
    if let Some(value) = strip_suffix(&text, "inch") {
        return format!("{value}\"");
    }
    if let Some(value) = strip_suffix(&text, "in") {
        return format!("{value}\"");
    }

    text
}

fn is_fractional_inch(length: Length) -> bool {
    matches!(length.unit(), LengthUnit::In | LengthUnit::Inch)
        && matches!(length.scalar(), ScalarValue::Fraction { .. })
}

pub fn length_unit_label(unit_system: UnitSystem) -> &'static str {
    match unit_system {
        UnitSystem::Metric => "mm",
        UnitSystem::Imperial => "\"",
        UnitSystem::Mil => "mil",
    }
}

pub fn feed_unit_label(unit_system: UnitSystem) -> &'static str {
    match unit_system {
        UnitSystem::Metric => "mm/min",
        UnitSystem::Imperial | UnitSystem::Mil => "in/min",
    }
}

pub fn angle_unit_label() -> &'static str {
    "°"
}

pub fn rotational_speed_unit_label() -> &'static str {
    "rpm"
}

pub fn percentage_unit_label() -> &'static str {
    "%"
}

pub fn length_input_step(unit_system: UnitSystem) -> &'static str {
    match unit_system {
        UnitSystem::Metric => "0.001",
        UnitSystem::Imperial => "0.00001",
        UnitSystem::Mil => "0.1",
    }
}

pub fn feed_input_step(unit_system: UnitSystem) -> &'static str {
    match unit_system {
        UnitSystem::Metric => "0.001",
        UnitSystem::Imperial | UnitSystem::Mil => "0.00001",
    }
}

pub fn default_length_suffix(unit_system: UnitSystem) -> &'static str {
    length_unit_label(unit_system)
}

pub fn default_feed_suffix(unit_system: UnitSystem) -> &'static str {
    feed_unit_label(unit_system)
}

pub fn format_angle_display(angle: Angle) -> String {
    let value = format_trimmed(angle.as_degrees(), 0.01, 2);
    format!("{value}{}", angle_unit_label())
}

pub fn format_rotational_speed_display(speed: RotationalSpeed) -> String {
    let value = format_trimmed(speed.as_rpm(), 1.0, 0);
    format!("{value} {}", rotational_speed_unit_label())
}

pub fn format_percentage_display(value: f64) -> String {
    let out = format_trimmed(value, 0.1, 1);
    format!("{out}{}", percentage_unit_label())
}

pub fn format_percentage_edit_display(value: f64) -> String {
    format_trimmed(value, 0.1, 1)
}

pub fn display_length_value_from_mm(value_mm: f64, unit_system: UnitSystem) -> f64 {
    match unit_system {
        UnitSystem::Metric => value_mm,
        UnitSystem::Imperial => value_mm / 25.4,
        UnitSystem::Mil => value_mm * 1000.0 / 25.4,
    }
}

pub fn mm_from_display_length(display_value: f64, unit_system: UnitSystem) -> f64 {
    match unit_system {
        UnitSystem::Metric => display_value,
        UnitSystem::Imperial => display_value * 25.4,
        UnitSystem::Mil => display_value * 25.4 / 1000.0,
    }
}

pub fn display_feed_value_from_mm_per_min(value_mm_per_min: f64, unit_system: UnitSystem) -> f64 {
    match unit_system {
        UnitSystem::Metric => value_mm_per_min,
        UnitSystem::Imperial | UnitSystem::Mil => value_mm_per_min / 25.4,
    }
}

pub fn mm_per_min_from_display_feed(display_value: f64, unit_system: UnitSystem) -> f64 {
    match unit_system {
        UnitSystem::Metric => display_value,
        UnitSystem::Imperial | UnitSystem::Mil => display_value * 25.4,
    }
}

pub fn format_length_input_value_from_mm(value_mm: f64, unit_system: UnitSystem) -> String {
    let display_value = display_length_value_from_mm(value_mm, unit_system);
    let (step, digits) = length_precision(unit_system);
    format_trimmed(display_value, step, digits)
}

pub fn format_feed_input_value_from_mm_per_min(
    value_mm_per_min: f64,
    unit_system: UnitSystem,
) -> String {
    let display_value = display_feed_value_from_mm_per_min(value_mm_per_min, unit_system);
    let (step, digits) = feed_precision(unit_system);
    format_trimmed(display_value, step, digits)
}

pub fn format_length_display(length: Length, unit_system: UnitSystem) -> String {
    let display_value = display_length_value_from_mm(length.as_mm(), unit_system);
    let (step, digits) = length_precision(unit_system);
    let display = if unit_system == UnitSystem::Imperial {
        format!(
            "{}{}",
            format_trimmed(display_value, step, digits),
            length_unit_label(unit_system)
        )
    } else {
        format!(
            "{} {}",
            format_trimmed(display_value, step, digits),
            length_unit_label(unit_system)
        )
    };

    let show_native = !preferred_length_matches(length, unit_system)
        || (unit_system == UnitSystem::Imperial && is_fractional_inch(length));

    if show_native {
        let native = if matches!(length.unit(), LengthUnit::In | LengthUnit::Inch) {
            symbolized_inch_value(length)
        } else {
            length.to_string()
        };
        format!("{} [{}]", display, native)
    } else {
        display
    }
}

pub fn format_length_edit_display(length: Length, unit_system: UnitSystem) -> String {
    let raw = length.to_string();

    if preferred_length_matches(length, unit_system) {
        return match length.unit() {
            LengthUnit::In | LengthUnit::Inch => strip_suffix(&raw, "inch")
                .or_else(|| strip_suffix(&raw, "in"))
                .unwrap_or(&raw)
                .to_string(),
            LengthUnit::Mm => strip_suffix(&raw, "mm").unwrap_or(&raw).to_string(),
            LengthUnit::Mil => strip_suffix(&raw, "mil").unwrap_or(&raw).to_string(),
            LengthUnit::Thou => strip_suffix(&raw, "thou").unwrap_or(&raw).to_string(),
            LengthUnit::Cm => strip_suffix(&raw, "cm").unwrap_or(&raw).to_string(),
            LengthUnit::Nm => strip_suffix(&raw, "nm").unwrap_or(&raw).to_string(),
            LengthUnit::Um => strip_suffix(&raw, "um").unwrap_or(&raw).to_string(),
        };
    }

    if matches!(length.unit(), LengthUnit::In | LengthUnit::Inch) {
        symbolized_inch_value(length)
    } else {
        raw
    }
}

pub fn format_feed_edit_display(feed_rate: FeedRate, unit_system: UnitSystem) -> String {
    let raw = feed_rate.to_string();

    if preferred_feed_matches(feed_rate, unit_system) {
        return match feed_rate.unit() {
            FeedRateUnit::MmPerMin => strip_suffix(&raw, "mm/min").unwrap_or(&raw).to_string(),
            FeedRateUnit::InPerMin => strip_suffix(&raw, "in/min").unwrap_or(&raw).to_string(),
            FeedRateUnit::Ipm => strip_suffix(&raw, "ipm").unwrap_or(&raw).to_string(),
            FeedRateUnit::InchPerMin => strip_suffix(&raw, "inch/min").unwrap_or(&raw).to_string(),
            FeedRateUnit::CmPerMin => strip_suffix(&raw, "cm/min").unwrap_or(&raw).to_string(),
            FeedRateUnit::MPerMin => strip_suffix(&raw, "m/min").unwrap_or(&raw).to_string(),
        };
    }

    raw
}

pub fn format_angle_edit_display(angle: Angle) -> String {
    let raw = angle.to_string();
    strip_suffix(&raw, "degree")
        .or_else(|| strip_suffix(&raw, "deg"))
        .unwrap_or(&raw)
        .to_string()
}

pub fn format_rotational_speed_edit_display(speed: RotationalSpeed) -> String {
    let raw = speed.to_string();
    strip_suffix(&raw, "rpm").unwrap_or(&raw).to_string()
}

pub fn format_feed_display(feed_rate: FeedRate, unit_system: UnitSystem) -> String {
    let display_value = display_feed_value_from_mm_per_min(feed_rate.as_mm_per_min(), unit_system);
    let (step, digits) = feed_precision(unit_system);
    let display = format!(
        "{} {}",
        format_trimmed(display_value, step, digits),
        feed_unit_label(unit_system)
    );

    if preferred_feed_matches(feed_rate, unit_system) {
        display
    } else {
        format!("{} [{}]", display, feed_rate)
    }
}

pub fn parse_length_with_preference(
    value: &str,
    unit_system: UnitSystem,
) -> Result<Length, UnitParseError> {
    let default = match unit_system {
        UnitSystem::Metric => LengthUnit::Mm,
        UnitSystem::Imperial => LengthUnit::Inch,
        UnitSystem::Mil => LengthUnit::Mil,
    };
    Length::from_string(value, Some(default))
}

pub fn parse_feed_with_preference(
    value: &str,
    unit_system: UnitSystem,
) -> Result<FeedRate, UnitParseError> {
    let default = match unit_system {
        UnitSystem::Metric => FeedRateUnit::MmPerMin,
        UnitSystem::Imperial | UnitSystem::Mil => FeedRateUnit::InPerMin,
    };
    FeedRate::from_string(value, Some(default))
}

pub fn parse_angle(value: &str) -> Result<Angle, UnitParseError> {
    Angle::from_string(value, Some(crate::units::AngleUnit::Degree))
}

pub fn parse_rotational_speed(value: &str) -> Result<RotationalSpeed, UnitParseError> {
    RotationalSpeed::from_string(value, Some(crate::units::RotationalSpeedUnit::Rpm))
}

pub fn parse_percentage(value: &str) -> Result<f64, UnitParseError> {
    let raw = value.trim();
    let value = strip_suffix(raw, "%").unwrap_or(raw);
    value
        .trim()
        .parse::<f64>()
        .map_err(|_| UnitParseError::InvalidNumberFormat)
}
