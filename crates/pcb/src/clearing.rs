//! Copper the isolation pass leaves standing, and the toolpath that takes it out.
//!
//! [`isolate`](crate::isolate) removes a channel of width `W` around each net and nothing
//! else. Where two nets sit further apart than `2·W` the copper between the two channels
//! survives as a strip that is on no net and exists on no layer of the design — a floating
//! conductor KiCad never drew, which solder will happily bridge. Fully enclosed by what the
//! pass cut, it is an **island**.
//!
//! # The rule
//!
//! > An island is a connected piece of copper that lies on no net, is entirely enclosed by
//! > what the isolation pass cut, and is nowhere wider than [`ISLAND_WIDTH_MULTIPLE`] × `W`.
//! > Every island is removed by the engraver that cut the channels around it, at the same
//! > depth. Anything wider is left, and counted.
//!
//! That bound is what makes this pass **opportunist**: at three channels the widest island
//! still costs three or four more passes of a bit that is already in the spindle. No new
//! tool, no rack slot, negligible time. Clearing copper that is *worth loading a router
//! for* is a different question with a different answer, and it is not asked here.
//!
//! Two properties hold by construction, and both are worth stating because everything
//! downstream leans on them:
//!
//! - **Net copper is never touched.** The region is derived by subtracting every piece of
//!   copper the design actually drew, so it cannot contain any.
//! - **No clearing cut ever forms an electrically significant edge.** Every wall that
//!   matters was already cut by the V-bit's channel; a clearing tool only widens waste.
//!   That is why the tool here can be the one already loaded, and why the choice of a
//!   better one is a tie-break rather than a requirement.
//!
//! # What is not here
//!
//! Two features are shaped for and deliberately absent. A **minimum copper width** — the
//! *thin parts* of free copper that runs out into the background rather than closing into
//! an island, which is what cleans between the pads of an SO8 — is a second source of
//! region for [`fill`], not a second engine. **Drawn clear areas** on the Job view are a
//! third. Both need a region and a tool width, which is all [`fill`] asks for; what they
//! also need, and what island removal does not, is a *router* — and with it a guard band
//! against the half of a channel that faces a net, which the V-bit does not require
//! because it has already been there.

use std::collections::BTreeMap;

use crate::copper::CopperSnapshot;
use crate::isolation::IsolationContour;
use crate::region::{
    area_nm2, components, difference, intersect, polygon_region, union, BBox, Ring,
};
use crate::stitching::{offset_group, stroke_open_paths};

/// How much wider than the channel an island may be and still be taken.
///
/// A stated constant with its reasoning rather than a setting, like
/// [`LADDER_STEP_NM`](crate::LADDER_STEP_NM): it is the bound that makes this pass
/// opportunist, and an operator who wants copper cleared *past* it wants a router, which is
/// a different question — which tool, at what threshold, costing which rack slot — and not
/// a larger number here.
pub const ISLAND_WIDTH_MULTIPLE: f64 = 3.0;

/// Stepover of a V-bit clearing copper, as a fraction of the channel it cuts.
///
/// **A property of the tool, not of the pass.** A cone's channel is full width *at the
/// surface*, and copper only exists at the surface: two grooves at 0.9 W leave no copper
/// between them, and the ridge they leave stands in the laminate below, where nothing is
/// being separated. This is why a V-bit is a perfectly good copper-clearing tool for small
/// areas and only a poor one for large ones — it is not the coverage that fails, it is the
/// number of passes.
///
/// A flat cutter would want a smaller fraction, because its ridge is copper. That number
/// belongs with the router.
const VBIT_STEPOVER_FRACTION: f64 = 0.9;

/// Copper thinner than twice this is not copper, nm.
///
/// A polygon subtraction leaves hairlines along its own cut edge — two curves that agree to
/// within the chord error they were each drawn with, and disagree below it. Left in, they
/// read as copper still standing and buy themselves a pass of the tool. 5 µm is above that
/// error and an order below anything a V-bit could be asked to remove.
const SLIVER_NM: f64 = 5_000.0;

/// How many passes one fill may take before it gives up and reports what is left.
///
/// Not a number the geometry should ever reach — a corridor round a trace takes three — but
/// the loop is driven by what a polygon subtraction says is left standing, and a cap is what
/// stops a disagreement below the chord error from becoming an app that does not come back.
const MAX_FILL_PASSES: usize = 16;

/// Bisection tolerance when measuring how wide a piece of copper is, nm.
///
/// 10 µm against a width reported to the operator in hundredths of a millimetre: finer than
/// the number is printed, and far finer than the bit that would cut it.
const WIDTH_TOLERANCE_NM: f64 = 10_000.0;

/// What a clearing pass took, and what it did not.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clearing {
    /// Cutter-centre rings, board nm, closed and ordered outermost-first within each piece
    /// of copper. Cut by the engraver at its own depth.
    pub paths: Vec<Ring>,
    /// How many islands were removed.
    pub removed: usize,
    /// Their total area, nm².
    pub removed_area_nm2: f64,
    /// How many pieces of free copper were left because they are wider than the bound.
    pub left: usize,
    /// The widest of those, nm. Zero when nothing was left.
    ///
    /// The useful half of the report: it says what a wider bound — or a router — would
    /// buy, the way [`IsolationResult::widest_workable_nm`](crate::IsolationResult) says
    /// what changing the channel width would buy.
    pub widest_left_nm: i64,
    /// Area of accepted island the tool could not reach, nm².
    ///
    /// Reported rather than dropped, for the reason the isolation module is built around: a
    /// piece of copper is cut, or it is accounted for, and there is no third outcome.
    pub missed_area_nm2: f64,
}

impl Clearing {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// Every island in `copper` that the pass which cut `contours` left standing, and the rings
/// that take them out.
///
/// `width_nm` is the channel the chosen bit **actually cuts** — `EngraveChoice::width`, not
/// the profile's requested minimum — which is the same rule §5.4 of the operation planner
/// applies to the contours themselves. A contour that had to narrow is stroked at its own
/// width, because that is the copper it really removed.
pub fn islands(copper: &CopperSnapshot, contours: &[IsolationContour], width_nm: i64) -> Clearing {
    let mut out = Clearing::default();
    if width_nm <= 0 || contours.is_empty() {
        return out;
    }
    let w = width_nm as f64;

    // Every piece of copper the design drew, netted or not. A fiducial is on no net and is
    // still deliberate; what makes an island an island is that nothing drew it at all.
    let keep = union(
        &copper
            .features
            .iter()
            .flat_map(|feature| feature.polygons.iter())
            .flat_map(polygon_region)
            .collect::<Vec<Ring>>(),
        &[],
    );
    let Some(bounds) = BBox::of(&keep) else {
        return out;
    };

    // The frame is the blank the board is cut from, near enough: wider than the copper by
    // more than any channel, so the background copper outside every net reaches it and the
    // enclosed pieces do not. That is the whole of the enclosure test — see `free` below.
    let frame = bounds.expand(2 * width_nm);
    let cuts = swept_by(contours);
    let free = difference(&difference(&[frame.ring()], &cuts), &keep);
    if free.is_empty() {
        return out;
    }

    let bound_nm = ISLAND_WIDTH_MULTIPLE * w;
    let mut accepted: Vec<Ring> = Vec::new();
    let mut left_widths: Vec<i64> = Vec::new();
    for piece in components(&free) {
        let Some(bbox) = BBox::of(&piece) else { continue };
        // Touching the frame is what tells the surrounding background from an island. The
        // background is the copper-clad blank, which is larger than the outline and so
        // reaches the frame by construction; an island by definition does not. Asked this
        // way, `pcb` needs to know nothing about the stitched outline.
        if bbox.x0 <= frame.x0 || bbox.y0 <= frame.y0 || bbox.x1 >= frame.x1 || bbox.y1 >= frame.y1
        {
            continue;
        }
        // Copper thinner than a chord error is not copper. Where two channels meet exactly
        // — which is the whole point of choosing a width that suits the board — they meet
        // along two curves each drawn to `OFFSET_ARC_TOLERANCE_NM`, and the crumbs left
        // between them are microns across. Counted, they would have the step report eight
        // islands removed from a board that had none.
        if offset_group(&piece, -SLIVER_NM).is_empty() {
            continue;
        }
        // Nowhere wider than the bound, tested by erosion — the same question `fit_cutout`
        // asks of a cutout, and the reason neither needs a medial axis, which `isolation`
        // explicitly declines as a large piece of work.
        if offset_group(&piece, -bound_nm / 2.0).is_empty() {
            out.removed += 1;
            out.removed_area_nm2 += net_area_nm2(&piece);
            accepted.extend(piece);
        } else {
            out.left += 1;
            left_widths.push(widest_nm(&piece, bound_nm, bbox));
        }
    }
    out.widest_left_nm = left_widths.into_iter().max().unwrap_or(0);
    if accepted.is_empty() {
        return out;
    }

    let (paths, missed) = fill(&accepted, &cuts, w, VBIT_STEPOVER_FRACTION * w);
    out.paths = paths;
    out.missed_area_nm2 = missed;
    out
}

/// The copper the isolation pass removed: each contour swept by the bit that cut it.
///
/// Grouped by width and stroked a group at a time. One call per distinct width rather than
/// one per contour, because Clipper's setup is paid per call and a board narrowed across
/// three tight pairs has three widths and several thousand contours.
fn swept_by(contours: &[IsolationContour]) -> Vec<Ring> {
    let mut by_width: BTreeMap<i64, Vec<Ring>> = BTreeMap::new();
    for contour in contours {
        if contour.path.len() < 2 || contour.width_nm <= 0 {
            continue;
        }
        let mut path = contour.path.clone();
        // A closed contour arrives as a ring — its last point is not its first. Stroked as
        // a polyline it would leave the one edge between them unswept, and that edge is
        // exactly where an island would appear to reach the background.
        if contour.closed {
            path.push(contour.path[0]);
        }
        by_width.entry(contour.width_nm).or_default().push(path);
    }

    let mut cuts: Vec<Ring> = Vec::new();
    for (width_nm, paths) in by_width {
        cuts.extend(stroke_open_paths(&paths, width_nm as f64 / 2.0));
    }
    union(&cuts, &[])
}

/// Rings that clear `region` with a tool of width `w_nm`, and the area of `region` they do
/// not reach.
///
/// `allowed` is the rest of the copper this pass has already removed — the channels. It is
/// not where the tool is *sent*; it is the licence that lets a cut sweep past the edge of an
/// island into a groove that is already there, which is what removes an island in one pass
/// rather than leaving a rim of it standing.
///
/// **Driven by what is still standing, not by a ladder of depths.** Stepping inward by a
/// fixed amount and stopping when the offset comes back empty is the obvious way to write
/// this and it is wrong on any island that is thin in one place and fat in another — a
/// corridor round a trace is exactly that, wider at the corners than down the sides. The
/// step that still finds copper at the corners has already passed the middle of the sides,
/// and the sides come off the machine with a hairline conductor down them. So each pass
/// asks the same question of what it has actually left: where the piece has room, run a
/// step in from its edge; where it does not, run down its middle, which sweeps it whole.
fn fill(region: &[Ring], allowed: &[Ring], w_nm: f64, stepover_nm: f64) -> (Vec<Ring>, f64) {
    let half = w_nm / 2.0;

    // Where the tool centre may go. Eroding the union of the island with the channels round
    // it — rather than the island alone — is what says the cut may run out into ground the
    // pass has already cleared. Eroding BEFORE anything is clipped away matters: a clip
    // introduces a boundary that is not a wall, and eroding from that would push the tool
    // off ground it may use.
    //
    // In the ordinary case this contains the island entire and changes nothing. It earns
    // its keep where a channel beside the island had to narrow: the cut here is a full
    // width one, and this is what keeps it out of the net on the far side.
    let a = offset_group(&union(region, allowed), -half);
    let mut standing = intersect(region, &a);

    // Each ring is set this far inside the copper it is taking, so that it sweeps `half`
    // past the edge — into the channel — and reaches `stepover_nm` deeper than the ring
    // before it.
    let inset = (stepover_nm - half).max(SLIVER_NM);

    let mut paths: Vec<Ring> = Vec::new();
    for _ in 0..MAX_FILL_PASSES {
        // Opened first, so that the hairlines a polygon subtraction leaves along its own cut
        // edge do not read as copper and buy themselves a pass. Nothing is hidden by this:
        // `missed` below is measured against the island as it came in.
        standing = open_by(&standing, SLIVER_NM);
        if standing.is_empty() {
            break;
        }

        let mut pieces = components(&standing);
        // Left-to-right, so a board gives the same program on every run whatever order
        // Clipper happened to return its rings in.
        pieces.sort_by_key(|p| BBox::of(p).map(|b| (b.x0, b.y0)).unwrap_or_default());

        let mut rings: Vec<Ring> = Vec::new();
        for piece in pieces {
            // Normalised through a union so the rings carry Clipper's own orientation, which
            // is what tells `offset_group` an inner ring is a hole to grow rather than an
            // outline to shrink.
            let piece = union(&piece, &[]);
            let ring = offset_group(&piece, -inset);
            if ring.is_empty() {
                // No room for a full step: this piece is nowhere wider than twice the inset,
                // so a single ring down its middle sweeps all of it.
                rings.extend(offset_group(&piece, -inradius_nm(&piece, 0.0, inset)));
            } else {
                rings.extend(ring);
            }
        }
        if rings.is_empty() {
            break;
        }

        let closed: Vec<Ring> = rings.iter().map(as_closed_polyline).collect();
        standing = difference(&standing, &stroke_open_paths(&closed, half));
        paths.extend(rings);
    }

    // Accounted from the rings themselves rather than from any region they were derived
    // from, so a fill that stopped short — on the pass cap, on a channel too narrow to let
    // the tool near, on anything — shows up here rather than in nobody's report.
    let closed: Vec<Ring> = paths.iter().map(as_closed_polyline).collect();
    let missed = net_area_nm2(&difference(region, &stroke_open_paths(&closed, half)));
    (paths, missed)
}

/// `region` with everything narrower than `2 · by_nm` opened away — eroded, then grown back.
fn open_by(region: &[Ring], by_nm: f64) -> Vec<Ring> {
    let eroded = offset_group(region, -by_nm);
    if eroded.is_empty() {
        return Vec::new();
    }
    offset_group(&eroded, by_nm)
}

/// A ring as a polyline that comes back to its start, for stroking.
fn as_closed_polyline(ring: &Ring) -> Ring {
    let mut out = ring.clone();
    if let Some(&first) = ring.first() {
        out.push(first);
    }
    out
}

/// Area of a region whose rings carry Clipper's orientation: outlines positive, the holes
/// that pierce them negative, so the sum is the copper and not the copper plus its holes.
fn net_area_nm2(region: &[Ring]) -> f64 {
    region.iter().map(|ring| area_nm2(ring)).sum::<i128>().abs() as f64
}

/// How wide `piece` is at its widest, nm — known already to exceed `over_nm`.
///
/// A region's width is twice its inradius, and its inradius is how far it can be eroded
/// before nothing is left, so this is [`inradius_nm`] and a factor of two. No medial axis,
/// which `isolation` explicitly declines as a large piece of work.
fn widest_nm(piece: &[Ring], over_nm: f64, bbox: BBox) -> i64 {
    // The piece fits inside its own bounding box, so it cannot be wider than the shorter
    // side of it — an erosion by half of that leaves nothing, whatever the shape.
    let ceiling = (bbox.x1 - bbox.x0).min(bbox.y1 - bbox.y0).max(0) as f64 + WIDTH_TOLERANCE_NM;
    let hi = (ceiling / 2.0).max(over_nm / 2.0 + WIDTH_TOLERANCE_NM);
    (2.0 * inradius_nm(piece, over_nm / 2.0, hi)).round() as i64
}

/// The deepest `region` can be eroded and still leave something, nm.
///
/// Bisected between an offset known to leave something and one known not to. A dozen
/// erosions of a polygon with tens of points, against an isolation pass that takes seconds:
/// the cost is not worth approximating away, and this answers both questions the module
/// asks about how big a piece of copper is — how wide it is, and how many passes it takes.
fn inradius_nm(region: &[Ring], mut nonempty: f64, mut empty: f64) -> f64 {
    while empty - nonempty > WIDTH_TOLERANCE_NM {
        let mid = (nonempty + empty) / 2.0;
        if offset_group(region, -mid).is_empty() {
            empty = mid;
        } else {
            nonempty = mid;
        }
    }
    nonempty
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copper::{CopperFeature, CopperSource, Polygon};

    const W: i64 = 200_000; // a 0.2 mm channel
    const MM: i64 = 1_000_000;

    /// A circle of radius `r` about the origin, as a 96-gon.
    ///
    /// Round on purpose. A rectangular corridor is *wider at its corners than down its
    /// sides* by the diagonal, so every count and width taken off one is really two
    /// measurements of different things; a circular one has exactly one width everywhere,
    /// which is what lets the tests below say a number rather than a range. The flat of a
    /// 96-gon at these radii is under 3 µm — an order below the tolerance anything here is
    /// measured to.
    fn circle(r: i64) -> Ring {
        (0..96)
            .map(|n| {
                let a = std::f64::consts::TAU * n as f64 / 96.0;
                ((r as f64 * a.cos()).round() as i64, (r as f64 * a.sin()).round() as i64)
            })
            .collect()
    }

    fn rect(x0: i64, y0: i64, x1: i64, y1: i64) -> Ring {
        vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    }

    fn feature(net: &str, outline: Ring, holes: Vec<Ring>) -> CopperFeature {
        CopperFeature {
            net: net.to_string(),
            source: CopperSource::Track,
            polygons: vec![Polygon { outline, holes }],
        }
    }

    fn snapshot(features: Vec<CopperFeature>) -> CopperSnapshot {
        CopperSnapshot {
            layer_id: crate::FRONT_COPPER,
            features,
            warnings: Vec::new(),
            partial: false,
        }
    }

    /// A round pad in a ground pour that is cut back from it by `gap`, isolated at `W`, and
    /// what clearing makes of the copper standing in the gap.
    ///
    /// **This is the shape the feature exists for**, and it is the commonest layout there
    /// is: the pour's clearance leaves a corridor of blank copper round everything on the
    /// board, the pass cuts a channel down each side of that corridor, and whatever the two
    /// channels cannot meet across survives as a closed ring of copper on no net.
    ///
    /// The corridor is `gap` wide and each channel takes `W` of it, so the island is
    /// `gap - 2·W` across.
    fn pad_in_pour(gap: i64) -> (CopperSnapshot, Clearing) {
        let copper = snapshot(vec![
            feature("SIG", circle(5 * MM), Vec::new()),
            feature(
                "GND",
                rect(-15 * MM, -15 * MM, 15 * MM, 15 * MM),
                vec![circle(5 * MM + gap)],
            ),
        ]);
        let result = crate::isolate(&copper, W, W);
        let cleared = islands(&copper, &result.contours, W);
        (copper, cleared)
    }

    /// The width of an annular island `gap - 2·W` across, as an area.
    fn annulus_area_nm2(gap: i64) -> f64 {
        let (outer, inner) = ((5 * MM + gap - W) as f64, (5 * MM + W) as f64);
        std::f64::consts::PI * (outer * outer - inner * inner)
    }

    #[test]
    fn copper_the_two_channels_strand_between_them_is_an_island() {
        // A 4·W corridor: each channel takes W from its own side, leaving a ring of copper
        // 2·W across standing between them, on no net and well inside the bound.
        let (_, cleared) = pad_in_pour(4 * W);
        assert_eq!(cleared.removed, 1, "{cleared:?}");
        assert_eq!(cleared.left, 0);
        assert!(!cleared.paths.is_empty(), "an island found and not cut");
        let expected = annulus_area_nm2(4 * W);
        assert!(
            (cleared.removed_area_nm2 - expected).abs() < 0.02 * expected,
            "removed {:.0} nm², expected about {expected:.0} nm²",
            cleared.removed_area_nm2,
        );
    }

    #[test]
    fn where_the_two_channels_meet_there_is_nothing_left_to_take() {
        // A 2·W corridor: the two channels meet, and there is nothing left to find.
        let (_, cleared) = pad_in_pour(2 * W);
        assert_eq!(cleared.removed, 0, "{cleared:?}");
        assert_eq!(cleared.left, 0);
        assert!(cleared.paths.is_empty());
    }

    #[test]
    fn copper_wider_than_the_bound_is_left_and_measured() {
        // An 8·W corridor leaves 6·W standing: twice the bound, so left — and reported at a
        // width an operator can hold against what a router would reach.
        let (_, cleared) = pad_in_pour(8 * W);
        assert_eq!(cleared.removed, 0, "{cleared:?}");
        assert_eq!(cleared.left, 1);
        let widest = cleared.widest_left_nm as f64;
        let expected = 6.0 * W as f64;
        assert!(
            (widest - expected).abs() < 0.05 * expected,
            "widest {widest} nm, expected about {expected} nm",
        );
    }

    #[test]
    fn an_island_at_the_bound_is_taken_and_one_past_it_is_not() {
        // The bound itself, from both sides — 2.8·W removed, 3.2·W left.
        let (_, under) = pad_in_pour(2 * W + (2.8 * W as f64) as i64);
        assert_eq!((under.removed, under.left), (1, 0), "{under:?}");
        let (_, over) = pad_in_pour(2 * W + (3.2 * W as f64) as i64);
        assert_eq!((over.removed, over.left), (0, 1), "{over:?}");
    }

    #[test]
    fn an_island_is_swept_end_to_end() {
        // Coverage, not just contact: what the rings sweep has to leave none of the island
        // standing, or the pass has made a floating conductor thinner rather than gone.
        //
        // Both widths matter. At 2·W one ring down the middle does it; at 2.8·W it takes
        // two, and the second is the one an offset fill written the obvious way — step in a
        // fixed amount, stop when the offset comes back empty — never lays down.
        for gap in [4 * W, 2 * W + (2.8 * W as f64) as i64] {
            let (_, cleared) = pad_in_pour(gap);
            assert_eq!(cleared.removed, 1, "gap {gap}: {cleared:?}");
            assert!(
                cleared.missed_area_nm2 < 0.01 * cleared.removed_area_nm2,
                "gap {gap}: {} nm² of {} nm² left standing",
                cleared.missed_area_nm2,
                cleared.removed_area_nm2,
            );
        }
    }

    #[test]
    fn no_clearing_cut_ever_reaches_copper_that_is_on_a_net() {
        // The invariant, asserted directly. It is the one that must never regress: a
        // clearing pass that grazes a net has cut the board in half.
        for gap in [3 * W, 4 * W, 5 * W, 6 * W, 10 * W] {
            let (copper, cleared) = pad_in_pour(gap);
            if cleared.paths.is_empty() {
                continue;
            }
            let keep = union(
                &copper
                    .features
                    .iter()
                    .flat_map(|f| f.polygons.iter())
                    .flat_map(polygon_region)
                    .collect::<Vec<Ring>>(),
                &[],
            );
            let closed: Vec<Ring> = cleared.paths.iter().map(as_closed_polyline).collect();
            let bitten = intersect(&stroke_open_paths(&closed, W as f64 / 2.0), &keep);
            assert!(
                net_area_nm2(&bitten) < 1.0,
                "gap {gap}: the clearing pass cut {} nm² of net copper",
                net_area_nm2(&bitten),
            );
        }
    }

    #[test]
    fn the_background_around_the_board_is_never_an_island() {
        // One lone pad on an otherwise bare blank: everything outside its channel is the
        // background, which reaches the frame however the copper is laid out. Nothing is
        // enclosed, so nothing is cleared — and nothing is reported as left standing
        // either, because the blank is not a defect.
        let copper = snapshot(vec![feature("A", circle(MM), Vec::new())]);
        let result = crate::isolate(&copper, W, W);
        let cleared = islands(&copper, &result.contours, W);
        assert_eq!(cleared.removed, 0, "{cleared:?}");
        assert_eq!(cleared.left, 0);
    }

    #[test]
    fn copper_that_runs_out_into_the_background_is_not_an_island() {
        // **The boundary of this feature, recorded on purpose.** Two traces side by side in
        // open field leave a strip between them that looks exactly like an island and is
        // not one: it opens out past the ends of the traces into the blank, so it is one
        // piece with the background and no width test asked of the *whole* piece can find
        // it.
        //
        // Reaching it means asking a different question — the thin *parts* of free copper
        // rather than whole enclosed pieces — which is the deferred minimum-copper-width
        // option, and it uses this module's [`fill`] unchanged. Until then this board gets
        // nothing, silently and correctly.
        let copper = snapshot(vec![
            feature("A", rect(0, 0, 10 * MM, MM), Vec::new()),
            feature("B", rect(0, MM + 4 * W, 10 * MM, 2 * MM + 4 * W), Vec::new()),
        ]);
        let result = crate::isolate(&copper, W, W);
        let cleared = islands(&copper, &result.contours, W);
        assert_eq!(cleared.removed, 0, "{cleared:?}");
        assert!(cleared.paths.is_empty());
    }

    #[test]
    fn a_board_gives_the_same_program_twice() {
        let (_, once) = pad_in_pour(4 * W);
        let (_, twice) = pad_in_pour(4 * W);
        assert_eq!(once, twice);
    }
}
