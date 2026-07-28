//! Feeds & speeds resolution for one tool on a specific machine.
//!
//! Policy (settled 2026-07-25): a tool's rated feed rate is defined **at its rated
//! spindle speed**. The quantity that must be preserved is therefore the ratio between
//! them — feed per revolution, the chip load:
//!
//! ```text
//! k = tool_feed / tool_rpm        (mm per rev)
//! ```
//!
//! Everything below holds `k` constant and moves the spindle instead. Two machine
//! constraints bear on it:
//!
//! 1. **The spindle range.** A speed the machine cannot reach is clamped into
//!    `[cnc_min, cnc_max]` and the feed follows it by the same ratio. The clamp is
//!    two-sided: a bit rated *below* the minimum is raised, and its feed raised with it.
//! 2. **The axis feed ceiling** (added 2026-07-28). A feed the machine cannot sustain is
//!    not simply capped — capping `F` while leaving `S` where it was *lowers the chip
//!    load*, which is precisely the failure this module exists to prevent: the tool rubs
//!    instead of cutting, which in FR4 dulls carbide and snaps small drills. So the
//!    spindle is brought **down** until the required feed fits the axis:
//!
//! ```text
//! rpm  = clamp(min(tool_rpm, feed_ceiling / k), cnc_min, cnc_max)
//! feed = k × rpm
//! ```
//!
//! Which ceiling applies depends on the moves the tool block makes — see [`Motion`].
//!
//! The two constraints can genuinely conflict: if `feed_ceiling / k` falls below the
//! spindle minimum, no speed both stays above the floor and keeps the feed within the
//! axis. There is no answer that preserves chip load, so the feed is capped at what the
//! machine can do (commanding more would be a program the machine cannot honour) and the
//! caller is told via [`Limited::Conflict`] that the chip load is *not* being met.
//!
//! Both tool values are **required**: a tool missing either cannot be run, and the
//! caller turns the [`FeedsError`] into a generation nogo (no silent default — a
//! `G81` with no `F` or an `M3` with no `S` is an unsafe program).
//!
//! Example: a bit rated 10000 mm/min @ 100000 rpm on a 20000-rpm spindle runs at
//! 20000 rpm and 2000 mm/min — one-fifth the feed for one-fifth the speed. Give the
//! machine a 1500 mm/min Z axis as well and it runs at 15000 rpm and 1500 mm/min, the
//! fastest plunge the axis can actually deliver at the rated chip load.

use units::{FeedRate, RotationalSpeed};

use super::routing::PLUNGE_FEED_FRACTION;

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

/// Everything about the machine that bounds a tool's running values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MachineLimits {
    pub spindle: SpindleRange,
    /// `machine.max_feed_xy` — the fastest lateral feed the machine can sustain.
    pub max_feed_xy: FeedRate,
    /// `machine.max_feed_z` — the fastest plunge. Usually the lower of the two, and the
    /// one that binds drilling, which is entirely Z motion.
    pub max_feed_z: FeedRate,
}

/// What a tool block does, which decides *which* axis ceiling binds it.
///
/// One value per block rather than per move, because a block commands its spindle speed
/// once (`start_spindle`) and every move in it then shares that speed. The binding
/// constraint is therefore the most restrictive across the moves the block will make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    /// Drill cycles: every move is a plunge at the full rated feed, so Z alone binds.
    Drilling,
    /// Routing: lateral passes at the rated feed (XY), entered by a plunge derated to
    /// [`PLUNGE_FEED_FRACTION`] of it (Z). Both bind, and the plunge's derating means a
    /// slow Z axis constrains the lateral feed only a third as hard.
    Routing,
}

/// The running values resolved for a tool on the machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedsSpeeds {
    pub rpm: RotationalSpeed,
    pub feed: FeedRate,
    /// Which machine limit, if any, moved the tool off its rated pair.
    pub limit: Limited,
}

/// Which constraint bound the result. Ordered by how much the operator should care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limited {
    /// Running exactly as rated.
    No,
    /// The spindle range moved the speed; the feed moved with it and chip load holds.
    Spindle,
    /// The axis feed ceiling forced the spindle below its rated speed; chip load holds,
    /// but the job is slower than the tool is capable of.
    Feed,
    /// The feed ceiling and the spindle minimum cannot both be satisfied. The feed is
    /// capped at the axis limit and **chip load is not met** — the tool is turning faster
    /// than its feed supports, which is the rubbing case.
    Conflict,
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

/// Relative tolerance for "these two rates are the same". Feeds and speeds are entered as
/// round numbers, so this only absorbs the float error of the divide-and-multiply below.
const EPS: f64 = 1e-9;

/// Resolves the running rpm/feed for a tool rated at (`tool_feed` @ `tool_rpm`) doing
/// `motion` on `limits`. See the module policy for the formula and its rationale.
pub fn resolve(
    tool_feed: Option<FeedRate>,
    tool_rpm: Option<RotationalSpeed>,
    limits: MachineLimits,
    motion: Motion,
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
        let (a, b) = (limits.spindle.min.as_rpm(), limits.spindle.max.as_rpm());
        if a <= b { (a, b) } else { (b, a) }
    };

    // Chip load: the mm of feed per spindle revolution the tool is rated for. Every
    // adjustment below holds this and moves the spindle.
    let chip_load = feed.as_mm_per_min() / rated_rpm;
    let ceiling = feed_ceiling(limits, motion);

    // The speed at which the rated chip load exactly fills the axis.
    let by_feed = if chip_load > 0.0 { ceiling / chip_load } else { f64::INFINITY };

    let target = rated_rpm.min(by_feed);
    let rpm = target.clamp(lo, hi);
    let at_chip_load = chip_load * rpm;

    // The spindle floor can hold the speed above what the axis can feed for. Chip load is
    // then unattainable; command what the machine can actually do and say so.
    let conflicted = at_chip_load > ceiling * (1.0 + EPS);
    let feed_out = if conflicted { ceiling } else { at_chip_load };

    let limit = if conflicted {
        Limited::Conflict
    } else if close(rpm, rated_rpm) {
        Limited::No
    } else if by_feed < rated_rpm && close(rpm, by_feed) {
        // The axis, not the spindle range, is what held the speed down.
        Limited::Feed
    } else {
        Limited::Spindle
    };

    Ok(FeedsSpeeds {
        rpm: RotationalSpeed::from_rpm(rpm),
        feed: FeedRate::from_mm_per_min(feed_out),
        limit,
    })
}

/// The fastest feed `motion` may be given on this machine, in mm/min.
///
/// Drilling is pure plunge, so Z binds it directly. A routing pass feeds laterally at the
/// full rate but *enters* at [`PLUNGE_FEED_FRACTION`] of it, so a Z limit of `z` permits a
/// lateral feed of `z / PLUNGE_FEED_FRACTION` — which is why the two axes are configured
/// separately: sharing one number would throttle routing to the plunge limit for nothing.
///
/// Returns `f64::INFINITY` for an axis whose limit is missing or non-positive. That is a
/// profile nobody has configured, not a machine that cannot move, and treating it as a
/// literal zero would cap every feed to nothing — which the caller would then have to
/// special-case in two more places.
fn feed_ceiling(limits: MachineLimits, motion: Motion) -> f64 {
    let usable = |feed: FeedRate| {
        let value = feed.as_mm_per_min();
        if value > 0.0 && value.is_finite() { value } else { f64::INFINITY }
    };
    match motion {
        Motion::Drilling => usable(limits.max_feed_z),
        Motion::Routing => {
            usable(limits.max_feed_xy).min(usable(limits.max_feed_z) / PLUNGE_FEED_FRACTION)
        }
    }
}

/// Equal within [`EPS`], relatively — the values here span 1e2..1e5.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= EPS * a.abs().max(b.abs()).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Limits with the axes set high enough not to bind, so a test can isolate the
    /// spindle range exactly as these tests did before the feed ceiling existed.
    fn range(min: f64, max: f64) -> MachineLimits {
        limits(min, max, 1e9, 1e9)
    }

    fn limits(rpm_min: f64, rpm_max: f64, xy: f64, z: f64) -> MachineLimits {
        MachineLimits {
            spindle: SpindleRange::new(
                RotationalSpeed::from_rpm(rpm_min),
                RotationalSpeed::from_rpm(rpm_max),
            ),
            max_feed_xy: FeedRate::from_mm_per_min(xy),
            max_feed_z: FeedRate::from_mm_per_min(z),
        }
    }

    #[test]
    fn within_range_passes_the_rated_values_through_unchanged() {
        let out = resolve(
            Some(FeedRate::from_mm_per_min(600.0)),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            range(5_000.0, 24_000.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 12_000.0);
        assert_eq!(out.feed.as_mm_per_min(), 600.0);
        assert_eq!(out.limit, Limited::No, "rated speed is reachable — nothing scaled");
    }

    #[test]
    fn clamping_down_scales_the_feed_by_the_same_ratio() {
        // The settled example: 10000 mm/min @ 100000 rpm on a 20000-rpm spindle.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(10_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            range(5_000.0, 20_000.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 20_000.0, "spindle capped at its max");
        assert_eq!(out.feed.as_mm_per_min(), 2_000.0, "feed scaled to one-fifth");
        assert_eq!(out.limit, Limited::Spindle);
    }

    #[test]
    fn a_bit_slower_than_the_spindle_minimum_is_raised_and_its_feed_scaled_up() {
        // Rated 1000 rpm but the spindle floors at 5000 → 5× speed, 5× feed.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(200.0)),
            Some(RotationalSpeed::from_rpm(1_000.0)),
            range(5_000.0, 24_000.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 5_000.0);
        assert_eq!(out.feed.as_mm_per_min(), 1_000.0);
        assert_eq!(out.limit, Limited::Spindle);
    }

    #[test]
    fn missing_tool_data_is_a_typed_error_not_a_default() {
        let r = range(5_000.0, 24_000.0);
        assert_eq!(
            resolve(None, Some(RotationalSpeed::from_rpm(12_000.0)), r, Motion::Drilling),
            Err(FeedsError::MissingFeed)
        );
        assert_eq!(
            resolve(Some(FeedRate::from_mm_per_min(600.0)), None, r, Motion::Drilling),
            Err(FeedsError::MissingSpeed)
        );
        assert_eq!(
            resolve(
                Some(FeedRate::from_mm_per_min(600.0)),
                Some(RotationalSpeed::from_rpm(0.0)),
                r,
                Motion::Drilling
            ),
            Err(FeedsError::NonPositiveSpeed)
        );
    }

    // --- the axis feed ceiling ---------------------------------------------

    /// The point of the whole exercise: the spindle comes *down* to fit the axis, so the
    /// chip load the tool is rated for is still delivered.
    #[test]
    fn a_feed_the_axis_cannot_sustain_lowers_the_spindle_rather_than_the_chip_load() {
        // 20000 mm/min @ 100000 rpm is 0.2 mm/rev. A 1500 mm/min Z can deliver that at
        // 7500 rpm and no faster.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(20_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(1_000.0, 100_000.0, 5_000.0, 1_500.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 7_500.0, "spindle brought down to fit the axis");
        assert_eq!(out.feed.as_mm_per_min(), 1_500.0, "feed exactly fills the Z axis");
        assert_eq!(out.limit, Limited::Feed);
        // The invariant the module exists for.
        let chip_load = out.feed.as_mm_per_min() / out.rpm.as_rpm();
        assert!((chip_load - 0.2).abs() < 1e-9, "chip load preserved: {chip_load}");
    }

    /// Drilling is all plunge, so Z binds it; routing's plunge is derated to a third, so
    /// the same Z permits three times the lateral feed. Sharing one limit would throttle
    /// routing for nothing — which is why the profile carries two.
    #[test]
    fn routing_is_bound_by_xy_and_by_three_times_the_z_limit() {
        let machine = limits(1_000.0, 100_000.0, 5_000.0, 1_500.0);
        let (feed, speed) =
            (Some(FeedRate::from_mm_per_min(20_000.0)), Some(RotationalSpeed::from_rpm(100_000.0)));

        // Z 1500 → a routing pass may feed 4500 laterally, but XY caps it at 5000... so
        // 4500 wins here.
        let out = resolve(feed, speed, machine, Motion::Routing).unwrap();
        assert_eq!(out.feed.as_mm_per_min(), 4_500.0, "3 × the Z limit, under the XY cap");
        assert_eq!(out.rpm.as_rpm(), 22_500.0);

        // Raise Z until XY is the binding one instead.
        let out = resolve(feed, speed, limits(1_000.0, 100_000.0, 5_000.0, 9_000.0), Motion::Routing)
            .unwrap();
        assert_eq!(out.feed.as_mm_per_min(), 5_000.0, "now XY caps it");
    }

    /// The unsolvable case: the spindle floor holds the speed above what the axis can
    /// feed for. Chip load cannot be met, so the feed is capped at what the machine can
    /// actually do — commanding more would be a program it cannot honour — and the caller
    /// is told the load is short.
    #[test]
    fn a_spindle_floor_above_the_axis_limit_is_reported_as_a_conflict() {
        // 0.2 mm/rev needs 7500 rpm for a 1500 mm/min axis, but the spindle will not run
        // below 10000.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(20_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(10_000.0, 100_000.0, 5_000.0, 1_500.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 10_000.0, "held at the spindle floor");
        assert_eq!(out.feed.as_mm_per_min(), 1_500.0, "capped at the axis, not commanded beyond it");
        assert_eq!(out.limit, Limited::Conflict);
        let chip_load = out.feed.as_mm_per_min() / out.rpm.as_rpm();
        assert!(chip_load < 0.2, "and the load really is short: {chip_load}");
    }

    /// Both constraints in play at once: whichever binds harder is the one reported, so
    /// the operator is told what to change.
    #[test]
    fn the_binding_constraint_is_the_one_reported() {
        let (feed, speed) =
            (Some(FeedRate::from_mm_per_min(10_000.0)), Some(RotationalSpeed::from_rpm(100_000.0)));

        // Spindle caps at 20000 (feed 2000); the axis would allow 5000 → spindle binds.
        let out = resolve(feed, speed, limits(1_000.0, 20_000.0, 5_000.0, 5_000.0), Motion::Drilling)
            .unwrap();
        assert_eq!(out.limit, Limited::Spindle);
        assert_eq!(out.feed.as_mm_per_min(), 2_000.0);

        // Same tool, faster spindle, slow axis → the axis binds.
        let out = resolve(feed, speed, limits(1_000.0, 100_000.0, 1_000.0, 1_000.0), Motion::Drilling)
            .unwrap();
        assert_eq!(out.limit, Limited::Feed);
        assert_eq!(out.feed.as_mm_per_min(), 1_000.0);
    }

    /// A profile with no usable limit must not silently produce a zero feed.
    #[test]
    fn a_zero_axis_limit_is_ignored_rather_than_stopping_the_machine() {
        let out = resolve(
            Some(FeedRate::from_mm_per_min(600.0)),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            limits(5_000.0, 24_000.0, 0.0, 0.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.feed.as_mm_per_min(), 600.0, "rated feed, not zero");
        assert_eq!(out.limit, Limited::No);
    }
}
