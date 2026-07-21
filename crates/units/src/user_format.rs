//! Part 2b — the operator-facing formatter for editable fields.
//!
//! This is the presentation layer the GUI calls to render and parse values in
//! editable inputs: converted to the user's [`UserUnitSystem`], with the native
//! value annotated in `[...]` when the source unit differs, and an "edit" form
//! that seeds a text field with the bare number when the source already matches
//! the selected system. It is the single owner of this behaviour (relocated out
//! of the former `ui::unit_format`), and rounds through the crate's shared
//! [`round_to_step`].
//!
//! It differs deliberately from the compact [`crate::display`] renderer used for
//! summaries: this layer uses editable-field precision (metric 3dp, imperial
//! 5dp, mil 1dp), a spaced/`"`-suffixed length format, and a `mil` mode.

use crate::display::{format_number, round_to_step, UserUnitSystem};
use crate::types::{
    Angle, AngleUnit, FeedRate, FeedRateUnit, Length, LengthUnit, RotationalSpeed,
    RotationalSpeedUnit, ScalarValue, UnitParseError,
};

const MM_PRECISION: f64 = 0.001;
const IN_PRECISION: f64 = 0.00001;
const MIL_PRECISION: f64 = 0.1;

fn format_trimmed(mut value: f64, precision: f64, digits: usize) -> String {
    value = round_to_step(value, precision);
    if value.abs() < precision / 2.0 {
        value = 0.0;
    }
    format_number(value, digits)
}

fn length_precision(unit_system: UserUnitSystem) -> (f64, usize) {
    match unit_system {
        UserUnitSystem::Metric => (MM_PRECISION, 3),
        UserUnitSystem::Imperial => (IN_PRECISION, 5),
        UserUnitSystem::Mil => (MIL_PRECISION, 1),
    }
}

fn feed_precision(unit_system: UserUnitSystem) -> (f64, usize) {
    match unit_system {
        UserUnitSystem::Metric => (MM_PRECISION, 3),
        UserUnitSystem::Imperial | UserUnitSystem::Mil => (IN_PRECISION, 5),
    }
}

fn preferred_length_matches(length: Length, unit_system: UserUnitSystem) -> bool {
    match unit_system {
        UserUnitSystem::Metric => matches!(length.unit(), LengthUnit::Mm),
        UserUnitSystem::Imperial => matches!(length.unit(), LengthUnit::In | LengthUnit::Inch),
        UserUnitSystem::Mil => matches!(length.unit(), LengthUnit::Mil | LengthUnit::Thou),
    }
}

fn preferred_feed_matches(feed_rate: FeedRate, unit_system: UserUnitSystem) -> bool {
    match unit_system {
        UserUnitSystem::Metric => matches!(feed_rate.unit(), FeedRateUnit::MmPerMin),
        UserUnitSystem::Imperial | UserUnitSystem::Mil => {
            matches!(feed_rate.unit(), FeedRateUnit::InPerMin)
        }
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

pub fn length_unit_label(unit_system: UserUnitSystem) -> &'static str {
    match unit_system {
        UserUnitSystem::Metric => "mm",
        UserUnitSystem::Imperial => "\"",
        UserUnitSystem::Mil => "mil",
    }
}

pub fn feed_unit_label(unit_system: UserUnitSystem) -> &'static str {
    match unit_system {
        UserUnitSystem::Metric => "mm/min",
        UserUnitSystem::Imperial | UserUnitSystem::Mil => "in/min",
    }
}

pub fn angle_unit_label() -> &'static str {
    "°"
}

pub fn rotational_speed_unit_label() -> &'static str {
    "rpm"
}

pub fn length_input_step(unit_system: UserUnitSystem) -> &'static str {
    match unit_system {
        UserUnitSystem::Metric => "0.001",
        UserUnitSystem::Imperial => "0.00001",
        UserUnitSystem::Mil => "0.1",
    }
}

pub fn format_angle_display(angle: Angle) -> String {
    let value = format_trimmed(angle.as_degrees(), 0.01, 2);
    format!("{value}{}", angle_unit_label())
}

pub fn format_rotational_speed_display(speed: RotationalSpeed) -> String {
    let value = format_trimmed(speed.as_rpm(), 1.0, 0);
    format!("{value} {}", rotational_speed_unit_label())
}

pub fn display_length_value_from_mm(value_mm: f64, unit_system: UserUnitSystem) -> f64 {
    match unit_system {
        UserUnitSystem::Metric => value_mm,
        UserUnitSystem::Imperial => value_mm / 25.4,
        UserUnitSystem::Mil => value_mm * 1000.0 / 25.4,
    }
}

pub fn mm_from_display_length(display_value: f64, unit_system: UserUnitSystem) -> f64 {
    match unit_system {
        UserUnitSystem::Metric => display_value,
        UserUnitSystem::Imperial => display_value * 25.4,
        UserUnitSystem::Mil => display_value * 25.4 / 1000.0,
    }
}

pub fn display_feed_value_from_mm_per_min(value_mm_per_min: f64, unit_system: UserUnitSystem) -> f64 {
    match unit_system {
        UserUnitSystem::Metric => value_mm_per_min,
        UserUnitSystem::Imperial | UserUnitSystem::Mil => value_mm_per_min / 25.4,
    }
}

pub fn format_length_input_value_from_mm(value_mm: f64, unit_system: UserUnitSystem) -> String {
    let display_value = display_length_value_from_mm(value_mm, unit_system);
    let (step, digits) = length_precision(unit_system);
    format_trimmed(display_value, step, digits)
}

pub fn format_length_display(length: Length, unit_system: UserUnitSystem) -> String {
    let display_value = display_length_value_from_mm(length.as_mm(), unit_system);
    let (step, digits) = length_precision(unit_system);
    let display = if unit_system == UserUnitSystem::Imperial {
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
        || (unit_system == UserUnitSystem::Imperial && is_fractional_inch(length));

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

pub fn format_length_edit_display(length: Length, unit_system: UserUnitSystem) -> String {
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

pub fn format_feed_edit_display(feed_rate: FeedRate, unit_system: UserUnitSystem) -> String {
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

pub fn format_feed_display(feed_rate: FeedRate, unit_system: UserUnitSystem) -> String {
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
    unit_system: UserUnitSystem,
) -> Result<Length, UnitParseError> {
    let default = match unit_system {
        UserUnitSystem::Metric => LengthUnit::Mm,
        UserUnitSystem::Imperial => LengthUnit::Inch,
        UserUnitSystem::Mil => LengthUnit::Mil,
    };
    Length::from_string(value, Some(default))
}

pub fn parse_feed_with_preference(
    value: &str,
    unit_system: UserUnitSystem,
) -> Result<FeedRate, UnitParseError> {
    let default = match unit_system {
        UserUnitSystem::Metric => FeedRateUnit::MmPerMin,
        UserUnitSystem::Imperial | UserUnitSystem::Mil => FeedRateUnit::InPerMin,
    };
    FeedRate::from_string(value, Some(default))
}

pub fn parse_angle(value: &str) -> Result<Angle, UnitParseError> {
    Angle::from_string(value, Some(AngleUnit::Degree))
}

pub fn parse_rotational_speed(value: &str) -> Result<RotationalSpeed, UnitParseError> {
    RotationalSpeed::from_string(value, Some(RotationalSpeedUnit::Rpm))
}
