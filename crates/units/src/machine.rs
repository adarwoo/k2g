//! Part 2c — machine (GCode) number formatting.
//!
//! Formats a typed quantity as a **bare number** (no unit suffix) in the
//! machine's active output system, for emission into GCode where the unit is
//! implied by the modal `G21`/`G20` state. This is the primitive the GTL engine's
//! emit-time `fmt` builds on (see `docs/gcode-engine.md` §4 and
//! `docs/unit-display.md` §6): sharing the crate's [`round_to_step`] and
//! [`format_number`] core keeps on-screen values and emitted coordinates rounded
//! identically.
//!
//! GCode is only ever metric or imperial — `Mil` is a UI display choice, not a
//! dialect — so a `Mil` system maps to `Imperial` here.

use crate::display::{format_number, UserUnitDisplay, UserUnitSystem};
use crate::types::{Angle, FeedRate, Length, RotationalSpeed};

/// Collapses the UI-only `Mil` presentation onto `Imperial`, since GCode emits
/// millimetres or inches only.
fn machine_system(system: UserUnitSystem) -> UserUnitSystem {
    match system {
        UserUnitSystem::Metric => UserUnitSystem::Metric,
        UserUnitSystem::Imperial | UserUnitSystem::Mil => UserUnitSystem::Imperial,
    }
}

/// Decimal places for a coordinate/feed in the given machine system: metric to
/// the micron (3 dp), imperial to 0.1 mil (4 dp) — matching the rounding
/// [`UserUnitDisplay::user_value`] applies.
fn decimals(system: UserUnitSystem) -> usize {
    match system {
        UserUnitSystem::Metric => 3,
        UserUnitSystem::Imperial | UserUnitSystem::Mil => 4,
    }
}

/// A length as a bare coordinate number in the active machine system
/// (e.g. `-40`, `-25.4`, or `-1` in imperial). No unit suffix.
pub fn number_length(length: Length, system: UserUnitSystem) -> String {
    let system = machine_system(system);
    format_number(length.user_value(system), decimals(system))
}

/// A feed rate as a bare number in the active machine system (mm/min or in/min).
pub fn number_feed(feed: FeedRate, system: UserUnitSystem) -> String {
    let system = machine_system(system);
    format_number(feed.user_value(system), decimals(system))
}

/// A spindle speed as a bare integer rpm (system-invariant).
pub fn number_speed(speed: RotationalSpeed, system: UserUnitSystem) -> String {
    format_number(speed.user_value(machine_system(system)), 0)
}

/// An angle as a bare number of degrees, to 0.01°.
///
/// System-invariant, like rpm — a degree is a degree in either dialect, so the
/// `system` argument is taken only to keep the four `number_*` calls uniform for the
/// engine's type-dispatched `fmt`. Two decimals matches what
/// [`UserUnitDisplay::user_value`] already rounds an `Angle` to, so a value shown in
/// the UI and the same value emitted into a program never disagree.
pub fn number_angle(angle: Angle, system: UserUnitSystem) -> String {
    format_number(angle.user_value(machine_system(system)), 2)
}

#[cfg(test)]
mod tests {
    use super::{number_angle, number_feed, number_length, number_speed};
    use crate::display::UserUnitSystem;
    use crate::types::{Angle, FeedRate, Length, RotationalSpeed};

    #[test]
    fn length_emits_bare_number_whole_and_fractional() {
        assert_eq!(number_length(Length::from_mm(-40.0), UserUnitSystem::Metric), "-40");
        assert_eq!(number_length(Length::from_mm(-25.4), UserUnitSystem::Metric), "-25.4");
    }

    #[test]
    fn length_converts_to_inches_in_imperial() {
        assert_eq!(number_length(Length::from_mm(-25.4), UserUnitSystem::Imperial), "-1");
    }

    #[test]
    fn mil_system_falls_back_to_imperial_for_machine_output() {
        assert_eq!(number_length(Length::from_mm(-25.4), UserUnitSystem::Mil), "-1");
    }

    #[test]
    fn feed_and_speed_emit_bare_numbers() {
        assert_eq!(number_feed(FeedRate::from_mm_per_min(300.0), UserUnitSystem::Metric), "300");
        assert_eq!(number_speed(RotationalSpeed::from_rpm(12000.0), UserUnitSystem::Metric), "12000");
    }

    /// Degrees are the same number in either dialect, so the system must not change
    /// the digits — an angle that shifted with the machine's unit mode would rotate a
    /// slot differently in an imperial program than a metric one.
    #[test]
    fn angle_emits_the_same_bare_degrees_in_either_system() {
        assert_eq!(number_angle(Angle::from_degrees(90.0), UserUnitSystem::Metric), "90");
        assert_eq!(number_angle(Angle::from_degrees(90.0), UserUnitSystem::Imperial), "90");
        assert_eq!(number_angle(Angle::from_degrees(22.5), UserUnitSystem::Metric), "22.5");
    }
}
