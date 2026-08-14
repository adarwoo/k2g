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
//! A contour whose width changes along its length is **split into spans**, because a span
//! is what becomes an operation and an operation has one depth. A contour that took one
//! width the whole way round stays a closed loop, which is what lets the planner choose
//! its lead-in freely later.
//!
//! Coordinates are board nanometres throughout, as everywhere else geometry is done here.

use std::collections::BTreeMap;

use clipper2_rust::{
    clipper::{difference_64, intersect_64, union_64},
    core::{FillRule, Path64, Paths64, Point64},
    engine::ClipType,
    engine_public::Clipper64,
};

use crate::copper::{CopperSnapshot, Polygon};
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct IsolationResult {
    pub layer_id: i32,
    pub contours: Vec<IsolationContour>,
    /// Every pair that had to give up width. Silence here would mean a board that looks
    /// isolated and is not.
    pub narrowed: Vec<NarrowedPair>,
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

/// The step between rungs of the width ladder.
///
/// 25 µm is a step no operator would notice against a width they chose in hundredths of a
/// millimetre, and coarse enough that a cramped board does not spend a hundred passes
/// walking down to its answer.
pub const LADDER_STEP_NM: i64 = 25_000;

/// Slack allowed when deciding whether a cut touches copper it should not.
///
/// A gap of exactly the cut width is the commonest case on a board laid out to a clearance
/// rule, and it is a *fit*, not a collision. Without this, every such pair would come back
/// narrowed by one whole rung because the geometry grazes itself to the nanometre.
const TANGENCY_SLACK_NM: f64 = 1_000.0;

type Ring = Vec<(i64, i64)>;

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

        let achieved = record_narrowed(&nets, index, &intrusion, &rungs, &mut narrowed);
        walk_ladder(net, &others, &walked_rungs(width_nm, &achieved), &mut result);
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

/// The rungs one net's contour is actually walked at: full width, and the widths its own
/// crowded neighbours turned out to allow.
///
/// The full ladder is the *vocabulary* of widths; it is not a list of passes to make. A
/// rung between the requested width and the widest gap this net has is a rung that fits
/// nowhere on it, and walking it would cost two polygon offsets over the whole net to
/// produce nothing. On a board where the layout has one clearance rule this collapses
/// thirteen rungs to two, which is the difference between a plan that appears and a plan
/// that is waited for.
///
/// Skipping rungs cannot cut anything it should not: a rung only ever takes what the
/// previous *walked* rung had to leave, so a wider skipped rung simply means the stretch
/// it would have claimed is cut at the next width down. Conservative, never generous.
fn walked_rungs(width_nm: i64, achieved: &[i64]) -> Vec<i64> {
    let mut rungs = vec![width_nm];
    rungs.extend(achieved.iter().copied().filter(|&w| w > 0 && w < width_nm));
    rungs.sort_unstable_by(|a, b| b.cmp(a));
    rungs.dedup();
    rungs
}

/// Walks the ladder for one net, emitting the widest cut that fits at every point.
///
/// Each rung is clipped twice. First against the copper it must not touch, which is what
/// makes the rung legal. Then against the reach of the rung above, which is what stops it
/// re-cutting ground already taken at a wider setting: a point is only this rung's work if
/// the previous rung could not have it.
fn walk_ladder(net: &NetCopper, others: &[Ring], rungs: &[i64], out: &mut IsolationResult) {
    let mut previous: Option<i64> = None;
    for &width in rungs {
        let half = width as f64 / 2.0;
        let contour = offset_group(&net.region, half);
        if contour.is_empty() {
            continue;
        }
        let forbidden = offset_group(others, half - TANGENCY_SLACK_NM);

        let mut pieces: Vec<Piece> = contour.into_iter().map(Piece::Closed).collect();
        pieces = clip_pieces(pieces, &forbidden, ClipType::Difference);
        if let Some(previous) = previous {
            // The rung above kept a stretch when the neighbouring copper was more than
            // `previous/2 - slack` from a contour drawn at `previous/2` — that is, when
            // the gap exceeded `previous - slack`. What it dropped is everything at or
            // under that, which on *this* contour, drawn at `half`, is everything within
            // `previous - half - slack` of the copper.
            //
            // The slack is subtracted for the same reason it was added up there, and the
            // two conditions are then exact complements: one is `>`, the other `<=`. Get
            // the sign wrong and each rung re-cuts the last stretch of its predecessor's.
            let taken = offset_group(others, previous as f64 - half - TANGENCY_SLACK_NM);
            pieces = clip_pieces(pieces, &taken, ClipType::Intersection);
        }

        for piece in pieces {
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
        previous = Some(width);
    }
}

/// Records, for every net crowding `index`, the widest rung that fits between the two, and
/// returns those widths so the contour can be walked at exactly them.
///
/// Done per *pair* rather than per span: the number the operator needs is "these two nets
/// only got 0.2 mm", and asking the question of the pair directly gives it exactly, with
/// no dependence on how the spans happened to be cut up.
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

/// One KiCad polygon as a properly wound region.
///
/// KiCad does not promise a winding, and a hole ring drawn the same way round as its
/// outline would fill rather than pierce under any winding rule. Differencing the holes
/// out asks Clipper to settle it, and its output is oriented the way everything downstream
/// assumes.
fn polygon_region(polygon: &Polygon) -> Vec<Ring> {
    if polygon.outline.len() < 3 {
        return Vec::new();
    }
    let outline = vec![polygon.outline.clone()];
    let holes: Vec<Ring> = polygon.holes.iter().filter(|h| h.len() >= 3).cloned().collect();
    if holes.is_empty() {
        union(&outline, &[])
    } else {
        difference(&outline, &holes)
    }
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

/// Splits a region into its connected pieces, each with the holes that belong to it.
///
/// Containment counting rather than winding: a ring inside an odd number of others is a
/// hole. That holds whatever orientation the rings arrive in, and the alternative — trust
/// the sign of the area — is one library convention away from silently pairing a hole with
/// the wrong island.
fn components(region: &[Ring]) -> Vec<Vec<Ring>> {
    let rings: Vec<&Ring> = region.iter().filter(|r| r.len() >= 3).collect();
    let depth: Vec<usize> = rings
        .iter()
        .map(|ring| {
            rings
                .iter()
                .filter(|other| !std::ptr::eq(*other, ring) && point_in_ring(ring[0], other))
                .count()
        })
        .collect();

    let mut out: Vec<Vec<Ring>> = Vec::new();
    let mut index_of: Vec<Option<usize>> = vec![None; rings.len()];
    for (i, ring) in rings.iter().enumerate() {
        if depth[i].is_multiple_of(2) {
            index_of[i] = Some(out.len());
            out.push(vec![(*ring).clone()]);
        }
    }
    for (i, ring) in rings.iter().enumerate() {
        if depth[i].is_multiple_of(2) {
            continue;
        }
        // The hole belongs to the smallest island that contains it — nesting means several
        // do, and only the innermost is its own.
        let owner = rings
            .iter()
            .enumerate()
            .filter(|(j, other)| {
                depth[*j].is_multiple_of(2) && *j != i && point_in_ring(ring[0], other)
            })
            .min_by_key(|(_, other)| area_nm2(other).unsigned_abs())
            .and_then(|(j, _)| index_of[j]);
        if let Some(owner) = owner {
            out[owner].push((*ring).clone());
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Clipper, kept behind names that say what they are for
// ---------------------------------------------------------------------------

fn to_paths(rings: &[Ring]) -> Paths64 {
    rings
        .iter()
        .filter(|r| r.len() >= 3)
        .map(|r| r.iter().map(|&(x, y)| Point64 { x, y }).collect::<Path64>())
        .collect()
}

fn from_paths(paths: &Paths64) -> Vec<Ring> {
    paths
        .iter()
        .map(|p| p.iter().map(|pt| (pt.x, pt.y)).collect::<Ring>())
        .filter(|r: &Ring| r.len() >= 2)
        .collect()
}

fn union(a: &[Ring], b: &[Ring]) -> Vec<Ring> {
    let (a, b) = (to_paths(a), to_paths(b));
    if a.is_empty() && b.is_empty() {
        return Vec::new();
    }
    from_paths(&union_64(&a, &b, FillRule::NonZero))
}

fn difference(a: &[Ring], b: &[Ring]) -> Vec<Ring> {
    let (a, b) = (to_paths(a), to_paths(b));
    if a.is_empty() {
        return Vec::new();
    }
    from_paths(&difference_64(&a, &b, FillRule::NonZero))
}

fn intersect(a: &[Ring], b: &[Ring]) -> Vec<Ring> {
    let (a, b) = (to_paths(a), to_paths(b));
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    from_paths(&intersect_64(&a, &b, FillRule::NonZero))
}

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

fn area_nm2(ring: &[(i64, i64)]) -> i128 {
    let n = ring.len();
    let mut sum: i128 = 0;
    for i in 0..n {
        let (x0, y0) = ring[i];
        let (x1, y1) = ring[(i + 1) % n];
        sum += (x0 as i128) * (y1 as i128) - (x1 as i128) * (y0 as i128);
    }
    sum / 2
}

fn point_in_ring(point: (i64, i64), ring: &[(i64, i64)]) -> bool {
    let (px, py) = (point.0 as i128, point.1 as i128);
    let mut inside = false;
    let n = ring.len();
    for i in 0..n {
        let (x0, y0) = (ring[i].0 as i128, ring[i].1 as i128);
        let (x1, y1) = (ring[(i + 1) % n].0 as i128, ring[(i + 1) % n].1 as i128);
        if (y0 > py) != (y1 > py) {
            let cross = (x1 - x0) * (py - y0) - (px - x0) * (y1 - y0);
            if (cross > 0) == (y1 > y0) {
                inside = !inside;
            }
        }
    }
    inside
}

#[derive(Clone, Copy, Debug, Default)]
struct BBox {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl BBox {
    fn of(rings: &[Ring]) -> Option<BBox> {
        let mut bbox: Option<BBox> = None;
        for &(x, y) in rings.iter().flatten() {
            bbox = Some(match bbox {
                None => BBox { x0: x, y0: y, x1: x, y1: y },
                Some(b) => BBox {
                    x0: b.x0.min(x),
                    y0: b.y0.min(y),
                    x1: b.x1.max(x),
                    y1: b.y1.max(y),
                },
            });
        }
        bbox
    }

    fn expand(self, by: i64) -> BBox {
        BBox {
            x0: self.x0.saturating_sub(by),
            y0: self.y0.saturating_sub(by),
            x1: self.x1.saturating_add(by),
            y1: self.y1.saturating_add(by),
        }
    }

    fn overlaps(self, other: BBox) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1 && self.y0 <= other.y1 && other.y0 <= self.y1
    }

    fn ring(self) -> Ring {
        vec![
            (self.x0, self.y0),
            (self.x1, self.y0),
            (self.x1, self.y1),
            (self.x0, self.y1),
        ]
    }
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
        CopperSnapshot { layer_id: 3, features, warnings: Vec::new() }
    }

    fn contours_of<'a>(r: &'a IsolationResult, net: &str) -> Vec<&'a IsolationContour> {
        r.contours.iter().filter(|c| c.net == net).collect()
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
