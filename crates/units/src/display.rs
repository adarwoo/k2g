//! Part 2 — presentation for the UI.
//!
//! This module turns the typed quantities from [`crate::types`] into
//! user-facing strings and rounded numbers. It is the layer a GUI calls to
//! render a value in the operator's preferred unit system, optionally annotated
//! with the original ("native") value when the two differ.
//!
//! Nothing here mutates or persists data — it only reads quantities through
//! their public accessors and formats them.

use crate::types::{Angle, FeedRate, Length, LengthUnit, RotationalSpeed};

/// The unit system the operator has selected for display.
///
/// `Mil` is a length-only presentation (thousandths of an inch); for feed rates
/// it behaves as `Imperial` (in/min), and angle/rpm are system-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserUnitSystem {
    Metric,
    Imperial,
    Mil,
}

impl UserUnitSystem {
    /// The stable token this system persists as in settings files
    /// (`"mm"` / `"in"` / `"mil"`).
    pub fn as_settings_str(self) -> &'static str {
        match self {
            Self::Metric => "mm",
            Self::Imperial => "in",
            Self::Mil => "mil",
        }
    }

    /// Parses a persisted settings token back into a system, tolerating the
    /// legacy `"imperial"` spelling and defaulting unknown/missing values to
    /// [`UserUnitSystem::Metric`].
    pub fn from_settings_str(value: Option<&str>) -> Self {
        match value {
            Some("mil") => Self::Mil,
            Some("in") | Some("imperial") => Self::Imperial,
            _ => Self::Metric,
        }
    }
}

/// A value rendered for the user, plus an optional "native" annotation.
///
/// `native` is populated only when the stored source unit differs from what the
/// user sees (e.g. an imperial fraction shown while the user works in metric),
/// so the UI can display the original alongside the converted value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDisplay {
    /// The value formatted in the user's selected unit system.
    pub user: String,
    /// The original value in its source unit, when it differs from `user`.
    pub native: Option<String>,
}

/// Rendering behaviour for a typed quantity in a chosen [`UserUnitSystem`].
pub trait UserUnitDisplay {
    /// Formats the value for display, including a native annotation when useful.
    fn unit_display(&self, user_unit_system: UserUnitSystem) -> UnitDisplay;
    /// The rounded numeric value in the user's selected unit system.
    fn user_value(&self, user_unit_system: UserUnitSystem) -> f64;
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
            UserUnitSystem::Mil => {
                let value = round_to_step(self.as_mil(), 0.1);
                format_with_unit(value, "mil", 1)
            }
        };

        let native = match (user_unit_system, self.unit()) {
            (UserUnitSystem::Metric, LengthUnit::Mm) => None,
            (UserUnitSystem::Imperial, LengthUnit::In | LengthUnit::Inch) => None,
            (UserUnitSystem::Mil, LengthUnit::Mil | LengthUnit::Thou) => None,
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
            UserUnitSystem::Mil => round_to_step(self.as_mil(), 0.1),
        }
    }
}

impl UserUnitDisplay for FeedRate {
    fn unit_display(&self, user_unit_system: UserUnitSystem) -> UnitDisplay {
        let user = match user_unit_system {
            UserUnitSystem::Metric => {
                let value = round_to_step(self.as_mm_per_min(), 0.001);
                format_with_unit(value, "mm/min", 3)
            }
            UserUnitSystem::Imperial | UserUnitSystem::Mil => {
                let value = round_to_step(self.as_in_per_min(), 0.0001);
                format_with_unit(value, "ipm", 4)
            }
        };

        let native_matches_user = matches!(
            (user_unit_system, self.unit()),
            (UserUnitSystem::Metric, crate::types::FeedRateUnit::MmPerMin)
                | (
                    UserUnitSystem::Imperial | UserUnitSystem::Mil,
                    crate::types::FeedRateUnit::Ipm
                        | crate::types::FeedRateUnit::InPerMin
                        | crate::types::FeedRateUnit::InchPerMin
                )
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
            UserUnitSystem::Imperial | UserUnitSystem::Mil => {
                round_to_step(self.as_in_per_min(), 0.0001)
            }
        }
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

/// Rounds `value` to the nearest multiple of `step`. Non-finite values and a
/// non-positive `step` are returned unchanged. This is the single rounder shared
/// across the crate's display layers (`display` and `user_format`).
pub(crate) fn round_to_step(value: f64, step: f64) -> f64 {
    if !value.is_finite() || step <= 0.0 {
        return value;
    }
    (value / step).round() * step
}

/// Formats `value` to at most `max_decimals` places, trimming trailing zeros.
///
/// The single number-formatting core shared across the crate's display layers
/// (`display`'s `format_with_unit` and `user_format`'s `format_trimmed`). For
/// `max_decimals == 0` it defers to `{:.0}` (round-half-to-even) with no trim;
/// callers that need round-half-away-from-zero handle that before calling.
pub(crate) fn format_number(value: f64, max_decimals: usize) -> String {
    let mut text = format!("{value:.max_decimals$}");
    if max_decimals > 0 {
        while text.contains('.') && text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// Formats `value` with `unit_suffix`, trimming trailing zeros up to
/// `max_decimals` places. Whole-number output at `max_decimals == 0` rounds
/// half away from zero (via integer cast) to match historical behaviour.
fn format_with_unit(value: f64, unit_suffix: &str, max_decimals: usize) -> String {
    let text = if max_decimals == 0 {
        format!("{}", value.round() as i64)
    } else {
        format_number(value, max_decimals)
    };
    format!("{text}{unit_suffix}")
}

/// Renders a length in its own source unit at that unit's natural precision.
fn format_native_length(value: Length) -> String {
    match value.unit() {
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

/// Renders a feed rate in its own source unit at that unit's natural precision.
fn format_native_feed_rate(value: FeedRate) -> String {
    use crate::types::FeedRateUnit;
    match value.unit() {
        FeedRateUnit::MmPerMin => {
            format_with_unit(round_to_step(value.as_mm_per_min(), 0.001), "mm/min", 3)
        }
        FeedRateUnit::CmPerMin => {
            format_with_unit(round_to_step(value.as_mm_per_min() / 10.0, 0.0001), "cm/min", 4)
        }
        FeedRateUnit::MPerMin => {
            format_with_unit(round_to_step(value.as_mm_per_min() / 1000.0, 0.000001), "m/min", 6)
        }
        FeedRateUnit::InPerMin => {
            format_with_unit(round_to_step(value.as_in_per_min(), 0.0001), "in/min", 4)
        }
        FeedRateUnit::Ipm => format_with_unit(round_to_step(value.as_in_per_min(), 0.0001), "ipm", 4),
        FeedRateUnit::InchPerMin => {
            format_with_unit(round_to_step(value.as_in_per_min(), 0.0001), "inch/min", 4)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UserUnitDisplay, UserUnitSystem};
    use crate::types::Length;

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
