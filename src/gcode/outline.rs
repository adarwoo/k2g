//! Board-outline toolpath geometry — turning an offset contour into the spans a router
//! actually cuts, with retaining tabs left uncut (operation-planner.md §3, `route_board`).
//!
//! The offset itself belongs to the `pcb` crate ([`pcb::routing_offset`]), which puts the
//! kerf on the waste side of each contour. What is left is the retention problem: a board
//! cut all the way round its outline is loose before the last millimetre of that cut, and
//! a loose board under a spinning cutter is scrap at best. So the loop is cut in **spans**
//! with short **tabs** of material left between them, and the operator snaps the finished
//! board out.
//!
//! Everything here works in **arc length along the loop**, normalised to `[0, 1)`:
//!
//! - it is how a tab is stored ([`crate::data::model::EdgeTab`]), so a tab survives the
//!   board being moved around the KiCad layout;
//! - it makes "spread N tabs evenly" a division rather than a geometry problem;
//! - and it is measured on the **offset** path, so a tab is the width the operator asked
//!   for at the place the cutter actually passes.
//!
//! A tab is a *gap in the cut*, not a feature added to it: [`cut_spans`] returns the parts
//! of the loop that are cut, and the gaps between them are the tabs. That framing is what
//! keeps the mouse-bite case from needing its own path — a mouse bite perforates the same
//! gap with drills, which are planned in the drill phase like any other hole.

use units::Length;

use crate::gcode::plan::Point;

/// A tab narrower than this (mm) is not a tab — it is a rounding artefact, and cutting
/// right up to it would leave a whisker rather than something to snap.
const MIN_TAB_MM: f64 = 0.05;

/// Clear space, as a multiple of the tab width, that must separate a tab from a corner
/// and from the next tab.
///
/// A tab too near a corner is a tab the operator cannot snap without cracking the corner
/// off with it, and two tabs too close together behave as one wide one. Three widths is
/// enough room to get a pair of cutters in on either side. Like the other geometry
/// constants here it is fixed rather than exposed: it follows from the operation.
const TAB_CLEARANCE_WIDTHS: f64 = 3.0;

/// One tab's computed home on the outline — where it sits before the job applies its
/// own offset.
///
/// Identified by `(segment, t)` rather than by a bare position so the job can key a
/// per-tab offset to it and have that offset survive the board changing shape: the tab
/// stays "the second one along the long edge", not "the one at 0.34 of the perimeter".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabAnchor {
    /// Index into the segments the anchor was distributed over.
    pub segment: usize,
    /// Where along that segment it sits, in `0..1`.
    pub t: f64,
    /// The point itself, in the segments' own coordinate space.
    pub point: (f64, f64),
}

/// How many tabs a segment of `length_mm` can hold at `tab_width_mm`.
///
/// `k` tabs need `k` widths of material plus `k + 1` clear gaps between them and the two
/// corners, so `W(4k + 3) ≤ L` at the default clearance. A segment shorter than seven tab
/// widths holds none — which is the right answer for the short side of a narrow board.
fn segment_capacity(length_mm: f64, tab_width_mm: f64) -> usize {
    if tab_width_mm <= 0.0 {
        return 0;
    }
    let k = (length_mm / tab_width_mm - TAB_CLEARANCE_WIDTHS) / (1.0 + TAB_CLEARANCE_WIDTHS);
    if k < 1.0 {
        0
    } else {
        k.floor() as usize
    }
}

/// Distributes `count` tabs over the outline's straight `segments`, longest first.
///
/// The rule, in order:
///
/// 1. **Share them out by segment count**, giving the remainder to the longest segments —
///    5 tabs over a triangle's 3 sides is 2, 2, 1, and 2 tabs is 1, 1, 0. Spreading tabs
///    across sides rather than crowding one is what actually holds a board square.
/// 2. **Clamp to what each segment can hold** ([`segment_capacity`]), and hand any tabs
///    that no longer fit back to the longest segment with room left. A board with one
///    long side and three stubs ends up with its tabs on the long side rather than
///    losing them.
/// 3. **Space them evenly within the segment**, including the gaps to its two corners, so
///    a single tab lands at the midpoint and `k` tabs split the side into `k + 1` equal
///    gaps. That spacing satisfies the clearance rule exactly at capacity, which is why
///    the two rules are stated with the same constant.
///
/// Returns fewer anchors than asked for when the outline genuinely cannot hold them; the
/// caller reports the shortfall rather than crowding the tabs together. Anchors come back
/// in **contour order** (segment, then position along it) so a job's per-tab offsets keep
/// a stable identity.
///
/// Only straight segments take part — curved ones are simply not passed in. A tab on an
/// arc is a snapping-off point the operator has to cut on a curve, and the offset path
/// through a corner is not a straight line to project onto.
pub fn distribute_tabs(
    segments: &[(f64, f64, f64, f64)],
    count: usize,
    tab_width_mm: f64,
) -> Vec<TabAnchor> {
    if count == 0 || segments.is_empty() || tab_width_mm < MIN_TAB_MM {
        return Vec::new();
    }

    let length = |&(x0, y0, x1, y1): &(f64, f64, f64, f64)| ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
    let capacity: Vec<usize> =
        segments.iter().map(|s| segment_capacity(length(s), tab_width_mm)).collect();

    // Longest first; ties on the lower index so the result is a total function of the
    // input (op-planner §8: no hash order, no clock, no RNG).
    let mut by_length: Vec<usize> = (0..segments.len()).collect();
    by_length.sort_by(|&a, &b| {
        length(&segments[b])
            .partial_cmp(&length(&segments[a]))
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });

    // 1. Even share, remainder to the longest.
    let base = count / segments.len();
    let remainder = count % segments.len();
    let mut assigned = vec![0usize; segments.len()];
    for (rank, &idx) in by_length.iter().enumerate() {
        assigned[idx] = base + usize::from(rank < remainder);
    }

    // 2. Clamp to capacity, then re-place what would not fit.
    let mut homeless = 0usize;
    for idx in 0..segments.len() {
        if assigned[idx] > capacity[idx] {
            homeless += assigned[idx] - capacity[idx];
            assigned[idx] = capacity[idx];
        }
    }
    for &idx in &by_length {
        if homeless == 0 {
            break;
        }
        let room = capacity[idx] - assigned[idx];
        let take = room.min(homeless);
        assigned[idx] += take;
        homeless -= take;
    }

    // 3. Even spacing within each segment, in contour order.
    let mut anchors = Vec::with_capacity(count - homeless);
    for (idx, &(x0, y0, x1, y1)) in segments.iter().enumerate() {
        let k = assigned[idx];
        for i in 0..k {
            let t = (i + 1) as f64 / (k + 1) as f64;
            anchors.push(TabAnchor {
                segment: idx,
                t,
                point: (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t),
            });
        }
    }
    anchors
}

/// A closed toolpath loop with its arc-length parameterisation precomputed.
///
/// Built once per contour so every tab lookup is a binary search rather than another walk
/// of the polyline.
pub struct Loop {
    /// The loop's vertices, first point repeated at the end so the closing edge is a
    /// segment like any other.
    points: Vec<Point>,
    /// Cumulative distance to each point; `cumulative[0] == 0`, last == total length.
    cumulative: Vec<f64>,
}

impl Loop {
    /// Builds a loop from an already-offset, already-placed closed path. `points` must
    /// **not** repeat its first vertex — the closure is added here.
    ///
    /// Returns `None` for anything that is not a loop (fewer than three vertices, or zero
    /// length): there is nothing to cut and nothing to hang a tab on.
    pub fn new(points: &[Point]) -> Option<Self> {
        if points.len() < 3 {
            return None;
        }
        let mut closed: Vec<Point> = points.to_vec();
        closed.push(points[0]);

        let mut cumulative = Vec::with_capacity(closed.len());
        let mut total = 0.0;
        cumulative.push(0.0);
        for pair in closed.windows(2) {
            total += pair[0].distance_mm(&pair[1]);
            cumulative.push(total);
        }
        if total <= 0.0 {
            return None;
        }
        Some(Self { points: closed, cumulative })
    }

    /// Total length of the loop, in millimetres.
    pub fn length_mm(&self) -> f64 {
        self.cumulative[self.cumulative.len() - 1]
    }

    /// The point at `distance` millimetres along the loop from its start, wrapping.
    pub fn point_at(&self, distance: f64) -> Point {
        let total = self.length_mm();
        let target = distance.rem_euclid(total);
        // The first vertex at or past the target; the point lies on the edge before it.
        let idx = self
            .cumulative
            .partition_point(|&d| d < target)
            .clamp(1, self.points.len() - 1);
        let (from, to) = (self.points[idx - 1], self.points[idx]);
        let span = self.cumulative[idx] - self.cumulative[idx - 1];
        let t = if span > 0.0 { (target - self.cumulative[idx - 1]) / span } else { 0.0 };
        Point::new(
            Length::from_mm(from.x.as_mm() + (to.x.as_mm() - from.x.as_mm()) * t),
            Length::from_mm(from.y.as_mm() + (to.y.as_mm() - from.y.as_mm()) * t),
        )
    }

    /// The fraction along the loop of the point on it nearest `probe` — the inverse of
    /// [`Self::point_at`], and what turns a click in the board view into a stored tab.
    ///
    /// Unused until the board view can place tabs by clicking; it is here (and tested)
    /// because it is the other half of the parameterisation and belongs beside it.
    #[allow(dead_code)]
    pub fn nearest_fraction(&self, probe: Point) -> f64 {
        let mut best = (f64::INFINITY, 0.0);
        for idx in 1..self.points.len() {
            let (from, to) = (self.points[idx - 1], self.points[idx]);
            let (dx, dy) = (to.x.as_mm() - from.x.as_mm(), to.y.as_mm() - from.y.as_mm());
            let len_sq = dx * dx + dy * dy;
            // Projection parameter onto this edge, clamped to the edge itself.
            let t = if len_sq > 0.0 {
                (((probe.x.as_mm() - from.x.as_mm()) * dx + (probe.y.as_mm() - from.y.as_mm()) * dy)
                    / len_sq)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (px, py) = (from.x.as_mm() + dx * t, from.y.as_mm() + dy * t);
            let d = (px - probe.x.as_mm()).powi(2) + (py - probe.y.as_mm()).powi(2);
            if d < best.0 {
                best = (d, self.cumulative[idx - 1] + t * len_sq.sqrt());
            }
        }
        best.1 / self.length_mm()
    }

    /// The vertices of the sub-path from `start` to `end` millimetres along the loop,
    /// wrapping if it crosses the start point. Both ends are exact points on the path;
    /// every original vertex strictly between them is preserved, so a span across a corner
    /// still turns the corner.
    fn sub_path(&self, start: f64, end: f64) -> Vec<Point> {
        let total = self.length_mm();
        let start = start.rem_euclid(total);
        let length = (end - start).rem_euclid(total);
        let mut out = vec![self.point_at(start)];
        // Walk the vertices from `start`, wrapping, until the accumulated run is spent.
        let first = self.cumulative.partition_point(|&d| d <= start);
        for step in 0..self.points.len() {
            let idx = (first + step) % (self.points.len() - 1);
            let along = (self.cumulative[idx] - start).rem_euclid(total);
            if along >= length || along <= 0.0 {
                continue;
            }
            out.push(self.points[idx]);
        }
        out.push(self.point_at(start + length));
        out
    }
}

/// Where the tabs go, as fractions of the loop, sorted and de-duplicated.
///
/// `placed` is the operator's own list; when it is empty `count` tabs are spread evenly
/// from the loop's start. Evenly spaced is the right default because it is the placement
/// that holds a board most symmetrically without knowing anything about its shape — and
/// the moment the operator places even one tab themselves, their list takes over entirely,
/// so what is generated is what they see.
pub fn tab_positions(placed: &[f64], count: usize) -> Vec<f64> {
    let mut positions: Vec<f64> = if placed.is_empty() {
        (0..count).map(|i| i as f64 / count.max(1) as f64).collect()
    } else {
        placed.iter().map(|t| t.rem_euclid(1.0)).collect()
    };
    positions.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    positions.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    positions
}

/// Splits a loop into the spans the cutter travels, leaving a `tab_width_mm` gap centred
/// on each tab.
///
/// With no tabs the result is the whole loop as one closed span — correct for a board held
/// down some other way (double-sided tape, a vacuum bed), which is what `retention: none`
/// means. With tabs, each span is open: it starts where one tab ends and finishes where the
/// next begins.
///
/// Two degenerate cases are handled rather than left to produce a broken program: tabs that
/// together exceed the loop leave nothing to cut (an empty result, which the caller reports
/// rather than emitting a program that frees nothing), and a tab narrower than
/// [`MIN_TAB_MM`] is dropped as a rounding artefact.
pub fn cut_spans(loop_: &Loop, tabs: &[f64], tab_width_mm: f64) -> Vec<Vec<Point>> {
    let total = loop_.length_mm();
    let tabs: Vec<f64> = tab_positions(tabs, 0)
        .into_iter()
        .filter(|_| tab_width_mm >= MIN_TAB_MM)
        .collect();
    if tabs.is_empty() {
        // Closed loop: back to the start, so nothing is left joined.
        let mut span = loop_.sub_path(0.0, total);
        span.push(loop_.point_at(0.0));
        return vec![span];
    }
    if tab_width_mm * tabs.len() as f64 >= total {
        return Vec::new();
    }

    let half = tab_width_mm / 2.0;
    tabs.iter()
        .enumerate()
        .map(|(i, &tab)| {
            let next = tabs[(i + 1) % tabs.len()];
            // From the far edge of this tab to the near edge of the next one.
            loop_.sub_path(tab * total + half, next * total - half)
        })
        .collect()
}

/// Pitch of a mouse bite's holes, as a multiple of the drill diameter.
///
/// Twice the diameter leaves about half the tab as web — weak enough to snap by hand,
/// strong enough to hold through the cut. It is the usual 0.5 mm-on-1 mm proportion.
///
/// **Not a setting.** How many holes perforate a tab is not a preference; it follows from
/// the tab width and the drill, and asking the operator to supply a count is asking them
/// to re-derive this. (There *was* a `bite_holes` field; it is gone.)
const MOUSE_BITE_PITCH_DIAMETERS: f64 = 2.0;

/// The drill centres for one mouse-bite tab, on the loop, in machine coordinates.
///
/// A mouse bite replaces the snap-and-file of a solid tab with a perforated line that
/// breaks cleanly and leaves a much smaller nib. The holes sit **on the offset path**, so
/// the perforation runs down the middle of what the tab would have been — the first and
/// last are inset half a pitch from the tab's edges, so no hole breaks into the routed
/// span on either side.
///
/// The count comes from the tab width at [`MOUSE_BITE_PITCH_DIAMETERS`], floored at two:
/// a single hole is a stress riser, not a perforation.
pub fn mouse_bite_centres(
    loop_: &Loop,
    tab: f64,
    tab_width_mm: f64,
    drill_diameter: Length,
) -> Vec<Point> {
    let diameter = drill_diameter.as_mm();
    if tab_width_mm < MIN_TAB_MM || diameter <= 0.0 {
        return Vec::new();
    }
    let holes = ((tab_width_mm / (MOUSE_BITE_PITCH_DIAMETERS * diameter)).round() as usize).max(2);

    let total = loop_.length_mm();
    let centre = tab.rem_euclid(1.0) * total;
    let pitch = tab_width_mm / holes as f64;
    let first = centre - tab_width_mm / 2.0 + pitch / 2.0;
    (0..holes).map(|i| loop_.point_at(first + pitch * i as f64)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 40 × 20 rectangle, anticlockwise from the origin — total length 120 mm, so an
    /// arc-length fraction is easy to read off by eye.
    fn rectangle() -> Loop {
        let pts: Vec<Point> = [(0.0, 0.0), (40.0, 0.0), (40.0, 20.0), (0.0, 20.0)]
            .iter()
            .map(|&(x, y)| Point::new(Length::from_mm(x), Length::from_mm(y)))
            .collect();
        Loop::new(&pts).expect("a rectangle is a loop")
    }

    fn at(p: Point) -> (f64, f64) {
        (p.x.as_mm(), p.y.as_mm())
    }

    #[test]
    fn arc_length_runs_all_the_way_round_and_wraps() {
        let r = rectangle();
        assert!((r.length_mm() - 120.0).abs() < 1e-9);
        assert_eq!(at(r.point_at(0.0)), (0.0, 0.0));
        assert_eq!(at(r.point_at(40.0)), (40.0, 0.0), "the first corner");
        assert_eq!(at(r.point_at(50.0)), (40.0, 10.0), "halfway up the right side");
        assert_eq!(at(r.point_at(120.0)), (0.0, 0.0), "a full lap returns to the start");
        assert_eq!(at(r.point_at(130.0)), at(r.point_at(10.0)), "past the end wraps");
    }

    /// The inverse of `point_at`: a click anywhere near the loop resolves to the fraction
    /// of the nearest point on it. This is what turns a click in the board view into a
    /// stored tab.
    #[test]
    fn the_nearest_fraction_inverts_the_walk() {
        let r = rectangle();
        for distance in [0.0, 7.5, 40.0, 63.0, 119.0] {
            let point = r.point_at(distance);
            let fraction = r.nearest_fraction(point);
            assert!(
                (fraction * 120.0 - distance).abs() < 1e-6,
                "{distance} mm → fraction {fraction}"
            );
        }
        // A probe off the path snaps to the nearest point on it, not to a vertex.
        let off = Point::new(Length::from_mm(10.0), Length::from_mm(-3.0));
        assert!((r.nearest_fraction(off) * 120.0 - 10.0).abs() < 1e-6);
    }

    /// With no tabs the board is cut free in one closed pass — `retention: none`.
    #[test]
    fn no_tabs_cuts_one_closed_loop() {
        let r = rectangle();
        let spans = cut_spans(&r, &[], 2.0);
        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span[0], span[span.len() - 1], "the pass closes on itself");
    }

    /// Every tab is a gap of exactly the asked-for width, and the spans between them cover
    /// the rest of the loop — nothing cut twice, nothing left joined that should not be.
    #[test]
    fn tabs_leave_gaps_of_the_asked_for_width() {
        let r = rectangle();
        let tabs = vec![0.0, 0.25, 0.5, 0.75];
        let spans = cut_spans(&r, &tabs, 2.0);
        assert_eq!(spans.len(), 4, "four tabs cut the loop into four spans");

        let span_length = |span: &Vec<Point>| -> f64 {
            span.windows(2).map(|w| w[0].distance_mm(&w[1])).sum()
        };
        let cut: f64 = spans.iter().map(span_length).sum();
        assert!(
            (cut - (120.0 - 4.0 * 2.0)).abs() < 1e-6,
            "cut length is the loop less the four 2 mm tabs, got {cut}"
        );

        // Each span begins 1 mm past its tab centre and ends 1 mm before the next.
        assert!((spans[0][0].distance_mm(&r.point_at(1.0))).abs() < 1e-6);
        let first_end = spans[0][spans[0].len() - 1];
        assert!((first_end.distance_mm(&r.point_at(29.0))).abs() < 1e-6);
    }

    /// A span that crosses a corner keeps the corner: the cutter must turn there, and a
    /// straight line from one side to the other would cut the corner off.
    #[test]
    fn a_span_across_a_corner_keeps_the_corner() {
        let r = rectangle();
        // One tab at the start, so the single span runs 1 → 119 mm, over three corners.
        let spans = cut_spans(&r, &[0.0], 2.0);
        assert_eq!(spans.len(), 1);
        for corner in [(40.0, 0.0), (40.0, 20.0), (0.0, 20.0)] {
            assert!(
                spans[0].iter().any(|p| at(*p) == corner),
                "corner {corner:?} missing from {:?}",
                spans[0].iter().map(|p| at(*p)).collect::<Vec<_>>()
            );
        }
    }

    /// Tabs wider than the loop would leave nothing to cut. Better an empty result the
    /// caller reports than a program that spins the tool and frees nothing.
    #[test]
    fn tabs_that_swallow_the_loop_cut_nothing() {
        assert!(cut_spans(&rectangle(), &[0.0, 0.5], 60.0).is_empty());
    }

    /// Absent an operator placement, tabs are spread evenly from the loop start.
    #[test]
    fn an_empty_placement_spreads_the_profiles_count_evenly() {
        assert_eq!(tab_positions(&[], 4), vec![0.0, 0.25, 0.5, 0.75]);
        assert_eq!(tab_positions(&[], 0), Vec::<f64>::new());
        // One placed tab takes over completely — the count is not topped up.
        assert_eq!(tab_positions(&[0.3], 4), vec![0.3]);
        // Placements are wrapped, ordered and de-duplicated, so the spans come out in
        // loop order and 1.2 does not become a second tab beside the one at 0.2.
        let wrapped = tab_positions(&[0.9, 1.2, 0.2], 4);
        assert_eq!(wrapped.len(), 2, "1.2 and 0.2 are the same place: {wrapped:?}");
        assert!((wrapped[0] - 0.2).abs() < 1e-9 && (wrapped[1] - 0.9).abs() < 1e-9);
    }

    // -- tab distribution --------------------------------------------------------

    /// An equilateral-ish triangle whose sides are long enough to hold any tab count
    /// the tests ask for, so allocation is tested without capacity interfering.
    fn triangle() -> Vec<(f64, f64, f64, f64)> {
        vec![(0.0, 0.0, 90.0, 0.0), (90.0, 0.0, 45.0, 80.0), (45.0, 80.0, 0.0, 0.0)]
    }

    /// How many tabs each segment received.
    fn per_segment(anchors: &[TabAnchor], segments: usize) -> Vec<usize> {
        let mut counts = vec![0usize; segments];
        for a in anchors {
            counts[a.segment] += 1;
        }
        counts
    }

    /// The worked examples: the remainder goes to the longest sides, and a count below
    /// the segment count leaves the shortest side bare rather than doubling up.
    #[test]
    fn tabs_are_shared_out_by_segment_longest_first() {
        let t = triangle();
        // Sides are 90, ~91.4, ~91.9 — so segment 0 is the shortest.
        let five = distribute_tabs(&t, 5, 2.0);
        assert_eq!(five.len(), 5);
        assert_eq!(per_segment(&five, 3), vec![1, 2, 2], "5 over 3 → 2, 2, 1 by length");

        let two = distribute_tabs(&t, 2, 2.0);
        assert_eq!(per_segment(&two, 3), vec![0, 1, 1], "2 over 3 → one each on the longest two");

        let three = distribute_tabs(&t, 3, 2.0);
        assert_eq!(per_segment(&three, 3), vec![1, 1, 1], "an exact share needs no remainder");
    }

    /// One tab sits at the midpoint; several split the side into equal gaps, counting the
    /// two corners — the spacing the clearance rule is stated against.
    #[test]
    fn tabs_are_evenly_spaced_within_a_segment_including_its_corners() {
        let single = distribute_tabs(&[(0.0, 0.0, 100.0, 0.0)], 1, 2.0);
        assert_eq!(single.len(), 1);
        assert!((single[0].t - 0.5).abs() < 1e-9, "a single tab is at the middle");
        assert!((single[0].point.0 - 50.0).abs() < 1e-9);

        let three = distribute_tabs(&[(0.0, 0.0, 100.0, 0.0)], 3, 2.0);
        let ts: Vec<f64> = three.iter().map(|a| a.t).collect();
        for (got, want) in ts.iter().zip([0.25, 0.5, 0.75]) {
            assert!((got - want).abs() < 1e-9, "even quarters, got {ts:?}");
        }
    }

    /// The clearance rule in force: every tab keeps three widths clear of its neighbours
    /// and of both corners. Checked on a segment loaded to exactly its capacity.
    #[test]
    fn tabs_keep_three_widths_clear_of_each_other_and_of_the_corners() {
        let width = 2.0;
        // Capacity of a 100 mm side: (100/2 − 3) / 4 = 11.75 → 11 tabs.
        let seg = [(0.0, 0.0, 100.0, 0.0)];
        assert_eq!(segment_capacity(100.0, width), 11);
        let anchors = distribute_tabs(&seg, 11, width);
        assert_eq!(anchors.len(), 11);

        let mut edges: Vec<(f64, f64)> =
            anchors.iter().map(|a| (a.point.0 - width / 2.0, a.point.0 + width / 2.0)).collect();
        edges.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let clearance = TAB_CLEARANCE_WIDTHS * width;
        assert!(edges[0].0 >= clearance - 1e-9, "clear of the first corner");
        assert!(100.0 - edges[10].1 >= clearance - 1e-9, "clear of the last corner");
        for pair in edges.windows(2) {
            assert!(pair[1].0 - pair[0].1 >= clearance - 1e-9, "tabs are {clearance} mm apart");
        }
    }

    /// A side too short for a tab gets none, and the tabs it would have had move to a
    /// side with room — better all four on the long edge than four crowded on a stub.
    #[test]
    fn tabs_that_do_not_fit_move_to_a_segment_with_room() {
        // A long side and two stubs: 7 × 2 mm = 14 mm is the minimum for one tab.
        let segments = vec![(0.0, 0.0, 200.0, 0.0), (200.0, 0.0, 205.0, 0.0), (205.0, 0.0, 0.0, 0.0)];
        assert_eq!(segment_capacity(5.0, 2.0), 0, "a 5 mm stub holds no 2 mm tab");
        let anchors = distribute_tabs(&segments, 4, 2.0);
        assert_eq!(anchors.len(), 4, "none are lost");
        let counts = per_segment(&anchors, 3);
        assert_eq!(counts[1], 0, "the stub gets none");
        assert!(counts[0] + counts[2] == 4);
    }

    /// An outline that genuinely cannot hold the asked-for tabs returns fewer, so the
    /// caller can say so — crowding them together would defeat the clearance rule.
    #[test]
    fn an_outline_with_too_little_room_returns_fewer_tabs() {
        // Four 10 mm sides hold no 3 mm tab at all (7 × 3 = 21 mm needed).
        let square = vec![
            (0.0, 0.0, 10.0, 0.0),
            (10.0, 0.0, 10.0, 10.0),
            (10.0, 10.0, 0.0, 10.0),
            (0.0, 10.0, 0.0, 0.0),
        ];
        assert!(distribute_tabs(&square, 4, 3.0).is_empty());
        // The same square holds one 1 mm tab per side.
        assert_eq!(distribute_tabs(&square, 4, 1.0).len(), 4);
    }

    /// Anchors come back in contour order, so a job keying per-tab offsets to their
    /// position in this list keeps the same tab when the count does not change.
    #[test]
    fn anchors_are_returned_in_contour_order() {
        let anchors = distribute_tabs(&triangle(), 5, 2.0);
        let keys: Vec<(usize, f64)> = anchors.iter().map(|a| (a.segment, a.t)).collect();
        let mut sorted = keys.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.partial_cmp(&b.1).unwrap()));
        assert_eq!(keys, sorted);
    }

    /// Degenerate asks are answered with an empty list, not a panic or a divide by zero.
    #[test]
    fn degenerate_distributions_are_empty() {
        assert!(distribute_tabs(&triangle(), 0, 2.0).is_empty(), "no tabs asked for");
        assert!(distribute_tabs(&[], 4, 2.0).is_empty(), "no segments to place them on");
        assert!(distribute_tabs(&triangle(), 4, 0.0).is_empty(), "a zero-width tab is not a tab");
    }

    /// Mouse-bite holes sit inside the tab, evenly pitched, without breaking into the
    /// routed spans on either side.
    #[test]
    fn mouse_bites_perforate_the_tab_without_reaching_its_edges() {
        let r = rectangle();
        // A 3 mm tab with a 0.5 mm drill: 3 / (2 × 0.5) = 3 holes, pitched 1 mm.
        let centres = mouse_bite_centres(&r, 0.0, 3.0, Length::from_mm(0.5));
        assert_eq!(centres.len(), 3, "the count comes from the tab and the drill");
        // Tab spans 118.5 → 1.5 mm, so the holes sit at 119, 120 and 1 mm.
        for (centre, distance) in centres.iter().zip([119.0, 120.0, 1.0]) {
            assert!(
                centre.distance_mm(&r.point_at(distance)) < 1e-6,
                "hole should be at {distance} mm, got {:?}",
                at(*centre)
            );
        }
    }

    /// The perforation count follows the tab width and the drill, and never drops to one
    /// — a single hole is a stress riser, not a perforation.
    #[test]
    fn the_mouse_bite_count_is_derived_and_never_below_two() {
        let r = rectangle();
        let holes = |width, drill| mouse_bite_centres(&r, 0.25, width, Length::from_mm(drill)).len();
        assert_eq!(holes(3.0, 0.5), 3);
        assert_eq!(holes(6.0, 0.5), 6, "a wider tab takes more");
        assert_eq!(holes(3.0, 1.0), 2, "a bigger drill takes fewer");
        assert_eq!(holes(1.0, 1.0), 2, "floored at two, not rounded to one");
        assert!(mouse_bite_centres(&r, 0.0, 3.0, Length::from_mm(0.0)).is_empty(), "no drill");
    }
}
