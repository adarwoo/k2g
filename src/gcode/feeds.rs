//! Feeds & speeds resolution for one tool on a specific machine.
//!
//! Policy (settled 2026-07-25): a tool's rated feed rate is defined **at its rated
//! spindle speed**. When the machine cannot reach that speed, the spindle is clamped
//! to the machine's range and the feed is scaled by the **same ratio**, holding
//! feed-per-revolution (chip load) constant:
//!
//! ```text
//! rpm  = clamp(tool_rpm, cnc_min, cnc_max)
//! feed = tool_feed × (rpm / tool_rpm)
//! ```
//!
//! Both tool values are **required**: a tool missing either cannot be run, and the
//! caller turns the [`FeedsError`] into a generation nogo (no silent default — a
//! `G81` with no `F` or an `M3` with no `S` is an unsafe program).
//!
//! Example: a bit rated 10000 mm/min @ 100000 rpm on a 20000-rpm spindle runs at
//! 20000 rpm and 2000 mm/min — one-fifth the feed for one-fifth the speed. The clamp
//! is two-sided: a bit rated *below* the spindle minimum is raised to the minimum and
//! its feed scaled *up* by the same ratio, which likewise preserves chip load.

use units::{FeedRate, RotationalSpeed};

/// The machine's usable spindle range, from the CNC profile's
/// `spindle_rpm_min`/`spindle_rpm_max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpindleRange {
    pub min: RotationalSpeed,
    pub max: RotationalSpeed,
}

impl SpindleRange {
    pub fn new(min: RotationalSpeed, max: RotationalSpeed) -> Self {
        Self { min, max }
    }
}

/// The running values resolved for a tool on the machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedsSpeeds {
    pub rpm: RotationalSpeed,
    pub feed: FeedRate,
    /// The spindle could not run at the tool's rated speed, so both rpm and feed were
    /// scaled to fit the machine — worth surfacing to the operator.
    pub clamped: bool,
}

/// Why a tool cannot be given running values. The tool's feed/speed are required, so
/// each variant is an actionable "fix the stock entry" message for the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedsError {
    /// The tool has no rated feed rate.
    MissingFeed,
    /// The tool has no rated spindle speed.
    MissingSpeed,
    /// The rated spindle speed is not positive, so a feed ratio cannot be formed.
    NonPositiveSpeed,
}

/// Resolves the running rpm/feed for a tool rated at (`tool_feed` @ `tool_rpm`) on a
/// machine whose spindle covers `range`. See the module policy for the formula.
pub fn resolve(
    tool_feed: Option<FeedRate>,
    tool_rpm: Option<RotationalSpeed>,
    range: SpindleRange,
) -> Result<FeedsSpeeds, FeedsError> {
    let feed = tool_feed.ok_or(FeedsError::MissingFeed)?;
    let rated = tool_rpm.ok_or(FeedsError::MissingSpeed)?;
    let rated_rpm = rated.as_rpm();
    if rated_rpm <= 0.0 {
        return Err(FeedsError::NonPositiveSpeed);
    }

    // Normalize the range so a misconfigured min>max never panics `clamp`; a CNC with
    // an invalid spindle range is caught by its own usability check upstream.
    let (lo, hi) = {
        let (a, b) = (range.min.as_rpm(), range.max.as_rpm());
        if a <= b { (a, b) } else { (b, a) }
    };
    let actual_rpm = rated_rpm.clamp(lo, hi);

    // Scale the feed by the same ratio so feed-per-rev (chip load) is preserved.
    let scale = actual_rpm / rated_rpm;
    Ok(FeedsSpeeds {
        rpm: RotationalSpeed::from_rpm(actual_rpm),
        feed: FeedRate::from_mm_per_min(feed.as_mm_per_min() * scale),
        clamped: (scale - 1.0).abs() > 1e-9,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(min: f64, max: f64) -> SpindleRange {
        SpindleRange::new(RotationalSpeed::from_rpm(min), RotationalSpeed::from_rpm(max))
    }

    #[test]
    fn within_range_passes_the_rated_values_through_unchanged() {
        let out = resolve(
            Some(FeedRate::from_mm_per_min(600.0)),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            range(5_000.0, 24_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 12_000.0);
        assert_eq!(out.feed.as_mm_per_min(), 600.0);
        assert!(!out.clamped, "rated speed is reachable — nothing scaled");
    }

    #[test]
    fn clamping_down_scales_the_feed_by_the_same_ratio() {
        // The settled example: 10000 mm/min @ 100000 rpm on a 20000-rpm spindle.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(10_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            range(5_000.0, 20_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 20_000.0, "spindle capped at its max");
        assert_eq!(out.feed.as_mm_per_min(), 2_000.0, "feed scaled to one-fifth");
        assert!(out.clamped);
    }

    #[test]
    fn a_bit_slower_than_the_spindle_minimum_is_raised_and_its_feed_scaled_up() {
        // Rated 1000 rpm but the spindle floors at 5000 → 5× speed, 5× feed.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(200.0)),
            Some(RotationalSpeed::from_rpm(1_000.0)),
            range(5_000.0, 24_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 5_000.0);
        assert_eq!(out.feed.as_mm_per_min(), 1_000.0);
        assert!(out.clamped);
    }

    #[test]
    fn missing_tool_data_is_a_typed_error_not_a_default() {
        let r = range(5_000.0, 24_000.0);
        assert_eq!(
            resolve(None, Some(RotationalSpeed::from_rpm(12_000.0)), r),
            Err(FeedsError::MissingFeed)
        );
        assert_eq!(
            resolve(Some(FeedRate::from_mm_per_min(600.0)), None, r),
            Err(FeedsError::MissingSpeed)
        );
        assert_eq!(
            resolve(
                Some(FeedRate::from_mm_per_min(600.0)),
                Some(RotationalSpeed::from_rpm(0.0)),
                r
            ),
            Err(FeedsError::NonPositiveSpeed)
        );
    }
}
