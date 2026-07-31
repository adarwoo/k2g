//! Feeds & speeds resolution for one tool on a specific machine.
//!
//! **A tool has two rated feeds, not one.** `table_feed` is what it cuts at laterally;
//! `z_feed` is what it plunges at. They are different numbers because they are different
//! moves: a straight plunge engages the tool's weak end-cutting geometry over its full
//! diameter at once, where a lateral pass engages its flutes. Catalogues state both.
//!
//! k2g used to carry one — `table_feed.or(z_feed)` — and derive the plunge from it as a
//! fixed third. That threw away a number the catalogue had already given, and it meant a
//! drill cycle (which is *entirely* plunge) ran at the lateral rate.
//!
//! Policy (settled 2026-08-01). Both rated feeds are defined **at the tool's rated spindle
//! speed**, so the spindle clamp scales both. Each axis ceiling then caps its own feed and
//! nothing else:
//!
//! ```text
//! s = min(1, spindle_max / rated_rpm)   // never above 1: nothing is scaled up
//! S = rated_rpm    × s
//! F_xy = table_feed × s   , capped at max_feed_xy
//! F_z  = z_feed     × s   , capped at max_feed_z
//! ```
//!
//! There is no longer a `Motion` discriminant deciding *which* ceiling applies to a single
//! feed. Each feed has an axis, and that axis's ceiling is the one that binds it — which
//! is what the two limits meant all along.
//!
//! **Why the spindle and feed clamps are asymmetric.** A spindle the machine cannot reach
//! means the tool is turning slower than it was rated for, so the feeds *must* come down
//! with it or the tool takes a bigger bite per revolution than it was designed to. An axis
//! that cannot feed fast enough is the other way round: it is perfectly fine to drill at
//! 60000 rpm with a slower feed. Chip load then falls below rated, which is a lighter cut,
//! not a dangerous one — and dropping the spindle to "fix" it would be an unnecessary
//! derate of a machine that was doing nothing wrong.
//!
//! **`spindle_rpm_min` is not part of the formula.** Nothing is scaled up, so a tool rated
//! below the machine's floor keeps its rated speed and feeds. The floor is still *reported*
//! ([`Limited::SpindleFloor`]) — the machine will not turn that slowly, and quietly raising
//! the speed without raising the feed would be the rubbing case this module exists to
//! avoid.
//!
//! A rated **spindle speed and at least one feed** are required: a tool missing them cannot
//! be run, and the caller turns the [`FeedsError`] into a generation nogo (no silent
//! default — a `G81` with no `F` or an `M3` with no `S` is an unsafe program).
//!
//! Example: a bit rated 1100 mm/min laterally and 400 mm/min in Z @ 30000 rpm, on a
//! 15000-rpm spindle, runs at 15000 rpm cutting at 550 and plunging at 200 — half of each,
//! for half the speed. Give the machine a 150 mm/min Z axis as well and it still runs at
//! 15000 rpm cutting at 550, with the plunge alone capped at 150.

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
    /// `machine.max_feed_xy` — the fastest lateral feed the machine can sustain. Caps the
    /// tool's `table_feed`.
    pub max_feed_xy: FeedRate,
    /// `machine.max_feed_z` — the fastest plunge. Caps the tool's `z_feed`, and so binds
    /// drilling, which is entirely Z motion.
    pub max_feed_z: FeedRate,
}

/// A tool's two rated feeds, as the catalogue states them.
///
/// Both optional because a catalogue may state only one. [`Self::for_tool`] is what fills
/// the gap, and it needs to know what the tool is; [`resolve`] falls one back to the other
/// plainly.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RatedFeeds {
    /// Lateral (XY) cutting feed.
    pub table: Option<FeedRate>,
    /// Plunge (Z-only) feed.
    pub z: Option<FeedRate>,
}

impl RatedFeeds {
    /// Both values as stated. [`Self::for_tool`] is this plus the rule for filling in a
    /// plunge rating the catalogue omitted.
    pub fn new(table: Option<FeedRate>, z: Option<FeedRate>) -> Self {
        Self { table, z }
    }

    /// A tool's rated feeds, with the plunge filled in when the catalogue states only one.
    ///
    /// `mills` is what decides the fallback, and the two answers are opposite:
    ///
    /// - A **router or end mill** cuts with its flutes and only incidentally plunges. Its
    ///   single quoted feed is the lateral one, and driving it into FR4 at that rate on its
    ///   weak end geometry snaps it — so the plunge falls back to
    ///   [`PLUNGE_FEED_FRACTION`] of it, the conventional derating for a straight
    ///   (non-ramped) plunge.
    /// - A **drill** does nothing but plunge. Its single quoted feed *is* its plunge feed,
    ///   and derating it would triple the time every hole takes for no reason at all.
    ///
    /// Which is why this takes the tool rather than living inside [`resolve`]: the rule is
    /// about what the tool is, and the solve has no way to know.
    pub fn for_tool(table: Option<FeedRate>, z: Option<FeedRate>, mills: bool) -> Self {
        let plunge = z.or_else(|| {
            table.map(|feed| {
                let rate = if mills { feed.as_mm_per_min() * PLUNGE_FEED_FRACTION } else { feed.as_mm_per_min() };
                FeedRate::from_mm_per_min(rate)
            })
        });
        Self::new(table, plunge)
    }
}

/// The running values resolved for a tool on the machine.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FeedsSpeeds {
    pub rpm: RotationalSpeed,
    /// What a lateral (XY) cut runs at.
    pub table: FeedRate,
    /// What a plunge (Z-only) move runs at — including a whole drill cycle.
    pub z: FeedRate,
    /// Which machine limit, if any, moved the tool off its rated values.
    pub limit: Limited,
}

/// Which constraint bound the result. Ordered by how much the operator should care.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limited {
    /// Running exactly as rated.
    No,
    /// The rated speed is above the machine's maximum, so the speed and **both** feeds
    /// were scaled down by the same ratio. Chip load holds.
    Spindle,
    /// An axis could not sustain its feed, so that feed alone was capped. The spindle
    /// keeps its speed and the cut is **lighter than rated** — deliberate, per the module
    /// policy, but worth telling the operator since the job is slower than the tool is
    /// capable of.
    Feed,
    /// The rated speed is below the machine's minimum. Nothing was changed — the speed and
    /// feeds are the tool's own — but the machine will not turn that slowly, so what it
    /// actually does is its own business and the values are no longer the rated ones.
    SpindleFloor,
}

/// Why a tool cannot be given running values. Each variant is an actionable "fix the stock
/// entry" message for the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedsError {
    /// The tool has neither a lateral nor a plunge feed rating.
    MissingFeed,
    /// The tool has no rated spindle speed.
    MissingSpeed,
    /// The rated spindle speed is not positive, so a feed ratio cannot be formed.
    NonPositiveSpeed,
}

/// Relative tolerance for "these two rates are the same". Feeds and speeds are entered as
/// round numbers, so this only absorbs the float error of the divide-and-multiply below.
const EPS: f64 = 1e-9;

/// Resolves the running rpm and both feeds for a tool rated at `feeds` @ `tool_rpm` on
/// `limits`. See the module policy for the formula and its rationale.
pub fn resolve(
    feeds: RatedFeeds,
    tool_rpm: Option<RotationalSpeed>,
    limits: MachineLimits,
) -> Result<FeedsSpeeds, FeedsError> {
    // A catalogue that states only one feed has not distinguished them, so the one it does
    // state stands in. Note this is a *plain* fallback: the tool-kind-dependent derating
    // belongs to [`RatedFeeds::for_tool`], which is the only place that knows whether a
    // missing plunge rating should be a third of the lateral one or equal to it.
    let table = feeds.table.or(feeds.z).ok_or(FeedsError::MissingFeed)?;
    let z = feeds.z.or(feeds.table).ok_or(FeedsError::MissingFeed)?;

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
    // ignoring it. Same treatment the axis ceilings get in `ceiling`.
    let scale = if hi > 0.0 && hi.is_finite() { (hi / rated_rpm).min(1.0) } else { 1.0 };
    let rpm = rated_rpm * scale;

    // Each feed is scaled by the spindle clamp, then capped by *its own* axis. The feed
    // clamp stands alone — it does **not** drag the spindle back down. Cutting lighter
    // than rated is fine; derating a spindle that was within its limits is not.
    let (table_out, table_capped) = clamp(table.as_mm_per_min() * scale, ceiling(limits.max_feed_xy));
    let (z_out, z_capped) = clamp(z.as_mm_per_min() * scale, ceiling(limits.max_feed_z));

    // Reported worst-first: a speed the machine cannot turn at all outranks one that
    // merely cuts light, which outranks a clean proportional derate.
    let limit = if rpm < lo * (1.0 - EPS) {
        Limited::SpindleFloor
    } else if table_capped || z_capped {
        Limited::Feed
    } else if scale < 1.0 - EPS {
        Limited::Spindle
    } else {
        Limited::No
    };

    Ok(FeedsSpeeds {
        rpm: RotationalSpeed::from_rpm(rpm),
        table: FeedRate::from_mm_per_min(table_out),
        z: FeedRate::from_mm_per_min(z_out),
        limit,
    })
}

/// `value` held to `ceiling`, and whether the ceiling bit.
fn clamp(value: f64, ceiling: f64) -> (f64, bool) {
    if value > ceiling * (1.0 + EPS) { (ceiling, true) } else { (value, false) }
}

/// An axis limit as a usable ceiling, in mm/min.
///
/// Returns `f64::INFINITY` for an axis whose limit is missing or non-positive. That is a
/// profile nobody has configured, not a machine that cannot move, and treating it as a
/// literal zero would cap every feed to nothing.
fn ceiling(limit: FeedRate) -> f64 {
    let value = limit.as_mm_per_min();
    if value > 0.0 && value.is_finite() { value } else { f64::INFINITY }
}

/// Equal within [`EPS`], relatively — the values here span 1e2..1e5.
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
        let out = resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(600.0)), None),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            range(5_000.0, 24_000.0))
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 12_000.0);
        assert_eq!(out.table.as_mm_per_min(), 600.0);
        assert_eq!(out.limit, Limited::No, "rated speed is reachable — nothing scaled");
    }

    #[test]
    fn clamping_down_scales_the_feed_by_the_same_ratio() {
        // The settled example: 10000 mm/min @ 100000 rpm on a 20000-rpm spindle.
        let out = resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(10_000.0)), None),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            range(5_000.0, 20_000.0))
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 20_000.0, "spindle capped at its max");
        assert_eq!(out.table.as_mm_per_min(), 2_000.0, "feed scaled to one-fifth");
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
        let out = resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(200.0)), None),
            Some(RotationalSpeed::from_rpm(1_000.0)),
            range(5_000.0, 24_000.0))
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 1_000.0, "the tool's own rated speed, unchanged");
        assert_eq!(out.table.as_mm_per_min(), 200.0, "and its own rated feed");
        assert_eq!(out.limit, Limited::SpindleFloor);
    }

    #[test]
    fn missing_tool_data_is_a_typed_error_not_a_default() {
        let r = range(5_000.0, 24_000.0);
        assert_eq!(
            resolve(RatedFeeds::new(None, None), Some(RotationalSpeed::from_rpm(12_000.0)), r),
            Err(FeedsError::MissingFeed)
        );
        assert_eq!(
            resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(600.0)), None), None, r),
            Err(FeedsError::MissingSpeed)
        );
        assert_eq!(
            resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(600.0)), None),
                Some(RotationalSpeed::from_rpm(0.0)),
                r),
            Err(FeedsError::NonPositiveSpeed)
        );
    }

    // --- the axis feed ceiling ---------------------------------------------

    /// **The feed clamp stands alone.** An axis that cannot keep up is no reason to derate
    /// a spindle that was well within its own limits — drilling at 100000 rpm with a
    /// slower feed is a lighter cut, not a fault.
    ///
    /// This is the reversal of the 2026-07-28 rule, which brought the spindle down to
    /// 7500 rpm here to hold chip load. It is the test that pins the policy.
    #[test]
    fn a_feed_the_axis_cannot_sustain_caps_the_feed_and_leaves_the_spindle_alone() {
        // 20000 mm/min laterally @ 100000 rpm on a machine whose XY tops out at 1500.
        let out = resolve(
            RatedFeeds::new(Some(FeedRate::from_mm_per_min(20_000.0)), None),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(1_000.0, 100_000.0, 1_500.0, 5_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 100_000.0, "the spindle can do it, so it keeps doing it");
        assert_eq!(out.table.as_mm_per_min(), 1_500.0, "only the feed is capped");
        assert_eq!(out.limit, Limited::Feed);
        // Chip load is now deliberately *below* rated — the whole point of the change.
        let chip_load = out.table.as_mm_per_min() / out.rpm.as_rpm();
        assert!(chip_load < 0.2, "a lighter cut, on purpose: {chip_load}");
    }

    /// The spindle clamp is the one that *does* drag the feeds: at a lower speed the rated
    /// feed would be a bigger bite per revolution than the tool was designed to take. Both
    /// clamps at once, so their asymmetry is visible in one result.
    #[test]
    fn the_spindle_clamp_scales_the_feed_before_the_axis_caps_it() {
        // Rated 10000 @ 100000; the spindle caps at 20000 -> s = 0.2, so F would be 2000.
        // The XY axis then caps that at 1200.
        let out = resolve(
            RatedFeeds::new(Some(FeedRate::from_mm_per_min(10_000.0)), None),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(1_000.0, 20_000.0, 1_200.0, 5_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 20_000.0, "scaled by the spindle ceiling only");
        assert_eq!(out.table.as_mm_per_min(), 1_200.0, "scaled to 2000, then capped to 1200");
        assert_eq!(out.limit, Limited::Feed, "the axis is what the operator should hear about");
    }

    /// **Each feed is bound by its own axis, and by no other.**
    ///
    /// This replaced a `Motion` discriminant that decided which single ceiling applied to
    /// a single feed — and which, for routing, had to reach for `max_feed_z / (1/3)` to
    /// undo a derating it had itself applied. Two rated feeds make that arithmetic
    /// unnecessary: XY caps the lateral one, Z caps the plunge, and neither says anything
    /// about the other.
    #[test]
    fn each_feed_is_capped_by_its_own_axis() {
        // Rated 4000 laterally, 900 in Z. A machine with plenty of XY and a crawling Z.
        let out = resolve(
            RatedFeeds::new(
                Some(FeedRate::from_mm_per_min(4_000.0)),
                Some(FeedRate::from_mm_per_min(900.0)),
            ),
            Some(RotationalSpeed::from_rpm(20_000.0)),
            limits(1_000.0, 20_000.0, 5_000.0, 400.0),
        )
        .unwrap();
        assert_eq!(out.table.as_mm_per_min(), 4_000.0, "XY is roomy, so the cut is unchanged");
        assert_eq!(out.z.as_mm_per_min(), 400.0, "Z alone caps the plunge");
        assert_eq!(out.limit, Limited::Feed);

        // And the other way round: a slow XY must not drag the plunge down with it.
        let out = resolve(
            RatedFeeds::new(
                Some(FeedRate::from_mm_per_min(4_000.0)),
                Some(FeedRate::from_mm_per_min(900.0)),
            ),
            Some(RotationalSpeed::from_rpm(20_000.0)),
            limits(1_000.0, 20_000.0, 1_000.0, 5_000.0),
        )
        .unwrap();
        assert_eq!(out.table.as_mm_per_min(), 1_000.0, "XY caps the cut");
        assert_eq!(out.z.as_mm_per_min(), 900.0, "the plunge keeps its own rating");
    }

    /// The spindle clamp scales **both** feeds by the same ratio, because both are rated
    /// at the same spindle speed. Scaling one and not the other would change the
    /// relationship between them, which is the tool's, not the machine's.
    #[test]
    fn the_spindle_clamp_scales_both_feeds_together() {
        let out = resolve(
            RatedFeeds::new(
                Some(FeedRate::from_mm_per_min(1_100.0)),
                Some(FeedRate::from_mm_per_min(400.0)),
            ),
            Some(RotationalSpeed::from_rpm(30_000.0)),
            limits(1_000.0, 15_000.0, 5_000.0, 5_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 15_000.0);
        assert_eq!(out.table.as_mm_per_min(), 550.0, "half the speed, half the cut");
        assert_eq!(out.z.as_mm_per_min(), 200.0, "and half the plunge");
        assert_eq!(out.limit, Limited::Spindle);
    }

    /// **A drill and a router fill in a missing plunge rating in opposite directions.**
    ///
    /// A router's single quoted feed is its lateral one, and plunging at it on the tool's
    /// weak end geometry snaps it. A drill does nothing but plunge, so its single quoted
    /// feed *is* the plunge feed — derating it would triple every hole for no reason.
    #[test]
    fn a_missing_plunge_rating_is_filled_in_by_what_the_tool_is() {
        let rated = Some(FeedRate::from_mm_per_min(1_200.0));

        let router = RatedFeeds::for_tool(rated, None, true);
        assert_eq!(router.z.unwrap().as_mm_per_min(), 400.0, "a third, for a straight plunge");

        let drill = RatedFeeds::for_tool(rated, None, false);
        assert_eq!(drill.z.unwrap().as_mm_per_min(), 1_200.0, "its rated feed IS its plunge");

        // A stated rating always wins over either fallback.
        let stated = RatedFeeds::for_tool(rated, Some(FeedRate::from_mm_per_min(900.0)), true);
        assert_eq!(stated.z.unwrap().as_mm_per_min(), 900.0, "the catalogue said so");
    }

    /// The case that used to be unsolvable is now simply the ordinary one: the spindle
    /// floor no longer participates in the arithmetic, so there is nothing left to
    /// conflict with. The speed is the tool's, the feed is the axis's.
    #[test]
    fn a_high_spindle_floor_no_longer_conflicts_with_the_axis() {
        let out = resolve(
            RatedFeeds::new(Some(FeedRate::from_mm_per_min(20_000.0)), None),
            Some(RotationalSpeed::from_rpm(100_000.0)),
            limits(10_000.0, 100_000.0, 1_500.0, 5_000.0),
        )
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 100_000.0, "rated, and well above the floor");
        assert_eq!(out.table.as_mm_per_min(), 1_500.0, "capped at the axis");
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
        let out = resolve(RatedFeeds::new(feed, None), speed, limits(1_000.0, 20_000.0, 5_000.0, 5_000.0))
            .unwrap();
        assert_eq!(out.limit, Limited::Spindle);
        assert_eq!(out.table.as_mm_per_min(), 2_000.0);

        // Same tool, spindle fine, slow axis → the axis binds and only the feed moves.
        let out = resolve(RatedFeeds::new(feed, None), speed, limits(1_000.0, 100_000.0, 1_000.0, 1_000.0))
            .unwrap();
        assert_eq!(out.limit, Limited::Feed);
        assert_eq!(out.table.as_mm_per_min(), 1_000.0);
        assert_eq!(out.rpm.as_rpm(), 100_000.0);

        // A tool under the floor *and* over the axis: the floor is the louder problem.
        let out = resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(10_000.0)), None),
            Some(RotationalSpeed::from_rpm(2_000.0)),
            limits(8_000.0, 100_000.0, 1_000.0, 1_000.0))
        .unwrap();
        assert_eq!(out.limit, Limited::SpindleFloor);
    }

    /// An unconfigured spindle maximum must not scale everything to zero. Same reasoning
    /// as the axis limits: a profile nobody has filled in is not a machine that cannot
    /// turn.
    #[test]
    fn a_zero_spindle_maximum_is_ignored_rather_than_stopping_the_spindle() {
        let out = resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(600.0)), None),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            limits(0.0, 0.0, 1e9, 1e9))
        .unwrap();
        assert_eq!(out.rpm.as_rpm(), 12_000.0, "rated speed, not zero");
        assert_eq!(out.table.as_mm_per_min(), 600.0);
        assert_eq!(out.limit, Limited::No);
    }

    /// The formula in one assertion, over the whole grid: `S = rated × s`,
    /// `F_xy = min(table × s, max_feed_xy)` and `F_z = min(z × s, max_feed_z)` with
    /// `s = min(1, max/rated)`. A property test rather than another example, so a future
    /// edit cannot satisfy the cases above while quietly breaking the rule they are drawn
    /// from — and it is what pins the two feeds to *separate* ceilings across the whole
    /// grid rather than at the handful of points an example test visits.
    #[test]
    fn the_result_always_matches_the_formula() {
        for rated_rpm in [1_000.0, 12_000.0, 60_000.0, 100_000.0] {
            for (rated_table, rated_z) in [(200.0, 80.0), (600.0, 600.0), (10_000.0, 2_500.0)] {
                for spindle_max in [5_000.0, 24_000.0, 100_000.0] {
                    for (xy, z) in [(500.0, 500.0), (5_000.0, 1_500.0), (1e9, 1e9)] {
                        let out = resolve(
                            RatedFeeds::new(
                                Some(FeedRate::from_mm_per_min(rated_table)),
                                Some(FeedRate::from_mm_per_min(rated_z)),
                            ),
                            Some(RotationalSpeed::from_rpm(rated_rpm)),
                            limits(1_000.0, spindle_max, xy, z),
                        )
                        .unwrap();

                        let s = (spindle_max / rated_rpm).min(1.0);
                        let expected_rpm = rated_rpm * s;
                        let expected_table = (rated_table * s).min(xy);
                        let expected_z = (rated_z * s).min(z);

                        assert!(
                            close(out.rpm.as_rpm(), expected_rpm),
                            "S: rated {rated_rpm} max {spindle_max} -> {} want {expected_rpm}",
                            out.rpm.as_rpm()
                        );
                        assert!(
                            close(out.table.as_mm_per_min(), expected_table),
                            "F_xy: rated {rated_table} s {s} xy {xy} -> {} want {expected_table}",
                            out.table.as_mm_per_min()
                        );
                        assert!(
                            close(out.z.as_mm_per_min(), expected_z),
                            "F_z: rated {rated_z} s {s} z {z} -> {} want {expected_z}",
                            out.z.as_mm_per_min()
                        );
                    }
                }
            }
        }
    }

    /// A profile with no usable limit must not silently produce a zero feed.
    #[test]
    fn a_zero_axis_limit_is_ignored_rather_than_stopping_the_machine() {
        let out = resolve(RatedFeeds::new(Some(FeedRate::from_mm_per_min(600.0)), None),
            Some(RotationalSpeed::from_rpm(12_000.0)),
            limits(5_000.0, 24_000.0, 0.0, 0.0))
        .unwrap();
        assert_eq!(out.table.as_mm_per_min(), 600.0, "rated feed, not zero");
        assert_eq!(out.limit, Limited::No);
    }
}

