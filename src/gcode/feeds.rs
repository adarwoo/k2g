//! Feeds & speeds resolution for one tool on a specific machine.
//!
//! Policy (settled 2026-07-30). A tool's rated feed is defined **at its rated spindle
//! speed**, so the two are scaled together — but only the *spindle* ceiling scales them.
//! The axis feed ceiling stands alone:
//!
//! ```text
//! s = min(1, spindle_max / rated_rpm)   // never above 1: nothing is scaled up
//! S = rated_rpm  × s
//! F = rated_feed × s                    // the spindle clamp drags the feed with it
//! if F > feed_ceiling { F = feed_ceiling }   // the feed clamp does not drag the spindle
//! ```
//!
//! Which ceiling applies depends on the moves the tool block makes — see [`Motion`].
//!
//! **Why the two clamps are asymmetric.** A spindle the machine cannot reach means the
//! tool is turning slower than it was rated for, so the feed *must* come down with it or
//! the tool takes a bigger bite per revolution than it was designed to. An axis that
//! cannot feed fast enough is the other way round: it is perfectly fine to drill at
//! 60000 rpm with a slower feed. Chip load then falls below rated, which is a lighter
//! cut, not a dangerous one — and dropping the spindle to "fix" it would be an
//! unnecessary derate of a machine that was doing nothing wrong.
//!
//! This reverses the 2026-07-28 rule, which brought the spindle down to hold chip load
//! constant against the axis ceiling. Holding chip load is the right instinct for milling
//! and the wrong one for the drilling this application mostly does.
//!
//! **`spindle_rpm_min` is not part of the formula.** Nothing is scaled up, so a tool
//! rated below the machine's floor keeps its rated speed and feed. The floor is still
//! *reported* ([`Limited::SpindleFloor`]) — the machine will not turn that slowly, and
//! quietly raising the speed without raising the feed would be the rubbing case this
//! module exists to avoid.
//!
//! Both tool values are **required**: a tool missing either cannot be run, and the
//! caller turns the [`FeedsError`] into a generation nogo (no silent default — a
//! `G81` with no `F` or an `M3` with no `S` is an unsafe program).
//!
//! Example: a bit rated 10000 mm/min @ 100000 rpm on a 20000-rpm spindle runs at
//! 20000 rpm and 2000 mm/min — one-fifth the feed for one-fifth the speed. Give the
//! machine a 1500 mm/min Z axis as well and it still runs at 20000 rpm, with the feed
//! capped at 1500: the spindle keeps its speed and the cut is lighter.

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
    /// The rated speed is above the machine's maximum, so both the speed and the feed
    /// were scaled down by the same ratio. Chip load holds.
    Spindle,
    /// The axis could not sustain the feed, so the feed alone was capped. The spindle
    /// keeps its speed and the cut is **lighter than rated** — deliberate, per the module
    /// policy, but worth telling the operator since the job is slower than the tool is
    /// capable of.
    Feed,
    /// The rated speed is below the machine's minimum. Nothing was changed — the speed
    /// and feed are the tool's own — but the machine will not turn that slowly, so what it
    /// actually does is its own business and the pair is no longer the rated one.
    SpindleFloor,
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

    // Normalize the range so a misconfigured min>max cannot make the scale nonsense; a
    // CNC with an invalid spindle range is caught by its own usability check upstream.
    let (lo, hi) = {
        let (a, b) = (limits.spindle.min.as_rpm(), limits.spindle.max.as_rpm());
        if a <= b { (a, b) } else { (b, a) }
    };

    // `s = min(1, spindle_max / rated_rpm)` — capped at 1 so nothing is ever scaled *up*.
    //
    // An unset or nonsensical maximum means nobody has configured this machine, not a
    // spindle that cannot turn; scaling everything to zero for it would be worse than
    // ignoring it. Same treatment the axis ceilings get in `feed_ceiling`.
    let scale = if hi > 0.0 && hi.is_finite() { (hi / rated_rpm).min(1.0) } else { 1.0 };

    let rpm = rated_rpm * scale;
    // The spindle clamp drags the feed with it: at a lower speed the rated feed would be
    // a bigger bite per revolution than the tool was designed to take.
    let scaled_feed = feed.as_mm_per_min() * scale;

    // The feed clamp stands alone — it does **not** drag the spindle back down. Cutting
    // lighter than rated is fine; derating a spindle that was within its limits is not.
    let ceiling = feed_ceiling(limits, motion);
    let feed_capped = scaled_feed > ceiling * (1.0 + EPS);
    let feed_out = if feed_capped { ceiling } else { scaled_feed };

    // Reported worst-first: a speed the machine cannot turn at all outranks one that
    // merely cuts light, which outranks a clean proportional derate.
    let limit = if rpm < lo * (1.0 - EPS) {
        Limited::SpindleFloor
    } else if feed_capped {
        Limited::Feed
    } else if scale < 1.0 - EPS {
        Limited::Spindle
    } else {
        Limited::No
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
///
/// Test-only since 2026-07-30: `resolve` used to compare its result against the rated
/// values to work out *which* limit had bound it. The new formula knows that directly
/// from `scale` and the cap, so the only remaining caller is the property test that
/// re-derives the formula.
#[cfg(test)]
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

    /// Nothing is ever scaled **up**: `s = min(1, …)`. A tool rated below the machine's
    /// floor keeps its own rated pair, and the floor is reported rather than applied.
    ///
    /// Raising the speed without raising the feed would starve the cut; raising both (the
    /// pre-2026-07-30 behaviour) runs a tool 5× faster than its rating on the operator's
    /// behalf. Saying so and changing nothing is the honest answer.
    #[test]
    fn a_bit_rated_below_the_spindle_minimum_is_reported_not_raised() {
        let out = resolve(
            Some(FeedRate::from_mm_per_min(200.0)),
            Some(RotationalSpeed::from_rpm(1_000.0)),
            range(5_000.0, 24_000.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 1_000.0, "the tool's own rated speed, unchanged");
        assert_eq!(out.feed.as_mm_per_min(), 200.0, "and its own rated feed");
        assert_eq!(out.limit, Limited::SpindleFloor);
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

    /// **The feed clamp stands alone.** An axis that cannot keep up is no reason to derate
    /// a spindle that was well within its own limits — drilling at 100000 rpm with a
    /// slower feed is a lighter cut, not a fault.
    ///
    /// This is the reversal of the 2026-07-28 rule, which brought the spindle down to
    /// 7500 rpm here to hold chip load. It is the test that pins the new policy.
    #[test]
    fn a_feed_the_axis_cannot_sustain_caps_the_feed_and_leaves_the_spindle_alone() {
        // 20000 mm/min @ 100000 rpm on a machine whose Z tops out at 1500.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(20_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(1_000.0, 100_000.0, 5_000.0, 1_500.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 100_000.0, "the spindle can do it, so it keeps doing it");
        assert_eq!(out.feed.as_mm_per_min(), 1_500.0, "only the feed is capped");
        assert_eq!(out.limit, Limited::Feed);
        // Chip load is now deliberately *below* rated — the whole point of the change.
        let chip_load = out.feed.as_mm_per_min() / out.rpm.as_rpm();
        assert!(chip_load < 0.2, "a lighter cut, on purpose: {chip_load}");
    }

    /// The spindle clamp is the one that *does* drag the feed: at a lower speed the rated
    /// feed would be a bigger bite per revolution than the tool was designed to take. Both
    /// clamps at once, so their asymmetry is visible in one result.
    #[test]
    fn the_spindle_clamp_scales_the_feed_before_the_axis_caps_it() {
        // Rated 10000 @ 100000; the spindle caps at 20000 → s = 0.2, so F would be 2000.
        // The Z axis then caps that at 1200.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(10_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(1_000.0, 20_000.0, 5_000.0, 1_200.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 20_000.0, "scaled by the spindle ceiling only");
        assert_eq!(out.feed.as_mm_per_min(), 1_200.0, "scaled to 2000, then capped to 1200");
        assert_eq!(out.limit, Limited::Feed, "the axis is what the operator should hear about");
    }

    /// Drilling is all plunge, so Z binds it; routing's plunge is derated to a third, so
    /// the same Z permits three times the lateral feed. Sharing one limit would throttle
    /// routing for nothing — which is why the profile carries two.
    #[test]
    fn routing_is_bound_by_xy_and_by_three_times_the_z_limit() {
        let machine = limits(1_000.0, 100_000.0, 5_000.0, 1_500.0);
        let (feed, speed) =
            (Some(FeedRate::from_mm_per_min(20_000.0)), Some(RotationalSpeed::from_rpm(100_000.0)));

        // Z 1500 → a routing pass may feed 4500 laterally, under the 5000 XY cap, so 4500
        // is the binding ceiling. The spindle is untouched either way.
        let out = resolve(feed, speed, machine, Motion::Routing).unwrap();
        assert_eq!(out.feed.as_mm_per_min(), 4_500.0, "3 × the Z limit, under the XY cap");
        assert_eq!(out.rpm.as_rpm(), 100_000.0, "the feed cap does not touch the spindle");

        // Raise Z until XY is the binding one instead.
        let out = resolve(feed, speed, limits(1_000.0, 100_000.0, 5_000.0, 9_000.0), Motion::Routing)
            .unwrap();
        assert_eq!(out.feed.as_mm_per_min(), 5_000.0, "now XY caps it");
    }

    /// The case that used to be unsolvable is now simply the ordinary one: the spindle
    /// floor no longer participates in the arithmetic, so there is nothing left to
    /// conflict with. The speed is the tool's, the feed is the axis's.
    #[test]
    fn a_high_spindle_floor_no_longer_conflicts_with_the_axis() {
        let out = resolve(
            Some(FeedRate::from_mm_per_min(20_000.0)),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(10_000.0, 100_000.0, 5_000.0, 1_500.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 100_000.0, "rated, and well above the floor");
        assert_eq!(out.feed.as_mm_per_min(), 1_500.0, "capped at the axis");
        assert_eq!(out.limit, Limited::Feed);
    }

    /// Both constraints in play at once: the one reported is the one the operator can act
    /// on, worst first — a speed the machine cannot reach beats a cut that is merely
    /// light, which beats a clean proportional derate.
    #[test]
    fn the_binding_constraint_is_the_one_reported() {
        let (feed, speed) =
            (Some(FeedRate::from_mm_per_min(10_000.0)), Some(RotationalSpeed::from_rpm(100_000.0)));

        // Spindle caps at 20000 (feed 2000); the axis would allow 5000 → spindle alone.
        let out = resolve(feed, speed, limits(1_000.0, 20_000.0, 5_000.0, 5_000.0), Motion::Drilling)
            .unwrap();
        assert_eq!(out.limit, Limited::Spindle);
        assert_eq!(out.feed.as_mm_per_min(), 2_000.0);

        // Same tool, spindle fine, slow axis → the axis binds and only the feed moves.
        let out = resolve(feed, speed, limits(1_000.0, 100_000.0, 1_000.0, 1_000.0), Motion::Drilling)
            .unwrap();
        assert_eq!(out.limit, Limited::Feed);
        assert_eq!(out.feed.as_mm_per_min(), 1_000.0);
        assert_eq!(out.rpm.as_rpm(), 100_000.0);

        // A tool under the floor *and* over the axis: the floor is the louder problem.
        let out = resolve(
            Some(FeedRate::from_mm_per_min(10_000.0)),
            Some(RotationalSpeed::from_rpm(2_000.0)),
            limits(8_000.0, 100_000.0, 1_000.0, 1_000.0),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.limit, Limited::SpindleFloor);
    }

    /// An unconfigured spindle maximum must not scale everything to zero. Same reasoning
    /// as the axis limits: a profile nobody has filled in is not a machine that cannot
    /// turn.
    #[test]
    fn a_zero_spindle_maximum_is_ignored_rather_than_stopping_the_spindle() {
        let out = resolve(
            Some(FeedRate::from_mm_per_min(600.0)),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            limits(0.0, 0.0, 1e9, 1e9),
            Motion::Drilling,
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 12_000.0, "rated speed, not zero");
        assert_eq!(out.feed.as_mm_per_min(), 600.0);
        assert_eq!(out.limit, Limited::No);
    }

    /// The formula in one assertion, over the whole grid: `S = rated × s` and
    /// `F = min(rated_feed × s, ceiling)` with `s = min(1, max/rated)`. A property test
    /// rather than another example, so a future edit cannot satisfy the cases above while
    /// quietly breaking the rule they are drawn from.
    #[test]
    fn the_result_always_matches_the_formula() {
        for rated_rpm in [1_000.0, 12_000.0, 60_000.0, 100_000.0] {
            for rated_feed in [200.0, 600.0, 10_000.0] {
                for spindle_max in [5_000.0, 24_000.0, 100_000.0] {
                    for z in [500.0, 1_500.0, 1e9] {
                        let out = resolve(
                            Some(FeedRate::from_mm_per_min(rated_feed)),
                            Some(RotationalSpeed::from_rpm(rated_rpm)),
                            limits(1_000.0, spindle_max, 1e9, z),
                            Motion::Drilling,
                        )
                        .unwrap();

                        let s = (spindle_max / rated_rpm).min(1.0);
                        let expected_rpm = rated_rpm * s;
                        let expected_feed = (rated_feed * s).min(z);

                        assert!(
                            close(out.rpm.as_rpm(), expected_rpm),
                            "S: rated {rated_rpm} max {spindle_max} -> {} want {expected_rpm}",
                            out.rpm.as_rpm()
                        );
                        assert!(
                            close(out.feed.as_mm_per_min(), expected_feed),
                            "F: rated {rated_feed} s {s} z {z} -> {} want {expected_feed}",
                            out.feed.as_mm_per_min()
                        );
                    }
                }
            }
        }
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

