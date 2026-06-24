use super::model::UnitSystem;
use crate::units::{
    FeedRate, FeedRateUnit, Length, LengthUnit, ScalarValue, UnitParseError,
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

fn is_fractional_inch(length: Length) -> bool {
    matches!(length.unit(), LengthUnit::In | LengthUnit::Inch)
        && matches!(length.scalar(), ScalarValue::Fraction { .. })
}

pub fn length_unit_label(unit_system: UnitSystem) -> &'static str {
    match unit_system {
        UnitSystem::Metric => "mm",
        UnitSystem::Imperial => "in",
        UnitSystem::Mil => "mil",
    }
}

pub fn feed_unit_label(unit_system: UnitSystem) -> &'static str {
    match unit_system {
        UnitSystem::Metric => "mm/min",
        UnitSystem::Imperial | UnitSystem::Mil => "in/min",
    }
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

pub fn format_length_display(length: Length, unit_system: UnitSystem) -> String {
    let display_value = display_length_value_from_mm(length.as_mm(), unit_system);
    let (step, digits) = length_precision(unit_system);
    let display = format!(
        "{} {}",
        format_trimmed(display_value, step, digits),
        length_unit_label(unit_system)
    );

    let show_native = !preferred_length_matches(length, unit_system)
        || (unit_system == UnitSystem::Imperial && is_fractional_inch(length));

    if show_native {
        format!("{} [{}]", display, length)
    } else {
        display
    }
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
