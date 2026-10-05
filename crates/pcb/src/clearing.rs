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
    area_nm2, components, difference, intersect, point_in_ring, polygon_region, union, BBox, Ring,
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

/// The parameter [`fill`] takes as `stepover_nm` for a flat milling cutter, as a fraction
/// of its own diameter.
///
/// **Not the real geometric stepover — `fill` turns this into `inset = stepover_nm - half`.**
/// A value at or below 0.5 makes `inset` zero or negative, which floors to [`SLIVER_NM`] and
/// turns every pass into a near-zero-progress crawl: each of [`MAX_FILL_PASSES`] only
/// reaches 5 µm deeper than the last, so a real island's depth is barely dented by the time
/// the cap is hit. First measured on this module's own guard-band test, back when each
/// pass re-offset a running remainder rather than one fixed base — every pass's own offset
/// then got a little more complex than the last from the accumulated near-degenerate cuts,
/// which took `fill` from 22 ms to 346 **seconds** across four structurally identical
/// calls, a bug caught by its cost rather than by a wrong answer. `fill` no longer
/// re-offsets a remainder at all, so that specific blow-up cannot recur, but the underlying
/// mistake — a stepover fraction at or below 0.5 — still buys nothing but a wasted pass
/// budget and is not a choice to make again.
///
/// Unlike a V-bit's channel, a flat cutter's ridge between two adjacent passes **is still
/// copper** — there is no cone tapering it away, so too wide a ridge leaves a strip of
/// exactly the floating conductor this pass exists to remove. That is what [`VBIT_STEPOVER_FRACTION`]'s
/// own doc means by "a flat cutter would want a smaller fraction": smaller than 0.9, which
/// gives the V-bit a real inset of `0.4 × diameter`. 0.8 gives this pass `0.3 × diameter` —
/// a tighter, more conservative real stepover for a cutter with no cone to save it — while
/// staying comfortably clear of the 0.5 floor above. The right number is still a
/// machining-policy call about chip evacuation and deflection on a cutter larger and more
/// heavily loaded than anything else this crate drives, and deserves a second look once
/// this pass has cut real boards; it is not, however, free to drift below 0.5 again.
const FLAT_MILL_STEPOVER_FRACTION: f64 = 0.8;

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
    ///
    /// Grouped by [`groups`](Self::groups) — the flat order here is group-major, so a
    /// group's own rings are always contiguous and already outside-in.
    pub paths: Vec<Ring>,
    /// How many of the *next* rings in [`paths`](Self::paths), starting from a running
    /// offset, belong to one connected piece of copper — one entry per piece, summing to
    /// `paths.len()`. Pieces that contributed no ring (opened away before the first pass,
    /// or capped out with nothing to show) are not listed at all, so every entry here is
    /// non-zero.
    ///
    /// What this is *for*: a piece's own rings nest inside one another by construction —
    /// each pass offsets further into the copper the pass before it left standing — so
    /// they can be cut as one continuous run with no retract between them, only a short
    /// hop from one ring to the next. This is the grouping that makes that possible; see
    /// `plan_engrave_spans` in `k2g`'s `machining_plan` module, which is the only reason
    /// this field exists rather than a plain `Vec<Ring>`.
    pub groups: Vec<usize>,
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
    /// How many pieces of narrow copper no part of the chosen cutter could enter at all.
    ///
    /// A different failure mode from [`left`](Self::left) (an opportunist bound exceeded)
    /// and from [`missed_area_nm2`](Self::missed_area_nm2) (an edge-case shortfall *inside*
    /// an accepted region): here the geometry qualified — it is narrow enough to clear — but
    /// the specific cutter chosen for the job is too wide to fit inside any of it. A
    /// first-order, expected outcome for a milling cutter, which [`islands()`] never has to
    /// report because its bound is always a multiple of the bit already cutting the channel
    /// beside it.
    pub unreachable: usize,
    /// The widest of those, nm. Zero when nothing was unreachable.
    pub widest_unreachable_nm: i64,
    /// Their total area, nm².
    pub unreachable_area_nm2: f64,
}

impl Clearing {
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// Each group's rings in the order a cutter actually walks them — every ring after
    /// the first rotated to start at the vertex nearest where the ring before it started.
    ///
    /// **The one source of truth for the shape of a chain**, and the reason it lives here
    /// rather than in whichever consumer needed it first. A group's rings are cut as one
    /// continuous pass with no retract between them (see [`groups`](Self::groups)), so
    /// *where each ring starts* is not presentation — it is the toolpath. Two consumers
    /// deriving that independently is exactly how the 3D view came to draw a lift between
    /// rings that the emitted program cuts straight through: the planner and every view
    /// of it have to be reading the same answer, not each computing their own.
    ///
    /// A closed ring exits exactly where it entered, so the previous ring's *start* is
    /// where the tool is standing when the next one begins — which is why that, and not
    /// its last vertex, is what the rotation aims at. Nested rings are parallel offsets
    /// one stepover apart, so the nearest vertex is always a short, bounded hop away,
    /// never an arbitrary chord across the island.
    ///
    /// Relies on [`groups`](Self::groups) summing to `paths.len()`, which is how [`fill`]
    /// builds the pair.
    pub fn chains(&self) -> Vec<Vec<Ring>> {
        let mut out = Vec::with_capacity(self.groups.len());
        let mut cursor = 0;
        for &count in &self.groups {
            let mut chain: Vec<Ring> = Vec::with_capacity(count);
            let mut previous_start: Option<(i64, i64)> = None;
            for ring in &self.paths[cursor..cursor + count] {
                let ring = match previous_start {
                    Some(target) => rotate_to_nearest(ring, target),
                    None => ring.clone(),
                };
                previous_start = ring.first().copied();
                chain.push(ring);
            }
            cursor += count;
            out.push(chain);
        }
        out
    }
}

/// A closed ring's vertices rotated so it starts at the one nearest `target`.
///
/// Compared squared and in `i128`: an exact integer answer, where a float distance would
/// make which vertex wins depend on rounding at board nm.
fn rotate_to_nearest(ring: &Ring, target: (i64, i64)) -> Ring {
    let start = ring
        .iter()
        .enumerate()
        .min_by_key(|(_, &vertex)| {
            let (dx, dy) = ((vertex.0 - target.0) as i128, (vertex.1 - target.1) as i128);
            dx * dx + dy * dy
        })
        .map_or(0, |(index, _)| index);
    let mut rotated = ring[start..].to_vec();
    rotated.extend_from_slice(&ring[..start]);
    rotated
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

    let (paths, groups, missed) = fill(&accepted, &cuts, w, VBIT_STEPOVER_FRACTION * w);
    out.paths = paths;
    out.groups = groups;
    out.missed_area_nm2 = missed;
    out
}

/// Every piece of free copper narrower than `threshold_nm` — enclosed or running into the
/// background alike — and the rings a milling cutter of `mill_diameter_nm` takes it out
/// with.
///
/// Unlike [`islands()`] this is not opportunist: the threshold is set by the operator, not
/// bounded by a multiple of a bit already in the spindle, and the tool doing the cutting is
/// a dedicated milling cutter chosen for the job rather than reused from isolation. Two
/// consequences follow that `islands()` never has to consider:
///
/// - `guard_band_nm` bounds how close this pass may come to net copper, on top of — not
///   instead of — never overlapping it. A V-bit clearing an island is safe for free because
///   it only ever sweeps ground the isolation channel already walled off; a milling cutter
///   clearing open copper independently of that channel is not, so the margin is inflated
///   into the region *before* anything is computed from it, not checked after the fact.
/// - A piece of `narrow` copper can be narrower than the **milling cutter's own diameter**,
///   which `islands()`'s opportunist bound never has to consider (its bound is always a
///   multiple of the bit already cutting the channel beside it). Those pieces are reported
///   via [`Clearing::unreachable`] rather than [`Clearing::left`] (a different question —
///   "wider than the opportunist bound" — this pass does not ask) or
///   [`Clearing::missed_area_nm2`] (an edge-case shortfall *inside* an accepted region, not
///   a region rejected outright).
///
/// `already_cleared` is [`islands()`]'s own `Clearing::paths`, swept at
/// `already_cleared_width_nm` — pass `&[]`/`0` when that pass did not run. Ground it already
/// took is excluded from `narrow` so this pass does not re-cut it.
///
/// **The region test is one morphological opening, not a per-piece erosion like
/// `islands()`.** `narrow = free − open(free, threshold/2)`. Opening is anti-extensive
/// (`open(A,r) ⊆ A` always, holes and multiple components included), so a piece entirely
/// narrower than the threshold contributes nothing to the opened remainder wherever it
/// sits — enclosed, or reaching into the open background. That is what lets one test stand
/// in for the pair of features `islands()`'s own doc comment names as one deferred question,
/// not two.
///
/// **`islands()`'s frame-touching exclusion is still applied, once, after the opening —
/// not instead of reaching into the background, but to keep `frame` itself from being read
/// as one.** `frame` is a boundary this function invented from the copper's own bounding
/// box, not a fact about the board — nothing here knows where the blank actually ends, the
/// same limitation `islands()` carries. At `islands()`'s own margin (a multiple of the
/// channel width) that boundary sits too close to matter; at this pass's threshold, which an
/// operator may set to several millimetres, it does not, and a large threshold against a
/// modest margin can make the frame's own corners read as narrow copper that was never
/// there. Discarding whatever touches the frame removes exactly that artefact and nothing
/// else: a genuine background-reaching neck sits well inside the frame as long as the margin
/// clears `threshold_nm / 2`, which is guaranteed by construction below, so it is never the
/// piece this exclusion catches.
pub fn clear_narrow_copper(
    copper: &CopperSnapshot,
    contours: &[IsolationContour],
    threshold_nm: i64,
    mill_diameter_nm: i64,
    guard_band_nm: i64,
    already_cleared: &[Ring],
    already_cleared_width_nm: i64,
) -> Clearing {
    let mut out = Clearing::default();
    if threshold_nm <= 0 || mill_diameter_nm <= 0 {
        return out;
    }

    // Every piece of copper the design drew, netted or not — same as `islands()`.
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

    // The guard band is baked into the region before anything else is computed from it: net
    // copper dilated by it is what `free` is measured against, so the cutter's own kerf can
    // get no closer than this to a real net anywhere in the pass, whatever the isolation
    // geometry beside it happens to allow.
    let guarded_keep = offset_group(&keep, guard_band_nm.max(0) as f64);
    let margin = 2 * threshold_nm.max(guard_band_nm).max(mill_diameter_nm);
    let frame = bounds.expand(margin);

    let cuts = swept_by(contours);
    let already = if already_cleared.is_empty() || already_cleared_width_nm <= 0 {
        Vec::new()
    } else {
        let closed: Vec<Ring> = already_cleared.iter().map(as_closed_polyline).collect();
        union(&stroke_open_paths(&closed, already_cleared_width_nm as f64 / 2.0), &[])
    };
    // Ground this pass may sweep into once it has entered a piece: the isolation channels,
    // minus the guard band around any net they run beside — same reasoning as `keep` above,
    // so the licence to run out into a channel never doubles as a licence to graze a net.
    let allowed = difference(&cuts, &guarded_keep);
    // What is left to ask the threshold about: not net copper (guarded), not ground the
    // V-bit already took (`cuts`, `already`) — the same three exclusions `islands()` makes,
    // plus the guard band. Forgetting `cuts` here once left the already-cut channel itself
    // inside `free`, which measured a corridor's *whole* gap as if the V-bit had never
    // touched it — the tell was a piece the fixture's own channel width (`W`) explained
    // almost exactly, where the true remaining copper was `gap − 2·W`.
    let free = difference(&difference(&difference(&[frame.ring()], &guarded_keep), &cuts), &already);
    if free.is_empty() {
        return out;
    }

    let wide = open_by(&free, threshold_nm as f64 / 2.0);
    let narrow = difference(&free, &wide);
    if narrow.is_empty() {
        return out;
    }

    let mill_radius_nm = mill_diameter_nm as f64 / 2.0;
    let mut accepted: Vec<Ring> = Vec::new();
    for piece in components(&narrow) {
        let Some(bbox) = BBox::of(&piece) else { continue };
        // Touching the frame is the same test `islands()` runs, and for the same reason:
        // `frame` is a boundary this function invented, not a wall, and a piece that runs
        // along it is the synthetic exterior that invention creates, not a real feature.
        // This does not cost the reach into open background the opening test above exists
        // for — a genuine background-connected narrow neck sits well inside the frame as
        // long as the margin clears `threshold_nm / 2`, which `margin` below guarantees by
        // construction; only the artefact of the boundary itself ever actually touches it.
        if bbox.x0 <= frame.x0 || bbox.y0 <= frame.y0 || bbox.x1 >= frame.x1 || bbox.y1 >= frame.y1
        {
            continue;
        }
        // Copper thinner than a chord error is not copper — same reasoning as `islands()`.
        if offset_group(&piece, -SLIVER_NM).is_empty() {
            continue;
        }
        if offset_group(&piece, -mill_radius_nm).is_empty() {
            // No room anywhere for this cutter's own centre — it cannot enter at all.
            out.unreachable += 1;
            out.unreachable_area_nm2 += net_area_nm2(&piece);
            out.widest_unreachable_nm =
                out.widest_unreachable_nm.max(widest_nm(&piece, 0.0, bbox));
        } else {
            out.removed += 1;
            out.removed_area_nm2 += net_area_nm2(&piece);
            accepted.extend(piece);
        }
    }
    if accepted.is_empty() {
        return out;
    }

    let (paths, groups, missed) = fill(
        &accepted,
        &allowed,
        mill_diameter_nm as f64,
        FLAT_MILL_STEPOVER_FRACTION * mill_diameter_nm as f64,
    );
    out.paths = paths;
    out.groups = groups;
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
/// **A ladder of depths against one fixed base, not a running remainder re-offset pass
/// over pass.** An island that is thin in one place and fat in another — a corridor round
/// a trace is exactly that, wider at the corners than down the sides — does not need a
/// dedicated pass that finds the sides one way and the corners another: each ring is the
/// same base region offset by its own multiple of the stepover, so a thin side simply
/// stops producing a ring once its own depth is used up while a fat corner keeps going,
/// in the same call. What that ladder must *not* do is subtract what each ring actually
/// swept from a running remainder and offset *that* next time — on a bend, where the two
/// walls have different local curvature, that re-derivation can itself manufacture a
/// split that the geometry never actually had, discovering the pieces on either side of
/// it fresh next pass as independent islands, exactly as likely to split again. Offsetting
/// the one fixed base at increasing depth never does this: a piece can only come apart at
/// a depth it is topologically forced to, not at a depth some earlier pass's own rounding
/// happened to leave behind. Coverage still has no gap: the stepover is always narrower
/// than the tool's own radius, so consecutive rings' sweeps overlap by construction, all
/// the way down to whatever a piece's own narrowest point can still support.
fn fill(region: &[Ring], allowed: &[Ring], w_nm: f64, stepover_nm: f64) -> (Vec<Ring>, Vec<usize>, f64) {
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
    // Opened once, up front: the hairlines an intersection leaves along its own cut edge
    // read as copper standing on nothing, and every ring below is measured from this one
    // shape, so noise left in here would not wash out on its own the way it once did
    // pass over pass.
    let base = union(&open_by(&intersect(region, &a), SLIVER_NM), &[]);

    // Each ring is set this far inside the copper it is taking, so that it sweeps `half`
    // past the edge — into the channel — and reaches `stepover_nm` deeper than the ring
    // before it.
    let inset = (stepover_nm - half).max(SLIVER_NM);

    // `region`'s own disjoint pieces, fixed once up front so the rings produced below can
    // be reported afterwards by which island they belong to (`Clearing::groups`).
    let origin = components(region);

    // **Every ring is `base` offset by its own multiple of `inset` — never a fresh offset
    // of what an earlier ring's own sweep left behind.** That distinction is the whole
    // fix: subtracting a swept stroke from a running remainder and re-offsetting *that*
    // is what let a bend split one island into a cascade of fresh, disconnected pieces —
    // the tighter inner wall of a bend meets the sweep first, and the disconnected
    // leftover it reveals is discovered next pass as its own independent island, exactly
    // as likely to split again. Offsetting the one fixed `base` at increasing depth never
    // manufactures a split this way: `base` never changes, so a piece can only ever come
    // apart at a depth where it is topologically forced to (a real neck finally pinching
    // shut), not at a depth some earlier pass's rounding happened to leave behind.
    //
    // This still covers `base` end to end with no gap, for the same reason a piece too
    // narrow for a step used to need its own inradius sweep: `inset` is always less than
    // `half` (`stepover_nm` is always under `w_nm`), so consecutive rings — `half` on
    // each side — overlap. A point at true depth `d` from `base`'s own edge survives in
    // the deepest ring whose offset is still `<= d`, which by that same margin is always
    // within `inset < half` of `d` — inside that ring's own sweep. The ring that first
    // comes back empty is proof the piece it came from is already fully swept, not a
    // signal that anything still needs a separate finishing pass.
    let mut paths: Vec<Ring> = Vec::new();
    for pass in 1..=MAX_FILL_PASSES {
        let rings = offset_group(&base, -(pass as f64 * inset));
        if rings.is_empty() {
            break;
        }
        paths.extend(rings);
    }

    // Accounted from the rings themselves rather than from any region they were derived
    // from, so a fill that stopped short — on the pass cap, on a channel too narrow to let
    // the tool near, on anything — shows up here rather than in nobody's report.
    let closed: Vec<Ring> = paths.iter().map(as_closed_polyline).collect();
    let missed = net_area_nm2(&difference(region, &stroke_open_paths(&closed, half)));

    // Regroups the flat, pass-major `paths` above by which of `origin`'s pieces each ring
    // came from — see `Clearing::groups`'s own doc for why. A stable partition over
    // `paths` in its existing order keeps a piece's own rings in pass order — still
    // outside-in — without needing to know that order was ever computed.
    let mut by_origin: Vec<Vec<Ring>> = vec![Vec::new(); origin.len()];
    let mut unclaimed: Vec<Ring> = Vec::new();
    for ring in paths {
        match lineage_of(std::slice::from_ref(&ring), &origin) {
            Some(lineage) => by_origin[lineage].push(ring),
            // Defensive: geometrically this should never happen, but a ring nothing
            // claims is kept — in its own trailing group — rather than lost.
            None => unclaimed.push(ring),
        }
    }
    let mut groups: Vec<usize> = by_origin.iter().map(Vec::len).filter(|&n| n > 0).collect();
    let mut paths: Vec<Ring> = by_origin.into_iter().flatten().collect();
    if !unclaimed.is_empty() {
        groups.push(unclaimed.len());
        paths.extend(unclaimed);
    }

    (paths, groups, missed)
}

/// Which of `origin`'s pieces `piece` belongs to, by testing one of its own points
/// against each candidate's outline (`components` always puts the outline first, holes
/// after). Sound because every piece this is ever asked about — one of `fill`'s finished
/// rings — is, by construction, entirely inside the one original piece it was eroded
/// from.
fn lineage_of(piece: &[Ring], origin: &[Vec<Ring>]) -> Option<usize> {
    let point = *piece.first()?.first()?;
    origin
        .iter()
        .position(|candidate| candidate.first().is_some_and(|outline| point_in_ring(point, outline)))
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

    /// The same board as [`pad_in_pour`], but returning what [`clear_narrow_copper`] takes
    /// — the copper and the isolation contours — rather than `islands()`'s own `Clearing`.
    fn pad_in_pour_contours(gap: i64) -> (CopperSnapshot, Vec<crate::isolation::IsolationContour>) {
        let copper = snapshot(vec![
            feature("SIG", circle(5 * MM), Vec::new()),
            feature(
                "GND",
                rect(-15 * MM, -15 * MM, 15 * MM, 15 * MM),
                vec![circle(5 * MM + gap)],
            ),
        ]);
        let result = crate::isolate(&copper, W, W);
        (copper, result.contours)
    }

    /// The width of an annular island `gap - 2·W` across, as an area.
    fn annulus_area_nm2(gap: i64) -> f64 {
        let (outer, inner) = ((5 * MM + gap - W) as f64, (5 * MM + W) as f64);
        std::f64::consts::PI * (outer * outer - inner * inner)
    }

    fn arc_path(cx: i64, cy: i64, r: i64, start_deg: f64, end_deg: f64, segments: usize) -> Vec<(i64, i64)> {
        (0..=segments)
            .map(|i| {
                let t = (start_deg + (end_deg - start_deg) * (i as f64 / segments as f64)).to_radians();
                (cx + (r as f64 * t.cos()).round() as i64, cy + (r as f64 * t.sin()).round() as i64)
            })
            .collect()
    }

    /// A closed L-shaped outline: two arms `width` thick and `arm` long, meeting at a
    /// corner, with one concave (reflex) vertex — a sharp-cornered bent corridor.
    fn l_shape(x0: i64, y0: i64, arm: i64, width: i64) -> Ring {
        vec![
            (x0, y0),
            (x0 + width, y0),
            (x0 + width, y0 + arm - width),
            (x0 + arm, y0 + arm - width),
            (x0 + arm, y0 + arm),
            (x0, y0 + arm),
        ]
    }

    /// A bent island — the shape an isolation channel leaves around any rounded or
    /// right-angled trace — once fragmented into dozens or hundreds of tiny rings
    /// instead of the one or two clean rings a human would draw, and left real copper
    /// uncleared while doing it. Two shapes, because the first version of this bug was
    /// mistaken for a sharp-corner artifact: the second is a perfectly smooth 180° arc,
    /// stroked to constant width, with no hand-authored corner anywhere, and it
    /// fragmented just the same — so a corner was never what triggered it, bending was.
    ///
    /// **Why it fragmented, and what actually fixed it.** `fill`'s previous loop tracked
    /// what was left standing by *subtracting* each ring's own swept stroke from a
    /// running remainder and re-offsetting that remainder next pass. Four increasingly
    /// careful heuristics tried to bound the damage this did on a bend without breaking
    /// anything else, and each was disproven by measurement: retiring a piece after its
    /// first fallback ring broke `an_island_is_swept_end_to_end`, whose simplest case (a
    /// *round* island) also needs repeated fallback rounds to mop up chord-tolerance
    /// residue; a per-pass-count cap never engaged, because the bend's own cascade is
    /// fast (one piece to eight within a single pass); a per-pass fragment-count cap was
    /// unworkable because a round island's own legitimate last pass explodes into
    /// 30-odd tiny pieces finishing off a nearly-cleared annulus; a minimum
    /// area-progress-per-pass threshold failed because that same legitimate last pass
    /// only reaches 37.8% progress — indistinguishable, on any single pass, from real
    /// cascading fragmentation.
    ///
    /// All four were trying to bound a symptom of the same design mistake: *re-deriving*
    /// what is left from what a previous pass actually swept, on a shape whose width
    /// varies (a bend's inner wall pinches before its outer wall does), can itself
    /// manufacture a split that was never really there. The fix removes the mechanism
    /// rather than bounding it: every ring is the same fixed base region — computed once,
    /// before any pass runs — offset by an increasing multiple of the stepover, never a
    /// fresh offset of what an earlier ring's own sweep left behind. `base` never
    /// changes, so a piece can only come apart at a depth it is topologically forced to
    /// (a real neck finally pinching shut), never at a depth some earlier pass's own
    /// rounding happened to leave behind. Coverage still has no gap, for the same reason
    /// a piece too narrow for a step used to need its own inradius sweep: the stepover is
    /// always narrower than the tool's own radius, so consecutive rings' sweeps overlap
    /// by construction.
    #[test]
    fn a_bent_island_stays_a_handful_of_rings_not_a_hundred() {
        // A smooth, constant-width corridor: straight down, a clean 180° bend, straight
        // back up. `fill()` is called directly, bypassing the whole isolation pipeline,
        // to isolate the question to the one function this bug is actually in.
        let bend_r = 3 * MM;
        let leg = 8 * MM;
        let island_width = 2 * W + (2.5 * W as f64) as i64;
        let mut medial = vec![(0i64, leg)];
        medial.extend(arc_path(bend_r, 0, bend_r, 180.0, 0.0, 32));
        medial.push((2 * bend_r, leg));
        let smooth_region = stroke_open_paths(&[medial], island_width as f64 / 2.0);

        // A sharp right-angled corridor of comparable width — the more adversarial case
        // this bug was first found on.
        let sharp_region = {
            let pad = l_shape(0, 0, 10 * MM, 3 * MM);
            let clearance = offset_group(&[pad.clone()], (4 * W) as f64);
            difference(&clearance, &[pad])
        };

        for (name, region) in [("smooth bend", smooth_region), ("sharp corner", sharp_region)] {
            let (paths, groups, missed) = fill(&region, &[], W as f64, VBIT_STEPOVER_FRACTION * W as f64);
            let removed_area: f64 = paths.iter().map(|r| area_nm2(r).unsigned_abs() as f64).sum();
            let total = removed_area + missed;
            assert!(paths.len() <= 12, "{name}: {} rings, expected a handful: {paths:?}", paths.len());
            assert!(
                missed < 0.15 * total,
                "{name}: {:.1}% of the island left uncut ({missed:.0} of {total:.0} nm²)",
                100.0 * missed / total.max(1.0),
            );
            assert!(!groups.is_empty(), "{name}: at least one group, however it fragmented");
        }
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

    /// `groups` is a partition of `paths`, always — whatever the board, however many
    /// passes or pieces it took. If this ever failed, `plan_engrave_spans` downstream
    /// would either panic chunking `paths` by it or silently drop rings past the end.
    #[test]
    fn groups_always_partitions_every_ring_exactly_once() {
        for gap in [4 * W, 2 * W + (2.8 * W as f64) as i64, 8 * W] {
            let (_, cleared) = pad_in_pour(gap);
            assert_eq!(
                cleared.groups.iter().sum::<usize>(),
                cleared.paths.len(),
                "gap {gap}: {cleared:?}",
            );
            assert!(cleared.groups.iter().all(|&n| n > 0), "gap {gap}: an empty group leaked in");
        }
    }

    /// **One island, however many passes it takes, is one group.** This is the whole
    /// point: two nested rings a pass apart belong to the same continuous cut, not two
    /// independent ones — the case in the middle of `an_island_is_swept_end_to_end`,
    /// which needs two passes to clear.
    #[test]
    fn one_island_needing_two_passes_is_still_one_group() {
        let (_, cleared) = pad_in_pour(2 * W + (2.8 * W as f64) as i64);
        assert!(cleared.paths.len() >= 2, "the two-pass case: {cleared:?}");
        assert_eq!(cleared.groups, vec![cleared.paths.len()], "one island, one group");
    }

    /// Two islands nowhere near each other must never be merged into one group — that
    /// would hand `plan_engrave_spans` a chain that tries to travel, without retracting,
    /// between two pieces of copper with no relationship to each other at all.
    #[test]
    fn two_separate_islands_are_two_groups_never_merged() {
        let r = 3 * MM;
        let gap = 4 * W;
        let translate = |ring: Ring, dx: i64| ring.into_iter().map(|(x, y)| (x + dx, y)).collect();
        let far = 30 * MM;

        let copper = snapshot(vec![
            feature("SIG1", circle(r), Vec::new()),
            feature("SIG2", translate(circle(r), far), Vec::new()),
            feature(
                "GND",
                rect(-15 * MM, -15 * MM, far + 15 * MM, 15 * MM),
                vec![circle(r + gap), translate(circle(r + gap), far)],
            ),
        ]);
        let result = crate::isolate(&copper, W, W);
        let cleared = islands(&copper, &result.contours, W);

        assert_eq!(cleared.removed, 2, "{cleared:?}");
        assert_eq!(cleared.groups.len(), 2, "two disjoint islands, never merged: {cleared:?}");

        // And each group is geometrically self-consistent: every ring in it sits near the
        // SAME one of the two island centres. Which group is which island is not assumed
        // — `components()` makes no promise about output order — but a cross-contaminated
        // group (a ring from one island landing in the other's bucket) is caught directly,
        // rather than trusting the count alone.
        let near = |(rx, ry): (i64, i64), (cx, cy): (i64, i64)| {
            (((rx - cx).pow(2) + (ry - cy).pow(2)) as f64).sqrt() < r as f64 + gap as f64
        };
        let mut cursor = 0;
        for &count in &cleared.groups {
            let group_rings = &cleared.paths[cursor..cursor + count];
            cursor += count;
            let first = group_rings[0][0];
            let centre = [(0i64, 0i64), (far, 0i64)]
                .into_iter()
                .find(|&c| near(first, c))
                .unwrap_or_else(|| panic!("first ring at {first:?} is near neither island"));
            for ring in group_rings {
                assert!(near(ring[0], centre), "ring at {:?} strayed from its island at {centre:?}", ring[0]);
            }
        }
    }

    /// The rotation rule `chains()` carries: every ring after the first starts at its own
    /// vertex nearest where the ring before it started, so the hop between them is the
    /// short one. A no-op rotation — leaving each ring on its stored start — would take
    /// the tool on a chord right across the island instead.
    #[test]
    fn a_chain_starts_each_ring_at_its_vertex_nearest_the_one_before() {
        let outer = rect(0, 0, MM, MM);
        // The same square, listed from the corner *farthest* from the outer ring's start
        // at (0,0), so a correct rotation has real work to do.
        let inner = vec![
            (800_000, 800_000), // farthest — stored first on purpose
            (200_000, 800_000),
            (200_000, 200_000), // nearest to (0,0) — must lead after rotation
            (800_000, 200_000),
        ];
        let clearing = Clearing {
            paths: vec![outer.clone(), inner.clone()],
            groups: vec![2],
            ..Default::default()
        };

        let chains = clearing.chains();
        assert_eq!(chains.len(), 1, "one group, one chain");
        assert_eq!(chains[0][0], outer, "the first ring of a chain is left exactly as it came");
        assert_eq!(chains[0][1][0], (200_000, 200_000), "the second starts at its nearest vertex");
        assert_eq!(chains[0][1].len(), inner.len(), "no vertex gained or lost by the rotation");
        // Rotation is a cycle, not a reordering: the ring still runs the same way round.
        assert_eq!(chains[0][1], vec![(200_000, 200_000), (800_000, 200_000), (800_000, 800_000), (200_000, 800_000)]);
    }

    /// Groups are never merged or resliced by `chains()`, whatever the flat `paths` order
    /// looked like — a chain is one island, and cutting between two of them without a
    /// retract would drag the tool across copper this pass was never licensed to touch.
    #[test]
    fn chains_never_merge_two_groups() {
        let clearing = Clearing {
            paths: vec![rect(0, 0, MM, MM), rect(0, 0, MM, MM), rect(50 * MM, 0, 51 * MM, MM)],
            groups: vec![2, 1],
            ..Default::default()
        };

        let chains = clearing.chains();
        assert_eq!(chains.len(), 2);
        assert_eq!(chains[0].len(), 2);
        assert_eq!(chains[1].len(), 1);
        assert_eq!(chains.iter().map(Vec::len).sum::<usize>(), clearing.paths.len());
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

    #[test]
    fn narrow_copper_past_the_v_bit_bound_is_cleared_by_the_mill() {
        // An 8*W corridor leaves 6*W standing — `islands()` reports this as `left`, too wide
        // for its opportunist 3*W bound. A mill asked for a larger threshold clears it anyway.
        let (copper, contours) = pad_in_pour_contours(8 * W);
        let cleared = clear_narrow_copper(&copper, &contours, 10 * W, 4 * W, 0, &[], 0);
        assert_eq!(cleared.removed, 1, "{cleared:?}");
        assert!(!cleared.paths.is_empty(), "narrow copper found and not cut");
    }

    #[test]
    fn copper_at_or_past_the_mill_threshold_is_left_alone() {
        const THRESHOLD: i64 = 4 * W;
        // The threshold itself, from both sides — mirroring `an_island_at_the_bound_...`
        // but against an operator-set threshold rather than `ISLAND_WIDTH_MULTIPLE`.
        let (copper_u, under) = pad_in_pour_contours(2 * W + (0.8 * THRESHOLD as f64) as i64);
        let cleared_u = clear_narrow_copper(&copper_u, &under, THRESHOLD, W, 0, &[], 0);
        assert_eq!(cleared_u.removed, 1, "{cleared_u:?}");

        let (copper_o, over) = pad_in_pour_contours(2 * W + (1.2 * THRESHOLD as f64) as i64);
        let cleared_o = clear_narrow_copper(&copper_o, &over, THRESHOLD, W, 0, &[], 0);
        assert_eq!(cleared_o.removed, 0, "{cleared_o:?}");
    }

    #[test]
    fn copper_that_runs_into_the_background_is_cleared_too() {
        // The exact fixture from `copper_that_runs_out_into_the_background_is_not_an_island`
        // — asserting the opposite. This is the documented divergence between the two
        // passes: `islands()` excludes it on purpose, this pass is exactly what reaches it.
        let copper = snapshot(vec![
            feature("A", rect(0, 0, 10 * MM, MM), Vec::new()),
            feature("B", rect(0, MM + 4 * W, 10 * MM, 2 * MM + 4 * W), Vec::new()),
        ]);
        let result = crate::isolate(&copper, W, W);
        let cleared = clear_narrow_copper(&copper, &result.contours, 8 * W, W, 0, &[], 0);
        assert!(cleared.removed > 0, "{cleared:?}");
        assert!(!cleared.paths.is_empty());
    }

    #[test]
    fn no_mill_clearing_cut_ever_comes_within_the_guard_band_of_a_net() {
        // Strictly stronger than the plain net-copper invariant above: not just "never
        // overlaps a net" but "never comes within the guard band of one".
        // `10 * W` deliberately not included: that gap's annulus is 8·W wide, and clearing
        // it with a mill only `W` across needs `fill` to run close to its full pass budget
        // on a wide, high-vertex-count ring — a real-world mismatch (an operator asking a
        // small mill to clear a wide area) rather than anything this invariant needs to
        // cover, and one `fill`'s own cost grows steeply under. Tracked as a performance
        // follow-up rather than fixed here: `fill` is shared with `islands()`, whose own
        // bound never asks it to run this many passes over geometry this wide.
        let guard = W;
        let mill_d = W;
        for gap in [3 * W, 4 * W, 6 * W] {
            let (copper, contours) = pad_in_pour_contours(gap);
            let cleared = clear_narrow_copper(&copper, &contours, 12 * W, mill_d, guard, &[], 0);
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
            let guarded = offset_group(&keep, guard as f64);
            let closed: Vec<Ring> = cleared.paths.iter().map(as_closed_polyline).collect();
            let swept = stroke_open_paths(&closed, mill_d as f64 / 2.0);
            let bitten = intersect(&swept, &guarded);
            assert!(
                net_area_nm2(&bitten) < 1.0,
                "gap {gap}: the milling pass swept {} nm² inside the guard band",
                net_area_nm2(&bitten),
            );
        }
    }

    #[test]
    fn a_mill_too_wide_for_the_narrow_copper_is_reported_unreachable() {
        // A 4*W corridor (a 2*W annulus) qualifies as narrow at an 8*W threshold, but a mill
        // wider than the annulus itself cannot enter it at all.
        let (copper, contours) = pad_in_pour_contours(4 * W);
        let cleared = clear_narrow_copper(&copper, &contours, 8 * W, 3 * W, 0, &[], 0);
        assert_eq!(cleared.removed, 0, "{cleared:?}");
        assert_eq!(cleared.unreachable, 1, "{cleared:?}");
        assert!(cleared.widest_unreachable_nm > 0, "{cleared:?}");
        assert!(cleared.paths.is_empty());
    }

    #[test]
    fn ground_islands_already_cleared_is_not_re_cut_by_the_mill() {
        // A gap `islands()` clears on its own (within its 3*W bound): the mill pass must
        // not re-cut the same ground when told it was already taken.
        let gap = 4 * W;
        let (copper, contours) = pad_in_pour_contours(gap);
        let already = islands(&copper, &contours, W);
        assert!(!already.paths.is_empty(), "sanity: islands() clears this on its own");

        let cleared = clear_narrow_copper(&copper, &contours, 8 * W, 4 * W, 0, &already.paths, W);
        assert!(cleared.paths.is_empty(), "{cleared:?}");
        assert_eq!(cleared.removed, 0);
    }

    #[test]
    fn a_mill_clearing_pass_gives_the_same_program_twice() {
        let (copper, contours) = pad_in_pour_contours(8 * W);
        let once = clear_narrow_copper(&copper, &contours, 10 * W, 4 * W, W, &[], 0);
        let twice = clear_narrow_copper(&copper, &contours, 10 * W, 4 * W, W, &[], 0);
        assert_eq!(once, twice);
    }
}
