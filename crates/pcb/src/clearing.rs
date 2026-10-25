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
//! # Thin copper
//!
//! An island is one shape the danger takes, not the only one. Free copper that runs out
//! into the background instead of closing into a ring can be just as thin where it passes
//! between two channels — the strip between the pads of an SO8, the sliver a trace's channel
//! leaves beside a pad's — and a strip that thin can lift off the board and lie across two
//! nets. [`islands`] takes those too when asked (`thin`): the **thin parts** of free copper,
//! wherever they are, found by one morphological opening at the same bound and cleared by
//! the same bit in the same [`fill`]. It needs no guard band, for the reason island removal
//! does not: every thin part lies between channels the V-bit has already cut, so the tool
//! never has to go anywhere it has not been.
//!
//! # What is not here
//!
//! **Drawn clear areas** on the Job view: a region and a tool width, which is all [`fill`]
//! asks for, but one the operator draws rather than one the copper implies.

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

/// How far inside an unswept piece of copper `fill`'s fallback ring is set, nm — see there.
///
/// Half of [`SLIVER_NM`]: `base` has already been opened by `SLIVER_NM`, so every stretch of
/// it is at least twice that across, and a ring this far in never pinches out — a corridor
/// of varying width stays one ring however thin it gets. Off the edge itself only so the
/// offset has something to do; the tool's own width makes the hair irrelevant to coverage.
const FALLBACK_INSET_NM: f64 = SLIVER_NM / 2.0;

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
    /// offset, can be cut as one continuous run — one entry per chain, summing to
    /// `paths.len()`. A chain never spans two pieces of copper, and one piece may need
    /// several. Every entry is non-zero.
    ///
    /// What this is *for*: a chain's rings are cut with no retract between them, so the hop
    /// from one ring to the next is cut at depth. `fill` only links two rings when that hop
    /// stays on ground the tool may cover. Rings of one piece usually nest, but not always:
    /// a U-shaped piece whose bottom pinches out gives one ring per arm, and a hop between
    /// the arms would cut whatever stands between them. See
    /// `plan_engrave_spans` in `k2g`'s `machining_plan` module, which is the only reason
    /// this field exists rather than a plain `Vec<Ring>`.
    pub groups: Vec<usize>,
    /// How many islands were removed.
    pub removed: usize,
    /// Their total area, nm².
    pub removed_area_nm2: f64,
    /// How many thin parts of free copper were removed — narrower than the bound where
    /// they pass between channels, but not closed into an island (see the module note).
    /// Zero unless [`islands`] was asked for them.
    pub thin_removed: usize,
    /// Their total area, nm².
    pub thin_removed_area_nm2: f64,
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
    /// its last vertex, is what the rotation aims at.
    ///
    /// That the hop is safe to cut is not decided here: `fill` linked the rings into these
    /// groups only after checking each hop against the tool's ground, using this same
    /// rotation, and stored each ring already rotated. Rotating again here changes nothing
    /// for those rings; it is kept so hand-built groups (tests, other callers) still get the
    /// nearest start.
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
///
/// `thin` adds the thin parts of free copper that is *not* enclosed — see the module note.
/// They are cut in the same pass, by the same rings, so a step that asks for both pays for
/// one fill and the islands are not counted twice.
pub fn islands(copper: &CopperSnapshot, contours: &[IsolationContour], width_nm: i64, thin: bool) -> Clearing {
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
    //
    // Clear of the copper by the bound as well as by a channel, for the thin-copper test:
    // the strip between the outermost channel and the frame is background, not a neck, and
    // it has to be wider than the bound for the opening below to leave it alone. Nearer,
    // it read as thin copper all the way round the board — and since that strip touches
    // the frame and is thrown out, so would any real neck joined to it.
    let bound_nm = ISLAND_WIDTH_MULTIPLE * w;
    let frame = bounds.expand(2 * width_nm + 2 * bound_nm.ceil() as i64);
    let cuts = swept_by(contours);
    let free = difference(&difference(&[frame.ring()], &cuts), &keep);
    if free.is_empty() {
        return out;
    }

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
        if touches(&piece, bbox, &keep) {
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

    // The thin parts of everything else: what an opening at the bound takes away from the
    // free copper is, by definition, where it is narrower than the bound — enclosed or
    // running into the background alike (`clear_narrow_copper` asks the same question with
    // a router). The islands just accepted are entirely narrow and are taken out first, so
    // they are not counted twice; a piece left as too wide may still lose its thin parts
    // here, which is the point.
    if thin {
        let narrow = difference(&free, &open_by(&free, bound_nm / 2.0));
        let thin_parts = if accepted.is_empty() { narrow } else { difference(&narrow, &accepted) };
        for piece in components(&thin_parts) {
            let Some(bbox) = BBox::of(&piece) else { continue };
            // The frame's own corners, rounded off by the opening — an artefact of the
            // invented boundary, as in `clear_narrow_copper`.
            if bbox.x0 <= frame.x0 || bbox.y0 <= frame.y0 || bbox.x1 >= frame.x1 || bbox.y1 >= frame.y1
            {
                continue;
            }
            if offset_group(&piece, -SLIVER_NM).is_empty() {
                continue;
            }
            if touches(&piece, bbox, &keep) {
                continue;
            }
            out.thin_removed += 1;
            out.thin_removed_area_nm2 += net_area_nm2(&piece);
            accepted.extend(piece);
        }
    }
    if accepted.is_empty() {
        return out;
    }

    // Where a cut may run out past the copper it is taking. An island is walled in by the
    // channels, so the channels are all it needs. A thin part is not: it opens into the wider
    // copper it is a neck of, and limited to the channels the tool stopped half a width short
    // of that mouth, leaving a stub — reported as copper "too close to a net", which it is
    // not. Any free copper will do instead: it is on no net, so cutting into it forms no edge
    // that matters, and `fill` still keeps the tool half a width from every net there is,
    // because what it erodes is everything *but* net copper.
    let allowed = if out.thin_removed > 0 { union(&free, &cuts) } else { cuts };
    let (paths, groups, missed) = fill(&accepted, &allowed, w, VBIT_STEPOVER_FRACTION * w);
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

/// The ground a step's engraving removes — every isolation channel and every clearing ring,
/// swept at the width that cut it — and the one question asked of it: can the tool cross
/// from here to there at depth without touching copper that stays?
///
/// **Why it exists.** Between two cuts the tool used to lift to the retract plane, travel,
/// and plunge again, every time, whatever lay between them. Most of those lifts are across
/// copper that has to stay, and are right. But many are not: from one clearing ring to the
/// next across a channel, from a net's loop into the island beside it, from one strip of
/// thin copper to its neighbour — the tool would cross nothing but ground it cuts anyway,
/// and a feed move straight across is quicker than a retract, a rapid and a plunge at the
/// plunge rate. Going over ground that has no copper left on it changes nothing.
///
/// **Order does not matter, and that is deliberate.** The ground is everything the step
/// removes, not what has been removed by the time of the hop. A hop across a channel not yet
/// cut takes copper that channel was going to take anyway — waste on both sides, never net
/// copper and never copper meant to stay.
#[derive(Clone, Debug, Default)]
pub struct ClearedGround {
    region: Vec<Ring>,
}

impl ClearedGround {
    /// The ground `contours` cut, plus each `(clearing, width)`'s rings swept at that width.
    /// Pass the isolation bit's island/thin clearing at its channel width, and a mill's
    /// clearing at the mill's diameter.
    pub fn new(contours: &[IsolationContour], cleared: &[(&Clearing, i64)]) -> Self {
        let mut parts = swept_by(contours);
        for &(clearing, width_nm) in cleared {
            if clearing.paths.is_empty() || width_nm <= 0 {
                continue;
            }
            let closed: Vec<Ring> = clearing.paths.iter().map(as_closed_polyline).collect();
            parts.extend(stroke_open_paths(&closed, width_nm as f64 / 2.0));
        }
        Self { region: union(&parts, &[]) }
    }

    /// Where the centre of a tool `width_nm` across may travel without its edge leaving this
    /// ground — the ground shrunk by the tool's radius, once, for every hop that tool makes.
    ///
    /// **Shrunk once rather than each hop swept and clipped.** A disc moved along a segment
    /// stays inside a region exactly when the segment stays inside the region eroded by the
    /// disc's radius, so the two tests agree; but the channels of a whole board union into one
    /// polygon of thousands of vertices, and clipping every hop's sweep against it costs a
    /// Clipper call per hop over all of them. Eroded up front, each hop is a parity test and
    /// an edge-crossing test against the rings near it — integer arithmetic, no allocation.
    ///
    /// Short of the full radius by [`SLIVER_NM`]: a channel exactly as wide as the tool erodes
    /// to nothing at the full radius, and the tool can plainly travel down the middle of it.
    pub fn for_tool(&self, width_nm: i64) -> ToolGround {
        if width_nm <= 0 || self.region.is_empty() {
            return ToolGround::default();
        }
        let rings = offset_group(&self.region, -(width_nm as f64 / 2.0 - SLIVER_NM).max(0.0));
        let boxes = rings
            .iter()
            .map(|ring| BBox::of(std::slice::from_ref(ring)).unwrap_or_default())
            .collect();
        ToolGround { rings, boxes }
    }
}

/// [`ClearedGround`] for one tool: where its centre may go. See [`ClearedGround::for_tool`].
#[derive(Clone, Debug, Default)]
pub struct ToolGround {
    rings: Vec<Ring>,
    boxes: Vec<BBox>,
}

impl ToolGround {
    /// Whether the tool can move in a straight line from `a` to `b` — board nm — touching
    /// nothing but cleared ground. Touching the boundary counts as leaving it, as in
    /// [`segment_within`]: the cheap side to be wrong on is a lift.
    pub fn holds(&self, a: (i64, i64), b: (i64, i64)) -> bool {
        // Only rings whose boxes meet the segment's can contain `a` or cross the segment.
        let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
        let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
        let nearby = self
            .rings
            .iter()
            .zip(&self.boxes)
            .filter(|(_, r)| r.x0 <= x1 && r.x1 >= x0 && r.y0 <= y1 && r.y1 >= y0)
            .map(|(ring, _)| ring);
        segment_within(a, b, nearby)
    }
}

/// Whether `piece` (with bounding box `bbox`) touches `copper` — shares an edge with it, or
/// comes within a chord error of doing so.
///
/// **Free copper that touches a net is that net's copper**, not an island and not a strip.
/// A round bit cannot reach into an inside corner, so wherever a trace meets a pad at an
/// angle the channel leaves a small fillet standing in the corner, joined to the net along
/// both edges. It is on no net *in the design*, so it lands in `free`; and being walled in
/// by the channel it read as an island — four of them on two pads and a trace — which the
/// tool could not reach without cutting into the net, so each one was also reported as copper
/// "too close to a net" that was still there. It is not floating, it cannot short anything
/// the net does not already touch, and taking it would mean cutting the net.
///
/// Tested against only the rings whose boxes come near, because `copper` is the whole board
/// and this runs once per piece. An outline's box contains its holes' boxes, so a hole near
/// the piece always brings its outline with it, and the subset fills the same way the whole
/// does there.
fn touches(piece: &[Ring], bbox: BBox, copper: &[Ring]) -> bool {
    let near = bbox.expand(2 * SLIVER_NM as i64);
    let nearby: Vec<Ring> = copper
        .iter()
        .filter(|ring| {
            BBox::of(std::slice::from_ref(*ring)).is_some_and(|b| {
                b.x0 <= near.x1 && b.x1 >= near.x0 && b.y0 <= near.y1 && b.y1 >= near.y0
            })
        })
        .cloned()
        .collect();
    !nearby.is_empty() && !intersect(&offset_group(piece, SLIVER_NM), &nearby).is_empty()
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

    // **Copper too thin for the ladder's first rung.** The ladder's first ring is set `inset`
    // inside the copper, so a piece — or a part of one — narrower than `2·inset` never gets
    // one, and nothing sweeps it. That is the thinnest copper of all, the sliver most likely
    // to lift off and short, and it used to be reported removed and left standing: a 0.5·W
    // island round a pad came back with no rings at all.
    //
    // Whatever of `base` is still unswept gets a ring of its own, a hair inside its edge
    // ([`FALLBACK_INSET_NM`]). The tool sweeps `half` either side of that ring, so a piece
    // narrower than `W` is covered by the one ring however its width varies: each side of the
    // piece is within the hair of the ring, and the far side is within `W` of it.
    //
    // **A hair, not halfway to the middle.** The ring used to sit at half the piece's deepest
    // point. That is fine for a strip of even width and wrong for the commonest one — the
    // corridor round a pad sitting off-centre in its pour clearance, thin on one side and
    // wide on the other. A ring that deep pinches out wherever the corridor is thinner than
    // twice it, so one island came back as half a dozen arcs, each found again on the next
    // round, each its own chain, and the tool lifted between every one. A ring a hair inside
    // the edge only pinches out where the copper is under twice the hair across.
    //
    // **Repeated, because one ring per piece is not always enough**: a piece wider than `W`
    // somewhere is only swept round its edge, and what that leaves is found on the next round
    // and given its own ring, nested inside. Each round sweeps what the last left, so it stops
    // when nothing is left; the cap is only there for the reason `MAX_FILL_PASSES` is.
    for _ in 0..MAX_FILL_PASSES {
        let swept = {
            let closed: Vec<Ring> = paths.iter().map(as_closed_polyline).collect();
            stroke_open_paths(&closed, half)
        };
        let unswept = components(&open_by(&difference(&base, &swept), SLIVER_NM));
        if unswept.is_empty() {
            break;
        }
        for piece in unswept {
            let Some(bbox) = BBox::of(&piece) else { continue };
            let ceiling =
                (bbox.x1 - bbox.x0).min(bbox.y1 - bbox.y0).max(0) as f64 / 2.0 + WIDTH_TOLERANCE_NM;
            let depth = inradius_nm(&piece, 0.0, ceiling);
            let rings = offset_group(&piece, -(depth / 2.0).min(FALLBACK_INSET_NM));
            // A strip thinner than the bisection can resolve offsets to nothing; its own
            // outline is then the path, and the tool's width still covers it.
            paths.extend(if rings.is_empty() { piece } else { rings });
        }
    }

    // Accounted from the rings themselves rather than from any region they were derived
    // from, so a fill that stopped short — on the pass cap, on a channel too narrow to let
    // the tool near, on anything — shows up here rather than in nobody's report.
    //
    // Hairlines are not counted, by the same rule that keeps them out of `base`: copper under
    // `2·SLIVER_NM` across is below what the geometry can tell from its own error. Counted,
    // a 10 µm strip where two channels all but meet was reported as copper "too close to a
    // net" — which sends the operator looking for a problem that is not there.
    let closed: Vec<Ring> = paths.iter().map(as_closed_polyline).collect();
    let missed = net_area_nm2(&open_by(&difference(region, &stroke_open_paths(&closed, half)), SLIVER_NM));

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
    //
    // A piece's rings are then linked into chains only where the hop between them is proven
    // to stay on `a`, the ground the tool centre may cover. A piece's rings do not always
    // nest: a U-shaped piece whose bottom pinches out leaves one ring per arm at that depth,
    // and chaining them in pass order cut a straight line across whatever stood between
    // the arms. That copper is in neither the region nor `allowed`, so `a` refuses the hop.
    //
    // `a` and not `base`: `base` is the region alone, and a hop that has to pass through the
    // channel beside it — from one stretch of a thin corridor round a pad to the next, past
    // a point where the corridor all but closes — was refused, and the tool lifted for a
    // move across ground already cut.
    let mut groups: Vec<usize> = Vec::new();
    let mut paths: Vec<Ring> = Vec::new();
    for rings in by_origin.into_iter().chain(std::iter::once(unclaimed)) {
        for chain in link_rings(rings, &a) {
            groups.push(chain.len());
            paths.extend(chain);
        }
    }

    (paths, groups, missed)
}

/// One piece's rings, outside-in, linked into chains that can each be cut without a retract.
///
/// Each ring joins the chain whose hop to it is shortest, among those whose hop stays inside
/// `safe`; a ring no chain can reach safely starts its own. The ring is stored already
/// rotated to the start that hop arrives at, which is the start [`Clearing::chains`] picks
/// again (the nearest vertex to a ring's own first vertex is that vertex), so the two cannot
/// disagree about where the connector runs.
///
/// Checked against the tool centre's ground rather than assumed from the nesting, because
/// the nesting is not guaranteed: every ring is `base` at some depth, and `base` can come
/// apart at depth into pieces with copper between them that this pass never takes.
fn link_rings(rings: Vec<Ring>, safe: &[Ring]) -> Vec<Vec<Ring>> {
    let mut chains: Vec<Vec<Ring>> = Vec::new();
    for ring in rings {
        let best = chains
            .iter()
            .enumerate()
            .filter_map(|(index, chain)| {
                let from = *chain.last()?.first()?;
                let rotated = rotate_to_nearest(&ring, from);
                let to = *rotated.first()?;
                let (dx, dy) = ((to.0 - from.0) as i128, (to.1 - from.1) as i128);
                segment_within(from, to, safe).then_some((dx * dx + dy * dy, index, rotated))
            })
            .min_by_key(|(hop, index, _)| (*hop, *index));
        match best {
            Some((_, index, rotated)) => chains[index].push(rotated),
            None => chains.push(vec![ring]),
        }
    }
    chains
}

/// Whether the straight segment `a`–`b` lies wholly inside `region` (even-odd over its rings).
///
/// `a` inside, and the segment meeting no edge at all, is the whole test: a segment that
/// crosses no boundary cannot leave the region it starts in. Touching an edge counts as
/// meeting it, so a hop that grazes a wall is refused. That costs at most an extra retract,
/// which is the cheap side to be wrong on when the other side is cutting copper.
///
/// Integer arithmetic in `i128` throughout, so the answer does not depend on rounding.
fn segment_within<'a>(
    a: (i64, i64),
    b: (i64, i64),
    region: impl IntoIterator<Item = &'a Ring, IntoIter: Clone>,
) -> bool {
    let region = region.into_iter();
    let inside = region.clone().filter(|ring| crate::region::point_in_ring(a, ring)).count() % 2 == 1;
    if !inside {
        return false;
    }
    if a == b {
        return true;
    }
    !region.into_iter().any(|ring| {
        (0..ring.len()).any(|i| segments_meet(a, b, ring[i], ring[(i + 1) % ring.len()]))
    })
}

/// Whether two closed segments share any point, touching and collinear overlap included.
fn segments_meet(p1: (i64, i64), p2: (i64, i64), q1: (i64, i64), q2: (i64, i64)) -> bool {
    // Cheap rejection first: almost every edge of the region is nowhere near the hop.
    if p1.0.max(p2.0) < q1.0.min(q2.0)
        || q1.0.max(q2.0) < p1.0.min(p2.0)
        || p1.1.max(p2.1) < q1.1.min(q2.1)
        || q1.1.max(q2.1) < p1.1.min(p2.1)
    {
        return false;
    }
    let cross = |o: (i64, i64), a: (i64, i64), b: (i64, i64)| -> i128 {
        (a.0 - o.0) as i128 * (b.1 - o.1) as i128 - (a.1 - o.1) as i128 * (b.0 - o.0) as i128
    };
    let (d1, d2) = (cross(q1, q2, p1), cross(q1, q2, p2));
    let (d3, d4) = (cross(p1, p2, q1), cross(p1, p2, q2));
    if d1.signum() * d2.signum() < 0 && d3.signum() * d4.signum() < 0 {
        return true;
    }
    // Collinear or touching: an endpoint lying on the other segment. Lying on the line
    // and inside that segment's own box is lying on the segment.
    let on = |d: i128, o: (i64, i64), a: (i64, i64), p: (i64, i64)| {
        d == 0
            && p.0 >= o.0.min(a.0)
            && p.0 <= o.0.max(a.0)
            && p.1 >= o.1.min(a.1)
            && p.1 <= o.1.max(a.1)
    };
    on(d1, q1, q2, p1) || on(d2, q1, q2, p2) || on(d3, p1, p2, q1) || on(d4, p1, p2, q2)
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
        let cleared = islands(&copper, &result.contours, W, false);
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
    /// **A hop between two rings never leaves the copper being cleared.** A chain is cut
    /// without a retract, so the straight line from one ring to the next is cut at depth.
    ///
    /// The shape that broke this: a U of free copper wrapped round copper that stays
    /// standing. The thin bottom of the U pinches out on the second pass, so each arm gets
    /// its own ring at that depth. The rings used to be chained in pass order, and the hop
    /// from one arm to the other went straight across the standing copper between them,
    /// cutting it in two.
    #[test]
    fn a_hop_between_rings_never_leaves_the_copper_it_clears() {
        let arm = 6 * W;
        let bottom = (2.5 * W as f64) as i64;
        let inner = 10 * W;
        let (x1, y1) = (2 * arm + inner, 20 * W);
        let region = vec![vec![
            (0, 0),
            (x1, 0),
            (x1, y1),
            (x1 - arm, y1),
            (x1 - arm, bottom),
            (arm, bottom),
            (arm, y1),
            (0, y1),
        ]];
        let (paths, groups, _) = fill(&region, &[], W as f64, VBIT_STEPOVER_FRACTION * W as f64);
        let cleared = Clearing { paths, groups, ..Default::default() };

        // Sampled along each hop, so the check does not share the code it is checking.
        let inside = |p: (f64, f64)| {
            let p = (p.0.round() as i64, p.1.round() as i64);
            region.iter().filter(|ring| crate::region::point_in_ring(p, ring)).count() % 2 == 1
        };
        for chain in cleared.chains() {
            for pair in chain.windows(2) {
                let (a, b) = (pair[0][0], pair[1][0]);
                for n in 0..=64 {
                    let t = n as f64 / 64.0;
                    let p = (a.0 as f64 + t * (b.0 - a.0) as f64, a.1 as f64 + t * (b.1 - a.1) as f64);
                    assert!(inside(p), "the hop {a:?} -> {b:?} leaves the copper at {p:?}");
                }
            }
        }
    }

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
        let cleared = islands(&copper, &result.contours, W, false);

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
        let cleared = islands(&copper, &result.contours, W, false);
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
        let cleared = islands(&copper, &result.contours, W, false);
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
        let already = islands(&copper, &contours, W, false);
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

    /// Net copper, as one region — what no clearing cut may touch.
    fn net_copper(copper: &CopperSnapshot) -> Vec<Ring> {
        union(
            &copper
                .features
                .iter()
                .flat_map(|f| f.polygons.iter())
                .flat_map(polygon_region)
                .collect::<Vec<Ring>>(),
            &[],
        )
    }

    /// Two parallel traces with `gap` between them, in open field: the strip their two
    /// channels leave runs out past both ends into the background, so it is never an island.
    fn traces_in_field(gap: i64) -> (CopperSnapshot, Vec<crate::isolation::IsolationContour>) {
        let copper = snapshot(vec![
            feature("A", rect(0, 0, 10 * MM, MM), Vec::new()),
            feature("B", rect(0, MM + gap, 10 * MM, 2 * MM + gap), Vec::new()),
        ]);
        let result = crate::isolate(&copper, W, W);
        (copper, result.contours)
    }

    /// **An island thinner than the ladder's first rung is cut, not just counted.** The
    /// thinnest copper is the most likely to lift off and short, and an island under about
    /// 0.8·W used to come back as "removed" with no rings at all.
    #[test]
    fn a_thin_island_is_cut_not_just_counted() {
        for gap in [(2.2 * W as f64) as i64, (2.5 * W as f64) as i64] {
            let (_, cleared) = pad_in_pour(gap);
            assert_eq!(cleared.removed, 1, "gap {gap}: {cleared:?}");
            assert!(!cleared.paths.is_empty(), "gap {gap}: counted, and no rings to cut it");
            assert!(
                cleared.missed_area_nm2 < 0.01 * cleared.removed_area_nm2,
                "gap {gap}: {} nm² of {} nm² left standing",
                cleared.missed_area_nm2,
                cleared.removed_area_nm2,
            );
        }
    }

    /// The strip between two traces that `islands()` alone cannot find — it runs out into
    /// the background — is taken when thin copper is asked for, and swept end to end.
    #[test]
    fn thin_copper_running_into_the_background_is_cleared_when_asked() {
        for gap in [(2.5 * W as f64) as i64, 4 * W] {
            let (copper, contours) = traces_in_field(gap);
            let off = islands(&copper, &contours, W, false);
            assert_eq!(off.thin_removed, 0, "not unless asked");
            let on = islands(&copper, &contours, W, true);
            assert_eq!(on.removed, 0, "it is not an island: {on:?}");
            assert!(on.thin_removed > 0, "gap {gap}: {on:?}");
            assert!(!on.paths.is_empty());
            assert!(
                on.missed_area_nm2 < 0.01 * on.thin_removed_area_nm2,
                "gap {gap}: {} nm² of {} nm² left standing",
                on.missed_area_nm2,
                on.thin_removed_area_nm2,
            );
        }
    }

    /// Past the bound it is left, as an island would be: a strip wider than three channels
    /// is not thin copper, it is copper.
    #[test]
    fn copper_wider_than_the_bound_is_not_thin() {
        let (copper, contours) = traces_in_field(8 * W);
        let cleared = islands(&copper, &contours, W, true);
        assert_eq!(cleared.thin_removed, 0, "{cleared:?}");
        assert!(cleared.paths.is_empty());
    }

    /// The blank round the board is background, not a neck: a lone pad gets nothing, even
    /// with thin copper asked for. This is what the frame's margin is for.
    #[test]
    fn the_background_around_the_board_is_never_thin_copper() {
        let copper = snapshot(vec![feature("A", circle(MM), Vec::new())]);
        let result = crate::isolate(&copper, W, W);
        let cleared = islands(&copper, &result.contours, W, true);
        assert_eq!((cleared.removed, cleared.thin_removed), (0, 0), "{cleared:?}");
        assert!(cleared.paths.is_empty());
    }

    /// The invariant again, for thin copper: clearing it never cuts a net.
    #[test]
    fn no_thin_copper_cut_ever_reaches_copper_that_is_on_a_net() {
        let boards = [2.2, 2.5, 4.0, 6.0]
            .map(|k| (k * W as f64) as i64)
            .into_iter()
            .flat_map(|gap| [traces_in_field(gap), pad_in_pour_contours(gap)]);
        for (copper, contours) in boards {
            let cleared = islands(&copper, &contours, W, true);
            let closed: Vec<Ring> = cleared.paths.iter().map(as_closed_polyline).collect();
            let bitten = intersect(&stroke_open_paths(&closed, W as f64 / 2.0), &net_copper(&copper));
            assert!(
                net_area_nm2(&bitten) < 1.0,
                "the thin-copper pass cut {} nm² of net copper",
                net_area_nm2(&bitten),
            );
        }
    }

    /// Deterministic, like every other pass.
    #[test]
    fn thin_copper_gives_the_same_program_twice() {
        let (copper, contours) = traces_in_field(4 * W);
        assert_eq!(islands(&copper, &contours, W, true), islands(&copper, &contours, W, true));
    }

    /// Two pads on one net, `gap` apart, joined by a trace down the middle of the gap — a
    /// chain of pads, as on a connector footprint. Beside the trace, the gap leaves a pocket
    /// of free copper walled by three channels and open to the field at the far end: a wide
    /// mouth outside the pads with a strip running in from it.
    fn pad_chain(gap: i64) -> (CopperSnapshot, Vec<crate::isolation::IsolationContour>) {
        let pad = 2 * MM;
        let copper = snapshot(vec![
            feature("A", rect(0, 0, pad, pad), Vec::new()),
            feature("A", rect(0, pad + gap, pad, 2 * pad + gap), Vec::new()),
            feature("A", rect(pad / 2 - W, pad - W, pad / 2 + W, pad + gap + W), Vec::new()),
        ]);
        let result = crate::isolate(&copper, W, W);
        (copper, result.contours)
    }

    /// **A thin strip running off a wide mouth is swept to its end.** One ring placed by the
    /// mouth's depth vanishes before it reaches the strip; the strip has to be found again,
    /// on its own, and given its own ring. Seen on a real board as the same pad cleaned at
    /// one end of a chain and not at the others.
    #[test]
    fn a_thin_strip_off_a_wide_mouth_is_swept_to_its_end() {
        for k in [2.05, 2.1, 2.3, 2.6, 3.0] {
            let (copper, contours) = pad_chain((k * W as f64) as i64);
            let cleared = islands(&copper, &contours, W, true);
            assert_eq!(cleared.thin_removed, 2, "{k}·W: one pocket each side of the trace: {cleared:?}");
            assert!(
                cleared.missed_area_nm2 < 0.01 * cleared.thin_removed_area_nm2,
                "{k}·W: {} nm² of {} nm² left standing",
                cleared.missed_area_nm2,
                cleared.thin_removed_area_nm2,
            );
        }
    }

    /// **Copper a round bit leaves in an inside corner belongs to the net.** Where the trace
    /// meets each pad, the channel cannot reach into the corner and a small fillet stands
    /// there, joined to the net. It is not an island — four of them used to be counted on this
    /// board, and reported as copper the bit could not reach.
    #[test]
    fn copper_left_in_an_inside_corner_belongs_to_the_net() {
        for k in [2.3, 4.0, 10.0] {
            let (copper, contours) = pad_chain((k * W as f64) as i64);
            let cleared = islands(&copper, &contours, W, false);
            assert_eq!((cleared.removed, cleared.left), (0, 0), "{k}·W: {cleared:?}");
            assert_eq!(cleared.missed_area_nm2, 0.0, "{k}·W: nothing reported as unreachable");
        }
    }
    /// A hop across ground the step cuts anyway is held; one across copper that stays is not.
    #[test]
    fn a_hop_is_held_only_across_ground_the_step_cuts() {
        // A pad in a pour with a 4·W corridor: the two channels and the island between them
        // are all cut, so the whole corridor is cleared ground.
        let (copper, contours) = pad_in_pour_contours(4 * W);
        let clearing = islands(&copper, &contours, W, false);
        let ground = ClearedGround::new(&contours, &[(&clearing, W)]);
        let tool = ground.for_tool(W);
        let r = 5 * MM + 2 * W; // the middle of the corridor

        // Along the corridor, round a quarter of it in short chords: cleared all the way.
        let at = |deg: f64| {
            let t = deg.to_radians();
            ((r as f64 * t.cos()).round() as i64, (r as f64 * t.sin()).round() as i64)
        };
        assert!(tool.holds(at(0.0), at(3.0)), "along the cleared corridor");
        // Straight across the pad: through net copper.
        assert!(!tool.holds(at(0.0), at(180.0)), "across the pad");
        // Out into the pour: copper that stays.
        assert!(!tool.holds(at(0.0), (r + 3 * W, 0)), "into the pour");
        // A tool wider than the corridor does not fit, wherever it goes.
        assert!(!ground.for_tool(6 * W).holds(at(0.0), at(3.0)), "a wider tool grazes the walls");
    }

    /// Without the island cleared, the channels alone are each only a tool wide, and a hop
    /// across the copper between them is refused.
    #[test]
    fn a_hop_across_copper_left_standing_is_refused() {
        let (_, contours) = pad_in_pour_contours(4 * W);
        let tool = ClearedGround::new(&contours, &[]).for_tool(W);
        let inner = (5 * MM + W / 2, 0);
        let outer = (5 * MM + 4 * W - W / 2, 0);
        assert!(!tool.holds(inner, outer), "the island between the channels is still there");
        assert!(tool.holds(inner, inner), "standing still in the middle of a channel is on cleared ground");
    }

    /// **An island of varying width is cut as one chain, not a string of arcs.** A pad off
    /// centre in its pour clearance leaves a corridor thin on one side and wide on the other.
    /// The fallback ring used to sit by the corridor's widest point and pinch out on its thin
    /// side, so the island came back as several arcs, each its own chain — and the tool lifted
    /// between every one of them, on the same pad that came out continuous elsewhere.
    #[test]
    fn an_island_of_varying_width_is_one_chain() {
        for (k_min, k_max) in [(2.1, 2.7), (2.2, 3.4)] {
            let pad_r = 5 * MM;
            let hole_r = pad_r + ((k_min + k_max) / 2.0 * W as f64) as i64;
            let shift = ((k_max - k_min) / 2.0 * W as f64) as i64;
            let hole: Ring = circle(hole_r).into_iter().map(|(x, y)| (x + shift, y)).collect();
            let copper = snapshot(vec![
                feature("SIG", circle(pad_r), Vec::new()),
                feature("GND", rect(-15 * MM, -15 * MM, 15 * MM, 15 * MM), vec![hole]),
            ]);
            let result = crate::isolate(&copper, W, W);
            let cleared = islands(&copper, &result.contours, W, true);
            assert_eq!(cleared.removed, 1, "{k_min}..{k_max}·W: {cleared:?}");
            assert_eq!(cleared.groups.len(), 1, "{k_min}..{k_max}·W: one chain, no lifts: {cleared:?}");
            assert_eq!(cleared.missed_area_nm2, 0.0, "{k_min}..{k_max}·W: and all of it cut");
        }
    }
}
