//! Isolation contours — where a mill has to cut for the nets to come apart.
//!
//! Isolation routing starts from a board that is *entirely* copper and removes a channel
//! around each net, leaving the net's copper standing as an island. So the toolpath is not
//! the copper outline: it is the outline pushed out by half the cut width, so that the cut
//! — which is as wide as the tool, centred on the path — just grazes the copper the design
//! asked for and takes everything up to the far side of the channel.
//!
//! **The hard case is a gap narrower than the cut.** Two nets 0.3 mm apart cannot be given
//! a 0.4 mm channel: the tool would eat into one of them. The remedy is a shallower pass —
//! a V-bit cuts narrower the less deep it goes — but the width must be reduced *only where
//! the board is tight*. Reducing it per net would be catastrophic on a real board: GND
//! touches everything, so one cramped pair anywhere would narrow the entire ground contour
//! and throw away isolation everywhere it was perfectly fine.
//!
//! So this walks a **descending ladder of widths**. The widest rung takes every stretch it
//! can reach; each narrower rung picks up only what the rung above it had to leave. Full
//! width where there is room, narrower across the tight stretch, and nothing narrowed that
//! did not have to be.
//!
//! The achieved width is therefore **quantised to the ladder**, and that is a deliberate
//! approximation. The exact answer is a medial axis — a large piece of work — and the
//! error is one ladder step against a width the operator picked to two decimal places.
//!
//! **The descent ends when the geometry says so, never when an estimate says so.** The
//! rung list was once computed up front, from a cheaper measurement of the same question —
//! how far apart two nets' *copper* is, rather than whether a *contour* drawn between them
//! clears the copper on its far side. Two measurements of one thing, over polygons whose
//! curves are chord approximations, do not always agree; and where they did not, the list
//! held one rung, the stretch that rung could not cut had nothing beneath it to fall to,
//! and it left here cut by nothing and mentioned by nothing. A board was machined with its
//! nets still joined and every diagnostic on the screen silent.
//!
//! So [`walk_ladder`] descends on what it actually failed to cut, and whatever survives the
//! floor rung comes back as [`UncutStretch`] to be reported. The estimate is still made —
//! it is the right shape for telling an operator what a clearance cost them, and it makes a
//! useful *hint* about which rung to try next — but it decides nothing. **A stretch of this
//! board is cut, or it is reported. There is no third outcome.**
//!
//! A contour whose width changes along its length is **split into spans**, because a span
//! is what becomes an operation and an operation has one depth. A contour that took one
//! width the whole way round stays a closed loop, which is what lets the planner choose
//! its lead-in freely later.
//!
//! Coordinates are board nanometres throughout, as everywhere else geometry is done here.

use std::collections::BTreeMap;

use clipper2_rust::{
    core::{FillRule, Path64, Paths64, Point64},
    engine::ClipType,
    engine_public::Clipper64,
};

use crate::copper::CopperSnapshot;
#[cfg(test)]
use crate::copper::Polygon;
use crate::region::{
    components, from_paths, intersect, polygon_region, to_paths, union, BBox, Ring,
};
#[cfg(test)]
use crate::region::area_nm2;
use crate::stitching::offset_group;

/// One cut, either a whole loop or a stretch of one.
#[derive(Clone, Debug, PartialEq)]
pub struct IsolationContour {
    /// Net name, or the generated name of a piece of copper on no net.
    pub net: String,
    /// Closed when the whole loop took one width; an open span where it did not.
    pub path: Vec<(i64, i64)>,
    pub closed: bool,
    /// The cut width actually achieved along this path, nm. The tool centre runs at half
    /// of it from the copper edge.
    pub width_nm: i64,
}

/// A pair of nets the requested width would not fit between.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NarrowedPair {
    /// The two net names, sorted, so a pair is reported once rather than from both sides.
    pub nets: (String, String),
    /// The widest ladder rung that fits between them, or 0 when none does.
    pub width_nm: i64,
}

/// Contour a net needed and no rung of the ladder could cut.
///
/// Separate from [`NarrowedPair`] because it is a different fact, arrived at a different
/// way. A narrowed pair is a *prediction* about two nets' copper, made before anything is
/// cut. This is the *outcome*: the ladder ran, and this much of the net's boundary came out
/// of it uncut. When the two disagree, this one is the one that happened.
#[derive(Clone, Debug, PartialEq)]
pub struct UncutStretch {
    pub net: String,
    /// Length of contour left uncut, nm.
    pub length_nm: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IsolationResult {
    pub layer_id: i32,
    pub contours: Vec<IsolationContour>,
    /// Every pair that had to give up width. Silence here would mean a board that looks
    /// isolated and is not.
    pub narrowed: Vec<NarrowedPair>,
    /// Contour the ladder could not cut at any width, by net.
    ///
    /// The pass's own account of what it failed to do, kept because nothing else in this
    /// result can show it: `contours` lists what *was* cut, and a stretch that is missing
    /// from it is indistinguishable from a stretch that was never needed. That gap is
    /// exactly how a board once came off the machine with nets joined and every diagnostic
    /// silent.
    pub uncut: Vec<UncutStretch>,
    /// Contour dropped by [`collapse_covered_cuts`] because another cut already removed
    /// the same copper, nm.
    ///
    /// Reported rather than simply not emitted, for the reason the whole module is built
    /// around: a channel is cut, or it is accounted for, and there is no third outcome. A
    /// pass that *deletes* contours has to answer to that rule too — this is the number
    /// that lets a reader check the collapse took what it claims and no more.
    pub collapsed_nm: f64,
    pub warnings: Vec<String>,
}

impl IsolationResult {
    /// The share of contours that took the requested width the whole way round, 0..=1.
    ///
    /// The one number that says whether a width suits a board. A contour only stays a
    /// closed loop if nothing forced it to narrow, so when this collapses the pass has
    /// stopped drawing outlines and started emitting fragments: ask a 0.2 mm board for a
    /// 0.8 mm channel and 349 tidy loops become 3416 slivers with a rapid between each.
    /// The geometry is right in both cases, which is exactly why the count is worth
    /// looking at — nothing else about the result announces that the width was absurd.
    pub fn intact_fraction(&self) -> f64 {
        if self.contours.is_empty() {
            return 1.0;
        }
        let closed = self.contours.iter().filter(|c| c.closed).count();
        closed as f64 / self.contours.len() as f64
    }

    /// The widest channel every crowded pair on this board could still take, nm.
    ///
    /// The tightest of the widths the pass had to fall back to — so setting the requested
    /// width to it makes every one of those pairs fit at full width, and the fragments
    /// become loops again. `None` when nothing was narrowed, which is when there is
    /// nothing to suggest.
    ///
    /// Pairs that got *nothing* are skipped: no width helps them, and letting a zero in
    /// here would suggest a channel of no width at all.
    pub fn widest_workable_nm(&self) -> Option<i64> {
        self.narrowed.iter().map(|p| p.width_nm).filter(|w| *w > 0).min()
    }
}

/// How many nets the uncut warning names before it gives up and states the total.
///
/// A board whose width simply does not suit it can leave every net short, and a warning
/// that lists two hundred of them is a warning nobody reads to the end.
const UNCUT_NETS_NAMED: usize = 5;

/// Length of a polyline, nm.
fn polyline_len_nm(points: &[(i64, i64)]) -> f64 {
    points
        .windows(2)
        .map(|w| ((w[1].0 - w[0].0) as f64).hypot((w[1].1 - w[0].1) as f64))
        .sum()
}

/// The step between rungs of the width ladder.
///
/// 25 µm is a step no operator would notice against a width they chose in hundredths of a
/// millimetre, and coarse enough that a cramped board does not spend a hundred passes
/// walking down to its answer.
pub const LADDER_STEP_NM: i64 = 25_000;

/// Slack allowed when deciding whether a cut touches copper it should not.
///
/// A gap of exactly the cut width is the commonest case on a board laid out to a clearance
/// rule — it is what setting the isolation width to the board's own clearance gives you,
/// which is the obvious thing to do — and it is a *fit*, not a collision. Without this,
/// every such pair would come back narrowed by one whole rung because the geometry grazes
/// itself to the nanometre.
///
/// **Stated as a multiple of the chord error it has to absorb**, not as a bare number that
/// happens to equal it. Every question here is asked of polygons whose curves are chord
/// approximations good to `OFFSET_ARC_TOLERANCE_NM`, and a single comparison can stack two
/// of them — a contour offset against a forbidden-region offset — before Clipper's own
/// integer rounding. At parity with the tessellator, which is where this constant started,
/// the slack could not absorb even one of those, so whether a channel got cut came down to
/// which side of a rounded pad the chords happened to fall.
///
/// The price is that a cut may pass this much nearer another net than the requested width
/// nominally allows. Four microns is smaller than the runout of the bit that cuts it, and
/// an order below the narrowest copper feature it could reach.
const TANGENCY_SLACK_NM: f64 = 4.0 * crate::stitching::OFFSET_ARC_TOLERANCE_NM;

/// A closed loop, or a stretch of one that survived clipping.
#[derive(Clone, Debug)]
enum Piece {
    Closed(Ring),
    Open(Ring),
}

/// Contours for one copper layer at `width_nm`, narrowing only where the board is tight.
///
/// `min_width_nm` is the floor of the ladder: the narrowest cut the tool can actually make,
/// which for a V-bit is its tip. Copper closer together than that cannot be isolated at
/// all, and is reported rather than quietly skipped.
pub fn isolate(copper: &CopperSnapshot, width_nm: i64, min_width_nm: i64) -> IsolationResult {
    let mut result = IsolationResult { layer_id: copper.layer_id, ..Default::default() };
    if width_nm <= 0 {
        result.warnings.push("The isolation width must be greater than zero.".into());
        return result;
    }

    // **An incomplete reading is not a board.** `collect_copper` sets this when KiCad
    // refused any part of the read — most often `AS_BUSY` while it re-pours — and the
    // resulting snapshot holds some of the copper, or none, with no way to tell which from
    // the geometry. Isolating it yields contours that look perfectly reasonable around what
    // was seen and account for nothing that was not, which is a plausible toolpath for a
    // board that is not the operator's. Refused here as well as at the caller, because this
    // is the function that turns copper into cuts and it is the last place that can know.
    if copper.partial {
        result.warnings.push(
            "This layer's copper could not be read completely, so no isolation was \
             attempted. Machining from a partial reading would leave the copper it missed \
             uncut and unmentioned."
                .into(),
        );
        return result;
    }

    let nets = net_regions(copper);
    if nets.is_empty() {
        result.warnings.push("No copper was found on this layer.".into());
        return result;
    }

    let rungs = ladder(width_nm, min_width_nm.max(1));
    let mut narrowed: BTreeMap<(String, String), i64> = BTreeMap::new();

    for (index, net) in nets.iter().enumerate() {
        let others = neighbouring_copper(&nets, index, width_nm);

        // Nothing near enough to be crowded by the widest cut: the whole net isolates at
        // full width, and none of the ladder work below is worth doing. This is the
        // common case, and skipping it is what keeps a dense board tractable.
        let intrusion = if others.is_empty() {
            Vec::new()
        } else {
            let reach = offset_group(&net.region, width_nm as f64 - TANGENCY_SLACK_NM);
            intersect(&reach, &others)
        };
        if intrusion.is_empty() {
            for ring in offset_group(&net.region, width_nm as f64 / 2.0) {
                result.contours.push(IsolationContour {
                    net: net.name.clone(),
                    path: ring,
                    closed: true,
                    width_nm,
                });
            }
            continue;
        }

        let hint = record_narrowed(&nets, index, &intrusion, &rungs, &mut narrowed);
        let uncut = walk_ladder(net, &others, &rungs, &hint, &mut result);
        let length_nm: f64 = uncut.iter().map(|span| polyline_len_nm(span)).sum();
        if length_nm > 0.0 {
            result.uncut.push(UncutStretch { net: net.name.clone(), length_nm });
        }
    }

    // After every net has been walked, never during: a contour's final width is only
    // settled once the ladder has finished with it, and coverage is a question about
    // widths. Before the warnings below, so `contours` is the surviving set everywhere
    // downstream.
    collapse_covered_cuts(&mut result);

    // Said whatever `narrowed` says. The two are arrived at differently on purpose — this
    // is what the ladder actually failed to cut, and it is the one that has been machined.
    if !result.uncut.is_empty() {
        let total: f64 = result.uncut.iter().map(|u| u.length_nm).sum();
        let mut worst: Vec<&UncutStretch> = result.uncut.iter().collect();
        worst.sort_by(|a, b| b.length_nm.total_cmp(&a.length_nm));
        result.warnings.push(format!(
            "{:.2} mm of channel could not be cut at any width, so the copper it should \
             have separated stays joined — worst on {}. Reduce the isolation width, or use \
             a bit with a finer tip.",
            total / 1e6,
            worst
                .iter()
                .take(UNCUT_NETS_NAMED)
                .map(|u| format!("{} ({:.2} mm)", u.net, u.length_nm / 1e6))
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }

    // **Nothing cut, and nothing said, is the one outcome this pass may not have.** Every
    // path above that produces no contours has its own account — no copper on the layer, a
    // width of zero, copper the ladder could not separate — but they are separate paths and
    // a future one need not remember to. This is the backstop: the result is empty and no
    // reason is attached, so a reason is attached. Silence here reaches the operator as a
    // step that engraved nothing and looked complete, which is exactly the fault that put
    // a board on the machine with its nets still joined.
    if result.contours.is_empty() && result.warnings.is_empty() {
        result.warnings.push(
            "The isolation pass produced no cuts at all for this layer, and cannot say \
             which part of the board defeated it. The copper will not be separated."
                .into(),
        );
    }

    for ((a, b), width) in narrowed {
        if width == 0 {
            result.warnings.push(format!(
                "{a} and {b} are closer together than the narrowest cut this tool can make; \
                 nothing was cut between them and they are not isolated."
            ));
        }
        result.narrowed.push(NarrowedPair { nets: (a, b), width_nm: width });
    }
    result.narrowed.sort_by(|l, r| l.width_nm.cmp(&r.width_nm).then(l.nets.cmp(&r.nets)));
    result
}

/// Drops contours whose channel another contour already cuts.
///
/// # The waste this removes
///
/// Every net is isolated on its own, so the channel between two neighbours is cut **once
/// from each side**. Where the requested width equals the board's clearance — the value an
/// operator naturally reaches for — both contours land on the gap's centre line and the
/// machine cuts the same path twice:
///
/// ```text
///   gap 0.150 .. 0.404 mm, width 0.254 mm
///     net A at x = 0.2770        net B at x = 0.2770      <- identical
/// ```
///
/// On a dense board that is half the engraving time, and half the life of the finest and
/// most fragile bit in the rack.
///
/// # Only strict coverage, and why that line is where it is
///
/// A contour is dropped **only** when the copper it would remove is already removed by
/// cuts that are staying. Two channels that merely *overlap* are both kept: each still
/// reaches copper the other does not, and dropping either leaves a ribbon standing along
/// that side. The distinction matters because the two look alike in a picture and are the
/// difference between a redundant pass and a wrong board.
///
/// So the test is on **swept regions**, not on paths. Two cuts of different widths can run
/// the same line and not cover each other; two cuts on different lines can. Comparing what
/// each actually removes is the only question worth asking.
///
/// # Leftovers that are artefacts rather than copper
///
/// Clipper works in integers over curves that are chord approximations, so a genuinely
/// covered contour leaves hair-thin slivers along its edges rather than nothing at all.
/// Those are erased by eroding the leftover by [`TANGENCY_SLACK_NM`] — the same tolerance
/// every other comparison here is drawn with. What survives an erosion is a region with
/// real width somewhere, which is copper; what does not was never more than rounding.
///
/// Deliberately *not* an area fraction. "98% covered" would discard a genuinely uncut 2%
/// of a long contour, which is exactly the class of silent loss this module exists to
/// prevent.
///
/// # It is a *stretch* that is redundant, not a contour
///
/// Two neighbouring tracks are two whole loops, and each loop coincides with the other
/// only along the edge they face across. Everywhere else it is the only cut there is. So a
/// pass that could drop nothing smaller than a whole contour would find nothing to drop on
/// a real board — the reported case included — and the collapse has to cut contours up.
///
/// **Which stretch is redundant is decided by geometry, not by a fraction.** The cut at a
/// point on the path removes a disc of half the width around it. That disc lies inside a
/// cut already being made exactly when the point lies within `(kept_width - this_width)/2`
/// of *that* cut's path — which is the identity
///
/// ```text
///     erode(stroke(P, a), b)  ==  stroke(P, a - b)
/// ```
///
/// read right to left. So the redundant part of a path is its intersection with the
/// neighbouring paths stroked by the width difference, and [`clip_pieces`] splits it there
/// — the same call the ladder uses to split a contour on copper it may not touch.
///
/// Written that way round for two reasons. It is **an order of magnitude cheaper**: the
/// direct form eroded a union that grew with every contour kept, which on a 1600-net board
/// cost 15 seconds against the 2 the rest of the pass takes. And it is **stricter**, since
/// a union can cover ground none of its parts covers alone — so this drops a stretch only
/// when one single cut subsumes it, never when two jointly happen to. A cut wider than the
/// candidate is the only kind that can subsume it at all, which is why the ordering below
/// puts width first.
///
/// A loop that loses a stretch comes back as open spans, which is a representation the
/// planner already handles: it is what the ladder produces whenever a width has to change
/// along a contour's length.
///
/// # Order
///
/// **Widest first**, then longest, then the contour's own identity. Width leads because
/// only a wider cut can subsume a narrower one, so placing the wide ones first is what lets
/// them absorb anything running inside them. Length and identity break the ties, which
/// makes the survivors a function of the geometry rather than of the order the nets
/// happened to be walked in — and keeps the more useful half of a coincident pair whole,
/// since one continuous loop beats two part-loops for lead-ins and travel.
fn collapse_covered_cuts(result: &mut IsolationResult) {
    if result.contours.len() < 2 {
        return;
    }

    // Sort keys only, so nothing is cloned per comparison.
    let mut order: Vec<usize> = (0..result.contours.len()).collect();
    let length: Vec<f64> = result.contours.iter().map(contour_len_nm).collect();
    order.sort_by(|&a, &b| {
        let (left, right) = (&result.contours[a], &result.contours[b]);
        right
            .width_nm
            .cmp(&left.width_nm)
            .then_with(|| length[b].total_cmp(&length[a]))
            .then_with(|| left.net.cmp(&right.net))
            .then(a.cmp(&b))
    });

    // The paths being kept, bucketed by grid cell so a candidate looks only at what is
    // actually beside it. Without this the scan is quadratic in the contour count, which on
    // a dense board is thousands — and the whole point of the identity above is that each
    // comparison is cheap enough for that to matter.
    let cell = grid_cell_nm(&result.contours);
    let mut grid: BTreeMap<(i64, i64), Vec<usize>> = BTreeMap::new();
    // (path with its closing edge, its box, the width it cuts)
    let mut kept: Vec<(Ring, BBox, i64)> = Vec::new();
    let mut survivors: Vec<Option<Vec<IsolationContour>>> = vec![None; result.contours.len()];

    for index in order {
        let contour = &result.contours[index];
        if contour.path.len() < 2 || contour.width_nm <= 0 {
            continue; // nothing to sweep; leave it exactly as it is
        }
        let Some(box_of) = BBox::of(std::slice::from_ref(&contour.path)) else {
            continue;
        };

        // Everything already kept whose cell this contour touches, and which is wide
        // enough to be able to subsume it at all.
        let mut candidates: Vec<usize> = cells_of(&contour.path, cell)
            .iter()
            .filter_map(|key| grid.get(key))
            .flatten()
            .copied()
            .collect();
        candidates.sort_unstable();
        candidates.dedup();

        // Grouped by the kept cut's width, so each distinct width is one stroke rather than
        // one per neighbour. On a real board almost every cut is the full requested width,
        // which makes this a single call however many neighbours there are.
        //
        // The box test comes before the clone: a grid cell is four of the widest cut and
        // holds contours that never come near each other, and copying a ground pour's
        // contour to find that out is most of what this pass costs.
        let mut by_width: BTreeMap<i64, Vec<Ring>> = BTreeMap::new();
        for candidate in candidates {
            let (path, other, width) = &kept[candidate];
            if *width < contour.width_nm {
                continue; // a narrower cut cannot contain a wider one
            }
            // The furthest this candidate can reach past its own line, plus slack.
            let reach = (*width - contour.width_nm) / 2 + TANGENCY_SLACK_NM as i64 + 1;
            if !other.expand(reach).overlaps(box_of) {
                continue;
            }
            by_width.entry(*width).or_default().push(path.clone());
        }

        let mut redundant: Vec<Ring> = Vec::new();
        for (width, paths) in by_width {
            // `(kept - this)/2`, plus the usual slack. The slack is load-bearing rather
            // than cosmetic: in the coincident case the two widths are equal, the stroke
            // would be by zero, and Clipper returns nothing at all — the reported fault
            // would survive the pass untouched. It is the same tolerance every other
            // comparison here is drawn with, so a rim thinner than it was never a real
            // difference.
            let reach = (width - contour.width_nm) as f64 / 2.0 + TANGENCY_SLACK_NM;
            redundant.extend(crate::stitching::stroke_open_paths(&paths, reach));
        }

        let pieces = match contour.closed {
            true => vec![Piece::Closed(contour.path.clone())],
            false => vec![Piece::Open(contour.path.clone())],
        };
        let remaining = match redundant.is_empty() {
            true => pieces,
            false => clip_pieces(pieces, &redundant, ClipType::Difference),
        };

        let mut parts: Vec<IsolationContour> = Vec::new();
        for piece in remaining {
            let (path, closed) = match piece {
                Piece::Closed(ring) => (ring, true),
                Piece::Open(span) => (span, false),
            };
            if path.len() < 2 {
                continue;
            }
            parts.push(IsolationContour { path, closed, ..contour.clone() });
        }

        let survived: f64 = parts.iter().map(contour_len_nm).sum();
        result.collapsed_nm += (length[index] - survived).max(0.0);

        // Only what is actually being cut joins the grid. Registering the whole contour
        // would let a stretch this pass has just dropped go on suppressing others.
        for part in &parts {
            let closed_path = with_closing_edge(part);
            let Some(part_box) = BBox::of(std::slice::from_ref(&closed_path)) else {
                continue;
            };
            for key in cells_of(&closed_path, cell) {
                grid.entry(key).or_default().push(kept.len());
            }
            kept.push((closed_path, part_box, part.width_nm));
        }
        survivors[index] = Some(parts);
    }

    result.contours = result
        .contours
        .drain(..)
        .zip(survivors)
        .flat_map(|(original, parts)| parts.unwrap_or_else(|| vec![original]))
        .collect();
}

/// Grid cell for the collapse's spatial index: four of the widest cut on the board.
///
/// Wide enough that two contours in non-adjacent cells cannot possibly interact (the
/// furthest a cut can reach past another is half a width), and small enough that a cell
/// holds a handful of contours rather than a quarter of the board.
fn grid_cell_nm(contours: &[IsolationContour]) -> i64 {
    let widest = contours.iter().map(|c| c.width_nm).max().unwrap_or(0);
    (widest * 4).max(100_000)
}

/// Every grid cell a path passes through or near, deduplicated.
///
/// Cells of the *vertices*, each grown to its eight neighbours. A long edge between two
/// distant vertices would skip the cells it crosses, so this is only sound because the
/// paths here are offset output — chord approximations whose vertices are at most
/// `OFFSET_ARC_TOLERANCE_NM` apart on curves and, on straights, are the copper's own
/// corners. The neighbour ring covers the rest with a cell of margin.
///
/// Deduplicated **before** the neighbours are added, and through a sorted `Vec` rather than
/// a set. A contour is hundreds of vertices spanning a handful of cells, so the collapse
/// spent more time inserting the same key into a `BTreeSet` nine times per vertex than it
/// did on the geometry it was there to do.
fn cells_of(path: &[(i64, i64)], cell: i64) -> Vec<(i64, i64)> {
    let mut cells: Vec<(i64, i64)> = path
        .iter()
        .map(|&(x, y)| (x.div_euclid(cell), y.div_euclid(cell)))
        .collect();
    cells.sort_unstable();
    cells.dedup();

    let mut out = Vec::with_capacity(cells.len() * 9);
    for (cx, cy) in cells {
        for dx in -1..=1 {
            for dy in -1..=1 {
                out.push((cx + dx, cy + dy));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// A contour's path with the closing edge a loop only implies.
///
/// `stroke_open_paths` takes paths at face value, so without this the segment from the last
/// point back to the first is left unswept and every loop keeps a gap in its own channel.
fn with_closing_edge(contour: &IsolationContour) -> Ring {
    let mut path = contour.path.clone();
    if contour.closed && !contour.path.is_empty() {
        path.push(contour.path[0]);
    }
    path
}

/// The copper a contour removes: its path swept by the width it cuts.
///
/// Only the tests need this now — the collapse works on the identity in
/// [`collapse_covered_cuts`] instead, which never builds a swept region at all.
#[cfg(test)]
fn swept_region(contour: &IsolationContour) -> Option<Vec<Ring>> {
    if contour.path.len() < 2 || contour.width_nm <= 0 {
        return None;
    }
    let swept = crate::stitching::stroke_open_paths(
        &[with_closing_edge(contour)],
        contour.width_nm as f64 / 2.0,
    );
    (!swept.is_empty()).then_some(swept)
}

/// How much cut a contour is, nm — the closing edge included when it is a loop.
fn contour_len_nm(contour: &IsolationContour) -> f64 {
    polyline_len_nm(&with_closing_edge(contour))
}

/// The widths to try, widest first, ending at the floor.
fn ladder(width_nm: i64, min_width_nm: i64) -> Vec<i64> {
    let mut rungs = Vec::new();
    let mut w = width_nm;
    while w > min_width_nm {
        rungs.push(w);
        w -= LADDER_STEP_NM;
    }
    rungs.push(min_width_nm.min(width_nm));
    rungs
}

/// Walks the ladder for one net, emitting the widest cut that fits at every point, and
/// returning the stretches **no** rung could cut.
///
/// Each rung does two things with its contour. It emits what it may legally cut — the part
/// that stays clear of copper this net does not own — and it hands what it could *not* cut
/// down to the next rung as that rung's job. The descent stops the moment a rung blocks
/// nothing, which for most crowded nets is the second rung.
///
/// **`blocked` decides when the descent stops; `hint` only decides where it looks next.**
/// The rung list used to be computed up front by [`record_narrowed`], which asks whether
/// two nets' *copper* is far enough apart, while this asks whether a *contour* drawn
/// between them clears the copper on the far side. Those are the same question with a
/// different number of polygon offsets under it, so they can disagree — and when they did
/// the list held a single rung, the stretch this rung dropped had nothing beneath it to be
/// picked up by, and it left `isolate` cut by nothing and mentioned by nothing. A board came
/// off the machine with nets joined and every diagnostic silent. A prediction may say where
/// to look; it may never be what declares a stretch finished with.
///
/// So the hint is followed while it works and abandoned the moment it does not: a wrong
/// hint costs one rung of extra searching, where trusting it cost the channel.
///
/// Following it matters for the *output*, not only the clock. Stepping rung by rung grades
/// the transition either side of a tight spot into slivers 25 µm apart in width — each its
/// own op, its own depth, its own rapid — where jumping to the width that fits leaves the
/// two spans an operator would draw by hand.
fn walk_ladder(
    net: &NetCopper,
    others: &[Ring],
    rungs: &[i64],
    hint: &[i64],
    out: &mut IsolationResult,
) -> Vec<Ring> {
    // Where the previous rung failed, as the region this rung has to work inside. `None`
    // on the first rung, whose business is the whole contour.
    let mut pending: Option<Vec<Ring>> = None;
    let mut n = 0;

    while let Some(&width) = rungs.get(n) {
        let half = width as f64 / 2.0;

        // After the first rung only the neighbourhood of the failure is still in play, so
        // only that much of the net is worth offsetting. Cropping wide and keeping narrow
        // (the rule `record_narrowed` states at its own windows): the straight edge a crop
        // invents is four widths away from anything `pending` will keep, and the offset
        // grows it by half a width.
        let region = match pending.as_ref().and_then(|p| BBox::of(p)) {
            Some(near) => intersect(&net.region, &[near.expand(4 * rungs[0]).ring()]),
            None => net.region.clone(),
        };
        let contour = offset_group(&region, half);
        if contour.is_empty() {
            return Vec::new();
        }
        let forbidden = offset_group(others, half - TANGENCY_SLACK_NM);

        let mut pieces: Vec<Piece> = contour.into_iter().map(Piece::Closed).collect();
        if let Some(pending) = pending.as_ref() {
            pieces = clip_pieces(pieces, pending, ClipType::Intersection);
        }

        // What this rung cannot have, and what it can. Taken from the same `pieces` so the
        // two are exact complements by construction rather than by arithmetic.
        let blocked = spans_of(clip_pieces(pieces.clone(), &forbidden, ClipType::Intersection));
        for piece in clip_pieces(pieces, &forbidden, ClipType::Difference) {
            let (path, closed) = match piece {
                Piece::Closed(ring) => (ring, true),
                Piece::Open(span) => (span, false),
            };
            if path.len() < 2 {
                continue;
            }
            out.contours.push(IsolationContour {
                net: net.name.clone(),
                path,
                closed,
                width_nm: width,
            });
        }

        if blocked.is_empty() {
            return Vec::new();
        }
        if n + 1 == rungs.len() {
            return blocked; // the floor could not cut it either
        }

        // Where to look next: the widest width the prediction says this net's crowded pairs
        // can take, if that is narrower than the rung just tried. Otherwise one rung down —
        // which is also what happens on the second time round, since the hint has by then
        // been tried and found wanting.
        n = hint
            .iter()
            .copied()
            .filter(|&w| w < width)
            .max()
            .and_then(|w| rungs.iter().position(|&rung| rung == w))
            .filter(|&at| at > n)
            .unwrap_or(n + 1);
        let next = rungs[n];

        // The next rung's contour runs `(width - next)/2` nearer the copper than this one,
        // so the ground this rung failed on lies within that of what it just traced. The
        // slack is the same tolerance the clips are drawn with; being a shade generous here
        // costs a little overlap between two rungs' cuts, where being a shade mean costs a
        // stretch of channel.
        let shift = (width - next) as f64 / 2.0 + TANGENCY_SLACK_NM;
        pending = Some(crate::stitching::stroke_open_paths(&blocked, shift));
    }
    Vec::new()
}

/// The rings out of a set of pieces, whether they closed or not.
fn spans_of(pieces: Vec<Piece>) -> Vec<Ring> {
    pieces
        .into_iter()
        .map(|piece| match piece {
            Piece::Closed(ring) | Piece::Open(ring) => ring,
        })
        .filter(|ring| ring.len() >= 2)
        .collect()
}

/// Records, for every net crowding `index`, the widest rung that fits between the two.
///
/// Done per *pair* rather than per span: the number the operator needs is "these two nets
/// only got 0.2 mm", and asking the question of the pair directly gives it exactly, with
/// no dependence on how the spans happened to be cut up.
///
/// The widths it found come back as a **hint** for [`walk_ladder`] — where to look next
/// when a rung is blocked, so the search jumps to the width that fits rather than grinding
/// down the ladder a rung at a time. It used to be taken as the definitive rung list, which
/// it cannot be: it measures the gap between two nets' *copper*, where the ladder measures
/// whether a *contour* drawn in that gap clears the copper on its far side. Those agree on
/// paper and not always in integers, and a stretch the ladder dropped with no rung beneath
/// it was simply lost — uncut, unreported, and machined.
///
/// `intrusion` — where the full-width cut would land on copper that is not this net's — is
/// what makes this affordable. It names the only nets that can possibly be narrowed, so a
/// ground pour is measured against the handful of nets crowding it rather than against
/// every net whose bounding box it happens to span, which on a pour is all of them.
fn record_narrowed(
    nets: &[NetCopper],
    index: usize,
    intrusion: &[Ring],
    rungs: &[i64],
    narrowed: &mut BTreeMap<(String, String), i64>,
) -> Vec<i64> {
    let net = &nets[index];
    let mut achieved = Vec::new();
    let Some(crowded) = BBox::of(intrusion) else { return achieved };
    let crowded = crowded.expand(rungs[0]);

    for (other_index, other) in nets.iter().enumerate() {
        if other_index == index || !crowded.overlaps(other.bbox) {
            continue;
        }
        let contact = intersect(intrusion, &other.region);
        let Some(zone) = BBox::of(&contact) else { continue };

        // Both nets can be board-sized; the question is not. Cropping to the crowded spot
        // turns a search over a pour into a search over a few hundred points.
        //
        // Two windows, not one. Cropping invents a straight edge at the boundary, and the
        // side that gets *dilated* would grow that invention inward. Keeping the dilated
        // side's window well outside the other's puts the fiction out of reach.
        let near = intersect(&net.region, &[zone.expand(2 * rungs[0]).ring()]);
        let far = intersect(&other.region, &[zone.expand(5 * rungs[0]).ring()]);

        let fits = rungs.iter().copied().find(|&w| {
            intersect(&near, &offset_group(&far, w as f64 - TANGENCY_SLACK_NM)).is_empty()
        });
        match fits {
            Some(w) if w < rungs[0] => {
                insert_narrowed(narrowed, &net.name, &other.name, w);
                achieved.push(w);
            }
            Some(_) => {}
            None => insert_narrowed(narrowed, &net.name, &other.name, 0),
        }
    }
    achieved
}

fn insert_narrowed(
    narrowed: &mut BTreeMap<(String, String), i64>,
    a: &str,
    b: &str,
    width: i64,
) {
    let key = if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    };
    // Both sides of a pair reach this, and the two can disagree by a rung when the copper
    // is not symmetric. The tighter answer is the true one.
    narrowed.entry(key).and_modify(|w| *w = (*w).min(width)).or_insert(width);
}

// ---------------------------------------------------------------------------
// Copper, grouped into the things that must end up separated
// ---------------------------------------------------------------------------

struct NetCopper {
    name: String,
    region: Vec<Ring>,
    bbox: BBox,
}

/// Every piece of copper that must be separated from every other, as one region each.
///
/// A named net is one group however many tracks, pads and pours make it up — copper on the
/// same net needs no channel between its parts, and unioning it is what stops one being
/// cut from another.
///
/// Copper on **no** net is not one group. Two unrelated fiducials share the absence of a
/// net and nothing else, and treating them as one would leave the channel between them
/// uncut. Each connected piece becomes its own pseudo-net.
fn net_regions(copper: &CopperSnapshot) -> Vec<NetCopper> {
    let mut by_net: BTreeMap<String, Vec<Ring>> = BTreeMap::new();
    let mut unnetted: Vec<Ring> = Vec::new();

    for feature in &copper.features {
        for polygon in &feature.polygons {
            let region = polygon_region(polygon);
            if region.is_empty() {
                continue;
            }
            if feature.net.is_empty() {
                unnetted.extend(region);
            } else {
                by_net.entry(feature.net.clone()).or_default().extend(region);
            }
        }
    }

    let mut nets: Vec<NetCopper> = Vec::new();
    for (name, paths) in by_net {
        let region = union(&paths, &[]);
        if let Some(bbox) = BBox::of(&region) {
            nets.push(NetCopper { name, region, bbox });
        }
    }

    let mut islands = components(&union(&unnetted, &[]));
    // Left-to-right, so the generated names are the same on every run.
    islands.sort_by_key(|r| BBox::of(r).map(|b| (b.x0, b.y0)).unwrap_or_default());
    for (n, region) in islands.into_iter().enumerate() {
        if let Some(bbox) = BBox::of(&region) {
            nets.push(NetCopper { name: format!("(no net) #{}", n + 1), region, bbox });
        }
    }
    nets
}

/// The copper of every other net near enough to be crowded by a cut of `width_nm`.
///
/// Clipped to the net's own neighbourhood, which is the difference between asking about a
/// pad and asking about the whole board. A ground pour spans the board; only the sliver of
/// it beside this net can affect this net's cut.
fn neighbouring_copper(nets: &[NetCopper], index: usize, width_nm: i64) -> Vec<Ring> {
    // A contour sits at most `width/2` from its own copper, and the widest question asked
    // of `others` dilates them by up to `width`. Anything further off than the sum cannot
    // reach, and doubling it is cheap insurance against that arithmetic drifting.
    let reach = nets[index].bbox.expand(2 * width_nm);
    let mut nearby: Vec<Ring> = Vec::new();
    for (other, net) in nets.iter().enumerate() {
        if other != index && reach.overlaps(net.bbox) {
            nearby.extend(net.region.iter().cloned());
        }
    }
    if nearby.is_empty() {
        return Vec::new();
    }
    intersect(&nearby, &[reach.ring()])
}

// ---------------------------------------------------------------------------
// Clipper, kept behind names that say what they are for
// ---------------------------------------------------------------------------

/// Clips loops and spans against a region, keeping whichever side `op` names.
///
/// Every piece goes through in **one** clipper call. Clipping them one at a time costs the
/// whole clip region again per piece, and the clip region here is the rest of the board's
/// copper: a ground pour's contour is dozens of rings against thirty thousand points, and
/// paying for those points dozens of times is most of what this pass would otherwise do.
///
/// A loop that comes back with its ends still together was never cut, so it stays a loop —
/// which is the distinction the planner needs, since only a closed loop is free to be
/// entered anywhere.
fn clip_pieces(pieces: Vec<Piece>, clips: &[Ring], op: ClipType) -> Vec<Piece> {
    let keep_untouched = matches!(op, ClipType::Difference);
    let clip_bounds = BBox::of(clips);
    let (Some(clip_bounds), false) = (clip_bounds, clips.is_empty()) else {
        return if keep_untouched { pieces } else { Vec::new() };
    };

    let mut out = Vec::new();
    let mut subject: Vec<Ring> = Vec::new();
    for piece in pieces {
        let ring = match &piece {
            Piece::Closed(ring) | Piece::Open(ring) => ring,
        };
        // Nowhere near the clip region: the answer is known without asking, and asking is
        // what costs. Sound in both directions — disjoint boxes cannot intersect.
        if !BBox::of(std::slice::from_ref(ring)).is_some_and(|b| b.overlaps(clip_bounds)) {
            if keep_untouched {
                out.push(piece);
            }
            continue;
        }
        subject.push(match piece {
            Piece::Closed(ring) => as_open_loop(&ring),
            Piece::Open(span) => span,
        });
    }

    for span in clip_open(&subject, clips, op) {
        if span.len() >= 4 && span.first() == span.last() {
            let mut ring = span;
            ring.pop();
            out.push(Piece::Closed(ring));
        } else {
            out.push(Piece::Open(span));
        }
    }
    out
}

fn clip_open(subject: &[Ring], clips: &[Ring], op: ClipType) -> Vec<Ring> {
    let subject: Paths64 = subject
        .iter()
        .filter(|p| p.len() >= 2)
        .map(|p| p.iter().map(|&(x, y)| Point64 { x, y }).collect::<Path64>())
        .collect();
    let clips = to_paths(clips);
    if subject.is_empty() || clips.is_empty() {
        return Vec::new();
    }

    let mut clipper = Clipper64::new();
    clipper.add_open_subject(&subject);
    clipper.add_clip(&clips);
    let mut closed = Paths64::new();
    let mut open = Paths64::new();
    if !clipper.execute(op, FillRule::NonZero, &mut closed, Some(&mut open)) {
        return Vec::new();
    }
    let mut spans = from_paths(&open);
    // Clipper's output order is its own business; ours has to be the same on every run.
    spans.sort_by_key(|s| (s[0].0, s[0].1, s.len()));
    spans
}

/// A ring as a polyline that comes back to its start.
///
/// Handed to an open-path clip, a ring is read as a polyline from its first point to its
/// last — the closing edge simply would not be there, and the one stretch of contour that
/// crosses the seam would escape clipping.
fn as_open_loop(ring: &Ring) -> Ring {
    let mut loop_ = ring.clone();
    loop_.push(ring[0]);
    loop_
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copper::{CopperFeature, CopperSource};
    use crate::stitching::path_perimeter_nm;

    const MM: i64 = 1_000_000;

    fn polyline_len_nm(points: &[(i64, i64)]) -> f64 {
        points
            .windows(2)
            .map(|w| ((w[1].0 - w[0].0) as f64).hypot((w[1].1 - w[0].1) as f64))
            .sum()
    }

    fn square(cx: i64, cy: i64, half: i64) -> Polygon {
        Polygon {
            outline: vec![
                (cx - half, cy - half),
                (cx + half, cy - half),
                (cx + half, cy + half),
                (cx - half, cy + half),
            ],
            holes: Vec::new(),
        }
    }

    fn feature(net: &str, polygons: Vec<Polygon>) -> CopperFeature {
        CopperFeature { net: net.into(), source: CopperSource::Pad, polygons }
    }

    fn snapshot(features: Vec<CopperFeature>) -> CopperSnapshot {
        CopperSnapshot { layer_id: 3, features, warnings: Vec::new(), partial: false }
    }

    fn contours_of<'a>(r: &'a IsolationResult, net: &str) -> Vec<&'a IsolationContour> {
        r.contours.iter().filter(|c| c.net == net).collect()
    }

    /// A round pad, tessellated the way KiCad's geometry arrives.
    fn disc(cx: i64, cy: i64, radius: i64) -> Polygon {
        let mut outline = Vec::new();
        crate::stitching::tessellate::tessellate_circle(
            &mut outline,
            cx as f64,
            cy as f64,
            (cx + radius) as f64,
            cy as f64,
        );
        Polygon { outline, holes: Vec::new() }
    }

    /// How much contour one net got, at any width.
    fn emitted_len(r: &IsolationResult, net: &str) -> f64 {
        contours_of(r, net)
            .iter()
            .map(|c| {
                let mut path = c.path.clone();
                if c.closed {
                    path.push(c.path[0]);
                }
                polyline_len_nm(&path)
            })
            .sum()
    }

    /// A column of round pads on one net, like the pad chains on the reported board.
    fn pad_column(cx: i64, radius: i64, pitch: i64, count: i64) -> Vec<Polygon> {
        (0..count).map(|n| disc(cx, n * pitch, radius)).collect()
    }

    /// Two parallel tracks with a gap of solid copper between them, as the blank arrives.
    fn two_tracks(gap: i64) -> (CopperSnapshot, i64, i64) {
        let half_w = 150_000;
        let track = |cx: i64| Polygon {
            outline: vec![
                (cx - half_w, -5_000_000),
                (cx + half_w, -5_000_000),
                (cx + half_w, 5_000_000),
                (cx - half_w, 5_000_000),
            ],
            holes: Vec::new(),
        };
        let a_right = half_w;
        let b_left = a_right + gap;
        (
            snapshot(vec![
                feature("A", vec![track(0)]),
                feature("B", vec![track(b_left + half_w)]),
            ]),
            a_right,
            b_left,
        )
    }

    /// Where the cuts cross `y = 0` inside the gap — one entry per pass through the
    /// channel, which is what an operator is counting when they say "two cuts".
    fn cuts_across_gap(r: &IsolationResult, from: i64, to: i64) -> Vec<f64> {
        let mut xs = Vec::new();
        for contour in &r.contours {
            let mut path = contour.path.clone();
            if contour.closed && !contour.path.is_empty() {
                path.push(contour.path[0]);
            }
            for w in path.windows(2) {
                let ((x1, y1), (x2, y2)) = (w[0], w[1]);
                if y1 == y2 || (y1 > 0) == (y2 > 0) {
                    continue;
                }
                let t = -(y1 as f64) / ((y2 - y1) as f64);
                let x = x1 as f64 + t * ((x2 - x1) as f64);
                if x > from as f64 - 1_000.0 && x < to as f64 + 1_000.0 {
                    xs.push(x);
                }
            }
        }
        xs.sort_by(f64::total_cmp);
        xs
    }

    /// The copper a set of cuts removes: the area of the union of their swept regions.
    ///
    /// The one measurement that says whether a collapse was safe. Counting contours says
    /// how many passes there are; this says what they take off the board, and only the
    /// second must be preserved.
    fn copper_removed(contours: &[IsolationContour]) -> i128 {
        let mut region: Vec<Ring> = Vec::new();
        for contour in contours {
            if let Some(swept) = swept_region(contour) {
                region = union(&region, &swept);
            }
        }
        region.iter().map(|ring| area_nm2(ring)).sum::<i128>().abs()
    }

    /// A straight cut of `width` from `(x, y0)` to `(x, y1)`.
    fn cut(net: &str, x: i64, y0: i64, y1: i64, width: i64) -> IsolationContour {
        IsolationContour {
            net: net.to_string(),
            path: vec![(x, y0), (x, y1)],
            closed: false,
            width_nm: width,
        }
    }

    fn collapsed(contours: Vec<IsolationContour>) -> IsolationResult {
        let mut result = IsolationResult { contours, ..Default::default() };
        collapse_covered_cuts(&mut result);
        result
    }

    /// **The reported case, at the unit.** Two cuts on the identical line, the same width:
    /// one of them is doing nothing and goes.
    #[test]
    fn a_cut_another_cut_repeats_exactly_is_dropped() {
        let before = vec![
            cut("A", 277_000, -5_000_000, 5_000_000, 254_000),
            cut("B", 277_000, -5_000_000, 5_000_000, 254_000),
        ];
        let removed = copper_removed(&before);

        let after = collapsed(before);

        assert_eq!(after.contours.len(), 1, "the identical second pass survived");
        assert_eq!(
            copper_removed(&after.contours),
            removed,
            "dropping the duplicate changed the copper removed",
        );
        assert!(after.collapsed_nm > 0.0, "the drop was not accounted for");
    }

    /// **Overlap is not coverage.** Two cuts that each reach copper the other does not are
    /// both kept — dropping either leaves a ribbon of copper standing along that side.
    ///
    /// This is the line the collapse must not cross, and it is the ordinary geometry: a bit
    /// finer than the gap cuts hard against each neighbour's edge and the channels overlap
    /// in the middle without either containing the other.
    #[test]
    fn two_cuts_that_overlap_without_covering_are_both_kept() {
        let before = vec![
            cut("A", 236_000, -5_000_000, 5_000_000, 172_000),
            cut("B", 318_000, -5_000_000, 5_000_000, 172_000),
        ];
        let removed = copper_removed(&before);

        let after = collapsed(before);

        assert_eq!(after.contours.len(), 2, "both cuts still do work");
        assert_eq!(copper_removed(&after.contours), removed);
        assert_eq!(after.collapsed_nm, 0.0);
    }

    /// A narrow cut running inside a wider one **is** covered, even though the paths are
    /// different lines. The test is on what each removes, not on where each runs.
    #[test]
    fn a_narrow_cut_inside_a_wider_one_is_dropped() {
        let before = vec![
            cut("A", 277_000, -5_000_000, 5_000_000, 254_000),
            cut("B", 300_000, -4_000_000, 4_000_000, 60_000),
        ];
        let removed = copper_removed(&before);

        let after = collapsed(before);

        assert_eq!(after.contours.len(), 1);
        assert_eq!(after.contours[0].net, "A", "the wider cut is the one that stays");
        assert_eq!(copper_removed(&after.contours), removed);
    }

    /// The converse: the same two cuts the other way round. A wide cut is **not** dropped
    /// because a narrow one runs down it — the wide one still takes copper the narrow one
    /// leaves, and length order must not be able to talk the pass into losing it.
    #[test]
    fn a_wide_cut_is_never_dropped_for_a_narrow_one_along_it() {
        let before = vec![
            // Longest first by path length, so the ordering rule sees the narrow one first.
            cut("B", 300_000, -5_000_000, 5_000_000, 60_000),
            cut("A", 277_000, -4_000_000, 4_000_000, 254_000),
        ];
        let after = collapsed(before);

        assert!(
            after.contours.iter().any(|c| c.width_nm == 254_000),
            "the wide cut was dropped for a narrow one running along it",
        );
    }

    /// Cuts far enough apart to leave copper between them are both kept, and no attempt is
    /// made to tidy the leftover away. Dropping one here would widen the track on that
    /// side by most of a channel.
    #[test]
    fn two_cuts_with_copper_between_them_are_both_kept() {
        let before = vec![
            cut("A", 200_000, -5_000_000, 5_000_000, 100_000),
            cut("B", 600_000, -5_000_000, 5_000_000, 100_000),
        ];
        let after = collapsed(before);

        assert_eq!(after.contours.len(), 2);
        assert_eq!(after.collapsed_nm, 0.0);
    }

    /// **The survivors are a function of the geometry, not of the order the nets arrive
    /// in.** A `BTreeMap` walk that happened to reverse would otherwise change which pass
    /// the machine makes.
    #[test]
    fn the_collapse_does_not_depend_on_input_order() {
        let build = || {
            vec![
                cut("A", 277_000, -5_000_000, 5_000_000, 254_000),
                cut("B", 277_000, -5_000_000, 5_000_000, 254_000),
                cut("C", 900_000, -5_000_000, 5_000_000, 172_000),
                cut("D", 980_000, -5_000_000, 5_000_000, 172_000),
            ]
        };
        let forward = collapsed(build());
        let mut reversed_input = build();
        reversed_input.reverse();
        let backward = collapsed(reversed_input);

        let names = |r: &IsolationResult| {
            let mut n: Vec<String> = r.contours.iter().map(|c| c.net.clone()).collect();
            n.sort();
            n
        };
        assert_eq!(names(&forward), names(&backward));
        assert_eq!(forward.collapsed_nm, backward.collapsed_nm);
    }

    /// **End to end, on the reported board.** Two tracks at the board's own clearance,
    /// engraved at exactly that width: the pass through the gap is made once.
    #[test]
    fn the_reported_board_cuts_its_channel_once() {
        let gap = 254_000;
        let (board, from, to) = two_tracks(gap);
        let result = isolate(&board, gap, 229_500);

        let cuts = cuts_across_gap(&result, from, to);
        assert_eq!(cuts.len(), 1, "expected one pass through the gap, got {cuts:?}");

        // And it is where the coincident pair was — the centre of the gap.
        let centre = (from + to) as f64 / 2.0;
        assert!(
            (cuts[0] - centre).abs() < 2_000.0,
            "the surviving cut moved: {} vs the centre {centre}",
            cuts[0],
        );
        assert!(result.collapsed_nm > 0.0);
    }

    /// End to end with a bit finer than the gap: two distinct channels, both kept, because
    /// each still reaches copper the other does not.
    #[test]
    fn a_fine_bit_on_the_reported_board_keeps_both_passes() {
        let gap = 254_000;
        let (board, from, to) = two_tracks(gap);
        let result = isolate(&board, 172_300, 109_600);

        assert_eq!(cuts_across_gap(&result, from, to).len(), 2);
        assert_eq!(result.collapsed_nm, 0.0);
    }

    /// Copper with nothing near it keeps every contour: there is no redundancy to find and
    /// the collapse must not invent any.
    #[test]
    fn isolated_copper_keeps_every_contour() {
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, 500_000)]),
            feature("B", vec![square(10_000_000, 0, 500_000)]),
        ]);
        let result = isolate(&board, 254_000, 150_000);

        assert_eq!(result.contours.len(), 2);
        assert_eq!(result.collapsed_nm, 0.0);
    }

    /// **The collapse must not cost more than the pass it is part of.**
    ///
    /// It has regressed twice, both times by an order of magnitude, and both times the
    /// symptom was only visible on a board far denser than any other test here builds:
    /// eroding a union that grew with every contour kept (15 s), then a `BTreeSet` insert
    /// per vertex per neighbouring cell (5 s). Neither showed up as a failure — only as a
    /// test run that had got slow, which is the kind of thing that gets lived with.
    ///
    /// The bound is deliberately loose. It is not measuring the machine; it is there to
    /// fail when the pass goes quadratic again, and a limit that tracked the current figure
    /// closely would fail on a busy CI runner instead.
    #[test]
    fn a_dense_board_collapses_in_proportion_to_its_size() {
        let radius = 300_000i64;
        let pitch = 900_000i64; // 0.3 mm between neighbours, on every side
        let (cols, rows) = (25i64, 25i64);
        let features: Vec<CopperFeature> = (0..cols)
            .flat_map(|c| {
                (0..rows).map(move |r| {
                    (format!("n{c}_{r}"), vec![disc(c * pitch, r * pitch, radius)])
                })
            })
            .map(|(net, polygons)| feature(&net, polygons))
            .collect();
        let board = snapshot(features);

        let started = std::time::Instant::now();
        let result = isolate(&board, 300_000, 150_000);
        let elapsed = started.elapsed();

        assert!(!result.contours.is_empty(), "the board produced no contours at all");
        assert!(
            result.collapsed_nm > 0.0,
            "a grid of neighbours at the cut width must have redundant channel to drop",
        );
        // Debug builds run Clipper an order slower than release, which is where this would
        // otherwise be a flake rather than a guard.
        let budget = if cfg!(debug_assertions) { 120 } else { 20 };
        assert!(
            elapsed.as_secs() < budget,
            "{} nets took {elapsed:?}, over the {budget}s ceiling — the collapse has most              likely gone quadratic again",
            cols * rows,
        );
    }

    /// **A partial reading is never isolated.** The fault that took an afternoon: KiCad
    /// answers `AS_BUSY` while it re-pours its zones, and a read landing in that window
    /// came back with some of the board's copper or none of it. Ten reads in a row on a
    /// board with a ground plane gave 1, 0, 1, 0, 0, 119, 0, 0, 119, 0 features — and the
    /// wrong ones reached this function as fact.
    ///
    /// The dangerous half is not the empty read but the *short* one. Copper that was seen
    /// gets a perfectly ordinary contour; copper that was not is indistinguishable from
    /// copper that is not there. Nothing downstream can tell the difference, so it has to
    /// be refused here.
    #[test]
    fn a_partial_reading_of_the_copper_is_refused_rather_than_isolated() {
        let mut short = snapshot(vec![feature("A", vec![square(0, 0, 500_000)])]);
        short.partial = true;

        let result = isolate(&short, 254_000, 150_000);

        assert!(result.contours.is_empty(), "a short read must produce no toolpath at all");
        assert!(
            result.warnings.iter().any(|w| w.contains("could not be read completely")),
            "and must say why: {:?}",
            result.warnings,
        );

        // The same copper, read whole, is isolated normally — so the refusal is about the
        // flag and not about the geometry.
        let whole = snapshot(vec![feature("A", vec![square(0, 0, 500_000)])]);
        assert!(!isolate(&whole, 254_000, 150_000).contours.is_empty());
    }

    /// **An empty result always carries a reason.** Whichever path produced no contours,
    /// the operator gets a sentence rather than a step that engraved nothing and looked
    /// complete — the shape of the reported fault.
    #[test]
    fn a_pass_that_cuts_nothing_always_says_why() {
        let cases: Vec<(&str, IsolationResult)> = vec![
            ("no copper on the layer", isolate(&snapshot(vec![]), 254_000, 150_000)),
            (
                "a width of zero",
                isolate(&snapshot(vec![feature("A", vec![square(0, 0, 500_000)])]), 0, 0),
            ),
            (
                "copper with no polygons",
                isolate(&snapshot(vec![feature("A", vec![])]), 254_000, 150_000),
            ),
        ];

        for (label, result) in cases {
            if result.contours.is_empty() {
                assert!(
                    !result.warnings.is_empty(),
                    "{label}: no contours and no reason given",
                );
            }
        }
    }

    /// **The invariant the pass exists to keep**: a channel is either cut, or reported.
    /// Never quietly absent.
    ///
    /// Driven across the clearances a real layout produces, at the width an operator
    /// actually picks — the board's own clearance rule. The fault this guards needed no
    /// unusual input at all, which is why it survived a full test file: the contour was
    /// clipped away where it would have grazed a neighbour, the rung list held one entry so
    /// no narrower rung picked it up, and `narrowed` stayed empty because the prediction
    /// that built that list measured the two nets' *copper* while the clip measured the
    /// *contour* between them. Cut by nothing, mentioned by nothing.
    #[test]
    fn no_gap_loses_a_channel_without_saying_so() {
        let radius = 500_000;
        for gap in [
            80_000, 120_000, 150_000, 175_000, 190_000, 195_000, 198_000, 199_000, 200_000,
            203_200, 210_000, 250_000,
        ] {
            let pitch = 2 * radius + gap;
            let board = snapshot(vec![
                feature("A", pad_column(0, radius, pitch, 6)),
                feature("B", pad_column(2 * radius + gap, radius, pitch, 6)),
            ]);
            let result = isolate(&board, 200_000, 100_000);

            // The same copper with nothing near it: the whole channel this net needs.
            let alone = isolate(
                &snapshot(vec![feature("A", pad_column(0, radius, pitch, 6))]),
                200_000,
                100_000,
            );
            let want = emitted_len(&alone, "A");
            let got = emitted_len(&result, "A");
            let told = !result.narrowed.is_empty() || !result.uncut.is_empty();

            assert!(
                got > want * 0.98 || told,
                "gap {gap}: net A got {got:.0} nm of the {want:.0} nm of channel it needs, \
                 and nothing was reported — narrowed {:?}, uncut {:?}",
                result.narrowed,
                result.uncut,
            );
        }
    }

    /// **The fix, stated directly.** A hint that says "full width fits" when it does not
    /// must cost a little searching and nothing else.
    ///
    /// This is the exact shape of the reported fault. `record_narrowed` returns an empty
    /// list when it believes every crowded pair takes the requested width; that list used
    /// to *be* the rungs, so a stretch the first rung could not cut had nothing beneath it
    /// and vanished. Here the hint is empty and the gap plainly cannot take full width, so
    /// if the descent still depended on the hint the tight stretch would come back uncut and
    /// unreported.
    #[test]
    fn a_hint_that_is_wrong_still_gets_the_channel_cut() {
        // Edges at 0.5 mm and 0.8 mm: a 0.3 mm gap, asked for a 0.4 mm channel.
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(1_300_000, 0, MM / 2)]),
        ]);
        let nets = net_regions(&board);
        let others = neighbouring_copper(&nets, 0, 400_000);
        let mut out = IsolationResult::default();

        let uncut = walk_ladder(&nets[0], &others, &ladder(400_000, 50_000), &[], &mut out);

        assert!(uncut.is_empty(), "a 0.3 mm gap takes a 0.3 mm cut — nothing is uncuttable");
        let widths: std::collections::BTreeSet<i64> =
            out.contours.iter().map(|c| c.width_nm).collect();
        assert!(widths.contains(&400_000), "full width where there is room: {widths:?}");
        assert!(
            widths.iter().any(|&w| w <= 300_000),
            "and the facing stretch cut at a width the gap allows: {widths:?}",
        );
    }

    /// Copper the ladder could not separate is measured and named.
    ///
    /// `narrowed` is a *prediction* about two nets' copper, made before anything is cut;
    /// this is the *outcome* of walking the ladder. They are reached differently on purpose,
    /// and it is the outcome that gets machined — so it is the outcome that has to reach the
    /// operator, whatever the prediction believed.
    #[test]
    fn what_the_ladder_could_not_cut_is_measured_and_named() {
        // A 20 µm gap, well under the 50 µm floor.
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(1_020_000, 0, MM / 2)]),
        ]);
        let result = isolate(&board, 400_000, 50_000);

        let names: Vec<&str> = result.uncut.iter().map(|u| u.net.as_str()).collect();
        assert_eq!(names, ["A", "B"], "both sides of the gap went uncut");
        assert!(
            result.uncut.iter().all(|u| u.length_nm > 0.0),
            "a stretch reported as uncut must have a length: {:?}",
            result.uncut,
        );
        assert!(
            result.warnings.iter().any(|w| w.contains("could not be cut at any width")),
            "warnings were {:?}",
            result.warnings,
        );
    }

    /// Room everywhere means nothing to report, which is what makes the report worth
    /// reading. A pass that cried uncut on a board it cut perfectly would be ignored on the
    /// board it did not.
    #[test]
    fn a_board_with_room_reports_nothing_uncut() {
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(2 * MM, 0, MM / 2)]),
        ]);
        let result = isolate(&board, 400_000, 50_000);

        assert!(result.uncut.is_empty(), "{:?}", result.uncut);
        assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    }

    /// The descent stops as soon as a rung blocks nothing.
    ///
    /// The ladder from 0.4 mm to a 0.05 mm floor is fourteen rungs, and each one costs a
    /// polygon offset of a net that may be a board-sized pour. A pair crowded at one width
    /// must cost two rungs, not fourteen — which shows up here as the number of distinct
    /// widths the net's contour comes back at.
    #[test]
    fn a_crowded_net_stops_descending_once_nothing_is_blocked() {
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(1_300_000, 0, MM / 2)]), // a 0.3 mm gap
        ]);
        let result = isolate(&board, 400_000, 50_000);

        let widths: std::collections::BTreeSet<i64> =
            contours_of(&result, "A").iter().map(|c| c.width_nm).collect();
        assert_eq!(
            widths.len(),
            2,
            "full width, then the one rung the gap allows — got {widths:?}",
        );
    }

    /// The base case, and the one that says the offset went the right way: with room to
    /// spare, every net gets one uninterrupted loop at exactly the width asked for.
    #[test]
    fn copper_with_room_around_it_isolates_at_the_full_width() {
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(2 * MM, 0, MM / 2)]),
        ]);
        let result = isolate(&board, 400_000, 50_000);

        assert!(result.narrowed.is_empty(), "a 1 mm gap has room for a 0.4 mm cut");
        for net in ["A", "B"] {
            let contours = contours_of(&result, net);
            assert_eq!(contours.len(), 1, "{net} is one island, so one loop");
            assert!(contours[0].closed);
            assert_eq!(contours[0].width_nm, 400_000);
        }
    }

    /// A gap narrower than the cut must not be cut at full width — the tool would take a
    /// bite out of the neighbour. The pass has to narrow, and has to say so, because a
    /// board that quietly cut through its own tracks would look finished and be scrap.
    #[test]
    fn a_gap_narrower_than_the_cut_narrows_and_is_reported() {
        // Edges at x = 0.5 mm and x = 0.8 mm: a gap of exactly 0.3 mm.
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(1_300_000, 0, MM / 2)]),
        ]);
        let result = isolate(&board, 400_000, 50_000);

        assert_eq!(
            result.narrowed,
            vec![NarrowedPair { nets: ("A".into(), "B".into()), width_nm: 300_000 }],
            "0.3 mm is the widest rung that fits in a 0.3 mm gap"
        );
        for net in ["A", "B"] {
            let widths: Vec<i64> = contours_of(&result, net).iter().map(|c| c.width_nm).collect();
            assert!(widths.contains(&400_000), "{net} keeps full width where it has room");
            assert!(widths.contains(&300_000), "{net} takes the narrow rung facing its neighbour");
            assert!(
                contours_of(&result, net).iter().all(|c| !c.closed),
                "{net} changes width part way round, so it can no longer be one loop"
            );
        }
    }

    /// The property the ladder exists for. A long net that is cramped at one point must
    /// keep its full width everywhere else — narrowing the whole net would be the easy
    /// implementation and would throw away isolation the board had already paid for.
    #[test]
    fn a_net_tight_in_one_place_keeps_full_width_everywhere_else() {
        // A 20 mm bar with a single pad crowding its top edge near the middle.
        let bar = Polygon {
            outline: vec![
                (0, -150_000),
                (20 * MM, -150_000),
                (20 * MM, 150_000),
                (0, 150_000),
            ],
            holes: Vec::new(),
        };
        let board = snapshot(vec![
            feature("BAR", vec![bar]),
            feature("PAD", vec![square(10 * MM, 850_000, MM / 2)]),
        ]);
        let result = isolate(&board, 400_000, 50_000);

        let bar_contours = contours_of(&result, "BAR");
        let full: f64 = bar_contours
            .iter()
            .filter(|c| c.width_nm == 400_000)
            .map(|c| polyline_len_nm(&c.path))
            .sum();
        let narrow: f64 = bar_contours
            .iter()
            .filter(|c| c.width_nm < 400_000)
            .map(|c| polyline_len_nm(&c.path))
            .sum();

        assert!(narrow > 0.0, "the crowded stretch had to narrow");
        assert!(
            full > 20.0 * narrow,
            "the narrowing must stay local: {full:.0} nm at full width against {narrow:.0} nm narrowed"
        );
    }

    /// Copper on no net is not one net. Two fiducials share only the absence of a net, and
    /// treating them as one group would leave the channel between them uncut.
    #[test]
    fn separate_pieces_of_unnetted_copper_are_isolated_from_each_other() {
        let board = snapshot(vec![feature(
            "",
            vec![square(0, 0, MM / 2), square(2 * MM, 0, MM / 2)],
        )]);
        let result = isolate(&board, 400_000, 50_000);

        assert_eq!(result.contours.len(), 2, "two islands, two loops");
        let names: std::collections::BTreeSet<&str> =
            result.contours.iter().map(|c| c.net.as_str()).collect();
        assert_eq!(names.len(), 2, "each island is its own pseudo-net, not one shared one");
    }

    /// A poured zone arrives from KiCad with its thermal reliefs already cut out, and those
    /// inner rings are copper boundaries like any other. Losing them would leave whatever
    /// sits in the relief connected to the plane.
    #[test]
    fn a_zone_keeps_the_holes_kicad_filled_around() {
        let zone = Polygon {
            outline: vec![(0, 0), (10 * MM, 0), (10 * MM, 10 * MM), (0, 10 * MM)],
            holes: vec![vec![
                (4 * MM, 4 * MM),
                (6 * MM, 4 * MM),
                (6 * MM, 6 * MM),
                (4 * MM, 6 * MM),
            ]],
        };
        let result = isolate(&snapshot(vec![feature("GND", vec![zone])]), 400_000, 50_000);

        assert_eq!(result.contours.len(), 2, "the outline and the relief are both cut");
        assert!(result.contours.iter().all(|c| c.closed));
        let inner = result
            .contours
            .iter()
            .min_by_key(|c| path_perimeter_nm(&c.path) as i64)
            .expect("a contour");
        // Dilating the copper shrinks its hole: a 2 mm relief loses half the cut width
        // from each side, so the loop runs at 1.6 mm across.
        let perimeter = path_perimeter_nm(&inner.path);
        assert!(
            (6_300_000.0..6_500_000.0).contains(&perimeter),
            "the relief loop should be about 6.4 mm round, was {perimeter:.0} nm"
        );
    }

    /// A track is one piece of copper, so it wants one loop around it. Two would mean the
    /// stroke and the offset had disagreed about what the track is.
    #[test]
    fn a_net_made_of_one_track_yields_one_closed_loop() {
        let track = crate::stitching::stroke_open_path(&[(0, 0), (10 * MM, 0)], 150_000.0);
        let polygons: Vec<Polygon> =
            track.into_iter().map(|outline| Polygon { outline, holes: Vec::new() }).collect();
        let result = isolate(&snapshot(vec![feature("SIG", polygons)]), 400_000, 50_000);

        assert_eq!(result.contours.len(), 1);
        assert!(result.contours[0].closed);
        assert_eq!(result.contours[0].width_nm, 400_000);
    }

    /// Copper closer together than the tool's narrowest cut cannot be isolated at all. The
    /// only honest outcome is to say so — an operator who is told nothing will assume the
    /// board came out separated.
    #[test]
    fn copper_too_close_for_the_narrowest_cut_is_reported_as_uncut() {
        // A 20 µm gap, against a 50 µm floor.
        let board = snapshot(vec![
            feature("A", vec![square(0, 0, MM / 2)]),
            feature("B", vec![square(1_020_000, 0, MM / 2)]),
        ]);
        let result = isolate(&board, 400_000, 50_000);

        println!("uncut {:?}
warnings {:?}", result.uncut, result.warnings);
        assert_eq!(result.narrowed, vec![NarrowedPair { nets: ("A".into(), "B".into()), width_nm: 0 }]);
        assert!(
            result.warnings.iter().any(|w| w.contains("not isolated")),
            "the operator has to be told, warnings were {:?}",
            result.warnings
        );
    }

    /// The ladder always ends at the floor, whatever the step leaves over, and never runs
    /// past the width that was asked for.
    #[test]
    fn the_ladder_starts_at_the_requested_width_and_ends_at_the_floor() {
        let rungs = ladder(400_000, 50_000);
        assert_eq!(rungs.first(), Some(&400_000));
        assert_eq!(rungs.last(), Some(&50_000));
        assert!(rungs.windows(2).all(|w| w[0] > w[1]), "the ladder only descends");

        assert_eq!(ladder(30_000, 50_000), vec![30_000], "a floor above the width is just the width");
    }
}
